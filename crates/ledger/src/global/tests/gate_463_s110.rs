// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 S1-10 (0.12.66, register §1.430): before the writer admits a pin whose reason
//! is `Lab`, it asks whether the pin's trigger record is related to the Lab. It now looks first
//! whether a Lab anchor, stored or staged in the batch, names the trigger's request or
//! correlation, by the link indexes; only when none does is every Lab relation read, as before,
//! which also relates the record by its run. The answer never changes. The writer's reads of
//! every Lab relation are counted (`GlobalLedgerConfig::with_lab_relations_reads`, tests only).
//! On the SQLite store and on the in-memory store alike:
//! - T1: a Lab pin whose trigger's request a Lab anchor names is admitted, and every Lab relation
//!   is not read;
//! - T2: a Lab pin whose trigger is related only through its run (no anchor names its request or
//!   correlation) is admitted, by one read of every Lab relation;
//! - T3: a Lab pin whose trigger is not related is refused `artifact_pin_reason_conflict`, fatal,
//!   as before, after one read of every Lab relation.
//!
//! On the SQLite store, T4: an anchor staged in the same batch as the trigger and the pin, none
//! of them stored yet, names the trigger's request, and the pin is admitted without that read.
//!
//! Each frame is published through an ArtifactStore and pinned as the Runtime pins the frame of
//! a Lab observation: the pin's identity and links are the frame's `artifact.verified`'s. Every
//! record time is a fixed offset; no clock and no sleep. `BARRIER_WAIT` only bounds a broken run.

use super::BARRIER_WAIT;
use super::missing_material::material_store;
use super::sqlite_contract;
use crate::codes::LedgerCode;
use crate::global::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use crate::{PersistedEvent, WriterReadHoldGate};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactPinReason,
    ArtifactPinRecord, ArtifactProducer, ArtifactRedactionState, ArtifactRetentionFact,
    ArtifactRetentionIdentity, AuditInput, CapturePayloadDraft, ClientPayloadDraft,
    EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventSeverity, EventSource, EventType, FRAME_RETENTION_POLICY_VERSION,
    IdentifierIssuer, IssuedCorrelationId, IssuedInstanceId, IssuedRequestId, IssuedRunId,
    IssuedTaskId, OriginModule, OwnerEpoch, OwnerResourceDisposition, ProjectedArtifactReference,
    ResourceQuiescence, RetentionClass, RuntimeLifecyclePhase, RuntimePayloadDraft,
    SanitizedEventDraft, TaskPayloadDraft, TerminalEvent,
};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

/// The writer's owner id: a neutral fixture word.
const OWNER: &str = "ledger-s110-fixture";
const AT_MS: u64 = 1_752_147_200_000;

fn at(seconds: u64) -> u64 {
    AT_MS + seconds * 1_000
}

/// The two stores of the writer's trait.
#[derive(Clone, Copy)]
enum Medium {
    Sqlite,
    Memory,
}

/// The writer of `medium` on `root`, its reads of every Lab relation counted on `reads`.
fn open(medium: Medium, root: &Path, reads: &Arc<AtomicU64>) -> GlobalLedger {
    let config = GlobalLedgerConfig::new(root, OWNER).with_lab_relations_reads(Arc::clone(reads));
    match medium {
        Medium::Sqlite => sqlite_contract::open(config),
        Medium::Memory => GlobalLedger::open(config),
    }
    .expect("the writer opens")
}

fn terminal(event: &PersistedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    }
}

fn runtime_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

/// A frame published through the store: its reference, the links of its records and its
/// `artifact.verified`.
struct Frame {
    reference: ProjectedArtifactReference,
    links: EventLinksDraft,
    verified: PersistedEvent,
}

/// The records of one instance in one owner epoch, appended to one writer.
struct Fixture<'a> {
    ledger: &'a GlobalLedger,
    artifacts: &'a ArtifactStore,
    ids: IdentifierIssuer,
    owner: OwnerEpoch,
    instance: IssuedInstanceId,
    salt: Sha256SecretFingerprinter,
}

