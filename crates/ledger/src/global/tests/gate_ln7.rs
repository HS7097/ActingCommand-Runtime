// SPDX-License-Identifier: AGPL-3.0-only

//! LN-7 (#422, LEAN-L §3.5, model row LN-7), and #422 LN-7c Q (`rd-1026/ln7c/final-query.md` §6,
//! §7 row Q, test T-eq with LN-7b's A6 case fixture):
//! - T-eq (adapted from A6, whose writer's frame view Q removes): the frame cleaner's query
//!   (statements W and L, then a moved frame's kept folder) decides every live frame of the case
//!   fixture as the offline full view (`GlobalLedgerEvidence::frame_retention_view`, the reference)
//!   classes it, with E1's due times (from the frame's ingestion time): its action and, for a frame
//!   it moves, its kept folder. The fixture holds A6's cases (marker chains within and across runs,
//!   Lab anchors, a run cut short by a start whose last frame is gone, a resource reading frame and
//!   a confirmation frame published with system links, a stuck running run), and Q's: F5 (a frame
//!   named only by a publication whose correlation has a Lab use), F6 (a run cut short with no frame
//!   left in the store, whose epoch-end point keeps a frame of another run) and E4, the one
//!   difference the design accounts that the fixture has (a run-less Warning that names a frame:
//!   the query keeps it as an error frame, the full view does not). E3 and E5 do not arise here;
//! - a record that references an artifact the writer no longer holds with another reference is
//!   refused `artifact_identity_conflict`: its identity is read through the `artifact_id` index.
//!
//! The root is a real formal SQLite root with its ArtifactStore (the assembly of
//! `missing_material.rs`); every timestamp is a constant after `T0`.

