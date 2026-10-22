// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #412 Lab B-3 S0 (ledger revision 4 row LR-10; register §1.153 LR-10 and §1.159;
//! design `rd-1021/train-0129/lab-b3-design.md` §1.1, §3, §5 S0 T0-1 to T0-3; contract keys K39
//! to K41): the records of a Lab operation that runs an operation its package declares. The
//! selection `operation`, the prepared record's `operation` and the terminal record's
//! `operation_result` round-trip, while element and coordinate records keep their bytes (T0-1);
//! the record's validation refuses each rule's violation, and a request refuses `operation`
//! with an arrival condition (T0-2); the shared evidence verifier passes a two-attempt operation
//! and refuses an input intent no attempt reports and a frame no entry lists (T0-3).
//!
//! The fixture is one neutral request on a 320x240 instance: its lease, an entry frame on
//! `page_a`, attempt 1's input frame, the prepared record, attempt 1's tap, a confirmation frame
//! still on `page_a`, attempt 2's input frame and tap, and a last frame: captured after the lease
//! expired (the operation fails at `after_frame`) or showing `page_b` (the run completes on
//! `key_a`). Every record is built through the contract's own drafts and artifact issuer.

use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::page_projection::{
    FrameIdentity, FrameKind, Geometry, PageProjection, Point, ProjectionMetrics, Rect,
    WindowCompleteness,
};
use actingcommand_contract::{
    ArtifactId, ArtifactIssuePolicy, ArtifactKind, ArtifactLinksDraft, ArtifactPayloadDraft,
    ArtifactProducer, ArtifactRedactionState, ArtifactStoreIssuer, AuditInput, CommandPayloadDraft,
    ContainedLabOperationRequest, ContainedLabOperationResult, ContainedPageObservation,
    DiagnosticCode, EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventPayloadDraft, EventSeverity, EventSource, IdentifierIssuer, InputAction,
    InputPayloadDraft, InputSamplingEvidence, InputSamplingRegion, IssuedActionId,
    IssuedCorrelationId, IssuedInstanceId, IssuedLeaseId, IssuedRequestId,
    LAB_OPERATION_PREPARED_SCHEMA, LAB_OPERATION_TERMINAL_SCHEMA, LabArrivalCondition,
    LabDeclaredOperation, LabDeclaredOperationResult, LabEvidenceReference, LabGuardOutcome,
    LabOperationAttempt, LabOperationEnd, LabOperationEvidence, LabOperationFailure,
    LabOperationFrame, LabOperationFrameEntry, LabOperationPrepared, LabOperationRecord,
    LabOperationSelection, LabOperationStage, LabProjectionHint, LeasePayloadDraft,
    ObservationFacts, OriginModule, PageObservationStatus, ProjectedArtifactReference,
    ProjectedEvent, ProjectionPayload, ReadonlyObservation, RecognitionVerdict, RetentionClass,
    RuntimeCaptureBackend, RuntimeErrorCode, RuntimeErrorProjection, SanitizationError,
    SecretField, SecretFingerprinter, Sha256Fingerprint, TerminalEvent,
    verify_lab_operation_evidence,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const RECORDED_FROM_UNIX_MS: u64 = 1_800_000_000_000;
const PACKAGE: &str = "neutral.lab_b3";
const OPERATION: &str = "op_a";
const PHASE: &str = "phase_a";
const PAGE_A: &str = "neutral/page_a";
const PAGE_B: &str = "neutral/page_b";
const GUARD_TARGET: &str = "page/page_a";
const KEY: &str = "key_a";
const SEED: u64 = 7;

/// The fixture records carry no secret.
struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the fixture records carry no secret")
    }
}

/// How the fixture operation ends after attempt 2's input.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// The last frame is captured after the lease expired: the operation fails at
    /// `after_frame`, both inputs performed.
    LeaseExpired,
    /// The last frame shows `page_b` and the step ends the package's run on `key_a`.
    RunCompleted,
}

/// What else the request's records hold.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Extra {
    Nothing,
    /// An input intent of the request that no attempt reports.
    Intent,
    /// A frame of the request that no entry lists.
    Frame,
}

