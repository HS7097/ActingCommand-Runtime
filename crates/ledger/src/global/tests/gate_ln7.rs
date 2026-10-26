// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5, model row LN-7). The writer keeps resident only the facts of the
//! frames present in the live frame store (and what their classes read), and drops the rest:
//! - A6: its frame view decides every live frame as the offline full view
//!   (`GlobalLedgerEvidence::frame_retention_view`) does (class, entry, due time, kept folder and
//!   windows), whether it dropped the other frames at a sweep start or opened anew;
//! - a record that references an artifact the writer no longer holds with another reference is
//!   refused `artifact_identity_conflict`: its identity is read through the `artifact_id` index.
//!
//! The root is a real formal SQLite root with its ArtifactStore (the assembly of
//! `missing_material.rs`); every timestamp is a constant after `T0`.

use super::missing_material::{formal_root, material_store};
use crate::PersistedEvent;
use crate::global::{
    FrameRetentionClass, FrameRetentionSwitches, FrameRetentionView, KeptFrameFolder,
    LedgerMaintenance, LedgerOpenTiming, Sha256SecretFingerprinter,
};
use crate::{GlobalLedger, GlobalLedgerEvidenceConfig};
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStore, ArtifactStoreError, ArtifactStoreResult,
    ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactId, ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft,
    ArtifactPinReason, ArtifactPinRecord, ArtifactProducer, ArtifactRedactionState,
    ArtifactRetentionFact, ArtifactRetentionIdentity, AuditInput, CapturePayloadDraft,
    CapturePersistedEvidence, CaptureSummaryRecord, CommandPayloadDraft, DiagnosticCode,
    DiagnosticDetailDraft, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventSeverity, EventSource, EventType, EvidenceCompleteness,
    FRAME_RETENTION_POLICY_VERSION, IdentifierIssuer, IssuedCorrelationId, IssuedInstanceId,
    IssuedRequestId, IssuedRunId, OriginModule, OwnerEpoch, OwnerResourceDisposition,
    ProjectedArtifactReference, ResourceQuiescence, RetentionClass, RuntimeLifecyclePhase,
    RuntimePayloadDraft, TaskOutcome, TaskPayloadDraft, TaskSemanticFact, TerminalEvent,
    outcome::Outcome,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

/// 2026-01-01 00:00:00 UTC.
const T0: u64 = 1_767_225_600_000;
const SECOND: u64 = 1_000;
const DAY_MS: u64 = 86_400_000;
/// The views are read two days after `T0`, past every frame's settle age.
const NOW: u64 = T0 + 2 * DAY_MS;
const SWITCHES: FrameRetentionSwitches = FrameRetentionSwitches {
    dedup_error: true,
    dedup_lab: false,
};

