// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 §3.6 H-5 G13 (inventory-host key 39; Workflow #382 H-5 G13): one Warning
//! with no run, request or correlation link must not stop the frame cleaner. Alice 10-08
//! cancelled the 10-02 latch (Workflow #375 R5d): once the cleaner's clock passes 1 day after
//! the frames' entry time, the due frame files are removed and the ledger holds no eviction
//! record (`contracts/ledger-store.md`, "Frame cleaner"). A formal writer ledger and its
//! artifact store share one temporary state root; every timestamp is a constant.

use super::super::{FrameRetention, FrameRetentionWarning};
use actingcommand_artifact_store::{
    ArtifactCapacityAdmission, ArtifactEventSink, ArtifactStore, ArtifactStoreError,
    ArtifactStoreResult, ArtifactWriteContext, ArtifactWriteRequest, KEPT_DIRECTORY,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactPinReason,
    ArtifactPinRecord, ArtifactProducer, ArtifactRedactionState, ArtifactRetentionFact,
    ArtifactRetentionIdentity, AuditInput, CapacityAdmissionOutcome, CapacityAdmissionReason,
    CapacityDecision, CapacityFactReference, CapturePayloadDraft, DiagnosticCode,
    DiagnosticDetailDraft, EffectDisposition, EventAction, EventActor, EventDraft, EventId,
    EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, EventType,
    FRAME_RETENTION_POLICY_VERSION, IdentifierIssuer, IssuedEventId, IssuedInstanceId,
    OriginModule, OwnerEpoch, OwnerResourceDisposition, ProjectedArtifactReference,
    ResourceQuiescence, RetentionClass, RuntimeLifecyclePhase, RuntimePayloadDraft, TerminalEvent,
};
use actingcommand_ledger::{
    FrameRetentionClass, FrameRetentionSwitches, GlobalLedger, GlobalLedgerError,
    LedgerMaintenance, PersistedEvent, Sha256SecretFingerprinter,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DAY_MS: u64 = 86_400_000;
const MINUTE_MS: u64 = 60_000;
/// The capture times of the three run-less frames.
const CAPTURED: [u64; 3] = [1_791_753_302_117, 1_791_753_303_117, 1_791_753_304_117];
/// The owner epoch is recorded before the first frame.
const OWNER_RECORDED: u64 = CAPTURED[0] - 1_000;
/// The unlinked Warning follows the last frame.
const WARNED: u64 = CAPTURED[2] + 1_000;
/// The cleaner's manual clock: past the 1-day boundary of every frame (the latest capture,
/// plus 1 day and 1 minute), so every frame's due time lies before it.
const NOW: u64 = CAPTURED[2] + DAY_MS + MINUTE_MS;

/// A formal writer ledger in `root`, as acsetup leaves it.
fn formal_writer(root: &Path) -> GlobalLedger {
    let database = Arc::new(
        RuntimeDatabase::open_with_initializer::<GlobalLedgerError>(
            root,
            b"gate-h5g13-state-seed",
            |_| Ok(()),
            |_| Ok(()),
        )
        .expect("state database"),
    );
    let limits = MaintenanceLimits::default();
    let maintenance =
        LedgerMaintenance::acquire(root, true, limits, limits.deadline().expect("deadline"))
            .expect("maintenance owner");
    maintenance
        .initialize_empty(&database)
        .expect("formal ledger");
    maintenance
        .open_writer(database, "gate-h5g13-writer".into())
        .expect("formal writer")
}

/// The store's capacity owner: every write is allowed by one committed capacity fact.
struct Capacity {
    owner: OwnerEpoch,
    fact: EventId,
}

impl ArtifactCapacityAdmission for Capacity {
    fn decide(&self, _path: &Path, bytes: u64) -> ArtifactStoreResult<CapacityDecision> {
        Ok(CapacityDecision {
            owner_epoch: self.owner,
            decided_at_unix_ms: 1,
            decided_at_monotonic_ms: 0,
            requested_bytes: bytes,
            target_volume: Some("volume-a".to_owned()),
            admission_result: CapacityAdmissionOutcome::Allowed,
            reason: CapacityAdmissionReason::FreshSample,
            fact: Some(CapacityFactReference {
                event_id: self.fact,
                sequence: 1,
                owner_epoch: self.owner,
                observed_at_unix_ms: 1,
                observed_at_monotonic_ms: 0,
            }),
        })
    }
}

/// The artifact store of the state root `root`, with its capacity owner.
fn artifact_store(root: &Path, ids: &IdentifierIssuer, owner: OwnerEpoch) -> ArtifactStore {
    let artifacts = ArtifactStore::open(root).expect("artifact store");
    artifacts
        .install_capacity_admission(Arc::new(Capacity {
            owner,
            fact: *ids.mint_event_id().expect("capacity fact").transport(),
        }))
        .expect("capacity owner");
    artifacts
}

/// Appends the store's events to the writer and keeps the frame's `artifact.verified`.
struct Sink<'a> {
    ledger: &'a GlobalLedger,
    fingerprinter: &'a Sha256SecretFingerprinter,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Sink<'_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft.sanitize(self.fingerprinter).map_err(|error| {
            ArtifactStoreError::fatal("fixture_sanitize_failed", "gate_sink", error.to_string())
        })?;
        let event = self.ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "gate_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

/// One fixture frame: its reference, the file at its object key and its capture time.
struct Frame {
    reference: ProjectedArtifactReference,
    material: PathBuf,
    captured: u64,
}

/// The writer, the fingerprinter and the identities every fixture event uses.
struct Fixture<'a> {
    ledger: &'a GlobalLedger,
    fingerprinter: &'a Sha256SecretFingerprinter,
    ids: &'a IdentifierIssuer,
    owner: OwnerEpoch,
    instance: IssuedInstanceId,
}

