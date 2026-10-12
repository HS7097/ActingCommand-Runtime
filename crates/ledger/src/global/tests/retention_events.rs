// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 L-2, I7 (inventory-ledger): the retention event builder. Each event is built
//! with the contract's public drafts and a fixed timestamp its caller passes (never a clock),
//! over one identifier issuer and one owner epoch, and appended to an I6 root's writer; a frame
//! is published through that root's ArtifactStore.

use super::fixture::FormalRoot;
use crate::PersistedEvent;
use crate::global::Sha256SecretFingerprinter;
use actingcommand_artifact_store::{
    ArtifactEventSink, ArtifactStoreResult, ArtifactWriteContext, ArtifactWriteRequest,
};
use actingcommand_contract::{
    ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft, ArtifactPinReason,
    ArtifactPinRecord, ArtifactProducer, ArtifactRedactionState, ArtifactRetentionFact,
    ArtifactRetentionIdentity, AuditInput, CapturePayloadDraft, DiagnosticCode,
    DiagnosticDetailDraft, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventSeverity, EventSource, EventType,
    FRAME_RETENTION_POLICY_VERSION, IdentifierIssuer, IssuedInstanceId, IssuedRunId, OriginModule,
    OwnerEpoch, OwnerResourceDisposition, PerformanceMetric, PerformanceMonitorHealth,
    PerformanceMonitorStateEventData, PerformancePayloadDraft, ProjectedArtifactReference,
    ResourceQuiescence, RetentionClass, RuntimeLifecyclePhase, RuntimePayloadDraft, TaskOutcome,
    TaskPayloadDraft, TaskSemanticFact, TerminalEvent, outcome::Outcome,
};

/// The salt of the drafts' secret fingerprints: a neutral fixture word.
const FINGERPRINT_SALT: &[u8] = b"ledger-gate-fixture";

/// Ledger revision 3 (Workflow #378 A3): a record's severity is its outcome's category, so a
/// fixture asks for a severity through a registered code of that category (none of them
/// requires a value).
pub(super) fn code_of(severity: EventSeverity) -> &'static str {
    match severity {
        EventSeverity::Info => "max_steps_reached",
        EventSeverity::Warning => "step_budget_exhausted",
        EventSeverity::Error => "capture_failed",
        EventSeverity::Fatal => "watchdog_fatal_hold",
        EventSeverity::Debug | EventSeverity::Success => {
            panic!("no fixture failure code has the category {severity:?}")
        }
    }
}

/// A run: its id and the links of its task events (instance, request, correlation, task and
/// run).
pub(super) struct Run {
    pub(super) id: IssuedRunId,
    pub(super) links: EventLinksDraft,
}

/// The builder over one I6 root, one identifier issuer and one owner epoch.
pub(super) struct RetentionEvents<'r> {
    root: &'r FormalRoot,
    ids: IdentifierIssuer,
    owner: OwnerEpoch,
    fingerprinter: Sha256SecretFingerprinter,
}

/// The ArtifactStore's events, appended in order; it keeps the frame's `artifact.verified`.
struct Sink<'b, 'r> {
    events: &'b RetentionEvents<'r>,
    verified: Option<PersistedEvent>,
}

impl ArtifactEventSink for Sink<'_, '_> {
    fn append(&mut self, draft: EventDraft) -> ArtifactStoreResult<()> {
        let event = self.events.append(draft);
        if event.event_type() == EventType::ArtifactVerified {
            self.verified = Some(event);
        }
        Ok(())
    }
}

fn runtime_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

impl<'r> RetentionEvents<'r> {
    pub(super) fn new(root: &'r FormalRoot) -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let owner = *ids.mint_owner_epoch().expect("owner epoch").transport();
        Self {
            root,
            ids,
            owner,
            fingerprinter: Sha256SecretFingerprinter::new(FINGERPRINT_SALT).expect("fingerprinter"),
        }
    }

    /// A new instance id of this builder's issuer.
    pub(super) fn instance(&self) -> IssuedInstanceId {
        self.ids.mint_instance_id().expect("instance")
    }

    /// A new run on `instance`, with its links built as `global/tests.rs` builds a task's.
    pub(super) fn run(&self, instance: IssuedInstanceId) -> Run {
        let id = self.ids.mint_run_id().expect("run");
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(self.ids.mint_request_id().expect("request"))
            .with_correlation_id(self.ids.mint_correlation_id().expect("correlation"))
            .with_task_id(self.ids.mint_task_id().expect("task"))
            .with_run_id(id);
        Run { id, links }
    }

    /// Sanitizes `draft` and appends it to the root's writer.
    fn append(&self, draft: EventDraft) -> PersistedEvent {
        let draft = draft.sanitize(&self.fingerprinter).expect("fixture draft");
        self.root.ledger().append(draft).expect("fixture append")
    }

