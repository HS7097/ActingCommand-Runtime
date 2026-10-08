// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #375: frames that are missing on disk. R5a: no opening of a formal root reads
//! artifact material; a frame deleted by hand restores `Unrecorded`, a `Failed` eviction
//! outcome restores `FailedEviction`, and `ledger-maintenance` refuses only an eviction
//! intent without its outcome. A real formal SQLite root with its State tables and its
//! ArtifactStore in the same directory.

use crate::global::tests::sealed_global_ledger::GlobalLedgerSink;
use crate::global::{
    ArtifactEvictionAdmission, ArtifactEvictionPermit, GlobalLedger, LedgerMaintenance,
    LedgerOpenTiming, Sha256SecretFingerprinter, WriterCommand, receive_response, send_command,
};
use crate::{ArtifactAvailability, PersistedEvent};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest, try_artifact_delete_guard,
};
use actingcommand_contract::{
    ArtifactEvictionDisposition, ArtifactEvictionIo, ArtifactId, ArtifactIssuePolicy, ArtifactKind,
    ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactPinReason, ArtifactPinRecord,
    ArtifactProducer, ArtifactRedactionState, ArtifactRetentionFact, ArtifactRetentionIdentity,
    AuditInput, CapturePayloadDraft, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, EventType,
    FRAME_RETENTION_POLICY_VERSION, FailedRunRetentionPolicy, IdentifierIssuer, IssuedInstanceId,
    OriginModule, OwnerEpoch, OwnerResourceDisposition, ProjectedArtifactReference,
    ResourceQuiescence, RetentionClass, RuntimeLifecyclePhase, RuntimePayloadDraft,
    SanitizedEventDraft, TerminalEvent,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_host::{
    LedgerMaintenanceOperation, LedgerMaintenanceRequest, LedgerMaintenanceRestoredMaterial,
    RuntimeHost, RuntimeHostConfig,
};
use actingcommand_runtime_state::RuntimeStateStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const STATE_KEY: &[u8] = b"runtime-host-test-salt";

fn fingerprinter() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"material-not-read-fixture").expect("fingerprinter")
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("timestamp")
}

fn terminal(event: &PersistedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    }
}

fn state_database(root: &Path) -> Arc<RuntimeDatabase> {
    Arc::new(RuntimeStateStore::open_database(root, STATE_KEY).expect("State database"))
}

/// A formal root: State tables, the formal ledger marker and the writer, as acsetup leaves it.
fn formal_root(root: &Path) -> (Arc<RuntimeDatabase>, GlobalLedger) {
    let database = state_database(root);
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(root, true, limits, limits.deadline().expect("deadline"))
            .expect("maintenance owner");
    maintenance
        .initialize_empty(&database)
        .expect("formal ledger");
    let ledger = maintenance
        .open_writer(Arc::clone(&database), "material-fixture".into())
        .expect("formal writer");
    (database, ledger)
}

fn reopen(database: &Arc<RuntimeDatabase>, timing: &mut LedgerOpenTiming) -> GlobalLedger {
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(Arc::clone(database), "material-reopen".into(), timing)
    .expect("the writer opens without reading material")
}

fn material_store(root: &Path) -> ArtifactStore {
    let artifacts = ArtifactStore::open(root).expect("artifact owner");
    let mut admission = GlobalLedgerSink::new(None);
    admission.capacity_root = Some(artifacts.root().to_path_buf());
    artifacts
        .install_capacity_admission(Arc::new(admission))
        .expect("fixture capacity owner");
    artifacts
}

fn maintain(
    root: &Path,
    operation: LedgerMaintenanceOperation,
    backup: Option<PathBuf>,
    target: Option<PathBuf>,
) -> Result<
    actingcommand_runtime_host::LedgerMaintenanceReceipt,
    actingcommand_runtime_host::LedgerMaintenanceFailure,
> {
    RuntimeHost::maintain_ledger(
        RuntimeHostConfig::new(root, STATE_KEY),
        LedgerMaintenanceRequest {
            operation,
            backup,
            target,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
        },
    )
}