/// One request's records, appended in sequence order, and the bytes of its artifacts.
struct Ledger {
    ids: IdentifierIssuer,
    artifact_issuer: ArtifactStoreIssuer,
    instance: IssuedInstanceId,
    request: IssuedRequestId,
    correlation: IssuedCorrelationId,
    lease: IssuedLeaseId,
    events: Vec<ProjectedEvent>,
    artifacts: BTreeMap<ArtifactId, Vec<u8>>,
}

impl Ledger {
    fn new() -> Self {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        Self {
            artifact_issuer: ArtifactStoreIssuer::new().expect("artifact issuer"),
            instance: ids.mint_instance_id().expect("instance id"),
            request: ids.mint_request_id().expect("request id"),
            correlation: ids.mint_correlation_id().expect("correlation id"),
            lease: ids.mint_lease_id().expect("lease id"),
            ids,
            events: Vec::new(),
            artifacts: BTreeMap::new(),
        }
    }

    /// The request's own links: its instance, request, correlation and lease.
    fn links(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance)
            .with_request_id(self.request)
            .with_correlation_id(self.correlation)
            .with_lease_id(self.lease)
    }

    /// Appends one record at the next sequence, as a Forensic read projects it.
    fn append(
        &mut self,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
        artifacts: Vec<ProjectedArtifactReference>,
    ) -> TerminalEvent {
        let sequence = u64::try_from(self.events.len()).expect("ledger length") + 1;
        let sanitized = EventDraft::new(
            self.ids.mint_event_id().expect("event id"),
            RECORDED_FROM_UNIX_MS + sequence,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            links,
            payload,
        )
        .sanitize(&NoSecrets)
        .expect("a fixture record");
        self.events.push(ProjectedEvent {
            views: Vec::new(),
            artifact_evictions: Vec::new(),
            schema_version: sanitized.schema_version().to_owned(),
            sequence,
            event_id: *sanitized.event_id(),
            timestamp_unix_ms: sanitized.timestamp_unix_ms(),
            event_type: sanitized.event_type(),
            severity: sanitized.severity(),
            sensitivity: sanitized.sensitivity(),
            origin: sanitized.origin().clone(),
            links: sanitized.links().clone(),
            payload_schema: sanitized.payload_schema().to_owned(),
            payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
            artifacts,
        });
        TerminalEvent {
            sequence,
            event_id: *sanitized.event_id(),
        }
    }

    /// Stores `bytes` as an artifact of the request (a frame's when `frame` is given): its
    /// created and verified records; the reference and the verified record.
    fn store(&mut self, frame: bool, bytes: &[u8]) -> (ProjectedArtifactReference, TerminalEvent) {
        let mut artifact_links =
            ArtifactLinksDraft::default().with_correlation_id(self.correlation);
        let mut event_links = self.links();
        let (kind, policy) = if frame {
            let frame = self.ids.mint_frame_id().expect("frame id");
            artifact_links = artifact_links.with_frame_id(frame);
            event_links = event_links.with_frame_id(frame);
            (
                ArtifactKind::CaptureFrame,
                ArtifactIssuePolicy::new(
                    ArtifactProducer::CaptureStore,
                    RetentionClass::DebugFull,
                    ArtifactRedactionState::NotRequired,
                ),
            )
        } else {
            (
                ArtifactKind::DiagnosticJson,
                ArtifactIssuePolicy::new(
                    ArtifactProducer::CapturePipeline,
                    RetentionClass::DebugFull,
                    ArtifactRedactionState::Pending,
                ),
            )
        };
        let reference = self
            .artifact_issuer
            .issue(kind, artifact_links, bytes, RECORDED_FROM_UNIX_MS, policy)
            .expect("a fixture artifact")
            .reference()
            .project(true);
        self.append(
            event_links.clone(),
            ArtifactPayloadDraft::created(AuditInput::new()).into(),
            vec![reference.clone()],
        );
        let verified = self.append(
            event_links,
            ArtifactPayloadDraft::verified(AuditInput::new()).into(),
            vec![reference.clone()],
        );
        self.artifacts.insert(reference.artifact_id, bytes.to_vec());
        (reference, verified)
    }

    /// Captures one frame of the request; `lease_valid`: its lease was still valid after it.
    fn frame(&mut self, lease_valid: bool) -> LabOperationFrame {
        let bytes = format!("neutral frame {}", self.events.len()).into_bytes();
        let (artifact, verified) = self.store(true, &bytes);
        LabOperationFrame {
            observation: ReadonlyObservation::new(
                WIDTH,
                HEIGHT,
                RecognitionVerdict::FrameDecoded,
                RuntimeCaptureBackend::NemuIpc,
                artifact,
            )
            .expect("a fixture frame"),
            verified,
            lease_valid_after_capture: lease_valid,
        }
    }

    /// One tap of the request: its intent and its committed outcome.
    fn tap(&mut self, action: IssuedActionId, tap: &InputAction) -> (TerminalEvent, TerminalEvent) {
        let links = self.links().with_action_id(action);
        let intent = self.append(
            links.clone(),
            InputPayloadDraft::intent(tap.event_action(), AuditInput::new()).into(),
            Vec::new(),
        );
        let input = self.append(
            links,
            InputPayloadDraft::committed(
                tap.event_action(),
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            Vec::new(),
        );
        (intent, input)
    }
}