/// Appends the ArtifactStore's events and keeps the frame's `artifact.verified`.
struct Sink<'b, 'a> {
    fixture: &'b Fixture<'a>,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Sink<'_, '_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft.sanitize(&self.fixture.salt).map_err(|error| {
            ArtifactStoreError::fatal("fixture_sanitize_failed", "s110_sink", error.to_string())
        })?;
        let event = self.fixture.ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "s110_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

impl<'a> Fixture<'a> {
    fn new(ledger: &'a GlobalLedger, artifacts: &'a ArtifactStore) -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let owner = *ids.mint_owner_epoch().expect("owner epoch").transport();
        let instance = ids.mint_instance_id().expect("instance");
        Self {
            ledger,
            artifacts,
            ids,
            owner,
            instance,
            salt: Sha256SecretFingerprinter::new(b"s110-fixture-salt").expect("fingerprinter"),
        }
    }

    fn request(&self) -> IssuedRequestId {
        self.ids.mint_request_id().expect("request")
    }

    fn correlation(&self) -> IssuedCorrelationId {
        self.ids.mint_correlation_id().expect("correlation")
    }

    /// Links of the instance in `request` and `correlation`.
    fn scoped(
        &self,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
    ) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance)
            .with_request_id(request)
            .with_correlation_id(correlation)
    }

    fn draft(
        &self,
        at: u64,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> SanitizedEventDraft {
        EventDraft::new(
            self.ids.mint_event_id().expect("event"),
            at,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&self.salt)
        .expect("a fixture draft")
    }

    fn append(
        &self,
        at: u64,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        self.ledger
            .append(self.draft(at, origin, links, payload))
            .expect("a fixture record")
    }

    /// The owner epoch, recorded before any frame by a confirmed quiescence of the instance.
    fn owner_recorded(&self, at: u64) {
        self.append(
            at,
            runtime_origin(),
            self.scoped(self.request(), self.correlation()),
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
        );
    }

    /// A Lab anchor: the Lab's request record of an observation of the instance, as the Runtime
    /// writes it, in `request` and `correlation`.
    fn anchor(
        &self,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
        at: u64,
    ) -> SanitizedEventDraft {
        self.draft(
            at,
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            self.scoped(request, correlation),
            ClientPayloadDraft::lab_request(EventAction::RuntimeReadonlyObserve, AuditInput::new())
                .into(),
        )
    }

    /// A capture frame captured at `at` in `request` and `correlation` (and in the run of `run`,
    /// its task and its run), published through the store.
    fn frame(
        &self,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
        run: Option<(IssuedTaskId, IssuedRunId)>,
        at: u64,
    ) -> Frame {
        let frame = self.ids.mint_frame_id().expect("frame");
        let mut links = self.scoped(request, correlation).with_frame_id(frame);
        let mut artifact_links = ArtifactLinksDraft::default()
            .with_frame_id(frame)
            .with_correlation_id(correlation);
        if let Some((task, run)) = run {
            links = links.with_task_id(task).with_run_id(run);
            artifact_links = artifact_links.with_run_id(run);
        }
        let mut sink = Sink {
            fixture: self,
            verified: None,
        };
        let bytes = format!("s110 frame captured at {at}").into_bytes();
        self.artifacts
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    &bytes,
                    ArtifactWriteContext::new(artifact_links, links.clone(), at),
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
        Frame {
            reference,
            links,
            verified,
        }
    }

    /// A `capture.completed` of `frame`'s scope.
    fn completed(&self, frame: &Frame, at: u64) -> SanitizedEventDraft {
        self.draft(
            at,
            EventOrigin::new(
                EventSource::Device,
                OriginModule::Capture,
                EventActor::Runtime,
            ),
            frame.links.clone(),
            CapturePayloadDraft::completed(
                EventAction::CaptureObserve,
                EffectDisposition::Performed,
                1280,
                720,
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// The pin of `frame` with the reason `Lab`, triggered by `trigger`, drafted as the Runtime
    /// drafts the pin of a frame a Lab observation stored: its identity and its links are those
    /// of the frame's `artifact.verified`.
    fn lab_pin(&self, frame: &Frame, trigger: TerminalEvent, at: u64) -> SanitizedEventDraft {
        let source = frame.verified.links();
        let identity = ArtifactRetentionIdentity {
            artifact: frame.reference.clone(),
            owner_epoch: self.owner,
            instance_id: *source.instance_id().expect("instance link"),
            request_id: *source.request_id().expect("request link"),
            correlation_id: *source.correlation_id().expect("correlation link"),
            run_id: source.run_id().copied(),
            lease_id: source.lease_id().copied(),
            policy_version: FRAME_RETENTION_POLICY_VERSION,
        };
        let pin = EventDraft::new(
            self.ids.mint_event_id().expect("event"),
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::ArtifactStore,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            ArtifactPayloadDraft::retention(
                ArtifactRetentionFact::PinRecorded(ArtifactPinRecord {
                    identity,
                    reason: ArtifactPinReason::Lab,
                    trigger,
                }),
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(&self.salt)
        .expect("pin draft");
        source
            .artifact_retention_source()
            .apply_to(pin)
            .expect("pin source links")
    }
}

/// Holds the writer in a read while `capture` captures records, so that the flush after the
/// read commits them as one batch.
fn in_one_batch(root: &TempDir, ledger: &GlobalLedger, capture: impl FnOnce()) {
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| ledger.latest_sequence());
        assert!(hold.wait_held(BARRIER_WAIT), "the read reached the writer");
        capture();
        hold.release();
        reader.join().expect("reader").expect("the held read");
    });
}

/// T1: the Lab's request record names the trigger's request (in a correlation of its own, so the
/// request alone relates it); the pin is admitted and every Lab relation is not read.
fn admitted_by_the_anchor_of_its_request(medium: Medium) {
    let root = TempDir::new().expect("state root");
    let frames = TempDir::new().expect("frame store");
    let reads = Arc::new(AtomicU64::new(0));
    let ledger = open(medium, root.path(), &reads);
    let artifacts = material_store(frames.path());
    let fixture = Fixture::new(&ledger, &artifacts);
    fixture.owner_recorded(at(1));
    let request = fixture.request();
    ledger
        .append(fixture.anchor(request, fixture.correlation(), at(2)))
        .expect("the Lab's request record");
    let frame = fixture.frame(request, fixture.correlation(), None, at(3));

    reads.store(0, Ordering::SeqCst);
    let pinned = ledger
        .append(fixture.lab_pin(&frame, terminal(&frame.verified), at(3)))
        .expect("the Lab pin is admitted");
    assert_eq!(pinned.event_type(), EventType::ArtifactPinRecorded);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "the anchor of the trigger's request answered: every Lab relation was not read"
    );
    ledger.close().expect("close");
}

/// T2: a record of the run names the Lab's request and the Lab's request record names that
/// request; the run's frame is in a request and a correlation no anchor names. The pin is
/// admitted by the one read of every Lab relation, which relates the trigger by its run.
fn admitted_through_its_run_by_the_full_read(medium: Medium) {
    let root = TempDir::new().expect("state root");
    let frames = TempDir::new().expect("frame store");
    let reads = Arc::new(AtomicU64::new(0));
    let ledger = open(medium, root.path(), &reads);
    let artifacts = material_store(frames.path());
    let fixture = Fixture::new(&ledger, &artifacts);
    fixture.owner_recorded(at(1));
    let task = fixture.ids.mint_task_id().expect("task");
    let run = fixture.ids.mint_run_id().expect("run");
    let lab_request = fixture.request();
    fixture.append(
        at(2),
        runtime_origin(),
        fixture
            .scoped(lab_request, fixture.correlation())
            .with_task_id(task)
            .with_run_id(run),
        TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
    );
    ledger
        .append(fixture.anchor(lab_request, fixture.correlation(), at(3)))
        .expect("the Lab's request record");
    let frame = fixture.frame(
        fixture.request(),
        fixture.correlation(),
        Some((task, run)),
        at(4),
    );

    reads.store(0, Ordering::SeqCst);
    let pinned = ledger
        .append(fixture.lab_pin(&frame, terminal(&frame.verified), at(4)))
        .expect("the Lab pin of the run's frame is admitted");
    assert_eq!(pinned.event_type(), EventType::ArtifactPinRecorded);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "no anchor names the trigger's request or correlation: every Lab relation was read once"
    );
    ledger.close().expect("close");
}

