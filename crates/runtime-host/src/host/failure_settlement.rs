// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.4-§12.6, R21, R22, R25; `contracts/policy-suspension.md`): the failure
//! identity a scheduled run's live settlement writes as its `policy.execution_recorded`
//! `failure.failure_identity`, beside the failure's `outcome` (Workflow #378 A2-P): a
//! `linear_steps` run's, and since Workflow #390 EXEC-3 (FCM E-02) a page-graph run's.
//!
//! The scheduled path records, by decision id, what preparation resolved: whether the main
//! package was admitted and is `linear_steps`, its game, and the prerequisite chain layer by
//! layer (§5.2.1 last item). The settlement reads it with the run's own task rows and error
//! frames, compares the error frame with the previous failure's only when the identity prefix
//! repeats, and writes the identity. Replay never compares: it reads the recorded identity.
//!
//! Workflow #336 PKG-S3 (M-PKG §3.3): the settlement is one of the two places a package's
//! content hash is measured. It measures the main package and each layer the run resolved, at
//! the paths the run used, and records their hash prefixes in the identity (`x` for a package
//! it could not measure, D18).
//!
//! Workflow #386 SR-3 (scheduling model §2.4 item 2): a scheduled run's preparation suspends its
//! task package at the first refusal with a suspend code, and the background admission after
//! start suspends the bound packages it refuses so (`crate::pack_admission`).

use super::contained_task::{PackageLoad, RefusedPackage, prepare_measured_naming_refusal};
use super::policy_outcome::policy_dispatch_intent;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::{
    FailureIdentity, IdentityFrame, IdentityLayer, IdentityPackage, MISSING_HASH,
    RERUN_ONLY_BACKEND_FAILURES, encode_failure_base, entry_checked, failure_accumulates,
    failure_detail_fingerprint, failure_key, failure_rows, failure_step, identifier_text,
    in_restart_segment, locate_error_frame, navigation_failure_code, navigation_layer_failure,
};
use crate::suspension_lift::{
    LiftCheckTime, SuspensionLiftView, measure_package_hash, standard_pack_root_hash,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::scheduled_failure_code_str;
use actingcommand_execution_kernel::{FailureFrameVerdict, compare_failure_frames};
use actingcommand_ledger::LedgerArtifactReference;

/// One prerequisite layer a scheduled run resolved, outermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedLayer {
    pub(super) package_id: String,
    /// The game and server whose return-home package this layer is.
    pub(super) return_home: Option<(String, String)>,
    /// Workflow #373 NAV-3a: where the standard-pack registry put the package for the run;
    /// `None` when it did not bind it.
    pub(super) package_path: Option<PathBuf>,
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
    /// The admitted package's `package_id` and the path the run read it from.
    package_id: String,
    package_path: String,
    game: String,
    layers: Vec<ResolvedLayer>,
    /// Whether the whole prerequisite chain was resolved and admitted.
    resolved: bool,
}

/// A scheduled run's admitted package, its prerequisite chain, its load and (Workflow #373
/// NAV-6, NAV-5) its standard-pack data.
pub(super) type PreparedScheduledPackage = (
    PreparedContainedTask,
    Vec<PreparedContainedTask>,
    PackageLoad,
    Option<Box<super::standard_pack::RunInterrupts>>,
);

