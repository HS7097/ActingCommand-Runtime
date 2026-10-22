// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #412 Lab B-3 S0, T0-4 (ledger revision 4 row LR-10; register §1.159; design
// `rd-1021/train-0129/lab-b3-design.md` §3.3 item 4, §5 S0, decision Q12; contract key K42):
// `resource restore` gives the record of a Lab operation that ran an operation its package
// declares the gap `lab_declared_operation` and restores no operation from it.
//
// A synthetic `neutral` game, server `test`, 320x240: page `page_a` is told by a blue first pixel
// and page `page_b` by a red one; the package's operation `op_a` goes from `page_a` to `page_b`.
// The record is that operation run once in the Lab and confirmed on `page_b`.

use crate::package_directory::admission_deadline;
use crate::{ResourceRestoreRecord, ResourceRestoreRequest, restore_authoring_draft};
use actingcommand_contract::{
    ArtifactKind, ArtifactProducer, ArtifactRedactionState, ContainedLabOperationResult,
    CorrelationId, EffectDisposition, EventType, FrameId, IdentifierIssuer, InputAction,
    LAB_OPERATION_PREPARED_SCHEMA, LAB_OPERATION_TERMINAL_SCHEMA, LabDeclaredOperation,
    LabDeclaredOperationResult, LabEvidenceReference, LabGuardOutcome, LabOperationAttempt,
    LabOperationEnd, LabOperationFrame, LabOperationFrameEntry, LabOperationPrepared,
    LabOperationRecord, LabOperationSelection, LabProjectionHint, ProjectedArtifactReference,
    ReadonlyObservation, RecognitionVerdict, RetentionClass, RuntimeCaptureBackend, TerminalEvent,
};
use actingcommand_pack_containment::{Containment, InstanceId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const RECORDED_FROM_UNIX_MS: u64 = 1_800_000_000_000;
const PACKAGE: &str = "neutral.lab_b3";
const OPERATION: &str = "op_a";
const PAGE_A: &str = "neutral/page_a";
const PAGE_B: &str = "neutral/page_b";

fn probe(page: &str, expected: [u8; 3]) -> Value {
    json!({
        "id": format!("{page}_colour"),
        "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
        "expected": expected
    })
}

/// The source package: `op_a` from `page_a` to `page_b`.
fn source_entries() -> BTreeMap<String, Vec<u8>> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "Lab B-3 S0 restore fixture",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [probe("page_a", [0, 0, 255]), probe("page_b", [255, 0, 0])],
        "page_rules": {
            "page_a": {"required": ["page_a_colour"]},
            "page_b": {"required": ["page_b_colour"]}
        },
        "entry_page": "page_a",
        "target_page": "page_b",
        "operations": [{
            "id": OPERATION, "from": "page_a", "to": "page_b",
            "click": {"kind": "point", "x": 50, "y": 34},
            "unguarded_trusted_coordinate": true, "retryable": false
        }],
        "scheduling_outcome": {"mappings": [{
            "outcome_key": "reached_k",
            "effect": "no_designated_effect",
            "terminal_pages": ["page_b"]
        }]}
    });
    BTreeMap::from([
        (
            "control.json".to_owned(),
            serde_json::to_vec_pretty(&control).expect("control JSON"),
        ),
        (
            "resources/operations/resources.json".to_owned(),
            br#"{"schema_version":"1.0","resources":[],"resource_count":0}"#.to_vec(),
        ),
        (
            "resources/operations/task/task.json".to_owned(),
            serde_json::to_vec_pretty(&task).expect("task JSON"),
        ),
    ])
}

/// The reference the artifact store gives `bytes` of `kind`, as a ledger read projects it.
fn artifact(
    ids: &IdentifierIssuer,
    kind: ArtifactKind,
    frame_id: Option<FrameId>,
    correlation_id: CorrelationId,
    bytes: &[u8],
) -> ProjectedArtifactReference {
    let artifact_id = *ids.mint_artifact_id().expect("artifact id").transport();
    let sha256 = format!("sha256:{:x}", Sha256::digest(bytes));
    let canonical = serde_json::to_value(artifact_id).expect("artifact id JSON");
    let (producer, redaction_state) = match kind {
        ArtifactKind::CaptureFrame => (
            ArtifactProducer::CaptureStore,
            ArtifactRedactionState::NotRequired,
        ),
        _ => (
            ArtifactProducer::CapturePipeline,
            ArtifactRedactionState::Pending,
        ),
    };
    ProjectedArtifactReference {
        artifact_id,
        kind,
        run_id: None,
        frame_id,
        correlation_id: Some(correlation_id),
        object_key: Some(format!(
            "artifacts/{}/{}.{}",
            &sha256[7..9],
            canonical.as_str().expect("artifact id text"),
            kind.extension()
        )),
        media_type: kind.media_type(),
        byte_count: u64::try_from(bytes.len()).expect("byte count"),
        sha256,
        created_at_unix_ms: RECORDED_FROM_UNIX_MS,
        producer,
        retention_class: RetentionClass::DebugFull,
        redaction_state,
    }
}

