// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #336 L6 (§12.4-§12.6, R21, R22, R25; `contracts/policy-suspension.md`): the failure
//! identity a scheduled `linear_steps` run's live settlement writes as its
//! `policy.execution_recorded` `failure.failure_identity`, beside the failure's `outcome`
//! (Workflow #378 A2-P).
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

use super::contained_task::prepare_contained_task;
use super::policy_outcome::policy_dispatch_intent;
use super::*;
use crate::codes::{HostCode, HostLocation};
use crate::failure_identity::{
    FailureIdentity, IdentityFrame, IdentityLayer, IdentityPackage, RERUN_ONLY_BACKEND_FAILURES,
    encode_failure_base, failure_accumulates, failure_detail_fingerprint, failure_key,
    failure_rows, failure_step, identifier_text, in_restart_segment, locate_error_frame,
};
use crate::suspension_lift::{LiftCheckTime, SuspensionLiftView, measure_package_hash};
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
        Ok(SuspensionLiftView {
            manifest,
            prerequisite_packages: &self.prerequisite_packages,
            return_home_packages: &self.return_home_packages,
            checks: &self.lift_checks,
            time,
        })
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
        Ok((prepared, chain?))
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
        let (outcome, class) = match input {
            PolicyExecutionInput::Failed { outcome, class, .. } => (outcome, class),
            PolicyExecutionInput::Succeeded => return Ok(PolicyExecutionInput::Succeeded),
        };
        let code = scheduled_failure_code_str(&outcome);
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
        // A main package that was not admitted (its mode is unknown) or is a page-graph
        // package records no identity.
        let Some(main) = resolution.main.filter(|main| main.linear) else {
            return Ok(failed(None));
        };
        let query = |query: EventQuery| {
            self.ledger
                .query(query)
                .map_err(|_| ledger_error(HostLocation::ReadFailureIdentityRows))
        };
        let (rows, scope, operation) = if main.resolved {
            let rows = failure_rows(context.run_id(), query)?;
            let (scope, operation) = failure_step(&rows, !main.layers.is_empty());
            (rows, scope, operation)
        } else {
            (Vec::new(), "prepare".to_owned(), "prepare".to_owned())
        };
        let restart_segment = scope == "main" && in_restart_segment(&rows, &main.game);
        // R25-2: an adb backend failure of a linear task that did not poison the run is rerun
        // only. Its class is already `Recoverable`: the terminal's `failure_severe` folds the
        // rule in where the failure arose, so the startup recovery and the ledger read the same
        // class (Workflow #378 A3 ruling 9).
        let rerun_only_backend =
            RERUN_ONLY_BACKEND_FAILURES.contains(&code.as_str()) && !failure.poison_runtime;
        let detail = failure_detail_fingerprint(
            failure
                .error
                .diagnostics()
                .native_detail()
                .map(|detail| detail.text()),
        );
        let (main_package, layers) = settled_packages(&main, &self.prerequisite_packages);
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
/// the run used (the mapping's, for a layer). A package that cannot be measured (removed, being
/// replaced, unreadable), or a layer the mapping does not name, is `x`, and the failure is
/// recorded as usual.
fn settled_packages(
    main: &ScheduledMain,
    prerequisite_packages: &BTreeMap<String, ContainedTaskRecoveryBinding>,
) -> (IdentityPackage, Vec<IdentityLayer>) {
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
                    prerequisite_packages
                        .get(&layer.package_id)
                        .map(ContainedTaskRecoveryBinding::package_path),
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
        let mapping = BTreeMap::from([
            (
                "neutral.layer".to_owned(),
                ContainedTaskRecoveryBinding::new(layer_path.to_string_lossy())
                    .expect("layer binding"),
            ),
            (
                "neutral.home".to_owned(),
                ContainedTaskRecoveryBinding::new(home_path.to_string_lossy())
                    .expect("home binding"),
            ),
        ]);
        let main = ScheduledMain {
            linear: true,
            package_id: "neutral.main".to_owned(),
            package_path: main_path.to_string_lossy().into_owned(),
            game: "neutral".to_owned(),
            layers: vec![
                ResolvedLayer {
                    package_id: "neutral.layer".to_owned(),
                    return_home: None,
                },
                ResolvedLayer {
                    package_id: "neutral.home".to_owned(),
                    return_home: Some(("neutral".to_owned(), "test".to_owned())),
                },
            ],
            resolved: true,
        };
        let (main_package, layers) = settled_packages(&main, &mapping);
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

        // S3-T8: the main package is gone and the home layer is no longer mapped at the
        // settlement; both are `x`, and the identity still reads back.
        std::fs::remove_file(&main_path).expect("remove the main package");
        let mapping =
            BTreeMap::from([("neutral.layer".to_owned(), mapping["neutral.layer"].clone())]);
        let (main_package, layers) = settled_packages(&main, &mapping);
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
