// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.4-§12.6, R21, R22, R25; `contracts/policy-suspension.md`): the failure
//! identity a scheduled `linear_steps` run's live settlement writes as its existing
//! `policy.execution_recorded` `failure.error_code`.
//!
//! The scheduled path records, by decision id, what preparation resolved: whether the main
//! package was admitted and is `linear_steps`, its game, and the prerequisite chain layer by
//! layer (§5.2.1 last item). The settlement reads it with the run's own task rows and error
//! frames, compares the error frame with the previous failure's only when the identity prefix
//! repeats, and writes the identity. Replay never compares: it reads the recorded code.

use super::contained_task::prepare_contained_task;
use super::policy_outcome::policy_dispatch_intent;
use super::*;
use crate::failure_identity::{
    FailureIdentity, IdentityFrame, IdentityLayer, RERUN_ONLY_BACKEND_FAILURES, SuspensionLiftView,
    encode_failure_base, failure_accumulates, failure_detail_fingerprint, failure_key,
    failure_rows, failure_step, identifier_text, in_restart_segment, locate_error_frame,
    package_digest_prefix,
};
use actingcommand_execution_kernel::{FailureFrameVerdict, compare_failure_frames};
use actingcommand_ledger::LedgerArtifactReference;

/// One prerequisite layer a scheduled run resolved, outermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedLayer {
    pub(super) package_id: String,
    /// The game and server whose return-home package this layer is.
    pub(super) return_home: Option<(String, String)>,
}

/// What a scheduled run's preparation resolved, from the scheduled path's entry until its
/// execution record is written. Memory only.
#[derive(Debug, Clone, Default)]
pub(super) struct ScheduledResolution {
    /// `None` until the main package is admitted.
    main: Option<ScheduledMain>,
}

#[derive(Debug, Clone)]
struct ScheduledMain {
    linear: bool,
    game: String,
    layers: Vec<ResolvedLayer>,
    /// Whether the whole prerequisite chain was resolved and admitted.
    resolved: bool,
}