impl Fixture<'_> {
    fn append(&self, draft: EventDraft) -> PersistedEvent {
        let draft = draft.sanitize(self.fingerprinter).expect("fixture draft");
        self.ledger.append(draft).expect("fixture append")
    }

    fn event_id(&self) -> IssuedEventId {
        self.ids.mint_event_id().expect("event")
    }

    /// Records the owner epoch: a confirmed resource quiescence of the instance.
    fn record_owner(&self) {
        self.append(EventDraft::new(
            self.event_id(),
            OWNER_RECORDED,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default()
                .with_instance_id(self.instance)
                .with_request_id(self.ids.mint_request_id().expect("request"))
                .with_correlation_id(self.ids.mint_correlation_id().expect("correlation")),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner,
                RuntimeLifecyclePhase::ResourceQuiescence {
                    instance_id: *self.instance.transport(),
                    resource_count: 1,
                    quiescence: ResourceQuiescence::Confirmed,
                    owner_disposition: OwnerResourceDisposition::ConfirmedClosed,
                    resource_dispositions: None,
                },
                AuditInput::new(),
            )
            .into(),
        ));
    }

    /// One run-less observe frame captured at `captured`: published through the store, pinned
    /// with its `Explicit` identity, and closed by its `capture.completed`.
    fn observe(&self, artifacts: &ArtifactStore, captured: u64, bytes: &[u8]) -> Frame {
        let request = self.ids.mint_request_id().expect("request");
        let correlation = self.ids.mint_correlation_id().expect("correlation");
        let frame = self.ids.mint_frame_id().expect("frame");
        let links = EventLinksDraft::default()
            .with_instance_id(self.instance)
            .with_request_id(request)
            .with_correlation_id(correlation)
            .with_frame_id(frame);
        let mut sink = Sink {
            ledger: self.ledger,
            fingerprinter: self.fingerprinter,
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
                        captured,
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
        // `put` has released the frame's use lock: the cleaner leaves a held frame as busy.
        let material = stored.path().to_path_buf();
        let verified = sink.verified.take().expect("artifact.verified");
        let reference = verified
            .artifacts()
            .iter()
            .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
            .expect("capture frame reference")
            .project(true);
        let pin = EventDraft::new(
            self.event_id(),
            captured,
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
                        owner_epoch: self.owner,
                        instance_id: *self.instance.transport(),
                        request_id: *request.transport(),
                        correlation_id: *correlation.transport(),
                        run_id: None,
                        lease_id: None,
                        policy_version: FRAME_RETENTION_POLICY_VERSION,
                    },
                    reason: ArtifactPinReason::Explicit,
                    trigger: TerminalEvent {
                        sequence: verified.sequence(),
                        event_id: *verified.event_id(),
                    },
                }),
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(self.fingerprinter)
        .expect("pin draft");
        let pin = verified
            .links()
            .artifact_retention_source()
            .apply_to(pin)
            .expect("pin source links");
        self.ledger.append(pin).expect("pin append");
        self.append(EventDraft::new(
            self.event_id(),
            captured,
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
        ));
        Frame {
            reference,
            material,
            captured,
        }
    }

    /// The 10-02 shape at the row's severity: a Warning `runtime.failed` with no links at all.
    fn unlinked_warning(&self) -> PersistedEvent {
        self.append(EventDraft::new(
            self.event_id(),
            WARNED,
            EventSeverity::Warning,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            // Ledger revision 3: the Warning is the record's outcome (a warning-category code
            // with its required frame); a detail has no message or sensitivity.
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                actingcommand_contract::outcome::Outcome::new(
                    crate::codes::HostCode::FrameRetentionRemoveFailed,
                )
                .operation(crate::codes::HostLocation::MaintainFrameRetention)
                .artifact_id("artifact_00000000000000000000000000000001"),
                DiagnosticDetailDraft::new(
                    "frame_retention",
                    "frame_retention.remove",
                    "artifact_store",
                    "maintain_frame_retention",
                ),
                AuditInput::new(),
            )
            .into(),
        ))
    }
}