/// T3: the only Lab anchor names another request and correlation, and no run relates the
/// run-less trigger; the pin is refused with the admission's code, fatal, as before.
fn refused_when_nothing_relates_its_trigger(medium: Medium) {
    let root = TempDir::new().expect("state root");
    let frames = TempDir::new().expect("frame store");
    let reads = Arc::new(AtomicU64::new(0));
    let ledger = open(medium, root.path(), &reads);
    let artifacts = material_store(frames.path());
    let fixture = Fixture::new(&ledger, &artifacts);
    fixture.owner_recorded(at(1));
    ledger
        .append(fixture.anchor(fixture.request(), fixture.correlation(), at(2)))
        .expect("the Lab's request record of another request");
    let frame = fixture.frame(fixture.request(), fixture.correlation(), None, at(3));

    reads.store(0, Ordering::SeqCst);
    let refused = ledger
        .append(fixture.lab_pin(&frame, terminal(&frame.verified), at(3)))
        .expect_err("a Lab pin whose trigger is not related to the Lab is refused");
    assert_eq!(
        (refused.code(), refused.is_fatal()),
        (LedgerCode::ArtifactPinReasonConflict.as_str(), true),
        "the admission's refusal"
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "no anchor names the trigger's request or correlation: every Lab relation was read once"
    );
    ledger
        .close()
        .expect_err("the fatal refusal ended the writer");
}