use super::missing_material::{formal_root, material_store};
use crate::PersistedEvent;
use crate::global::{
    FrameQueryAction, FrameQueryOutcome, FrameQuerySettings, FrameRetentionClass,
    FrameRetentionFrame, FrameRetentionSwitches, FrameRetentionView, KeptFrameFolder,
    LedgerMaintenance, LedgerOpenTiming, LiveFrameAnswer, Sha256SecretFingerprinter,
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
    FRAME_RETENTION_POLICY_VERSION, FactContent, FactPayloadDraft, FactRecord, FactScope,
    FactValue, IdentifierIssuer, IssuedCorrelationId, IssuedInstanceId, IssuedRequestId,
    IssuedRunId, OriginModule, OwnerEpoch, OwnerResourceDisposition, ProjectedArtifactReference,
    RESOURCE_ADJUSTMENT_DETECTOR_PREFIX, RESOURCE_READING_DETECTOR_PREFIX, ResourceQuiescence,
    RetentionClass, RuntimeLifecyclePhase, RuntimePayloadDraft, TaskOutcome, TaskPayloadDraft,
    TaskSemanticFact, TerminalEvent, outcome::Outcome, resource_adjustment_snapshot_id,
    resource_reading_snapshot_id,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

/// 2026-01-01 00:00:00 UTC.
pub(super) const T0: u64 = 1_767_225_600_000;
const SECOND: u64 = 1_000;
pub(super) const DAY_MS: u64 = 86_400_000;
/// The views are read two days after `T0`, past every frame's settle age.
const NOW: u64 = T0 + 2 * DAY_MS;
pub(super) const SWITCHES: FrameRetentionSwitches = FrameRetentionSwitches {
    dedup_error: true,
    dedup_lab: false,
};
/// `frame_retention_evidence_max_age_days`'s default.
pub(super) const EVIDENCE_MAX_AGE_MS: u64 = 15 * DAY_MS;
/// `ledger_observer.frame_query_chunk_frames`'s default.
pub(super) const CHUNK_FRAMES: usize = 256;

pub(super) fn at(seconds: u64) -> u64 {
    T0 + seconds * SECOND
}

/// What a pass at `now` decides with: the switches' defaults and the evidence age's.
pub(super) fn settings(now: u64) -> FrameQuerySettings {
    FrameQuerySettings {
        now_unix_ms: now,
        switches: SWITCHES,
        evidence_max_age_ms: EVIDENCE_MAX_AGE_MS,
    }
}

/// #422 LN-7c Q: the query's answer for each frame of `live` at `now` (W, then L in chunks), and
/// for each frame it moves the kept folder it names, read on the writer's own frame query.
pub(super) fn query_decisions(
    ledger: &GlobalLedger,
    live: &BTreeSet<ArtifactId>,
    now: u64,
) -> BTreeMap<ArtifactId, (LiveFrameAnswer, Option<KeptFrameFolder>)> {
    let mut query = ledger
        .frame_query(Arc::new(|| false))
        .expect("the writer's frame query");
    let FrameQueryOutcome::Answered { value: waits, .. } = query.waits().expect("statement W")
    else {
        panic!("statement W answered nothing: the frame index does not cover the ledger");
    };
    let ids = live.iter().copied().collect::<Vec<_>>();
    let mut decisions = BTreeMap::new();
    for chunk in ids.chunks(CHUNK_FRAMES) {
        let FrameQueryOutcome::Answered { value, .. } = query
            .live(chunk, &waits, &settings(now))
            .expect("statement L")
        else {
            panic!("statement L answered nothing");
        };
        for answer in value {
            let folder = answer.action.moves().then(|| {
                let frame = query
                    .frame(answer.artifact_id)
                    .expect("the frame's verified record")
                    .expect("a moved frame has its verified record");
                query
                    .folder(&frame, answer.action == FrameQueryAction::MoveLab)
                    .expect("the moved frame's kept folder")
            });
            decisions.insert(answer.artifact_id, (answer, folder));
        }
    }
    decisions
}

/// The reference's action for a frame of the full view at `now`, with E1's due times: a default
/// frame at 1 day and a resource frame at 7 days after its ingestion time.
pub(super) fn expected_action(frame: &FrameRetentionFrame, now: u64) -> FrameQueryAction {
    let at = frame.reference.created_at_unix_ms;
    match frame.class {
        FrameRetentionClass::Running => FrameQueryAction::Running,
        FrameRetentionClass::Lab => FrameQueryAction::MoveLab,
        FrameRetentionClass::Error => FrameQueryAction::MoveError,
        FrameRetentionClass::Resource if at + 7 * DAY_MS <= now => FrameQueryAction::Remove,
        FrameRetentionClass::Resource => FrameQueryAction::KeepResource,
        FrameRetentionClass::Duplicate => FrameQueryAction::RemoveDuplicate,
        FrameRetentionClass::Default if at + DAY_MS <= now => FrameQueryAction::Remove,
        FrameRetentionClass::Default => FrameQueryAction::Keep,
    }
}

pub(super) fn terminal(event: &PersistedEvent) -> TerminalEvent {
    TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    }
}