    fn event(
        &self,
        at: u64,
        severity: EventSeverity,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> PersistedEvent {
        self.append(EventDraft::new(
            self.ids.mint_event_id().expect("event"),
            at,
            severity,
            origin,
            links,
            payload,
        ))
    }

    /// Item 1: the confirmed resource quiescence of `instance` in this builder's owner epoch.
    fn quiescence(&self, instance: IssuedInstanceId, at: u64) {
        self.event(
            at,
            EventSeverity::Info,
            runtime_origin(),
            EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(self.ids.mint_request_id().expect("request"))
                .with_correlation_id(self.ids.mint_correlation_id().expect("correlation")),
            RuntimePayloadDraft::lifecycle_observed(
                self.owner,
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
        );
    }

    /// Item 2: one run-less observe frame of `instance` captured at `at`, published through the
    /// root's ArtifactStore, with its Explicit `PinRecorded` identity (`at + 1`) and its
    /// `capture.completed` (`at + 2`). Returns the frame's projected reference.
    fn observe(
        &self,
        instance: IssuedInstanceId,
        bytes: &[u8],
        at: u64,
    ) -> ProjectedArtifactReference {
        let request = self.ids.mint_request_id().expect("request");
        let correlation = self.ids.mint_correlation_id().expect("correlation");
        let frame = self.ids.mint_frame_id().expect("frame");
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(request)
            .with_correlation_id(correlation)
            .with_frame_id(frame);
        let mut sink = Sink {
            events: self,
            verified: None,
        };
        self.root
            .artifacts()
            .put(
                ArtifactWriteRequest::new(
                    ArtifactKind::CaptureFrame,
                    bytes,
                    ArtifactWriteContext::new(
                        ArtifactLinksDraft::default()
                            .with_frame_id(frame)
                            .with_correlation_id(correlation),
                        links.clone(),
                        at,
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
            self.ids.mint_event_id().expect("event"),
            at + 1,
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
                        instance_id: *instance.transport(),
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
        .sanitize(&self.fingerprinter)
        .expect("pin draft");
        let pin = verified
            .links()
            .artifact_retention_source()
            .apply_to(pin)
            .expect("pin source links");
        self.root.ledger().append(pin).expect("pin append");
        self.event(
            at + 2,
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
        );
        reference
    }

    /// Item 3: a closed capture on `instance` in this builder's owner epoch: a quiescence at
    /// `at`, the frame captured at `at + 1` (its pin and completion at `at + 2` and `at + 3`),
    /// and the closing quiescence at `at + 4`.
    pub(super) fn closed_capture(
        &self,
        instance: IssuedInstanceId,
        bytes: &[u8],
        at: u64,
    ) -> ProjectedArtifactReference {
        self.quiescence(instance, at);
        let reference = self.observe(instance, bytes, at + 1);
        self.quiescence(instance, at + 4);
        reference
    }

    /// Item 4: the run's first event, an Info `task.started` of an ordinary task run.
    pub(super) fn task_started(&self, run: &Run, at: u64) {
        self.event(
            at,
            EventSeverity::Info,
            runtime_origin(),
            run.links.clone(),
            TaskPayloadDraft::started(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
    }

    /// Item 5: a task terminal (`TerminalCommitted`) with `task_result` and the outcome of
    /// `failure_code` (its severity is that code's category, ledger revision 3).
    pub(super) fn terminal(
        &self,
        links: EventLinksDraft,
        task_result: TaskOutcome,
        failure_code: Option<&str>,
        severity: EventSeverity,
        at: u64,
    ) -> PersistedEvent {
        self.event(
            at,
            severity,
            runtime_origin(),
            links,
            TaskPayloadDraft::semantic(
                TaskSemanticFact::TerminalCommitted {
                    task_result,
                    final_page: None,
                    executed_steps: Some(0),
                    outcome: failure_code.map(Outcome::relayed),
                    scheduling_disposition: None,
                    task_timing: None,
                },
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// Item 6: a `runtime.failed` with fixture detail words, whose outcome has the category
    /// `severity` (ledger revision 3).
    pub(super) fn runtime_failed(
        &self,
        links: EventLinksDraft,
        severity: EventSeverity,
        at: u64,
    ) -> PersistedEvent {
        self.event(
            at,
            severity,
            runtime_origin(),
            links,
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::NotPerformed,
                Outcome::relayed(code_of(severity)),
                DiagnosticDetailDraft::new(
                    "fixture_category",
                    "fixture.stage",
                    "fixture_backend",
                    "fixture_operation",
                ),
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// Item 7: a `perf.monitor_degraded`, built as the capacity monitor builds one, whose
    /// outcome has the category `severity` (ledger revision 3).
    pub(super) fn monitor_degraded(
        &self,
        links: EventLinksDraft,
        severity: EventSeverity,
        at: u64,
    ) -> PersistedEvent {
        self.event(
            at,
            severity,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::PerformanceMonitor,
                EventActor::Runtime,
            ),
            links,
            PerformancePayloadDraft::monitor_degraded(
                PerformanceMonitorStateEventData {
                    observed_at_unix_ms: at,
                    health: PerformanceMonitorHealth::Degraded,
                    outcome: Some(Outcome::relayed(code_of(severity))),
                    consecutive_failures: 1,
                    terminal: false,
                    unavailable_metrics: vec![PerformanceMetric::DiskCapacity],
                },
                AuditInput::new(),
            )
            .into(),
        )
    }
}