fn at(seconds: u64) -> u64 {
    T0 + seconds * SECOND
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

/// The writer of the existing formal root, opened anew as a start opens it.
fn reopen(database: &Arc<RuntimeDatabase>) -> GlobalLedger {
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(
        database.root(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner")
    .open_writer_timed(
        Arc::clone(database),
        "ln7-reopen".into(),
        None,
        &mut LedgerOpenTiming::default(),
    )
    .expect("the writer opens anew")
}

/// One frame: its reference, its file and the links of its records.
struct Frame {
    reference: ProjectedArtifactReference,
    material: PathBuf,
    links: EventLinksDraft,
}

impl Frame {
    fn id(&self) -> ArtifactId {
        self.reference.artifact_id
    }

    /// The frame leaves the live frame store.
    fn delete(&self) {
        std::fs::remove_file(&self.material).expect("remove the frame file");
    }
}

/// A run: its id, the links of its task records and their correlation.
struct RunLinks {
    id: IssuedRunId,
    links: EventLinksDraft,
    correlation: IssuedCorrelationId,
}

/// The records of one instance in one owner epoch, appended to the root's writer.
struct Corpus<'a> {
    ledger: &'a GlobalLedger,
    artifacts: &'a ArtifactStore,
    ids: IdentifierIssuer,
    owner: OwnerEpoch,
    instance: IssuedInstanceId,
    fingerprinter: Sha256SecretFingerprinter,
}

/// Appends the ArtifactStore's events and keeps the frame's `artifact.verified`.
struct Sink<'b, 'a> {
    corpus: &'b Corpus<'a>,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Sink<'_, '_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let draft = draft
            .sanitize(&self.corpus.fingerprinter)
            .map_err(|error| {
                ArtifactStoreError::fatal("fixture_sanitize_failed", "ln7_sink", error.to_string())
            })?;
        let event = self.corpus.ledger.append(draft).map_err(|error| {
            ArtifactStoreError::fatal(error.code(), "ln7_sink", error.to_string())
        })?;
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

impl<'a> Corpus<'a> {
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
            fingerprinter: Sha256SecretFingerprinter::new(b"ln7-fixture-salt")
                .expect("fingerprinter"),
        }
    }

    fn append(
        &self,
        at: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        let draft = EventDraft::new(
            self.ids.mint_event_id().expect("event"),
            at,
            severity,
            origin,
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("fixture draft");
        self.ledger.append(draft).expect("fixture append")
    }

    fn request(&self) -> IssuedRequestId {
        self.ids.mint_request_id().expect("request")
    }

    fn correlation(&self) -> IssuedCorrelationId {
        self.ids.mint_correlation_id().expect("correlation")
    }

    /// Links of the instance in their own request and correlation.
    fn scoped(&self) -> EventLinksDraft {
        self.scoped_in(self.request(), self.correlation())
    }

    fn scoped_in(
        &self,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
    ) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance)
            .with_request_id(request)
            .with_correlation_id(correlation)
    }

    /// A `runtime.started`: the owner epoch of every run first seen before it has ended.
    fn start(&self, at: u64) -> PersistedEvent {
        self.append(
            at,
            EventSeverity::Info,
            runtime_origin(),
            EventLinksDraft::default()
                .with_request_id(self.request())
                .with_correlation_id(self.correlation()),
            RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into(),
        )
    }

    /// The owner epoch, recorded before any frame by a confirmed quiescence of the instance.
    fn owner_recorded(&self, at: u64) {
        self.append(
            at,
            EventSeverity::Info,
            runtime_origin(),
            self.scoped(),
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

    /// A capture frame captured at `at` in the scope of `links` (whose correlation is
    /// `correlation`), published through the store, with its Explicit pin.
    fn frame(
        &self,
        links: EventLinksDraft,
        correlation: IssuedCorrelationId,
        run: Option<IssuedRunId>,
        at: u64,
    ) -> Frame {
        let frame = self.ids.mint_frame_id().expect("frame");
        let links = links.with_frame_id(frame);
        let mut artifact_links = ArtifactLinksDraft::default()
            .with_frame_id(frame)
            .with_correlation_id(correlation);
        if let Some(run) = run {
            artifact_links = artifact_links.with_run_id(run);
        }
        let mut sink = Sink {
            corpus: self,
            verified: None,
        };
        let bytes = format!("ln7 frame captured at {at}").into_bytes();
        let stored = self
            .artifacts
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
        let source = verified.links();
        let identity = ArtifactRetentionIdentity {
            artifact: reference.clone(),
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
                    reason: ArtifactPinReason::Explicit,
                    trigger: terminal(&verified),
                }),
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(&self.fingerprinter)
        .expect("pin draft");
        let pin = source
            .artifact_retention_source()
            .apply_to(pin)
            .expect("pin source links");
        self.ledger.append(pin).expect("pin append");
        Frame {
            reference,
            material: stored.path().to_path_buf(),
            links,
        }
    }

    /// A run-less observe frame of the instance at `at` in its own request and correlation, and
    /// its `capture.completed`.
    fn observe(&self, at: u64) -> Frame {
        self.observe_in(self.request(), self.correlation(), at)
    }

    fn observe_in(
        &self,
        request: IssuedRequestId,
        correlation: IssuedCorrelationId,
        at: u64,
    ) -> Frame {
        let frame = self.frame(self.scoped_in(request, correlation), correlation, None, at);
        self.append(
            at,
            EventSeverity::Info,
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
        );
        frame
    }

    /// A `capture.dedup_window` linked to `representative` that preserves `preserved`, as the
    /// capture pipeline records its markers.
    fn marker(&self, representative: &Frame, preserved: &Frame, at: u64) {
        self.append(
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::System,
                OriginModule::CapturePipeline,
                EventActor::System,
            ),
            representative.links.clone(),
            CapturePayloadDraft::dedup_window_preserving_material(
                &preserved.links,
                1_000,
                AuditInput::new(),
            )
            .into(),
        );
    }

    /// A run of the instance and its `task.started` at `at`.
    fn run(&self, at: u64) -> RunLinks {
        let run = self.ids.mint_run_id().expect("run");
        let correlation = self.correlation();
        let links = self
            .scoped_in(self.request(), correlation)
            .with_task_id(self.ids.mint_task_id().expect("task"))
            .with_run_id(run);
        self.append(
            at,
            EventSeverity::Info,
            runtime_origin(),
            links.clone(),
            TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
        RunLinks {
            id: run,
            links,
            correlation,
        }
    }

    /// A frame of `run` captured at `at`.
    fn run_frame(&self, run: &RunLinks, at: u64) -> Frame {
        self.frame(run.links.clone(), run.correlation, Some(run.id), at)
    }

    /// The run's capture summary naming `frames`, then its successful terminal.
    fn close(&self, links: &EventLinksDraft, frames: &[&Frame], at: u64) {
        let evidence = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                CapturePersistedEvidence::new(index as u64, frame.reference.clone())
                    .expect("summary frame")
            })
            .collect::<Vec<_>>();
        let count = frames.len() as u64;
        let summary = CaptureSummaryRecord::new(
            count,
            0,
            0,
            count,
            EvidenceCompleteness::Complete,
            evidence,
            Vec::new(),
        )
        .expect("summary");
        self.append(
            at,
            EventSeverity::Info,
            runtime_origin(),
            links.clone(),
            CapturePayloadDraft::summary_committed(summary, AuditInput::new()).into(),
        );
        self.append(
            at,
            EventSeverity::Info,
            runtime_origin(),
            links.clone(),
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    final_page: None,
                    executed_steps: Some(0),
                    outcome: None,
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
        );
    }

    /// An Error `runtime.failed` of the instance without a run: an error point at `at`.
    fn error_point(&self, at: u64) -> PersistedEvent {
        self.append(
            at,
            EventSeverity::Error,
            runtime_origin(),
            self.scoped(),
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::relayed("capture_failed"),
                DiagnosticDetailDraft::new(
                    "ln7_fixture",
                    crate::LedgerLocation::AppendEvent.location(),
                    "ln7_backend",
                    "ln7_operation",
                ),
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// A Lab anchor (a record of the Lab) with `links`.
    fn lab_anchor(&self, links: EventLinksDraft, at: u64) {
        self.append(
            at,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            links,
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        );
    }
}

/// What a view decides for one frame: its class, entry, due time and kept folder, and its
/// windows named by their points (sequence, epoch end, t and leaf), so views with other point
/// lists compare.
type Decision = (
    FrameRetentionClass,
    Option<u64>,
    Option<u64>,
    Option<KeptFrameFolder>,
    Vec<(u64, bool, u64, String)>,
);

fn decisions(
    view: &FrameRetentionView,
    live: &BTreeSet<ArtifactId>,
) -> BTreeMap<ArtifactId, Decision> {
    view.frames
        .iter()
        .filter(|frame| live.contains(&frame.reference.artifact_id))
        .map(|frame| {
            let windows = frame
                .windows
                .iter()
                .map(|index| {
                    let point = &view.error_points[*index];
                    (
                        point.sequence,
                        point.epoch_end,
                        point.at_unix_ms,
                        point.leaf.clone(),
                    )
                })
                .collect();
            (
                frame.reference.artifact_id,
                (
                    frame.class,
                    frame.entry_unix_ms,
                    frame.due_unix_ms,
                    frame.folder.clone(),
                    windows,
                ),
            )
        })
        .collect()
}

fn listed(view: &FrameRetentionView) -> BTreeSet<ArtifactId> {
    view.frames
        .iter()
        .map(|frame| frame.reference.artifact_id)
        .collect()
}

/// The live frames of the corpus, by role.
struct Live {
    f2: ArtifactId,
    f3: ArtifactId,
    g1: ArtifactId,
    g2: ArtifactId,
    g3: ArtifactId,
    l1: ArtifactId,
    l2: ArtifactId,
    c1: ArtifactId,
    c2: ArtifactId,
    c3_at: u64,
    d1: ArtifactId,
    n: ArtifactId,
    m: ArtifactId,
    all: BTreeSet<ArtifactId>,
}

/// The corpus of A6, with two frames deleted (`f1`, a marker neighbour, and `c3`, the last
/// frame of a run cut short by a start).
fn corpus(ledger: &GlobalLedger, artifacts: &ArtifactStore) -> Live {
    let corpus = Corpus::new(ledger, artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    // Markers and neighbours: F1 -> F2 -> F3, an Error point after them, and F1 deleted.
    let f1 = corpus.observe(at(10));
    let f2 = corpus.observe(at(11));
    let f3 = corpus.observe(at(12));
    corpus.marker(&f1, &f2, at(11));
    corpus.marker(&f2, &f3, at(12));
    corpus.error_point(at(13));
    // A marker chain across two runs: G1 of run A, then G2 and G3 of run B.
    let run_a = corpus.run(at(100));
    let run_b = corpus.run(at(104));
    let g1 = corpus.run_frame(&run_a, at(101));
    let g2 = corpus.run_frame(&run_b, at(105));
    let g3 = corpus.run_frame(&run_b, at(106));
    corpus.marker(&g1, &g2, at(105));
    corpus.marker(&g2, &g3, at(106));
    corpus.close(&run_a.links, &[&g1], at(103));
    corpus.close(&run_b.links, &[&g2, &g3], at(108));
    // Lab anchors: one on L1's request, one on L2's correlation.
    let l1_request = corpus.request();
    let l1 = corpus.observe_in(l1_request, corpus.correlation(), at(200));
    corpus.lab_anchor(
        EventLinksDraft::default()
            .with_request_id(l1_request)
            .with_correlation_id(corpus.correlation()),
        at(201),
    );
    let l2_correlation = corpus.correlation();
    let l2 = corpus.observe_in(corpus.request(), l2_correlation, at(210));
    corpus.lab_anchor(
        EventLinksDraft::default()
            .with_request_id(corpus.request())
            .with_correlation_id(l2_correlation),
        at(211),
    );
    // A run cut short: C1, C2, C3 without a terminal; the next start ends its epoch, and C3,
    // the last frame of its span, is deleted.
    let run_c = corpus.run(at(300));
    let c1 = corpus.run_frame(&run_c, at(301));
    let c2 = corpus.run_frame(&run_c, at(310));
    let c3 = corpus.run_frame(&run_c, at(320));
    corpus.start(at(400));
    // A stuck running run D after that start, and two run-less frames before its first frame:
    // N within 30 s of it, M not.
    let m = corpus.observe(at(470));
    let n = corpus.observe(at(480));
    let run_d = corpus.run(at(500));
    let d1 = corpus.run_frame(&run_d, at(505));
    f1.delete();
    c3.delete();
    let all = [&f2, &f3, &g1, &g2, &g3, &l1, &l2, &c1, &c2, &d1, &n, &m]
        .iter()
        .map(|frame| frame.id())
        .collect();
    Live {
        f2: f2.id(),
        f3: f3.id(),
        g1: g1.id(),
        g2: g2.id(),
        g3: g3.id(),
        l1: l1.id(),
        l2: l2.id(),
        c1: c1.id(),
        c2: c2.id(),
        c3_at: c3.reference.created_at_unix_ms,
        d1: d1.id(),
        n: n.id(),
        m: m.id(),
        all,
    }
}

/// The classes the requirement fixes for the corpus, so the equality below is not vacuous: the
/// window drops the interior duplicate F2 next to the deleted F1 and keeps F3; the chain across
/// runs makes G2 a duplicate; both Lab anchors keep their frame; the start that cut run C short
/// dates its epoch-end point at its deleted last frame, which keeps C1 and C2; the stuck run D
/// holds itself and N; M settles.
fn assert_required_classes(view: &FrameRetentionView, live: &Live) {
    let class = |id: &ArtifactId| {
        view.frames
            .iter()
            .find(|frame| frame.reference.artifact_id == *id)
            .map(|frame| frame.class)
            .expect("a live frame is in the view")
    };
    use FrameRetentionClass as C;
    for (id, expected) in [
        (live.f2, C::Duplicate),
        (live.f3, C::Error),
        (live.g1, C::Default),
        (live.g2, C::Duplicate),
        (live.g3, C::Default),
        (live.l1, C::Lab),
        (live.l2, C::Lab),
        (live.c1, C::Error),
        (live.c2, C::Error),
        (live.d1, C::Running),
        (live.n, C::Running),
        (live.m, C::Default),
    ] {
        assert_eq!(class(&id), expected);
    }
    let epoch_end = view
        .error_points
        .iter()
        .find(|point| point.epoch_end)
        .expect("the epoch-end point of run C");
    assert_eq!(epoch_end.at_unix_ms, live.c3_at);
}

/// A6 (LN-7 row): the writer's view, after it dropped the frames that left the live store at a
/// sweep start and after it opened anew, decides every live frame as the full view does, and
/// lists only the live frames.
#[test]
fn ln7_a6_bounded_frame_decisions_equal_the_full_views() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let live = corpus(&ledger, &artifacts);

    let swept = ledger
        .frame_retention_view_live(NOW, SWITCHES, artifacts.root())
        .expect("the view at a sweep start");
    ledger.close().expect("close the writer");
    let full = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .expect("the whole opening")
    .frame_retention_view(NOW, SWITCHES)
    .expect("the full view");
    let reopened = reopen(&database);
    let opened = reopened
        .frame_retention_view(NOW, SWITCHES)
        .expect("the view of the writer opened anew");
    reopened.close().expect("close the reopened writer");

    assert_required_classes(&full, &live);
    let expected = decisions(&full, &live.all);
    assert_eq!(
        expected.len(),
        live.all.len(),
        "the full view lists every live frame"
    );
    for (view, when) in [(&swept, "at a sweep start"), (&opened, "opened anew")] {
        assert_eq!(
            listed(view),
            live.all,
            "the view lists the live frames only ({when})"
        );
        assert_eq!(decisions(view, &live.all), expected, "the decisions {when}");
    }
}

/// LN-7 (§3.5, "write-side validate"): a frame the writer no longer holds (its file left the
/// store before the writer opened) and that a record references again with another reference is
/// refused `artifact_identity_conflict`; its identity is its first reference, read through the
/// `artifact_id` index.
#[test]
fn ln7_an_old_artifacts_identity_refusal_goes_through_the_index() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.owner_recorded(at(1));
    let old = corpus.observe(at(10));
    ledger.close().expect("close the writer");
    old.delete();
    let reopened = reopen(&database);
    let corpus = Corpus::new(&reopened, &artifacts);
    corpus.owner_recorded(at(20));
    let run = corpus.run(at(21)).links;
    let mut changed = old.reference.clone();
    changed.byte_count += 1;
    let summary = CaptureSummaryRecord::new(
        1,
        0,
        0,
        1,
        EvidenceCompleteness::Complete,
        vec![CapturePersistedEvidence::new(0, changed).expect("summary frame")],
        Vec::new(),
    )
    .expect("summary");
    let draft = EventDraft::new(
        corpus.ids.mint_event_id().expect("event"),
        at(22),
        EventSeverity::Info,
        runtime_origin(),
        run,
        CapturePayloadDraft::summary_committed(summary, AuditInput::new()).into(),
    )
    .sanitize(&corpus.fingerprinter)
    .expect("summary draft");
    let refused = reopened
        .append(draft)
        .expect_err("another reference of an old artifact is refused");
    assert_eq!(refused.code(), "artifact_identity_conflict");
}