#[test]
fn gate_an_unlinked_warning_does_not_stop_the_cleaner_removing_due_frames() {
    let temp = tempfile::tempdir().expect("state root");
    let ledger = formal_writer(temp.path());
    let fingerprinter =
        Sha256SecretFingerprinter::new(b"gate-h5g13-fixture").expect("fingerprinter");
    let ids = IdentifierIssuer::new().expect("issuer");
    let owner = *ids.mint_owner_epoch().expect("owner").transport();
    let fixture = Fixture {
        ledger: &ledger,
        fingerprinter: &fingerprinter,
        ids: &ids,
        owner,
        instance: ids.mint_instance_id().expect("instance"),
    };
    let artifacts = artifact_store(temp.path(), &ids, owner);
    fixture.record_owner();
    let frames = [
        fixture.observe(&artifacts, CAPTURED[0], b"frame a"),
        fixture.observe(&artifacts, CAPTURED[1], b"frame b"),
        fixture.observe(&artifacts, CAPTURED[2], b"frame c"),
    ];
    let switches = FrameRetentionSwitches::default();

    // Before the Warning: every frame is a settled Default frame, due 1 day after its entry.
    let view = ledger
        .frame_retention_view(NOW, switches)
        .expect("the frame view before the Warning");
    for frame in &frames {
        let listed = view
            .frames
            .iter()
            .find(|listed| listed.reference.artifact_id == frame.reference.artifact_id)
            .expect("every fixture frame is in the cleaner's view");
        assert_eq!(
            (listed.class, listed.entry_unix_ms, listed.due_unix_ms),
            (
                FrameRetentionClass::Default,
                Some(frame.captured),
                Some(frame.captured + DAY_MS)
            ),
            "a run-less frame is Default, enters at its capture time and is due 1 day later \
             (ledger-store.md, Frame cleaner)"
        );
        assert!(
            frame.material.try_exists().expect("stat the frame file"),
            "the frame file is at its object key before the cleaner runs"
        );
    }

    // One Warning with no run, request or correlation link: the 10-02 latch's trigger.
    let warning = fixture.unlinked_warning();
    assert_eq!(warning.severity(), EventSeverity::Warning);
    assert!(
        warning.links().run_id().is_none()
            && warning.links().request_id().is_none()
            && warning.links().correlation_id().is_none()
            && warning.links().instance_id().is_none(),
        "the Warning carries no link"
    );
    assert!(
        warning.payload().artifact_retention().is_none(),
        "the Warning is no retention fact"
    );
    assert_ne!(
        warning.event_type(),
        EventType::LedgerRecovered,
        "the Warning is no stale-owner report"
    );
    let head = ledger.latest_sequence().expect("head after the Warning");

    // The cleaner on its manual clock, until its sweep completes.
    let mut retention = FrameRetention::new(NOW, switches);
    let mut warnings: Vec<FrameRetentionWarning> = Vec::new();
    let bound = 2 * frames.len();
    let mut calls = 0;
    loop {
        calls += 1;
        assert!(
            calls <= bound,
            "the cleaner's sweep did not complete within {bound} calls"
        );
        retention
            .maintain(&ledger, artifacts.root(), NOW, &|| false, &mut |warning| {
                warnings.push(warning.clone());
                Ok(())
            })
            .expect("one unlinked Warning must not stop the cleaner: every call returns Ok");
        if retention.sweep.is_none() {
            break;
        }
    }
    assert!(
        warnings.is_empty(),
        "the cleaner warns about no frame: {warnings:?}"
    );

    // Removed at the object key, nothing moved.
    for frame in &frames {
        assert!(
            !frame.material.try_exists().expect("stat the frame file"),
            "one unlinked Warning must not stop the cleaner (Alice 10-08: the 10-02 latch is \
             cancelled): a Default frame's file is removed 1 day after its entry time"
        );
    }
    assert!(
        !artifacts
            .root()
            .join(KEPT_DIRECTORY)
            .try_exists()
            .expect("stat the kept folder"),
        "a Default frame is removed, never moved into a kept folder"
    );

    // No eviction record: the cleaner writes nothing to the ledger.
    assert_eq!(
        ledger.latest_sequence().expect("head after the sweep"),
        head,
        "the cleaner writes nothing to the ledger (ledger-store.md, Frame cleaner)"
    );
    let records = ledger
        .query(EventQuery::default())
        .expect("every ledger event")
        .into_iter()
        .filter(|event| {
            matches!(
                event.payload().artifact_retention(),
                Some(
                    ArtifactRetentionFact::EvictionIntent(_)
                        | ArtifactRetentionFact::EvictionOutcome(_)
                )
            )
        })
        .count();
    assert_eq!(
        records, 0,
        "the cleaner records no eviction intent or outcome"
    );
    ledger
        .check_writer_health()
        .expect("the writer stays healthy");
}