/// Two error frames compared (§12.5); `Unavailable` names why they could not be.
enum FrameComparison {
    Similar,
    Different,
    Unavailable(&'static str),
}

impl HostShared {
    /// The configuration a paused pair's suspension is lifted against (§12.7).
    pub(super) fn suspension_lift_view<'a>(
        &'a self,
        manifest: &'a ProcedureManifest,
    ) -> SuspensionLiftView<'a> {
        SuspensionLiftView {
            manifest,
            prerequisite_packages: &self.prerequisite_packages,
            return_home_packages: &self.return_home_packages,
        }
    }

    /// Registers a scheduled run's decision as it enters the scheduled path.
    pub(super) fn register_scheduled_resolution(
        &self,
        decision_id: &str,
    ) -> Result<(), RequestFailure> {
        lock(&self.scheduled_resolutions, "register_scheduled_resolution")?
            .insert(decision_id.to_owned(), ScheduledResolution::default());
        Ok(())
    }

    /// Admits a scheduled run's main package and resolves its prerequisite chain, recording
    /// both for the settlement, a refusal included.
    pub(super) fn prepare_scheduled_package(
        &self,
        decision_id: &str,
        instance_alias: &str,
        task_request: &ContainedTaskRequest,
        material_deadline: Instant,
        prerequisite_deadline: impl FnOnce() -> Result<Instant, RequestFailure>,
    ) -> Result<(PreparedContainedTask, Vec<PreparedContainedTask>), RequestFailure> {
        let prepared = prepare_contained_task(
            instance_alias,
            task_request,
            self.execution()?.vision_provider(),
            material_deadline,
        )?;
        let mut layers = Vec::new();
        let chain = self.resolve_prerequisite_chain_recorded(
            instance_alias,
            &prepared,
            prerequisite_deadline,
            &mut layers,
        );
        let main = ScheduledMain {
            linear: prepared.execution_mode() == "linear_steps",
            game: prepared.game().to_owned(),
            layers,
            resolved: chain.is_ok(),
        };
        lock(&self.scheduled_resolutions, "record_scheduled_resolution")?
            .entry(decision_id.to_owned())
            .or_default()
            .main = Some(main);
        Ok((prepared, chain?))
    }

    /// Drops a decision's resolution once its execution record is written.
    pub(super) fn forget_scheduled_resolution(&self, decision_id: &str) -> RuntimeHostResult<()> {
        lock(&self.scheduled_resolutions, "forget_scheduled_resolution")?.remove(decision_id);
        Ok(())
    }

    /// The policy execution input of a failed scheduled run (§12.4): a `linear_steps` run's
    /// failure identity, or the original input for any other run. A fatal error is marked.
    pub(super) fn scheduled_failure_identity(
        &self,
        context: &PolicyRunContext,
        failure: &RequestFailure,
        input: PolicyExecutionInput,
    ) -> RuntimeHostResult<PolicyExecutionInput> {
        let result = self.build_scheduled_failure_identity(context, failure, input);
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.fatal.mark(error.clone())?;
        }
        result
    }

    fn build_scheduled_failure_identity(
        &self,
        context: &PolicyRunContext,
        failure: &RequestFailure,
        input: PolicyExecutionInput,
    ) -> RuntimeHostResult<PolicyExecutionInput> {
        let (code, class) = match input {
            PolicyExecutionInput::Failed { error_code, class } => (error_code, class),
            PolicyExecutionInput::Succeeded => return Ok(PolicyExecutionInput::Succeeded),
        };
        let decision_id = context.decision_id();
        let (recorded, previous) = {
            let policy = lock(self.policy()?, "read_policy_failure_identity")?;
            (
                policy.recorded_execution(decision_id).cloned(),
                policy
                    .latest_failure(context.catalog_task_id(), context.instance_alias())
                    .map(|(code, decision)| (code.to_owned(), decision.to_owned())),
            )
        };
        // §12.6 point 4: a dispatch that already has its execution record keeps its code.
        if let Some(recorded) = recorded {
            return recorded_failure_input(recorded, code, class);
        }
        let resolution = lock(&self.scheduled_resolutions, "read_scheduled_resolution")?
            .get(decision_id)
            .cloned();
        let Some(resolution) = resolution else {
            // An internal miss keeps the original code and says so.
            self.record_scheduled_policy_diagnostic(
                context,
                &RuntimeHostError::request(
                    "policy_failure_identity_unavailable",
                    "build_failure_identity",
                    RuntimeErrorCode::RuntimeUnavailable,
                )
                .with_native_detail(format!("decision_id={decision_id}")),
            )?;
            return Ok(PolicyExecutionInput::Failed {
                error_code: code,
                class,
            });
        };
        // A main package that was not admitted (its mode is unknown) or is a page-graph
        // package keeps the original code.
        let Some(main) = resolution.main.filter(|main| main.linear) else {
            return Ok(PolicyExecutionInput::Failed {
                error_code: code,
                class,
            });
        };
        let query = |query: EventQuery| {
            self.ledger
                .query(query)
                .map_err(|_| ledger_error("read_failure_identity_rows"))
        };
        let (rows, scope, operation) = if main.resolved {
            let rows = failure_rows(context.run_id(), query)?;
            let (scope, operation) = failure_step(&rows, !main.layers.is_empty());
            (rows, scope, operation)
        } else {
            (Vec::new(), "prepare".to_owned(), "prepare".to_owned())
        };
        let restart_segment = scope == "main" && in_restart_segment(&rows, &main.game);
        // R25-2: an adb backend failure of a linear task that did not poison the run is
        // recoverable and rerun only.
        let rerun_only_backend =
            RERUN_ONLY_BACKEND_FAILURES.contains(&code.as_str()) && !failure.poison_runtime;
        let class = if rerun_only_backend {
            PolicyFailureClass::Recoverable
        } else {
            class
        };
        let detail = failure_detail_fingerprint(
            failure
                .error
                .diagnostics()
                .native_detail()
                .map(|detail| detail.text()),
        );
        let mut identity = FailureIdentity {
            base: encode_failure_base(&code),
            key: failure_key(&code, &scope, &operation, &detail),
            main_digest: package_digest_prefix(context.package_digest()),
            layers: main
                .layers
                .iter()
                .map(|layer| {
                    IdentityLayer::resolved(
                        &layer.package_id,
                        layer
                            .return_home
                            .as_ref()
                            .map(|(game, server)| (game.as_str(), server.as_str())),
                        &self.prerequisite_packages,
                    )
                })
                .collect(),
            frame: IdentityFrame::unique(decision_id),
        };
        if rerun_only_backend || !failure_accumulates(&code, &scope, restart_segment) {
            return Ok(PolicyExecutionInput::Failed {
                error_code: identity.encode(),
                class,
            });
        }
        let current = if scope == "prepare" {
            None
        } else {
            locate_error_frame(context.run_id(), query)?
        };
        identity.frame =
            IdentityFrame::of_artifact(current.as_ref().map(LedgerArtifactReference::sha256));
        let previous = previous.filter(|(previous, _)| {
            FailureIdentity::parse(previous).is_some_and(|previous| {
                previous.accumulates() && previous.prefix() == identity.prefix()
            })
        });
        let Some((previous_code, previous_decision)) = previous else {
            return Ok(PolicyExecutionInput::Failed {
                error_code: identity.encode(),
                class,
            });
        };
        let (comparison, previous_run) =
            self.compare_error_frames(&previous_decision, current.as_ref())?;
        let error_code = match comparison {
            FrameComparison::Different => identity.encode(),
            FrameComparison::Similar => previous_code,
            // A comparison that cannot be made counts as the same problem: the prefix already
            // matched. Only the main scope has frames to miss (§12.4).
            FrameComparison::Unavailable(reason) => {
                if scope == "main" {
                    self.record_scheduled_policy_diagnostic(
                        context,
                        &RuntimeHostError::request(
                            "policy_failure_frame_compare_unavailable",
                            "compare_failure_frames",
                            RuntimeErrorCode::RecognitionFailed,
                        )
                        .with_native_detail(format!(
                            "reason={reason} previous_run={} current_run={}",
                            previous_run
                                .as_ref()
                                .map(identifier_text)
                                .unwrap_or_default(),
                            identifier_text(&context.run_id())
                        )),
                    )?;
                }
                previous_code
            }
        };
        Ok(PolicyExecutionInput::Failed { error_code, class })
    }

    /// Compares the error frame of the previous failed dispatch's run with the current run's.
    fn compare_error_frames(
        &self,
        previous_decision: &str,
        current: Option<&LedgerArtifactReference>,
    ) -> RuntimeHostResult<(FrameComparison, Option<RunId>)> {
        let through = self
            .ledger
            .latest_sequence()
            .map_err(|_| ledger_error("compare_failure_frames"))?;
        let previous_run = policy_dispatch_intent(
            &self.ledger,
            previous_decision,
            through,
            "compare_failure_frames",
        )?
        .links()
        .run_id()
        .copied();
        let previous = match previous_run {
            Some(run_id) => locate_error_frame(run_id, |query: EventQuery| {
                self.ledger
                    .query(query)
                    .map_err(|_| ledger_error("compare_failure_frames"))
            })?,
            None => None,
        };
        let (Some(previous), Some(current)) = (previous, current) else {
            return Ok((FrameComparison::Unavailable("frame_missing"), previous_run));
        };
        let read = |artifact: &LedgerArtifactReference| {
            read_projected_verified(self.artifacts.root(), &artifact.project(true))
        };
        let (Ok(previous), Ok(current)) = (read(&previous), read(current)) else {
            return Ok((
                FrameComparison::Unavailable("artifact_unreadable"),
                previous_run,
            ));
        };
        let comparison = match compare_failure_frames(&previous, &current) {
            Ok(compared) if compared.verdict == FailureFrameVerdict::Similar => {
                FrameComparison::Similar
            }
            Ok(_) => FrameComparison::Different,
            Err(error) => FrameComparison::Unavailable(error.reason()),
        };
        Ok((comparison, previous_run))
    }

    /// A Warning `runtime.failed` diagnostic of the settlement, written as a scheduled policy
    /// failure is, with its lifecycle record at the same severity.
    fn record_scheduled_policy_diagnostic(
        &self,
        context: &PolicyRunContext,
        error: &RuntimeHostError,
    ) -> RuntimeHostResult<()> {
        let links = self.policy_run_event_links(context)?;
        let diagnostic = self.append_event_raw(
            EventSeverity::Warning,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links.clone(),
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                DiagnosticDetailDraft::new(
                    "policy_driver",
                    RuntimeLifecycleFailureStage::PolicyDriver.as_str(),
                    "runtime_host",
                    error.operation(),
                    error.code(),
                    Sensitivity::Internal,
                ),
                AuditInput::new(),
            ),
        )?;
        self.record_required_failure_with_severity(
            error,
            &diagnostic,
            links,
            Some(EventSeverity::Warning),
        )
    }
}

/// §12.6 point 4: the recorded execution's own input, after its base segment is checked against
/// the terminal's code; any other record is left to the replay, which refuses it.
fn recorded_failure_input(
    recorded: PolicyExecutionEventData,
    code: String,
    class: PolicyFailureClass,
) -> RuntimeHostResult<PolicyExecutionInput> {
    let PolicyExecutionOutcome::Failed { failure } = recorded.outcome else {
        return Ok(PolicyExecutionInput::Failed {
            error_code: code,
            class,
        });
    };
    if failure.reported_success {
        return Ok(PolicyExecutionInput::Failed {
            error_code: code,
            class,
        });
    }
    let consistent = match FailureIdentity::parse(&failure.error_code) {
        Some(identity) => identity.base == encode_failure_base(&code),
        None => failure.error_code == code,
    };
    if !consistent {
        return Err(RuntimeHostError::fatal(
            "policy_execution_identity_conflict",
            "replay_policy_failure_identity",
            RuntimeErrorCode::RuntimeFatal,
        ));
    }
    Ok(PolicyExecutionInput::Failed {
        error_code: failure.error_code,
        class: failure.original_class,
    })
}