struct Sink<'a> {
    ledger: &'a GlobalLedger,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Sink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft.sanitize(&fingerprinter()).map_err(|error| {
            ArtifactStoreError::fatal("fixture_sanitize_failed", "fixture_sink", error.to_string())
        })?;
        let event = self.ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "fixture_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

/// One frame captured with no run and no lease in its own owner epoch, which a quiescence
/// records before the frame, closed by a later confirmed quiescence of its instance (the shape
/// of an actingctl readonly observe).
struct Capture {
    reference: ProjectedArtifactReference,
    material: PathBuf,
}

fn append(ledger: &GlobalLedger, draft: SanitizedEventDraft) -> PersistedEvent {
    ledger.append(draft).expect("fixture append")
}

fn quiescence(
    ledger: &GlobalLedger,
    ids: &IdentifierIssuer,
    owner: OwnerEpoch,
    instance: IssuedInstanceId,
) {
    let draft = EventDraft::new(
        ids.mint_event_id().expect("event"),
        now_ms(),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(ids.mint_request_id().expect("request"))
            .with_correlation_id(ids.mint_correlation_id().expect("correlation")),
        RuntimePayloadDraft::lifecycle_observed(
            owner,
            RuntimeLifecyclePhase::ResourceQuiescence {
                instance_id: *instance.transport(),
                resource_count: 1,
                quiescence: ResourceQuiescence::Confirmed,
                owner_disposition: OwnerResourceDisposition::ConfirmedClosed,
                resource_dispositions: None,
            },
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("quiescence draft");
    append(ledger, draft);
}

fn capture(ledger: &GlobalLedger, artifacts: &ArtifactStore, bytes: &[u8]) -> Capture {
    let ids = IdentifierIssuer::new().expect("issuer");
    let owner = *ids.mint_owner_epoch().expect("owner").transport();
    let instance = ids.mint_instance_id().expect("instance");
    // The owner epoch is recorded before the frame, so its pin's trigger is in that epoch.
    quiescence(ledger, &ids, owner, instance);
    let request = ids.mint_request_id().expect("request");
    let correlation = ids.mint_correlation_id().expect("correlation");
    let frame = ids.mint_frame_id().expect("frame");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(request)
        .with_correlation_id(correlation)
        .with_frame_id(frame);
    let mut sink = Sink {
        ledger,
        verified: None,
    };
    let stored = artifacts
        .put(
            ArtifactWriteRequest::new(
                ArtifactKind::CaptureFrame,
                bytes,
                ArtifactWriteContext::new(
                    ArtifactLinksDraft::default()
                        .with_frame_id(frame)
                        .with_correlation_id(correlation),
                    links.clone(),
                    now_ms(),
                ),
                ArtifactIssuePolicy::new(
                    ArtifactProducer::CaptureStore,
                    RetentionClass::DebugFull,
                    ArtifactRedactionState::Pending,
                ),
            ),
            &mut sink,
        )
        .expect("frame publication");
    let verified = sink.verified.take().expect("verified receipt");
    let reference = verified
        .artifacts()
        .iter()
        .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .expect("capture frame reference")
        .project(true);
    let pin = EventDraft::new(
        ids.mint_event_id().expect("event"),
        now_ms(),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::ArtifactStore,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        ArtifactPayloadDraft::retention(
            ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
                identity: ArtifactRetentionIdentity {
                    artifact: reference.clone(),
                    owner_epoch: owner,
                    instance_id: *instance.transport(),
                    request_id: *request.transport(),
                    correlation_id: *correlation.transport(),
                    run_id: None,
                    lease_id: None,
                    policy_version: FRAME_RETENTION_POLICY_VERSION,
                },
                reason: ArtifactPinReason::Explicit,
                trigger: terminal(&verified),
            }),
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("pin draft");
    let pin = verified
        .links()
        .artifact_retention_source()
        .apply_to(pin)
        .expect("pin source links");
    append(ledger, pin);
    let completed = EventDraft::new(
        ids.mint_event_id().expect("event"),
        now_ms(),
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Device,
            OriginModule::Capture,
            EventActor::Runtime,
        ),
        links,
        CapturePayloadDraft::completed(
            EventAction::CaptureObserve,
            EffectDisposition::Performed,
            1280,
            720,
            AuditInput::new(),
        )
        .into(),
    )
    .sanitize(&fingerprinter())
    .expect("capture completed draft");
    append(ledger, completed);
    // The close follows the pin, so the Explicit pin is releasable at admission.
    quiescence(ledger, &ids, owner, instance);
    Capture {
        reference,
        material: stored.path().to_path_buf(),
    }
}

fn admit(
    ledger: &GlobalLedger,
    artifacts: &ArtifactStore,
    capture: &Capture,
) -> Box<ArtifactEvictionPermit> {
    let guard = try_artifact_delete_guard(artifacts.root(), &capture.reference)
        .expect("delete guard")
        .expect("material guard is free");
    match ledger
        .admit_artifact_eviction(guard, FailedRunRetentionPolicy::default())
        .expect("admission")
    {
        ArtifactEvictionAdmission::Committed(permit) => permit,
        ArtifactEvictionAdmission::Deferred => panic!("the closed capture is admitted"),
    }
}

/// The writer's own finish command with a `Failed` disposition, as when the unlink fails;
/// the writer records the outcome and then stops.
fn finish_failed(ledger: &GlobalLedger, permit: Box<ArtifactEvictionPermit>) {
    let (response, receiver) = std::sync::mpsc::sync_channel(1);
    send_command(
        ledger.sender.as_ref().expect("writer"),
        WriterCommand::FinishArtifactEviction {
            permit,
            disposition: ArtifactEvictionDisposition::Failed,
            io: Some(ArtifactEvictionIo::from_io(&std::io::Error::from(
                std::io::ErrorKind::PermissionDenied,
            ))),
            response,
        },
        "finish_failed_fixture",
    )
    .expect("finish command");
    let finished = receive_response(receiver, "finish_failed_fixture").expect("finish reply");
    assert_eq!(
        finished
            .expect_err("a failed outcome stops the writer")
            .code(),
        "artifact_eviction_failed"
    );
}

/// Every reference of the artifact in the reopened history has `expected` availability.
fn assert_availability(
    events: &[PersistedEvent],
    artifact: ArtifactId,
    expected: fn(&ArtifactAvailability) -> bool,
) {
    let references = events
        .iter()
        .flat_map(PersistedEvent::artifacts)
        .filter(|reference| *reference.artifact_id() == artifact)
        .collect::<Vec<_>>();
    assert!(!references.is_empty(), "the artifact is referenced");
    assert!(
        references
            .iter()
            .all(|reference| expected(reference.availability())),
        "unexpected availability: {:?}",
        references
            .iter()
            .map(|reference| reference.availability())
            .collect::<Vec<_>>()
    );
}

#[test]
fn formal_openings_and_maintenance_read_no_frame() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let evicted = capture(&ledger, &artifacts, b"frame evicted with its proof");
    let permit = admit(&ledger, &artifacts, &evicted);
    ledger
        .finish_artifact_eviction(permit)
        .expect("recorded deletion");
    assert!(!evicted.material.exists());
    let deleted_by_hand = capture(&ledger, &artifacts, b"frame deleted by hand");
    let failed = capture(&ledger, &artifacts, b"frame whose eviction failed");
    let permit = admit(&ledger, &artifacts, &failed);
    finish_failed(&ledger, permit);
    ledger
        .close()
        .expect_err("the writer stopped after the failed outcome");
    std::fs::remove_file(&deleted_by_hand.material).expect("hand deletion");

    // The writer open: no frame is read, a missing one is Unrecorded, a failed eviction is
    // ledger state.
    let mut timing = LedgerOpenTiming::default();
    let ledger = reopen(&database, &mut timing);
    assert!(timing.events > 0);
    assert_eq!(
        (
            timing.artifacts,
            timing.artifact_bytes,
            timing.workers,
            timing.material_ms
        ),
        (0, 0, 0, 0)
    );
    let events = ledger.query(EventQuery::default()).expect("history");
    assert_availability(&events, evicted.reference.artifact_id, |availability| {
        matches!(availability, ArtifactAvailability::Evicted(_))
    });
    assert_availability(
        &events,
        deleted_by_hand.reference.artifact_id,
        |availability| matches!(availability, ArtifactAvailability::Unrecorded),
    );
    assert_availability(&events, failed.reference.artifact_id, |availability| {
        matches!(availability, ArtifactAvailability::FailedEviction(_))
    });
    ledger.close().expect("close the reopened writer");
    drop(database);

    // verify passes Unrecorded, Evicted and FailedEviction.
    let verified = maintain(root.path(), LedgerMaintenanceOperation::Verify, None, None)
        .expect("verify reads no frame");
    assert_eq!(verified.status, "verified-sqlite");

    // A backup that records an eviction restores; the copy skips the evicted and the absent
    // frame and copies the frame whose eviction failed.
    let external = tempfile::tempdir().expect("maintenance destinations");
    let backup = external.path().join("backup");
    let backed_up = maintain(
        root.path(),
        LedgerMaintenanceOperation::Backup,
        Some(backup.clone()),
        None,
    )
    .expect("backup");
    assert_eq!(backed_up.status, "backed-up");
    let target = external.path().join("restored");
    let restored = maintain(
        root.path(),
        LedgerMaintenanceOperation::Restore,
        Some(backup),
        Some(target.clone()),
    )
    .expect("restore of a backup with evictions");
    assert_eq!(restored.status, "restored");
    assert_eq!(
        restored.restored_material,
        Some(LedgerMaintenanceRestoredMaterial {
            copied: 1,
            evicted: 1,
            absent: 1,
        })
    );
    let failed_key = failed.reference.object_key().expect("object key");
    assert_eq!(
        std::fs::read(target.join(failed_key)).expect("copied frame"),
        std::fs::read(&failed.material).expect("source frame")
    );
    assert!(
        !target
            .join(deleted_by_hand.reference.object_key().expect("object key"))
            .exists()
    );
    let members = |receipt: &actingcommand_runtime_host::LedgerMaintenanceReceipt| {
        serde_json::to_value(receipt)
            .expect("receipt")
            .as_object()
            .expect("receipt object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        members(&restored),
        members(&backed_up),
        "the receipt schema is unchanged"
    );
}

#[test]
fn maintenance_verify_refuses_only_a_pending_eviction() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let pending = capture(&ledger, &artifacts, b"frame of an unfinished eviction");
    // A hard stop between the intent and the unlink leaves the intent without its outcome.
    drop(admit(&ledger, &artifacts, &pending));
    ledger.close().expect("close with a pending intent");
    drop(database);
    let refused = maintain(root.path(), LedgerMaintenanceOperation::Verify, None, None)
        .expect_err("a pending intent refuses verify");
    assert_eq!(refused.code, "maintenance_artifact_material_unavailable");
    assert!(
        refused.cause.starts_with("pending_evictions=1;"),
        "{}",
        refused.cause
    );
    assert!(pending.material.exists(), "nothing was read or removed");
}