#[test]
fn s110_t1_sqlite_a_lab_pin_whose_trigger_request_an_anchor_names_reads_no_full_relations() {
    admitted_by_the_anchor_of_its_request(Medium::Sqlite);
}

#[test]
fn s110_t1_memory_a_lab_pin_whose_trigger_request_an_anchor_names_reads_no_full_relations() {
    admitted_by_the_anchor_of_its_request(Medium::Memory);
}

#[test]
fn s110_t2_sqlite_a_lab_pin_related_only_through_its_run_is_admitted_by_the_full_read() {
    admitted_through_its_run_by_the_full_read(Medium::Sqlite);
}

#[test]
fn s110_t2_memory_a_lab_pin_related_only_through_its_run_is_admitted_by_the_full_read() {
    admitted_through_its_run_by_the_full_read(Medium::Memory);
}

#[test]
fn s110_t3_sqlite_a_lab_pin_whose_trigger_nothing_relates_is_refused_as_before() {
    refused_when_nothing_relates_its_trigger(Medium::Sqlite);
}

#[test]
fn s110_t3_memory_a_lab_pin_whose_trigger_nothing_relates_is_refused_as_before() {
    refused_when_nothing_relates_its_trigger(Medium::Memory);
}

/// T4: on the SQLite store, one batch holds the Lab's request record of the frame's request, a
/// record of the frame's scope that triggers the pin, and the Lab pin; neither the anchor nor
/// the trigger is stored when the pin is checked. The staged anchor names the trigger's request:
/// the pin is admitted, the three commit in their order, and every Lab relation is not read.
#[test]
fn s110_t4_an_anchor_staged_with_its_trigger_and_the_pin_admits_it_without_the_full_read() {
    let root = TempDir::new().expect("state root");
    let frames = TempDir::new().expect("frame store");
    let reads = Arc::new(AtomicU64::new(0));
    let ledger = open(Medium::Sqlite, root.path(), &reads);
    let artifacts = material_store(frames.path());
    let fixture = Fixture::new(&ledger, &artifacts);
    fixture.owner_recorded(at(1));
    let request = fixture.request();
    let frame = fixture.frame(request, fixture.correlation(), None, at(2));
    let head = ledger.latest_sequence().expect("the head");
    let anchor = fixture.anchor(request, fixture.correlation(), at(3));
    let trigger = fixture.completed(&frame, at(3));
    // The batch stages the anchor at `head + 1` and the trigger at `head + 2`.
    let pin = fixture.lab_pin(
        &frame,
        TerminalEvent {
            sequence: head + 2,
            event_id: *trigger.event_id(),
        },
        at(3),
    );
    let expected = [*anchor.event_id(), *trigger.event_id(), *pin.event_id()];

    reads.store(0, Ordering::SeqCst);
    in_one_batch(&root, &ledger, || {
        for draft in [anchor, trigger, pin] {
            ledger.append_deferred(draft).expect("accepted");
        }
    });
    let confirmed = ledger.confirm_deferred(BARRIER_WAIT).expect("confirmation");
    assert!(confirmed.failed.is_empty(), "{:?}", confirmed.failed);
    assert_eq!(
        confirmed
            .committed
            .iter()
            .map(|reference| *reference.event_id())
            .collect::<Vec<_>>(),
        expected,
        "the anchor, the trigger and the Lab pin commit in their order"
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        0,
        "the staged anchor of the trigger's request answered: every Lab relation was not read"
    );
    ledger.close().expect("close");
}