/// An identifier's canonical text, as a snapshot id names it.
fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
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
pub(super) fn reopen(database: &Arc<RuntimeDatabase>) -> GlobalLedger {
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
pub(super) struct Frame {
    pub(super) reference: ProjectedArtifactReference,
    pub(super) material: PathBuf,
    pub(super) links: EventLinksDraft,
}

impl Frame {
    pub(super) fn id(&self) -> ArtifactId {
        self.reference.artifact_id
    }

    /// The canonical text of its frame id.
    fn frame_text(&self) -> String {
        text(
            &self
                .reference
                .frame_id
                .expect("a capture frame has a frame id"),
        )
    }

    /// The frame leaves the live frame store.
    pub(super) fn delete(&self) {
        std::fs::remove_file(&self.material).expect("remove the frame file");
    }
}

/// A run: its id, the links of its task records and their correlation.
pub(super) struct RunLinks {
    pub(super) id: IssuedRunId,
    pub(super) links: EventLinksDraft,
    pub(super) correlation: IssuedCorrelationId,
}

/// The records of one instance in one owner epoch, appended to the root's writer.
pub(super) struct Corpus<'a> {
    pub(super) ledger: &'a GlobalLedger,
    artifacts: &'a ArtifactStore,
    pub(super) ids: IdentifierIssuer,
    owner: OwnerEpoch,
    pub(super) instance: IssuedInstanceId,
    pub(super) fingerprinter: Sha256SecretFingerprinter,
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
    pub(super) fn new(ledger: &'a GlobalLedger, artifacts: &'a ArtifactStore) -> Self {
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

    pub(super) fn append(
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

    pub(super) fn request(&self) -> IssuedRequestId {
        self.ids.mint_request_id().expect("request")
    }

    pub(super) fn correlation(&self) -> IssuedCorrelationId {
        self.ids.mint_correlation_id().expect("correlation")
    }

    /// Links of the instance in their own request and correlation.
    pub(super) fn scoped(&self) -> EventLinksDraft {
        self.scoped_in(self.request(), self.correlation())
    }

    pub(super) fn scoped_in(
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
    pub(super) fn start(&self, at: u64) -> PersistedEvent {
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
    pub(super) fn owner_recorded(&self, at: u64) {
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
    pub(super) fn frame(
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
    pub(super) fn observe(&self, at: u64) -> Frame {
        self.observe_in(self.request(), self.correlation(), at)
    }

    pub(super) fn observe_in(
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
    pub(super) fn marker(&self, representative: &Frame, preserved: &Frame, at: u64) {
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
    pub(super) fn run(&self, at: u64) -> RunLinks {
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
    pub(super) fn run_frame(&self, run: &RunLinks, at: u64) -> Frame {
        self.frame(run.links.clone(), run.correlation, Some(run.id), at)
    }

    /// The run's capture summary naming `frames`, then its successful terminal.
    pub(super) fn close(&self, links: &EventLinksDraft, frames: &[&Frame], at: u64) {
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
    pub(super) fn error_point(&self, at: u64) -> PersistedEvent {
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

    /// A `fact.published` of one instance fact with `detector` and `snapshot`, published as the
    /// Runtime publishes a resource reading or an adjustment: with system links (a new request
    /// and correlation and an action; no run, instance, lease or frame link) and inline content.
    pub(super) fn publish(&self, detector: String, snapshot: String, at: u64) -> PersistedEvent {
        let record = FactRecord {
            scope: FactScope::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            key: "resource.ln7_fixture_counter".to_owned(),
            content: FactContent::Inline {
                value: FactValue::Integer(1),
            },
            observed_at_unix_ms: at,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: detector,
            source_snapshot_id: snapshot,
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "5".repeat(64),
            invalidate_on: Vec::new(),
        };
        self.append(
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
            ),
            EventLinksDraft::default()
                .with_request_id(self.request())
                .with_correlation_id(self.correlation())
                .with_action_id(self.ids.mint_action_id().expect("action")),
            FactPayloadDraft::published(record, AuditInput::new()).into(),
        )
    }

    /// A resource reading taken on `frame` of `run`.
    pub(super) fn reading(&self, run: &RunLinks, frame: &Frame, at: u64) -> PersistedEvent {
        self.publish(
            format!("{RESOURCE_READING_DETECTOR_PREFIX}ln7_task/ln7_reading"),
            resource_reading_snapshot_id(
                &text(run.id.transport()),
                &frame.frame_text(),
                "ln7_reading",
            ),
            at,
        )
    }

    /// An adjustment of `base` whose operation was confirmed on `frame` of `run`.
    pub(super) fn adjustment(&self, run: &RunLinks, frame: &Frame, base: &PersistedEvent, at: u64) {
        self.publish(
            format!("{RESOURCE_ADJUSTMENT_DETECTOR_PREFIX}ln7_task/ln7_adjustment"),
            resource_adjustment_snapshot_id(
                &text(run.id.transport()),
                &frame.frame_text(),
                "ln7_adjustment",
                &text(base.event_id()),
            ),
            at,
        );
    }

    /// A Lab anchor (a record of the Lab) with `links`.
    pub(super) fn lab_anchor(&self, links: EventLinksDraft, at: u64) {
        self.append(
            at,
            EventSeverity::Info,
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            links,
            CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
        );
    }

    /// #422 LN-7c Q (F5): a `fact.published` that holds `frame`'s artifact as its content, with
    /// system links in the correlation `correlation` (no run, instance, lease or frame link).
    pub(super) fn publish_artifact(
        &self,
        frame: &Frame,
        correlation: IssuedCorrelationId,
        at: u64,
    ) -> PersistedEvent {
        let record = FactRecord {
            scope: FactScope::Instance {
                instance_id: "fixture-instance-a".to_owned(),
            },
            key: "resource.ln7_fixture_artifact".to_owned(),
            content: FactContent::Artifact {
                artifact: frame.reference.clone(),
            },
            observed_at_unix_ms: at,
            expires_at_unix_ms: None,
            ttl_policy: None,
            confidence_milli: 1_000,
            source_detector: "ln7_fixture_detector".to_owned(),
            source_snapshot_id: "snapshot:ln7_fixture".to_owned(),
            schema_version: "fact.v1".to_owned(),
            resource_bundle_hash: "5".repeat(64),
            invalidate_on: Vec::new(),
        };
        self.append(
            at,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::FactStore,
                EventActor::Runtime,
            ),
            EventLinksDraft::default()
                .with_request_id(self.request())
                .with_correlation_id(correlation)
                .with_action_id(self.ids.mint_action_id().expect("action")),
            FactPayloadDraft::published(record, AuditInput::new()).into(),
        )
    }

    /// #422 LN-7c Q (E4): a run-less Warning `runtime.failed` that names `frame` by its frame link
    /// (its severity is its outcome's category, ledger revision 3).
    pub(super) fn warning_naming(&self, frame: &Frame, at: u64) -> PersistedEvent {
        self.append(
            at,
            EventSeverity::Warning,
            runtime_origin(),
            frame.links.clone(),
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::relayed("step_budget_exhausted"),
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
    p1: ArtifactId,
    r1: ArtifactId,
    h1: ArtifactId,
    h2: ArtifactId,
    h3: ArtifactId,
    d1: ArtifactId,
    n: ArtifactId,
    m: ArtifactId,
    /// #422 LN-7c Q: F5, F6 and E4.
    f5: ArtifactId,
    k1: ArtifactId,
    w1: ArtifactId,
    all: BTreeSet<ArtifactId>,
}

/// The corpus of A6, with three frames deleted (`f1`, a marker neighbour, `c3`, the last frame
/// of a run cut short by a start, and `q1`, a marker neighbour in another run).
fn corpus(ledger: &GlobalLedger, artifacts: &ArtifactStore) -> Live {
    let corpus = Corpus::new(ledger, artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    // #422 LN-7c Q, F6: run E, cut short by the start at 400 s, has no frame left in the store;
    // its epoch-end point at its last frame (45 s) keeps K1, a frame of the closed run K.
    let run_k = corpus.run(at(28));
    let k1 = corpus.run_frame(&run_k, at(30));
    corpus.close(&run_k.links, &[&k1], at(31));
    let run_e = corpus.run(at(40));
    let e1 = corpus.run_frame(&run_e, at(45));
    // E4: W1, a run-less frame named by a run-less Warning.
    let w1 = corpus.observe(at(60));
    corpus.warning_naming(&w1, at(61));
    // F5: F5's frame is named only by a publication with system links whose correlation has a
    // Lab use.
    let run_f = corpus.run(at(70));
    let f5 = corpus.run_frame(&run_f, at(72));
    corpus.close(&run_f.links, &[&f5], at(73));
    let published = corpus.correlation();
    corpus.publish_artifact(&f5, published, at(74));
    corpus.lab_anchor(
        EventLinksDraft::default()
            .with_request_id(corpus.request())
            .with_correlation_id(published),
        at(75),
    );
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
    // A marker chain across runs to a deleted neighbour: P1 of run P is the representative of Q1
    // of run Q, both runs end before the start, and Q1 is deleted.
    let run_p = corpus.run(at(330));
    let run_q = corpus.run(at(331));
    let p1 = corpus.run_frame(&run_p, at(340));
    let q1 = corpus.run_frame(&run_q, at(342));
    corpus.marker(&p1, &q1, at(342));
    corpus.close(&run_p.links, &[&p1], at(343));
    corpus.close(&run_q.links, &[&q1], at(344));
    // A resource reading frame: R1, the frame of run R a reading is taken on, its fact published
    // with system links after the run's terminal.
    let run_r = corpus.run(at(350));
    let r1 = corpus.run_frame(&run_r, at(355));
    corpus.close(&run_r.links, &[&r1], at(356));
    let reading = corpus.reading(&run_r, &r1, at(357));
    // A confirmation frame: H2, the interior of the chain H1 -> H2 -> H3 of run S, on which an
    // adjustment's operation was confirmed; its fact too is published with system links.
    let run_s = corpus.run(at(360));
    let h1 = corpus.run_frame(&run_s, at(365));
    let h2 = corpus.run_frame(&run_s, at(366));
    let h3 = corpus.run_frame(&run_s, at(367));
    corpus.marker(&h1, &h2, at(366));
    corpus.marker(&h2, &h3, at(367));
    corpus.close(&run_s.links, &[&h1, &h2, &h3], at(368));
    corpus.adjustment(&run_s, &h2, &reading, at(369));
    corpus.start(at(400));
    // A stuck running run D after that start, and two run-less frames before its first frame:
    // N within 30 s of it, M not.
    let m = corpus.observe(at(470));
    let n = corpus.observe(at(480));
    let run_d = corpus.run(at(500));
    let d1 = corpus.run_frame(&run_d, at(505));
    f1.delete();
    c3.delete();
    q1.delete();
    e1.delete();
    let all = [
        &f2, &f3, &g1, &g2, &g3, &l1, &l2, &c1, &c2, &p1, &r1, &h1, &h2, &h3, &d1, &n, &m, &f5,
        &k1, &w1,
    ]
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
        p1: p1.id(),
        r1: r1.id(),
        h1: h1.id(),
        h2: h2.id(),
        h3: h3.id(),
        d1: d1.id(),
        n: n.id(),
        m: m.id(),
        f5: f5.id(),
        k1: k1.id(),
        w1: w1.id(),
        all,
    }
}

/// The classes the requirement fixes for the corpus, so the equality below is not vacuous: the
/// window drops the interior duplicate F2 next to the deleted F1 and keeps F3; the chain across
/// runs makes G2 a duplicate; both Lab anchors keep their frame; the start that cut run C short
/// dates its epoch-end point at its deleted last frame, which keeps C1 and C2; P1, whose deleted
/// neighbour's run Q ended before the start, gets no epoch-end point of Q; R1 is a reading frame,
/// due 7 days after its entry; the published confirmation exempts the interior H2 from being a
/// duplicate; the stuck run D holds itself and N; M settles. #422 LN-7c Q: F5's publication's Lab
/// use makes it Lab; run E's epoch-end point keeps K1; the run-less Warning naming W1 is no error
/// point of the full view, so W1 is a default frame there.
fn assert_required_classes(view: &FrameRetentionView, live: &Live) {
    let frame_of = |id: &ArtifactId| {
        view.frames
            .iter()
            .find(|frame| frame.reference.artifact_id == *id)
            .expect("a live frame is in the view")
    };
    let class = |id: &ArtifactId| frame_of(id).class;
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
        (live.p1, C::Default),
        (live.r1, C::Resource),
        (live.h1, C::Default),
        (live.h2, C::Default),
        (live.h3, C::Default),
        (live.d1, C::Running),
        (live.n, C::Running),
        (live.m, C::Default),
        (live.f5, C::Lab),
        (live.k1, C::Error),
        (live.w1, C::Default),
    ] {
        assert_eq!(class(&id), expected);
    }
    let reading = frame_of(&live.r1);
    assert_eq!(
        reading.due_unix_ms,
        reading.entry_unix_ms.map(|entry| entry + 7 * DAY_MS),
        "a reading frame is due 7 days after its entry"
    );
    let epoch_end = view
        .error_points
        .iter()
        .find(|point| point.epoch_end && point.at_unix_ms == live.c3_at);
    assert!(
        epoch_end.is_some(),
        "the epoch-end point of run C, at its deleted last frame"
    );
}

/// #422 LN-7c Q, T-eq on the case fixture (adapted from A6, whose writer's views are gone): the
/// query's action for each live frame is the full view's class with E1's due time, and the kept
/// folder it names for a frame it moves is the full view's; the one accounted difference is E4's.
#[test]
fn ln7c_q_teq_the_query_decides_each_live_frame_of_the_case_fixture_as_the_full_view() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let live = corpus(&ledger, &artifacts);
    let decided = query_decisions(&ledger, &live.all, NOW);
    ledger.close().expect("close the writer");
    let full = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .expect("the whole opening")
    .frame_retention_view(NOW, SWITCHES)
    .expect("the full view");

    assert_required_classes(&full, &live);
    let reference = full
        .frames
        .iter()
        .filter(|frame| live.all.contains(&frame.reference.artifact_id))
        .collect::<Vec<_>>();
    assert_eq!(
        reference.len(),
        live.all.len(),
        "the full view lists every live frame"
    );
    assert_eq!(
        decided.keys().copied().collect::<BTreeSet<_>>(),
        live.all,
        "the query answers every live frame (all are settled)"
    );
    for frame in reference {
        let id = frame.reference.artifact_id;
        let (answer, folder) = &decided[&id];
        assert_eq!(
            answer.created_at_unix_ms, frame.reference.created_at_unix_ms,
            "the ingestion time of {id:?}"
        );
        if id == live.w1 {
            // E4: any Warning or higher record naming a frame keeps it as an error frame.
            assert_eq!(
                (frame.class, answer.class, answer.action),
                (
                    FrameRetentionClass::Default,
                    FrameRetentionClass::Error,
                    FrameQueryAction::MoveError
                ),
                "E4: the run-less Warning naming W1"
            );
            continue;
        }
        assert_eq!(answer.class, frame.class, "the class of {id:?}");
        assert_eq!(
            answer.action,
            expected_action(frame, NOW),
            "the action of {id:?} ({:?})",
            frame.class
        );
        assert_eq!(folder, &frame.folder, "the kept folder of {id:?}");
    }
}

/// LN-7 (§3.5, "write-side validate"): a frame the writer no longer holds (its file left the
/// store before the writer opened) and that a record references again with another reference is
/// refused `artifact_identity_conflict`; its identity is its first reference, read through the
/// `artifact_id` index. The refusal is fatal, as the write-side identity check's is today
/// (`retention.rs` `invalid`): the writer ends, so its close fails.
#[test]
fn ln7_an_old_artifacts_identity_refusal_goes_through_the_index() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.owner_recorded(at(1));
    let first = corpus.run(at(5));
    let old = corpus.run_frame(&first, at(10));
    corpus.close(&first.links, &[&old], at(11));
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
    assert!(refused.is_fatal(), "an identity conflict ends the writer");
    reopened
        .close()
        .expect_err("the fatal identity refusal terminated the writer");
}