/// Workflow #375 R5b: on an SQLite root a frame deleted by hand is listed once as
/// `material_missing`; the export and the stability report complete.
#[test]
fn forensic_reports_list_a_missing_frame_and_complete() {
    use actingcommand_ledger_forensics::{
        ForensicCommand, ForensicEventsRequest, ForensicOutput, ForensicReport, ForensicRequest,
        run,
    };
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let missing = capture(&ledger, &artifacts, b"frame deleted before the export");
    ledger.close().expect("close the writer");
    drop(database);
    std::fs::remove_file(&missing.material).expect("hand deletion");

    let ForensicOutput::Human(export) =
        run(ForensicRequest::new(root.path(), ForensicCommand::Export))
            .expect("the export completes")
    else {
        panic!("human export");
    };
    let listed = export
        .lines()
        .find_map(|line| line.strip_prefix("material_missing: "))
        .expect("material_missing line");
    assert_eq!(
        serde_json::from_str::<Vec<ProjectedArtifactReference>>(listed).expect("listed references"),
        vec![missing.reference.clone()]
    );

    let ForensicOutput::Machine(ForensicReport::Stability(report)) = run(
        ForensicRequest::stability(root.path(), ForensicEventsRequest::default()),
    )
    .expect("stability report") else {
        panic!("stability report");
    };
    assert_eq!(report.material_missing, vec![missing.reference]);
    assert!(report.failures.is_empty());
    assert!(report.gaps.is_empty());
}