/// `op_a` run once in the Lab and confirmed on `page_b`, records at the sequences of its
/// request: the lease 1, the input frame 3, the prepared record 5, the intent 6, the tap 7, the
/// confirmation frame 9, the terminal record 12 and the command 13.
fn declared_record() -> ResourceRestoreRecord {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let correlation_id = *ids
        .mint_correlation_id()
        .expect("correlation id")
        .transport();
    let at = |sequence: u64| TerminalEvent {
        sequence,
        event_id: *ids.mint_event_id().expect("event id").transport(),
    };
    let frame = |sequence: u64| {
        let frame_id = *ids.mint_frame_id().expect("frame id").transport();
        let bytes = format!("neutral frame {sequence}").into_bytes();
        LabOperationFrame {
            observation: ReadonlyObservation::new(
                WIDTH,
                HEIGHT,
                RecognitionVerdict::FrameDecoded,
                RuntimeCaptureBackend::NemuIpc,
                artifact(
                    &ids,
                    ArtifactKind::CaptureFrame,
                    Some(frame_id),
                    correlation_id,
                    &bytes,
                ),
            )
            .expect("a fixture frame"),
            verified: at(sequence),
            lease_valid_after_capture: true,
        }
    };
    let input_frame = frame(3);
    let tap = InputAction::Tap { x: 50, y: 34 };
    let prepared = LabOperationPrepared {
        schema_version: LAB_OPERATION_PREPARED_SCHEMA.to_owned(),
        request_id: *ids.mint_request_id().expect("request id").transport(),
        correlation_id,
        instance_id: *ids.mint_instance_id().expect("instance id").transport(),
        package_label: Some(PACKAGE.to_owned()),
        lease_id: Some(*ids.mint_lease_id().expect("lease id").transport()),
        selection: LabOperationSelection::Operation {
            operation_id: OPERATION.to_owned(),
            seed: None,
        },
        operation: Some(LabDeclaredOperation {
            phase_id: None,
            from_page: PAGE_A.to_owned(),
            destination_pages: vec![PAGE_B.to_owned()],
            max_attempts: 1,
            recheck_frames: 0,
            guard: LabGuardOutcome::TrustedCoordinate,
            sampling: None,
        }),
        projection_hint: LabProjectionHint::default(),
        after: None,
        before_frame: Some(input_frame.clone()),
        before_projection: None,
        selected_element: None,
        geometry: None,
        action: Some(tap.clone()),
    };
    let prepared_bytes = serde_json::to_vec(&prepared).expect("prepared record JSON");
    let action_id = *ids.mint_action_id().expect("action id").transport();
    let intent = at(6);
    let input = at(7);
    let confirmation = frame(9);
    let record = LabOperationRecord {
        schema_version: LAB_OPERATION_TERMINAL_SCHEMA.to_owned(),
        prepared,
        prepared_artifact: LabEvidenceReference {
            artifact: artifact(
                &ids,
                ArtifactKind::DiagnosticJson,
                None,
                correlation_id,
                &prepared_bytes,
            ),
            verified: at(5),
        },
        input_returned: true,
        input_action_id: Some(action_id),
        input_intent: Some(intent),
        input_event: Some(input),
        effect: EffectDisposition::Performed,
        after_frame: Some(confirmation.clone()),
        after_projection: None,
        arrival: None,
        operation_result: Some(LabDeclaredOperationResult {
            end: LabOperationEnd::Confirmed,
            final_page: Some(PAGE_B.to_owned()),
            outcome_key: None,
            attempts: vec![LabOperationAttempt {
                attempt: 1,
                input_frame_index: 0,
                guard: LabGuardOutcome::TrustedCoordinate,
                action: tap,
                sampling: None,
                input_returned: true,
                input_action_id: Some(action_id),
                input_intent: Some(intent),
                input_event: Some(input),
                effect: EffectDisposition::Performed,
                end_page: Some(PAGE_B.to_owned()),
            }],
            frames: vec![
                LabOperationFrameEntry {
                    frame: input_frame,
                    attempt: 1,
                    after_input: false,
                    page: Some(PAGE_A.to_owned()),
                },
                LabOperationFrameEntry {
                    frame: confirmation,
                    attempt: 1,
                    after_input: true,
                    page: Some(PAGE_B.to_owned()),
                },
            ],
        }),
        failure: None,
        cleanup_failure: None,
    };
    let terminal_bytes = serde_json::to_vec(&record).expect("terminal record JSON");
    ResourceRestoreRecord {
        operation: ContainedLabOperationResult {
            record,
            terminal_artifact: LabEvidenceReference {
                artifact: artifact(
                    &ids,
                    ArtifactKind::DiagnosticJson,
                    None,
                    correlation_id,
                    &terminal_bytes,
                ),
                verified: at(12),
            },
        },
        terminal: at(13),
        input_event_type: Some(EventType::InputCommitted),
    }
}

// T0-4 (contract key K42): a declared operation's record restores to the gap
// `lab_declared_operation` and to no operation.
#[test]
fn b3_s0_t0_4_restore_gives_a_declared_operation_the_gap_lab_declared_operation_and_no_operation() {
    let mut containment = Containment::new();
    let package = containment
        .load_content_entries(
            &InstanceId::new("neutral-lab-b3").expect("instance"),
            source_entries(),
            false,
            admission_deadline(),
        )
        .expect("the source package is admitted");
    let record = declared_record();
    let request_id = record.operation.record.prepared.request_id;
    let restored = restore_authoring_draft(
        package,
        &[record],
        &[],
        &ResourceRestoreRequest {
            task_id: "restored_task".to_owned(),
            through_sequence: 13,
            entry_page: None,
            target_pages: Vec::new(),
            goal: None,
        },
    )
    .expect("the restore succeeds");
    assert_eq!(
        restored.report["gaps"],
        json!([{"request_id": request_id, "codes": ["lab_declared_operation"]}]),
        "the declared operation's record is the gap lab_declared_operation"
    );
    assert_eq!(
        restored.report["operation_count"], 0,
        "the declared operation's record restores no operation"
    );
}