/// Two error frames compared (§12.5); `Unavailable` names why they could not be.
enum FrameComparison {
    Similar,
    Different,
    Unavailable(&'static str),
}

impl HostShared {
    /// The configuration a paused pair's suspension is lifted against (§12.7), checked now:
    /// a package measured within `scheduling.suspension.lift_check_min_interval_ms` is not
    /// measured again (Workflow #336 PKG-S3).
    pub(super) fn suspension_lift_view<'a>(
        &'a self,
        manifest: &'a ProcedureManifest,
    ) -> RuntimeHostResult<SuspensionLiftView<'a>> {
        let time = LiftCheckTime {
            monotonic_ms: self.monotonic_ms()?,
            min_interval_ms: self.lift_check_min_interval_ms()?,
        };
        Ok(SuspensionLiftView::new(
            manifest,
            self.standard_packs.root(),
            &self.lift_checks,
            time,
        ))
    }

    /// Registers a scheduled run's decision as it enters the scheduled path.
    pub(super) fn register_scheduled_resolution(
        &self,
        decision_id: &str,
    ) -> Result<(), RequestFailure> {
        lock(
            &self.scheduled_resolutions,
            HostLocation::RegisterScheduledResolution,
        )?
        .insert(decision_id.to_owned(), ScheduledResolution::default());
        Ok(())
    }

    /// Admits a scheduled run's main package and resolves its prerequisite chain, recording
    /// both for the settlement, a refusal included. Workflow #373 NAV-6 and NAV-5: the run's
    /// standard-pack data are read first, so a run that navigates takes no return-home fallback.
    pub(super) fn prepare_scheduled_package(
        &self,
        decision_id: &str,
        instance_alias: &str,
        task_request: &ContainedTaskRequest,
        material_deadline: Instant,
        prerequisite_deadline: impl FnOnce() -> Result<Instant, RequestFailure>,
    ) -> Result<PreparedScheduledPackage, RequestFailure> {
        // Workflow #385 MEM-1: the main package's load, which the run's timing evidence carries.
        let (prepared, package_load) = match prepare_measured_naming_refusal(
            instance_alias,
            task_request,
            self.execution()?.vision_provider(),
            material_deadline,
        ) {
            Ok(prepared) => prepared,
            Err((failure, refused)) => {
                // Workflow #386 SR-3 (§2.4 item 2): the first refusal with a suspend code
                // suspends the task package.
                if let Some(refused) = refused {
                    self.suspend_refused_package(task_request.package_path(), &refused)?;
                }
                return Err(failure);
            }
        };
        self.pack_admission
            .learn(task_request.package_path(), prepared.package_label())?;
        // A broken game root refuses the run as before, after its main package is recorded.
        let interrupts = self.prepare_run_interrupts(&prepared);
        let navigated = interrupts
            .as_ref()
            .is_ok_and(|interrupts| super::navigation::navigation_applies(interrupts.as_deref()));
        let mut layers = Vec::new();
        let chain = match &interrupts {
            Ok(_) => self.resolve_prerequisite_chain_recorded(
                instance_alias,
                &prepared,
                navigated,
                prerequisite_deadline,
                &mut layers,
            ),
            Err(_) => Ok(Vec::new()),
        };
        let main = ScheduledMain {
            linear: prepared.execution_mode() == "linear_steps",
            package_id: prepared.package_label().to_owned(),
            package_path: task_request.package_path().to_owned(),
            game: prepared.game().to_owned(),
            layers,
            resolved: chain.is_ok(),
        };
        lock(
            &self.scheduled_resolutions,
            HostLocation::RecordScheduledResolution,
        )?
        .entry(decision_id.to_owned())
        .or_default()
        .main = Some(main);
        let interrupts = interrupts?;
        Ok((prepared, chain?, package_load, interrupts))
    }

    /// Workflow #386 SR-3 (§2.4 item 2): suspends the task package a scheduled run's preparation
    /// refused, when the refusal's code is a suspend code in effect.
    fn suspend_refused_package(
        &self,
        path: &str,
        refused: &RefusedPackage,
    ) -> RuntimeHostResult<()> {
        let settings = self.pack_admission_settings()?;
        if crate::pack_admission::suspends(&settings.suspend_codes, refused.code) {
            self.pack_admission
                .suspend(path, &refused.package_id, refused.code)?;
        }
        Ok(())
    }

    /// Workflow #386 SR-3 (§2.4 item 2): the task package admission after start, unless
    /// `scheduling.pack_admission.check_after_start` is off: each package location the procedure
    /// bindings name is admitted once; a refusal with a suspend code suspends its package.
    pub(super) fn check_pack_admission(&self) -> RuntimeHostResult<()> {
        let settings = self.pack_admission_settings()?;
        if !settings.check_after_start {
            return Ok(());
        }
        let paths = match lock(&self.procedure_manifest, HostLocation::CheckPackAdmission)?.as_ref()
        {
            Some(manifest) => crate::pack_admission::bound_paths(manifest),
            None => return Ok(()),
        };
        for path in paths {
            if self.fatal.is_shutdown_requested() {
                break;
            }
            match crate::pack_admission::admit(&path, &settings.suspend_codes) {
                crate::pack_admission::Admission::Admitted { package_id } => {
                    self.pack_admission.learn(&path, &package_id)?;
                }
                crate::pack_admission::Admission::Suspend { package_id, code } => {
                    self.pack_admission.suspend(&path, &package_id, code)?;
                }
                crate::pack_admission::Admission::Refused => {}
            }
        }
        Ok(())
    }

    /// Drops a decision's resolution once its execution record is written.
    pub(super) fn forget_scheduled_resolution(&self, decision_id: &str) -> RuntimeHostResult<()> {
        lock(
            &self.scheduled_resolutions,
            HostLocation::ForgetScheduledResolution,
        )?
        .remove(decision_id);
        Ok(())
    }

    /// The policy execution input of a failed scheduled run (§12.4): the failure identity of a
    /// run whose main package was admitted (`linear_steps` or, Workflow #390 EXEC-3, page
    /// graph), or the original input for any other run. A fatal error is marked.
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
        let (outcome, class) = match input {
            PolicyExecutionInput::Failed { outcome, class, .. } => (outcome, class),
            PolicyExecutionInput::Succeeded => return Ok(PolicyExecutionInput::Succeeded),
        };
        let code = scheduled_failure_code_str(&outcome);
        // Workflow #373 NAV-5 (M-373 §8.5): the package a navigation package refusal names.
        let invalid_package = (code.as_str()
            == HostCode::ContainedTaskNavigationPackageInvalid.as_str())
        .then(|| {
            outcome
                .value("package_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .flatten();
        let decision_id = context.decision_id();
        let (recorded, previous) = {
            let policy = lock(self.policy()?, HostLocation::ReadPolicyFailureIdentity)?;
            (
                policy.recorded_execution(decision_id).cloned(),
                policy
                    .latest_failure(context.catalog_task_id(), context.instance_alias())
                    .and_then(|(identity, decision)| {
                        identity.map(|identity| (identity.to_owned(), decision.to_owned()))
                    }),
            )
        };
        // §12.6 point 4: a dispatch that already has its execution record keeps its failure.
        if let Some(recorded) = recorded {
            return recorded_failure_input(recorded, outcome, class);
        }
        // Workflow #378 A2-P: the failure's outcome is recorded as it is; only the identity
        // beside it is built here.
        let failed = |failure_identity: Option<String>| PolicyExecutionInput::Failed {
            outcome,
            failure_identity,
            class,
        };
        let resolution = lock(
            &self.scheduled_resolutions,
            HostLocation::ReadScheduledResolution,
        )?
        .get(decision_id)
        .cloned();
        let Some(resolution) = resolution else {
            // An internal miss records no identity and says so.
            self.record_scheduled_policy_diagnostic(
                context,
                &RuntimeHostError::request(
                    HostCode::PolicyFailureIdentityUnavailable,
                    HostLocation::BuildFailureIdentity,
                    RuntimeErrorCode::RuntimeUnavailable,
                )
                .with_native_detail(format!("decision_id={decision_id}")),
            )?;
            return Ok(failed(None));
        };
        // A main package that was not admitted (its mode is unknown) records no identity.
        // Workflow #390 EXEC-3 (FCM E-02): a page-graph package records one as a `linear_steps`
        // package does, so its failure is rerun at once (R22) by the same rules.
        let Some(main) = resolution.main else {
            return Ok(failed(None));
        };
        let query = |query: EventQuery| {
            self.ledger
                .query(query)
                .map_err(|_| ledger_error(HostLocation::ReadFailureIdentityRows))
        };
        let (rows, scope, operation) = if main.resolved {
            let rows = failure_rows(context.run_id(), query)?;
            // A page-graph run's entry check is its gate, and its entry recovery package its
            // layer (EXEC-3).
            let gated = !main.layers.is_empty() || (!main.linear && entry_checked(&rows));
            let (scope, operation) = failure_step(&rows, gated);
            (rows, scope, operation)
        } else {
            (Vec::new(), "prepare".to_owned(), "prepare".to_owned())
        };
        let restart_segment = scope == "main" && in_restart_segment(&rows, &main.game, main.linear);
        // R25-2: an adb backend failure of a linear task that did not poison the run is rerun
        // only. Its class is already `Recoverable`: the terminal's `failure_severe` folds the
        // rule in where the failure arose, so the startup recovery and the ledger read the same
        // class (Workflow #378 A3 ruling 9).
        let rerun_only_backend = main.linear
            && RERUN_ONLY_BACKEND_FAILURES.contains(&code.as_str())
            && !failure.poison_runtime;
        let detail = failure_detail_fingerprint(
            failure
                .error
                .diagnostics()
                .native_detail()
                .map(|detail| detail.text()),
        );
        let (main_package, mut layers) = settled_packages(&main);
        // Workflow #373 NAV-5 (M-373 §8.5): a navigation or shared interrupt failure, or one in a
        // navigation, recovery chain or interrupt layer, also names that layer's package (`n`)
        // and the game root's data files (`g`), so a fix of either lifts its pause.
        let navigation_layer = navigation_layer_failure(&rows).or(invalid_package);
        if navigation_failure_code(&code) || navigation_layer.is_some() {
            layers.extend(self.settled_navigation(&main.game, navigation_layer.as_deref()));
        }
        let mut identity = FailureIdentity {
            base: encode_failure_base(&code),
            key: failure_key(&code, &scope, &operation, &detail),
            main: main_package,
            layers,
            frame: IdentityFrame::unique(decision_id),
        };
        if rerun_only_backend || !failure_accumulates(&code, &scope, restart_segment) {
            return Ok(failed(Some(identity.encode())));
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
        let Some((previous_identity, previous_decision)) = previous else {
            return Ok(failed(Some(identity.encode())));
        };
        let (comparison, previous_run) =
            self.compare_error_frames(&previous_decision, current.as_ref())?;
        let failure_identity = match comparison {
            FrameComparison::Different => identity.encode(),
            FrameComparison::Similar => previous_identity,
            // A comparison that cannot be made counts as the same problem: the prefix already
            // matched. Only the main scope has frames to miss (§12.4).
            FrameComparison::Unavailable(reason) => {
                if scope == "main" {
                    self.record_scheduled_policy_diagnostic(
                        context,
                        &RuntimeHostError::request(
                            HostCode::PolicyFailureFrameCompareUnavailable,
                            HostLocation::CompareFailureFrames,
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
                previous_identity
            }
        };
        Ok(failed(Some(failure_identity)))
    }

    /// Workflow #373 NAV-5 (M-373 §8.5): the navigation segments of a settled failure's
    /// identity: the layer package `package_id` (measured at the path the registry of `game`
    /// gives it now, `x` when it gives none or it cannot be measured) and the hash of the game
    /// root's data files (`x` when they cannot be read).
    fn settled_navigation(&self, game: &str, package_id: Option<&str>) -> Vec<IdentityLayer> {
        let mut layers = Vec::new();
        if let Some(package_id) = package_id {
            let hash = self
                .peek_game_registry(game)
                .ok()
                .and_then(|registry| registry.package_path(package_id).ok())
                .and_then(|path| measure_package_hash(&path.to_string_lossy()).ok());
            layers.push(IdentityLayer::Navigation(IdentityPackage::measured(
                package_id,
                hash.as_deref(),
            )));
        }
        let root_hash = self
            .standard_packs
            .root()
            .and_then(|root| standard_pack_root_hash(&root.join(game)));
        layers.push(IdentityLayer::StandardPackRoot(
            root_hash.unwrap_or_else(|| MISSING_HASH.to_owned()),
        ));
        layers
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
            .map_err(|_| ledger_error(HostLocation::CompareFailureFrames))?;
        let previous_run = policy_dispatch_intent(
            &self.ledger,
            previous_decision,
            through,
            HostLocation::CompareFailureFrames,
        )?
        .links()
        .run_id()
        .copied();
        let previous = match previous_run {
            Some(run_id) => locate_error_frame(run_id, |query: EventQuery| {
                self.ledger
                    .query(query)
                    .map_err(|_| ledger_error(HostLocation::CompareFailureFrames))
            })?,
            None => None,
        };
        let (Some(previous), Some(current)) = (previous, current) else {
            return Ok((FrameComparison::Unavailable("frame_missing"), previous_run));
        };
        let comparison = compare_frame_files(
            self.artifacts.root(),
            &previous.project(true),
            &current.project(true),
        );
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
                error.record_outcome(),
                DiagnosticDetailDraft::new(
                    "policy_driver",
                    RuntimeLifecycleFailureStage::PolicyDriver.location(),
                    "runtime_host",
                    error.operation(),
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

/// Workflow #336 PKG-S3 (M-PKG §3.3, D18): the packages of a settled failure's identity: the
/// main package and each layer the run resolved, with the content hash measured now at the path
/// the run used (the registry's, for a layer; Workflow #373 NAV-3a). A package that cannot be
/// measured (removed, being replaced, unreadable), or a layer the registry did not bind, is `x`,
/// and the failure is recorded as usual.
fn settled_packages(main: &ScheduledMain) -> (IdentityPackage, Vec<IdentityLayer>) {
    let measured = |path: Option<&str>| path.and_then(|path| measure_package_hash(path).ok());
    let main_package = IdentityPackage::measured(
        &main.package_id,
        measured(Some(main.package_path.as_str())).as_deref(),
    );
    let layers = main
        .layers
        .iter()
        .map(|layer| {
            IdentityLayer::measured(
                &layer.package_id,
                layer
                    .return_home
                    .as_ref()
                    .map(|(game, server)| (game.as_str(), server.as_str())),
                measured(
                    layer
                        .package_path
                        .as_deref()
                        .map(|path| path.to_string_lossy())
                        .as_deref(),
                )
                .as_deref(),
            )
        })
        .collect();
    (main_package, layers)
}

/// Reads both error frames, a frame the cleaner moved into `kept\` included (Workflow #375 R5d),
/// and compares them.
fn compare_frame_files(
    root: &std::path::Path,
    previous: &actingcommand_contract::ProjectedArtifactReference,
    current: &actingcommand_contract::ProjectedArtifactReference,
) -> FrameComparison {
    let (Ok(previous), Ok(current)) = (
        read_projected_verified(root, previous),
        read_projected_verified(root, current),
    ) else {
        return FrameComparison::Unavailable("artifact_unreadable");
    };
    match compare_failure_frames(&previous, &current) {
        Ok(compared) if compared.verdict == FailureFrameVerdict::Similar => {
            FrameComparison::Similar
        }
        Ok(_) => FrameComparison::Different,
        Err(error) => FrameComparison::Unavailable(error.reason().as_str()),
    }
}

/// §12.6 point 4: the recorded execution's own input, after its failure is checked against the
/// terminal's `outcome` (the same code, and an identity whose base segment is that code's); any
/// other record is left to the replay, which refuses it.
fn recorded_failure_input(
    recorded: PolicyExecutionEventData,
    outcome: Outcome,
    class: PolicyFailureClass,
) -> RuntimeHostResult<PolicyExecutionInput> {
    let PolicyExecutionOutcome::Failed { failure } = recorded.outcome else {
        return Ok(PolicyExecutionInput::Failed {
            outcome,
            failure_identity: None,
            class,
        });
    };
    if failure.reported_success {
        return Ok(PolicyExecutionInput::Failed {
            outcome,
            failure_identity: None,
            class,
        });
    }
    let code = scheduled_failure_code_str(&outcome);
    let consistent = scheduled_failure_code_str(&failure.outcome) == code
        && failure.failure_identity.as_deref().is_none_or(|identity| {
            FailureIdentity::parse(identity)
                .is_some_and(|parsed| parsed.base == encode_failure_base(&code))
        });
    if !consistent {
        return Err(RuntimeHostError::fatal(
            HostCode::PolicyExecutionIdentityConflict,
            HostLocation::ReplayPolicyFailureIdentity,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    Ok(PolicyExecutionInput::Failed {
        outcome: failure.outcome,
        failure_identity: failure.failure_identity,
        class: failure.original_class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        ArtifactKind, ArtifactProducer, ArtifactRedactionState, IdentifierIssuer,
        ProjectedArtifactReference, RetentionClass,
    };
    use sha2::{Digest, Sha256};
    use std::path::Path;

    /// A capture frame published at its object key, as the store writes one.
    fn frame(root: &Path, png: &[u8]) -> ProjectedArtifactReference {
        let ids = IdentifierIssuer::new().expect("issuer");
        let artifact_id = *ids.mint_artifact_id().expect("artifact").transport();
        let sha256 = format!("sha256:{:x}", Sha256::digest(png));
        let object_key = format!(
            "artifacts/{}/{}.png",
            &sha256[7..9],
            identifier_text(&artifact_id)
        );
        let path = root.join(object_key.as_str());
        std::fs::create_dir_all(path.parent().expect("shard")).expect("shard folder");
        std::fs::write(path, png).expect("frame file");
        let reference = ProjectedArtifactReference {
            artifact_id,
            kind: ArtifactKind::CaptureFrame,
            run_id: None,
            frame_id: Some(*ids.mint_frame_id().expect("frame").transport()),
            correlation_id: None,
            object_key: Some(object_key),
            media_type: ArtifactKind::CaptureFrame.media_type(),
            byte_count: png.len() as u64,
            sha256,
            created_at_unix_ms: 1_791_753_302_117,
            producer: ArtifactProducer::CaptureStore,
            retention_class: RetentionClass::Adaptive,
            redaction_state: ArtifactRedactionState::NotRequired,
        };
        reference.validate().expect("valid reference");
        reference
    }

    // A package file the content hash reads as an assembled package (it holds
    // `resources/manifest.json`), so its hash is the SHA-256 of the whole file (M-PKG D3).
    fn assembled_package(path: &Path, marker: &str) -> String {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("resources/manifest.json", options)
            .expect("manifest entry");
        zip.write_all(br#"{"schema_version":"0.3","entry_task_id":"task"}"#)
            .expect("manifest content");
        zip.start_file("control.json", options)
            .expect("control entry");
        zip.write_all(marker.as_bytes()).expect("control content");
        let bytes = zip.finish().expect("finish package").into_inner();
        std::fs::write(path, &bytes).expect("write package");
        format!("{:x}", Sha256::digest(&bytes))
    }

    fn hash12(value: &str) -> String {
        value.chars().take(12).collect()
    }

    fn id8(value: &str) -> String {
        format!("{:x}", Sha256::digest(value.as_bytes()))
            .chars()
            .take(8)
            .collect()
    }

    // Workflow #336 PKG-S3, M-PKG §4.4 S3-T2 and S3-T8 (settlement half): a settled failure's
    // identity names the main package and every layer its run went through by the id hash of
    // its `package_id` and the first twelve hex digits of the content hash measured at the
    // settlement; it reads back from its v2 string. A package that cannot be read then, and a
    // layer the mapping does not name, is `x`.
    #[test]
    fn s3_t2_a_settled_identity_names_each_package_by_its_content_hash() {
        let temp = tempfile::tempdir().expect("root");
        let main_path = temp.path().join("main.zip");
        let layer_path = temp.path().join("layer.zip");
        let home_path = temp.path().join("home.zip");
        let main_hash = assembled_package(&main_path, "main");
        let layer_hash = assembled_package(&layer_path, "layer");
        let home_hash = assembled_package(&home_path, "home");
        // Workflow #373 NAV-3a: each layer at the path the run's registry bound it to.
        let mut main = ScheduledMain {
            linear: true,
            package_id: "neutral.main".to_owned(),
            package_path: main_path.to_string_lossy().into_owned(),
            game: "neutral".to_owned(),
            layers: vec![
                ResolvedLayer {
                    package_id: "neutral.layer".to_owned(),
                    return_home: None,
                    package_path: Some(layer_path.clone()),
                },
                ResolvedLayer {
                    package_id: "neutral.home".to_owned(),
                    return_home: Some(("neutral".to_owned(), "test".to_owned())),
                    package_path: Some(home_path.clone()),
                },
            ],
            resolved: true,
        };
        let (main_package, layers) = settled_packages(&main);
        assert_eq!(
            main_package,
            IdentityPackage {
                id_hash: id8("neutral.main"),
                hash: hash12(&main_hash),
            }
        );
        assert_eq!(
            layers,
            vec![
                IdentityLayer::Declared(IdentityPackage {
                    id_hash: id8("neutral.layer"),
                    hash: hash12(&layer_hash),
                }),
                IdentityLayer::ReturnHome {
                    key_hash: id8("neutral\0test"),
                    package: IdentityPackage {
                        id_hash: id8("neutral.home"),
                        hash: hash12(&home_hash),
                    },
                },
            ]
        );
        let identity = FailureIdentity {
            base: "page_confirmation_failed".to_owned(),
            key: "0".repeat(12),
            main: main_package,
            layers,
            frame: IdentityFrame::None,
        };
        let encoded = identity.encode();
        assert_eq!(
            encoded,
            format!(
                "page_confirmation_failed~v2~k{}~m{}{}~p{}{}~r{}{}{}~fna",
                "0".repeat(12),
                id8("neutral.main"),
                hash12(&main_hash),
                id8("neutral.layer"),
                hash12(&layer_hash),
                id8("neutral\0test"),
                id8("neutral.home"),
                hash12(&home_hash),
            )
        );
        assert_eq!(FailureIdentity::parse(&encoded), Some(identity));

        // S3-T8: the main package is gone and the home layer is one the registry did not bind;
        // both are `x`, and the identity still reads back.
        std::fs::remove_file(&main_path).expect("remove the main package");
        main.layers[1].package_path = None;
        let (main_package, layers) = settled_packages(&main);
        assert_eq!(main_package.hash, "x");
        assert_eq!(layers[0].hash(), hash12(&layer_hash));
        assert_eq!(layers[1].hash(), "x");
        let unmeasured = FailureIdentity {
            base: "page_confirmation_failed".to_owned(),
            key: "0".repeat(12),
            main: main_package,
            layers,
            frame: IdentityFrame::None,
        };
        assert_eq!(
            FailureIdentity::parse(&unmeasured.encode()),
            Some(unmeasured)
        );
    }

    /// Workflow #375 R5d: the previous failure's error frame, moved by the cleaner into its
    /// kept folder, is still read and compared, not counted as unreadable.
    #[test]
    fn a_moved_previous_error_frame_is_compared() {
        let temp = tempfile::tempdir().expect("root");
        let root = temp.path().canonicalize().expect("canonical root");
        let pixels = vec![40_u8; 64 * 36 * 3];
        let png = actingcommand_device::encode_png_fast(
            64,
            36,
            &pixels,
            actingcommand_device::PixelFormat::Rgb8,
        )
        .expect("frame PNG");
        let previous = frame(&root, &png);
        let current = frame(&root, &png);
        let source = root.join(previous.object_key().expect("object key"));
        let leaf = root
            .join("kept")
            .join("2026-10-12")
            .join("node_a-151234-contained_task_page_unknown");
        std::fs::create_dir_all(&leaf).expect("kept leaf");
        std::fs::rename(
            &source,
            leaf.join(format!(
                "061502-117_{}.png",
                identifier_text(&previous.artifact_id)
            )),
        )
        .expect("move the frame as the cleaner does");
        assert!(matches!(
            compare_frame_files(&root, &previous, &current),
            FrameComparison::Similar
        ));
    }
}