fn entry(
    frame: &LabOperationFrame,
    attempt: u32,
    after_input: bool,
    page: Option<&str>,
) -> LabOperationFrameEntry {
    LabOperationFrameEntry {
        frame: frame.clone(),
        attempt,
        after_input,
        page: page.map(str::to_owned),
    }
}

/// The draw of a tap in the operation's click box: three centre-hotspot candidates, the landing
/// history chose `chosen`.
fn sampling(seed: u64, chosen: u32) -> InputSamplingEvidence {
    InputSamplingEvidence::center_hotspot_least_recent(
        seed,
        InputSamplingRegion::new(40, 30, 20, 10).expect("the click box"),
        250,
        8,
        3,
        chosen,
    )
    .expect("a fixture draw")
}

fn passed_guard() -> LabGuardOutcome {
    LabGuardOutcome::Passed {
        page_id: PAGE_A.to_owned(),
        target_id: GUARD_TARGET.to_owned(),
        target_kind: "template".to_owned(),
    }
}

/// The fixture request's evidence: a declared operation of two attempts that ends as `ending`,
/// with `extra` records.
fn declared(ending: Ending, extra: Extra) -> LabOperationEvidence {
    let completed = ending == Ending::RunCompleted;
    let mut ledger = Ledger::new();
    let instance_id = *ledger.instance.transport();
    let request_id = *ledger.request.transport();
    let correlation_id = *ledger.correlation.transport();
    let lease_id = *ledger.lease.transport();
    ledger.append(
        ledger.links(),
        LeasePayloadDraft::granted(
            EventAction::LeaseAcquire,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
        .into(),
        Vec::new(),
    );
    let entry_frame = ledger.frame(true);
    let step_1 = ledger.frame(true);
    let tap_1 = InputAction::Tap { x: 50, y: 34 };
    let tap_2 = InputAction::Tap { x: 47, y: 36 };
    let prepared = LabOperationPrepared {
        schema_version: LAB_OPERATION_PREPARED_SCHEMA.to_owned(),
        request_id,
        correlation_id,
        instance_id,
        package_label: Some(PACKAGE.to_owned()),
        lease_id: Some(lease_id),
        selection: LabOperationSelection::Operation {
            operation_id: OPERATION.to_owned(),
            seed: Some(SEED),
        },
        operation: Some(LabDeclaredOperation {
            phase_id: Some(PHASE.to_owned()),
            from_page: PAGE_A.to_owned(),
            destination_pages: vec![PAGE_B.to_owned()],
            max_attempts: 2,
            recheck_frames: 0,
            guard: passed_guard(),
            sampling: Some(sampling(SEED, 1)),
        }),
        projection_hint: LabProjectionHint::default(),
        after: None,
        before_frame: Some(step_1.clone()),
        before_projection: None,
        selected_element: None,
        geometry: None,
        action: Some(tap_1.clone()),
    };
    let (prepared_artifact, prepared_verified) = ledger.store(
        false,
        &serde_json::to_vec(&prepared).expect("prepared record JSON"),
    );
    let action_1 = ledger.ids.mint_action_id().expect("action id");
    let (intent_1, input_1) = ledger.tap(action_1, &tap_1);
    let check_1 = ledger.frame(true);
    let step_2 = ledger.frame(true);
    let action_2 = ledger.ids.mint_action_id().expect("action id");
    let (intent_2, input_2) = ledger.tap(action_2, &tap_2);
    if extra == Extra::Intent {
        let stray = ledger.ids.mint_action_id().expect("action id");
        ledger.append(
            ledger.links().with_action_id(stray),
            InputPayloadDraft::intent(tap_2.event_action(), AuditInput::new()).into(),
            Vec::new(),
        );
    }
    let check_2 = ledger.frame(completed);
    if extra == Extra::Frame {
        ledger.frame(completed);
    }
    if completed {
        ledger.append(
            ledger.links(),
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
            Vec::new(),
        );
    }
    let attempts = vec![
        LabOperationAttempt {
            attempt: 1,
            input_frame_index: 1,
            guard: passed_guard(),
            action: tap_1,
            sampling: Some(sampling(SEED, 1)),
            input_returned: true,
            input_action_id: Some(*action_1.transport()),
            input_intent: Some(intent_1),
            input_event: Some(input_1),
            effect: EffectDisposition::Performed,
            end_page: Some(PAGE_A.to_owned()),
        },
        LabOperationAttempt {
            attempt: 2,
            input_frame_index: 3,
            guard: passed_guard(),
            action: tap_2,
            sampling: Some(sampling(SEED ^ 2, 0)),
            input_returned: true,
            input_action_id: Some(*action_2.transport()),
            input_intent: Some(intent_2),
            input_event: Some(input_2),
            effect: EffectDisposition::Performed,
            end_page: completed.then(|| PAGE_B.to_owned()),
        },
    ];
    let frames = vec![
        entry(&entry_frame, 0, false, Some(PAGE_A)),
        entry(&step_1, 1, false, Some(PAGE_A)),
        entry(&check_1, 1, true, Some(PAGE_A)),
        entry(&step_2, 2, false, Some(PAGE_A)),
        entry(&check_2, 2, true, completed.then_some(PAGE_B)),
    ];
    let record = LabOperationRecord {
        schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_owned(),
        prepared,
        prepared_artifact: LabEvidenceReference {
            artifact: prepared_artifact,
            verified: prepared_verified,
        },
        input_returned: true,
        input_action_id: Some(*action_2.transport()),
        input_intent: Some(intent_2),
        input_event: Some(input_2),
        effect: EffectDisposition::Performed,
        after_frame: Some(check_2),
        after_projection: None,
        arrival: None,
        operation_result: Some(LabDeclaredOperationResult {
            end: if completed {
                LabOperationEnd::RunCompleted
            } else {
                LabOperationEnd::Failed
            },
            final_page: completed.then(|| PAGE_B.to_owned()),
            outcome_key: completed.then(|| KEY.to_owned()),
            attempts,
            frames,
        }),
        failure: (!completed).then(|| LabOperationFailure {
            stage: LabOperationStage::AfterFrame,
            code: RuntimeErrorCode::LeaseExpired.as_str().to_owned(),
            error: RuntimeErrorProjection::new(RuntimeErrorCode::LeaseExpired, false),
            event: None,
        }),
        cleanup_failure: None,
    };
    let (terminal_artifact, terminal_verified) = ledger.store(
        false,
        &serde_json::to_vec(&record).expect("terminal record JSON"),
    );
    let command = if completed {
        CommandPayloadDraft::validated(
            EventAction::RuntimeDebugDo,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
    } else {
        CommandPayloadDraft::rejected(
            EventAction::RuntimeDebugDo,
            DiagnosticCode::CommandRejected,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
    };
    let terminal = ledger.append(ledger.links(), command.into(), Vec::new());
    let actions = [*action_1.transport(), *action_2.transport()];
    let action_events = ledger
        .events
        .iter()
        .filter(|event| {
            event
                .links
                .action_id()
                .is_some_and(|action| actions.contains(action))
        })
        .cloned()
        .collect();
    let lease_events = ledger
        .events
        .iter()
        .filter(|event| event.links.lease_id() == Some(&lease_id))
        .cloned()
        .collect();
    LabOperationEvidence {
        operation: ContainedLabOperationResult {
            record,
            terminal_artifact: LabEvidenceReference {
                artifact: terminal_artifact,
                verified: terminal_verified,
            },
        },
        terminal,
        events: ledger.events,
        lease_events,
        action_events,
        artifacts: ledger.artifacts,
    }
}

/// A request of `selection` (an element or explicit coordinates) whose before frame was not
/// projected: the record as the Runtime writes it before LR-10.
fn refused(selection: LabOperationSelection) -> ContainedLabOperationResult {
    let mut ledger = Ledger::new();
    let lease_id = *ledger.lease.transport();
    ledger.append(
        ledger.links(),
        LeasePayloadDraft::granted(
            EventAction::LeaseAcquire,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
        .into(),
        Vec::new(),
    );
    let before = ledger.frame(true);
    let prepared = LabOperationPrepared {
        schema_version: LAB_OPERATION_PREPARED_SCHEMA.to_owned(),
        request_id: *ledger.request.transport(),
        correlation_id: *ledger.correlation.transport(),
        instance_id: *ledger.instance.transport(),
        package_label: Some(PACKAGE.to_owned()),
        lease_id: Some(lease_id),
        selection,
        operation: None,
        projection_hint: LabProjectionHint::default(),
        after: None,
        before_frame: Some(before),
        before_projection: None,
        selected_element: None,
        geometry: None,
        action: None,
    };
    let (artifact, verified) = ledger.store(
        false,
        &serde_json::to_vec(&prepared).expect("prepared record JSON"),
    );
    let record = LabOperationRecord {
        schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_owned(),
        prepared,
        prepared_artifact: LabEvidenceReference { artifact, verified },
        input_returned: false,
        input_action_id: None,
        input_intent: None,
        input_event: None,
        effect: EffectDisposition::NotPerformed,
        after_frame: None,
        after_projection: None,
        arrival: None,
        operation_result: None,
        failure: Some(LabOperationFailure {
            stage: LabOperationStage::BeforeProjection,
            code: RuntimeErrorCode::RecognitionFailed.as_str().to_owned(),
            error: RuntimeErrorProjection::new(RuntimeErrorCode::RecognitionFailed, false),
            event: None,
        }),
        cleanup_failure: None,
    };
    let (artifact, verified) = ledger.store(
        false,
        &serde_json::to_vec(&record).expect("terminal record JSON"),
    );
    ContainedLabOperationResult {
        record,
        terminal_artifact: LabEvidenceReference { artifact, verified },
    }
}

/// The bytes of a prepared record as the members before LR-10 spell it, in their order.
fn prepared_bytes_before_lr10(prepared: &LabOperationPrepared) -> Value {
    assert!(
        prepared.after.is_none(),
        "the fixture has no arrival condition"
    );
    json!({
        "schema_version": prepared.schema_version,
        "request_id": prepared.request_id,
        "correlation_id": prepared.correlation_id,
        "instance_id": prepared.instance_id,
        "package_label": prepared.package_label,
        "lease_id": prepared.lease_id,
        "selection": prepared.selection,
        "projection_hint": prepared.projection_hint,
        "before_frame": prepared.before_frame,
        "before_projection": prepared.before_projection,
        "selected_element": prepared.selected_element,
        "geometry": prepared.geometry,
        "action": prepared.action,
    })
}

fn code_of(result: Result<(), actingcommand_contract::RuntimeContractError>) -> &'static str {
    result.expect_err("refused").code().as_str()
}

// T0-1 (contract key K39): a record of every LR-10 member, attempts and frames encodes as the
// register spells it, decodes to itself and passes validation; the guard's trusted-coordinate
// form and every `lab_operation_end` token read back; element and coordinate records serialize
// byte for byte as before LR-10.
#[test]
fn b3_s0_t0_1_every_lr10_member_round_trips_and_element_and_coordinate_records_keep_their_bytes() {
    let operation = declared(Ending::RunCompleted, Extra::Nothing).operation;
    let bytes = serde_json::to_vec(&operation).expect("encode the record");
    let decoded: ContainedLabOperationResult =
        serde_json::from_slice(&bytes).expect("decode the record");
    assert_eq!(decoded, operation, "every LR-10 member round-trips");
    decoded
        .validate()
        .expect("the decoded record passes validation");

    let value = serde_json::to_value(&operation).expect("record JSON");
    let prepared = &value["record"]["prepared"];
    assert_eq!(
        prepared["selection"],
        json!({"mode": "operation", "operation_id": OPERATION, "seed": SEED})
    );
    assert_eq!(prepared["operation"]["phase_id"], PHASE);
    assert_eq!(prepared["operation"]["from_page"], PAGE_A);
    assert_eq!(prepared["operation"]["destination_pages"], json!([PAGE_B]));
    assert_eq!(prepared["operation"]["max_attempts"], 2);
    assert_eq!(prepared["operation"]["recheck_frames"], 0);
    assert_eq!(
        prepared["operation"]["guard"],
        json!({"status": "passed", "page_id": PAGE_A, "target_id": GUARD_TARGET,
               "target_kind": "template"})
    );
    let result = &value["record"]["operation_result"];
    assert_eq!(result["end"], "run_completed");
    assert_eq!(result["final_page"], PAGE_B);
    assert_eq!(result["outcome_key"], KEY);
    assert_eq!(result["attempts"][1]["attempt"], 2);
    assert_eq!(result["attempts"][1]["input_frame_index"], 3);
    assert_eq!(result["attempts"][1]["end_page"], PAGE_B);
    assert_eq!(result["frames"][0]["attempt"], 0);
    assert_eq!(result["frames"][2]["after_input"], true);
    assert_eq!(result["frames"][4]["page"], PAGE_B);

    let trusted = serde_json::to_value(LabGuardOutcome::TrustedCoordinate).expect("guard JSON");
    assert_eq!(trusted, json!({"status": "trusted_coordinate"}));
    assert_eq!(
        serde_json::from_value::<LabGuardOutcome>(trusted).expect("decode the guard"),
        LabGuardOutcome::TrustedCoordinate
    );
    for end in LabOperationEnd::ALL {
        let spelled = serde_json::to_value(end).expect("end JSON");
        assert_eq!(spelled, json!(end.as_str()), "the token of {end}");
        assert_eq!(
            serde_json::from_value::<LabOperationEnd>(spelled).expect("decode the end"),
            *end
        );
    }

    for selection in [
        LabOperationSelection::Element {
            id: OPERATION.to_owned(),
        },
        LabOperationSelection::Coordinates {
            action: InputAction::Tap { x: 50, y: 34 },
        },
    ] {
        let operation = refused(selection);
        let record = &operation.record;
        assert_eq!(
            serde_json::to_vec(&record.prepared).expect("prepared JSON"),
            serde_json::to_vec(&prepared_bytes_before_lr10(&record.prepared))
                .expect("prepared JSON before LR-10"),
            "a prepared record without a declared operation keeps its bytes"
        );
        let before_lr10 = json!({
            "schema_version": record.schema_version,
            "prepared": prepared_bytes_before_lr10(&record.prepared),
            "prepared_artifact": record.prepared_artifact,
            "input_returned": record.input_returned,
            "input_action_id": record.input_action_id,
            "input_intent": record.input_intent,
            "input_event": record.input_event,
            "effect": record.effect,
            "after_frame": record.after_frame,
            "after_projection": record.after_projection,
            "failure": record.failure,
            "cleanup_failure": record.cleanup_failure,
        });
        assert_eq!(
            serde_json::to_vec(record).expect("terminal JSON"),
            serde_json::to_vec(&before_lr10).expect("terminal JSON before LR-10"),
            "a terminal record without a declared operation keeps its bytes"
        );
        operation
            .validate()
            .expect("the record still passes validation");
    }
}

/// A recognised projection of `frame` on `page_a`, as a before projection.
fn projection_of(
    frame: &LabOperationFrame,
    operation: &ContainedLabOperationResult,
) -> ContainedPageObservation {
    let record = &operation.record;
    ContainedPageObservation {
        instance_id: record.prepared.instance_id,
        package_label: record.prepared.package_label.clone(),
        frame: frame.observation.clone(),
        status: PageObservationStatus::Recognized,
        projection: PageProjection {
            schema_version: "actingcommand.page-projection.v1".to_string(),
            page: PAGE_A.to_string(),
            state: "recognized".to_string(),
            matched: true,
            standby: false,
            frame: FrameIdentity {
                kind: FrameKind::Artifact,
                sha256: frame
                    .observation
                    .artifact()
                    .sha256
                    .trim_start_matches("sha256:")
                    .to_string(),
                width: WIDTH,
                height: HEIGHT,
            },
            elements: Vec::new(),
            unscoped_controls: Vec::new(),
            missing: Vec::new(),
            fields: Vec::new(),
            truncated: false,
            output_truncated: false,
            omitted_count: 0,
            page_window_completeness: WindowCompleteness::Complete,
            window: None,
            metrics: ProjectionMetrics::default(),
            content_sha256: "0".repeat(64),
        },
        facts: ObservationFacts::default(),
        artifact: record.prepared_artifact.artifact.clone(),
        projection_sequence: record.prepared_artifact.verified.sequence,
        projection_event_id: record.prepared_artifact.verified.event_id,
    }
}

type Change = fn(&mut ContainedLabOperationResult);

// T0-2 (contract key K40): the record's validation refuses the violation of each LR-10 rule
// (`invalid_lab_operation_record`), each change alone on an otherwise valid two-attempt record;
// a request refuses `operation` with an arrival condition and an operation id that is empty or
// holds NUL (`invalid_lab_operation_selection`).
#[test]
fn b3_s0_t0_2_validation_refuses_each_lr10_rule_violation_and_operation_with_after() {
    let valid = declared(Ending::LeaseExpired, Extra::Nothing).operation;
    valid.validate().expect("the two-attempt record is valid");
    let rows: [(&str, Change); 22] = [
        ("the prepared operation without its result", |o| {
            o.record.operation_result = None;
        }),
        ("the result without the prepared operation", |o| {
            o.record.prepared.operation = None;
        }),
        ("both members on an element selection", |o| {
            o.record.prepared.selection = LabOperationSelection::Element {
                id: OPERATION.to_owned(),
            };
        }),
        ("a projection geometry", |o| {
            let rect = Rect {
                x: 40,
                y: 30,
                width: 20,
                height: 10,
            };
            o.record.prepared.geometry = Some(Geometry::Tap {
                rect,
                point: Point { x: 50, y: 34 },
            });
        }),
        ("a selected element", |o| {
            o.record.prepared.selected_element = Some(json!({"id": OPERATION}));
        }),
        ("a before projection", |o| {
            let frame = o
                .record
                .prepared
                .before_frame
                .clone()
                .expect("before frame");
            o.record.prepared.before_projection = Some(projection_of(&frame, o));
        }),
        ("an arrival condition", |o| {
            o.record.prepared.after = Some(LabArrivalCondition {
                page_id: PAGE_B.to_owned(),
                timeout_ms: 1000,
            });
        }),
        ("attempts that do not run 1, 2", |o| {
            let result = o.record.operation_result.as_mut().expect("result");
            result.attempts[1].attempt = 3;
            result.frames[3].attempt = 3;
            result.frames[4].attempt = 3;
        }),
        ("more attempts than max_attempts", |o| {
            o.record
                .prepared
                .operation
                .as_mut()
                .expect("operation")
                .max_attempts = 1;
        }),
        ("an intent before the previous attempt's outcome", |o| {
            let result = o.record.operation_result.as_mut().expect("result");
            let intent = result.attempts[0].input_intent;
            result.attempts[1].input_intent = intent;
            o.record.input_intent = intent;
        }),
        ("an outcome before its intent", |o| {
            let result = o.record.operation_result.as_mut().expect("result");
            let mut event = result.attempts[0].input_event.expect("outcome");
            event.sequence = result.attempts[0].input_intent.expect("intent").sequence - 1;
            result.attempts[0].input_event = Some(event);
        }),
        ("an input bound to a frame after the input", |o| {
            o.record.operation_result.as_mut().expect("result").attempts[1].input_frame_index = 4;
        }),
        ("an input bound to another attempt's frame", |o| {
            o.record.operation_result.as_mut().expect("result").attempts[1].input_frame_index = 1;
        }),
        ("an input frame index past the frames", |o| {
            o.record.operation_result.as_mut().expect("result").attempts[1].input_frame_index = 5;
        }),
        ("frames not in capture order", |o| {
            let result = o.record.operation_result.as_mut().expect("result");
            result.frames[2].frame.verified.sequence = result.frames[1].frame.verified.sequence;
        }),
        ("more than 256 frames", |o| {
            let result = o.record.operation_result.as_mut().expect("result");
            let last = result.frames.last().expect("frames").clone();
            for added in 1..=252 {
                let mut frame = last.clone();
                frame.frame.verified.sequence += added;
                result.frames.push(frame);
            }
            o.record.after_frame = result.frames.last().map(|entry| entry.frame.clone());
        }),
        ("input members that are not the last attempt's", |o| {
            o.record.input_action_id =
                o.record.operation_result.as_ref().expect("result").attempts[0].input_action_id;
        }),
        ("an effect that is not the attempts' aggregate", |o| {
            o.record.effect = EffectDisposition::Indeterminate;
        }),
        ("a completed end with a failure", |o| {
            o.record.operation_result.as_mut().expect("result").end = LabOperationEnd::Confirmed;
        }),
        ("a failed end without a failure", |o| {
            o.record.failure = None;
        }),
        ("a prepared record that does not state attempt 1", |o| {
            o.record
                .prepared
                .operation
                .as_mut()
                .expect("operation")
                .guard = LabGuardOutcome::TrustedCoordinate;
        }),
        ("an after frame that is not the last frame", |o| {
            o.record.after_frame = o.record.operation_result.as_ref().expect("result").frames[3]
                .frame
                .clone()
                .into();
        }),
    ];
    for (row, change) in rows {
        let mut operation = valid.clone();
        change(&mut operation);
        assert_eq!(
            code_of(operation.validate()),
            RuntimeContractCode::InvalidLabOperationRecord.as_str(),
            "{row}"
        );
    }
    let mut pages = valid.clone();
    pages
        .record
        .prepared
        .operation
        .as_mut()
        .expect("operation")
        .destination_pages = (0..17).map(|page| format!("neutral/page_{page}")).collect();
    assert_eq!(
        code_of(pages.validate()),
        RuntimeContractCode::InvalidLabOperationRecord.as_str(),
        "more than 16 destination pages"
    );
    // A selection `operation` without its members is only a request that failed before any
    // input; one that did not fail is refused.
    let mut unfailed = refused(LabOperationSelection::Operation {
        operation_id: OPERATION.to_owned(),
        seed: None,
    });
    unfailed.record.failure = None;
    assert_eq!(
        code_of(unfailed.validate()),
        RuntimeContractCode::InvalidLabOperationRecord.as_str(),
        "a selection operation without its members that did not fail"
    );

    let request = |selection: LabOperationSelection, after: Option<LabArrivalCondition>| {
        ContainedLabOperationRequest {
            package_path: "/neutral/package.zip".to_owned(),
            selection,
            projection_hint: LabProjectionHint::default(),
            after,
        }
    };
    let operation = |id: &str| LabOperationSelection::Operation {
        operation_id: id.to_owned(),
        seed: Some(SEED),
    };
    request(operation(OPERATION), None)
        .validate()
        .expect("a declared operation without an arrival condition is a valid request");
    for (row, refused) in [
        (
            "operation with an arrival condition",
            request(
                operation(OPERATION),
                Some(LabArrivalCondition {
                    page_id: PAGE_B.to_owned(),
                    timeout_ms: 1000,
                }),
            ),
        ),
        ("an empty operation id", request(operation(""), None)),
        (
            "an operation id with NUL",
            request(operation("op\0a"), None),
        ),
    ] {
        assert_eq!(
            code_of(refused.validate()),
            RuntimeContractCode::InvalidLabOperationSelection.as_str(),
            "{row}"
        );
    }
}

// T0-3 (contract key K41): the shared verifier passes a two-attempt declared operation, one
// intent and one outcome per attempt; it refuses an input intent of the request that no attempt
// reports (`runtime_lab_unreported_input`) and a frame of the request that no entry lists
// (`runtime_lab_frame_budget_mismatch`).
#[test]
fn b3_s0_t0_3_the_verifier_passes_two_attempts_and_refuses_an_extra_intent_or_an_unlisted_frame() {
    verify_lab_operation_evidence(&declared(Ending::LeaseExpired, Extra::Nothing))
        .expect("a two-attempt declared operation verifies");
    assert_eq!(
        code_of(verify_lab_operation_evidence(&declared(
            Ending::LeaseExpired,
            Extra::Intent
        ))),
        RuntimeContractCode::RuntimeLabUnreportedInput.as_str(),
        "an input intent no attempt reports"
    );
    assert_eq!(
        code_of(verify_lab_operation_evidence(&declared(
            Ending::LeaseExpired,
            Extra::Frame
        ))),
        RuntimeContractCode::RuntimeLabFrameBudgetMismatch.as_str(),
        "a frame no entry lists"
    );
}
