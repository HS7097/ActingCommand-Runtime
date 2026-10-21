// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 BT-LR, ledger revision 4 (contracts/v012-register.md §1.153; B-train plan §3 and
// §5.3 BT-LR; contract keys K31, K32). The revision is written shape first: types, readers and
// validation, no writer (the SH-1 pattern of v0.12.0). So every stored record below is built
// from the contract's types, and every expected spelling comes from the register's rows:
// - T1: each shape of LR-1 to LR-9 encodes as its row names it, decodes to itself and passes its
//   reader's validation (the event records through `EventPayload`, the task diagnostic rows
//   through `TaskDiagnosticRecord`, which the stream reader decodes).
// - T2: the readers refuse what no writer writes: a decision confirmation with a malformed hash
//   or on a scan projection (LR-1), a refused effect without its step's completed effect or a
//   second one of the step (LR-3), a second capped operation in one run (LR-7), candidate scores
//   out of their rules (LR-8) and an empty package version (LR-9).

use super::sh1::{read_back, refused, written};
use super::*;
use crate::codes::SanitizationCode;
use crate::{
    CandidateFeature, CandidateFeatureMap, CandidateFrame, CandidateIdentitySource,
    CandidateLayoutKind, CandidateProjection, CandidateRect, ProjectedCandidate, ScanAxis,
    ScanCandidate, ScanCandidateProjection, ScanCandidateProjectionParts, ScanCoverage,
    ScanCoverageState, ScanEndEvidence, ScanGatedOut, ScanReposition, ScanStartEvidence,
    ScanViewport, ScanViewportMotion, TaskEntryRecoveryRole, ValueTestProblem,
};
use serde_json::{Value, json};

// TCLOCK: every instant is a fixed constant.
const AT_MS: u64 = 1_752_147_200_000;
const LAYOUT: &str = "list/rows";
const PAGE: &str = "list_page";
const STEP: u32 = 3;
const OPERATION: &str = "confirm_choice";
// SHA-256 of no identities: the compact JSON array `[]` (`gated_out.identities_sha256`).
const NO_IDENTITIES: &str = "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";

fn hex(digit: char) -> String {
    digit.to_string().repeat(64)
}

fn frame_id(digit: char) -> crate::FrameId {
    serde_json::from_value(Value::String(format!(
        "frame_{}",
        digit.to_string().repeat(32)
    )))
    .expect("a frame id")
}

fn candidate_id(index: u32) -> String {
    format!("{LAYOUT}#{index:02}")
}

fn identity(value: &str) -> CandidateFeatureMap {
    CandidateFeatureMap::from([(
        "member".to_owned(),
        CandidateFeature::Identity {
            value: value.to_owned(),
            variant: None,
            source: CandidateIdentitySource::OcrAlias,
            distance: 0,
            confidence: Some(950),
        },
    )])
}

// A single-frame list of two fixed slots.
fn frame_projection() -> CandidateProjection {
    let slot = |index: u32, value: &str| ProjectedCandidate {
        id: candidate_id(index),
        instance_index: index,
        actionable: true,
        rect: CandidateRect {
            x: 200 + 300 * index as i32,
            y: 200,
            width: 240,
            height: 120,
        },
        click: CandidateRect {
            x: 210 + 300 * index as i32,
            y: 210,
            width: 220,
            height: 100,
        },
        features: identity(value),
    };
    CandidateProjection::new(
        PAGE,
        LAYOUT,
        CandidateLayoutKind::FixedSlots,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        vec![slot(0, "member_a"), slot(1, "member_b")],
    )
    .expect("the frame projection is valid")
}

// A complete scan of one member, read in three viewports.
fn scan_projection() -> ScanCandidateProjection {
    let viewport = |motion, content_offset_px, frame| ScanViewport {
        motion,
        content_offset_px,
        tracked_instances: 9,
        readable_instances: 6,
        unknown_instances: 0,
        frame_id: frame_id(frame),
    };
    ScanCandidateProjection::new(ScanCandidateProjectionParts {
        page_id: PAGE.to_owned(),
        layout_id: LAYOUT.to_owned(),
        axis: ScanAxis::Vertical,
        frame: CandidateFrame {
            width: 1280,
            height: 720,
        },
        coverage: ScanCoverage {
            state: ScanCoverageState::Complete,
            start: Some(ScanStartEvidence::StartCheck),
            end: Some(ScanEndEvidence::EndCheck),
            extent_px: Some(1_440),
        },
        viewports: vec![
            viewport(ScanViewportMotion::Start, 0, 'a'),
            viewport(ScanViewportMotion::Forward, 230, 'b'),
            viewport(ScanViewportMotion::Corrective, 210, 'c'),
        ],
        reposition: ScanReposition {
            rewind_viewports: 0,
            return_viewports: 1,
        },
        candidates: vec![ScanCandidate {
            id: candidate_id(0),
            instance_index: 0,
            actionable: true,
            content_rect: CandidateRect {
                x: 40,
                y: 120,
                width: 200,
                height: 200,
            },
            fixed_viewport: 0,
            features: identity("member_a"),
        }],
        unknown: Vec::new(),
        gated_out: ScanGatedOut {
            count: 0,
            identities_sha256: NO_IDENTITIES.to_owned(),
        },
    })
    .expect("the scan projection is valid")
}

// A select step that chose member `#00` and confirmed it with `confirmation`.
fn selection(
    projection: impl Into<TaskSelectionProjection>,
    confirmation: TaskSelectionConfirmation,
) -> TaskSelectionRecord {
    let projection: TaskSelectionProjection = projection.into();
    // A scan records its unknown check, with nothing unknown; a plain single-frame select none.
    let unknown_check =
        matches!(projection, TaskSelectionProjection::V2(_)).then_some(TaskSelectionUnknownCheck {
            unknown_instances: 0,
            blocking: TaskSelectionBlocking::None,
            upper_bound: TaskSelectionUpperBound::NoneUnknown,
        });
    TaskSelectionRecord {
        layout_id: LAYOUT.to_owned(),
        page_id: PAGE.to_owned(),
        projection,
        policy: TaskSelectionPolicy {
            path: "policy.json".to_owned(),
            policy_sha256: format!("sha256:{}", hex('d')),
            policy_id: "policy-a".to_owned(),
        },
        fact_snapshot_id: "snapshot-a".to_owned(),
        input_ledger_position: 1,
        input_fact_revisions: None,
        now_unix_ms: AT_MS,
        input_sha256: format!("sha256:{}", hex('e')),
        selection_result: TaskSelectionOutcome::Selected { count: 1 },
        outcome_key: "selected".to_owned(),
        selected: vec![candidate_id(0)],
        verdicts: vec![TaskSelectionVerdict {
            candidate_id: candidate_id(0),
            status: TaskSelectionCandidateStatus::Ranked,
            score_milli: Some(1_000),
            rank: Some(1),
            gates: Vec::new(),
            terms: Vec::new(),
            reasons: Vec::new(),
        }],
        reasons: vec![PolicyReasonRecord(Outcome::relayed("selection_selected"))],
        confirmation,
        unknown_check,
        multi_select: None,
    }
}

fn selected_fact(selection: TaskSelectionRecord) -> TaskSemanticFact {
    TaskSemanticFact::SelectionEvaluated {
        step_index: STEP,
        operation_label: OPERATION.to_owned(),
        selection: Box::new(selection),
    }
}

fn semantic(fact: TaskSemanticFact) -> Value {
    written(TaskPayloadDraft::semantic(fact, AuditInput::new()).into())
}

// The `contained_task_operation_runs_capped` info outcome as a record carries it (b-plan
// §2.11 item 5); its code is registered by its writer slice.
fn capped_outcome() -> Outcome {
    serde_json::from_value(json!({
        "code": "contained_task_operation_runs_capped",
        "category": "info",
        "values": {}
    }))
    .expect("the capped outcome decodes")
}

fn capped(step_index: u32) -> TaskSemanticFact {
    TaskSemanticFact::OperationCapped {
        step_index,
        operation_label: OPERATION.to_owned(),
        count: 4,
        limit_count: 4,
        outcome: capped_outcome(),
    }
}

fn effect_completed(step_index: u32) -> TaskSemanticFact {
    TaskSemanticFact::EffectCompleted {
        step_index,
        operation_label: OPERATION.to_owned(),
    }
}

fn effect_refused(step_index: u32) -> TaskSemanticFact {
    TaskSemanticFact::EffectRefused {
        step_index,
        operation_label: OPERATION.to_owned(),
        target_id: "marker/refused".to_owned(),
    }
}

fn candidate_pages(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("page_{index:02}")).collect()
}

fn score(page_id: &str, passed: bool) -> RecognitionCandidateScore {
    RecognitionCandidateScore {
        page_id: page_id.to_owned(),
        passed,
        best_score_milli: Some(if passed { 930 } else { 410 }),
        first_failed_target_id: (!passed).then(|| format!("{page_id}/title")),
    }
}

// A recognition of `pages` that matched `matched`, with `scores` of them and `omitted`.
fn recognition(
    pages: Vec<String>,
    matched: Option<&str>,
    scores: Vec<RecognitionCandidateScore>,
    omitted: u32,
) -> TaskSemanticFact {
    TaskSemanticFact::RecognitionCompleted {
        candidate_pages: pages,
        matched_page: matched.map(str::to_owned),
        frame_width: 1280,
        frame_height: 720,
        targets: Vec::new(),
        candidate_scores: Some(Box::new(RecognitionCandidateScores {
            pages: scores,
            omitted,
        })),
    }
}

fn layer_admitted(package_version: &str) -> TaskSemanticFact {
    TaskSemanticFact::EntryRecoveryPackageAdmitted {
        package_label: "neutral.layer".to_owned(),
        role: Some(TaskEntryRecoveryRole::Prerequisite),
        package_version: Some(package_version.to_owned()),
    }
}

fn layer_completed(package_version: &str) -> TaskSemanticFact {
    TaskSemanticFact::EntryRecoveryCompleted {
        package_label: "neutral.layer".to_owned(),
        final_page: "home".to_owned(),
        executed_steps: 2,
        role: Some(TaskEntryRecoveryRole::Prerequisite),
        package_version: Some(package_version.to_owned()),
    }
}

// T1 for an event record: the fact's `member` (a JSON pointer below the fact, empty for the
// whole fact) is stored as `expected` under the event type `event_type`, and the reader
// decodes, validates and encodes the record back unchanged.
fn fact_reads_back(
    label: &str,
    fact: TaskSemanticFact,
    member: &str,
    expected: &Value,
    event_type: &str,
) {
    let stored = semantic(fact);
    assert_eq!(
        stored.pointer(&format!("/payload/data/fact{member}")),
        Some(expected),
        "{label}: the stored fact"
    );
    let payload = read_back(&stored);
    assert_eq!(
        serde_json::to_value(payload.event_type()).expect("the event type encodes"),
        json!(event_type),
        "{label}: the event type"
    );
}

// T1 for a task diagnostic row: the row encodes as `expected` and decodes to itself.
fn row_reads_back(label: &str, row: &TaskDiagnosticRecord, expected: &Value) {
    assert_eq!(
        &serde_json::to_value(row).expect("the row encodes"),
        expected,
        "{label}: the row"
    );
    let decoded: TaskDiagnosticRecord = serde_json::from_value(expected.clone())
        .unwrap_or_else(|error| panic!("{label}: the row decodes: {error}"));
    assert_eq!(&decoded, row, "{label}: the row read back");
}

fn diagnostic_row(index: u64, payload: TaskDiagnosticPayload) -> TaskDiagnosticRecord {
    TaskDiagnosticRecord {
        index,
        frame_id: None,
        step_action_id: None,
        physical_action_id: None,
        parent_index: None,
        payload,
    }
}

fn target_row(
    id: &str,
    kind: &str,
    passed: bool,
    phase: &str,
    template: Option<TaskDiagnosticTemplateData>,
    value_test: Option<TaskDiagnosticValueTestData>,
) -> TaskDiagnosticPayload {
    TaskDiagnosticPayload::Target(Box::new(TaskDiagnosticTargetData {
        id: id.to_owned(),
        kind: kind.to_owned(),
        passed,
        template,
        color: None,
        source: TaskDiagnosticTargetSource::Guard {
            phase: phase.to_owned(),
        },
        sampling: None,
        failure: None,
        uniqueness: None,
        correction: None,
        value_test: value_test.map(Box::new),
        skipped: None,
    }))
}

fn row_json(index: u64, kind: &str, data: Value) -> Value {
    json!({
        "index": index,
        "frame_id": null,
        "step_action_id": null,
        "physical_action_id": null,
        "parent_index": null,
        "kind": kind,
        "data": data,
    })
}

// T1: every shape of ledger revision 4 reads back through its reader, spelled as the register
// names it (rows LR-1 to LR-9).
#[test]
fn bt_lr_t1_every_revision_4_shape_reads_back_as_registered() {
    // LR-1: a single-frame select confirmed by its decision, matched and mismatched; F2's
    // candidate-set hash may differ from the evaluated projection's.
    let projection = frame_projection();
    let evaluated = projection.candidate_set_sha256().to_owned();
    for (label, confirmation, expected) in [
        (
            "LR-1 decision_matched",
            TaskSelectionConfirmation::DecisionMatched {
                candidate_set_sha256: hex('7'),
                decision_sha256: hex('a'),
                projection: None,
            },
            json!({
                "kind": "decision_matched",
                "candidate_set_sha256": hex('7'),
                "decision_sha256": hex('a'),
            }),
        ),
        (
            "LR-1 decision_mismatched",
            TaskSelectionConfirmation::DecisionMismatched {
                candidate_set_sha256: evaluated.clone(),
                decision_sha256: hex('b'),
                projection: Some(Box::new(projection.clone())),
            },
            json!({
                "kind": "decision_mismatched",
                "candidate_set_sha256": evaluated,
                "decision_sha256": hex('b'),
                "projection": serde_json::to_value(&projection)
                    .expect("the projection encodes"),
            }),
        ),
    ] {
        let record = selection(projection.clone(), confirmation);
        record
            .validate_for_append()
            .unwrap_or_else(|error| panic!("{label}: the writer's checks pass: {error}"));
        fact_reads_back(
            label,
            selected_fact(record),
            "/selection/confirmation",
            &expected,
            "task.selection_evaluated",
        );
    }

    // LR-3: a refused effect, after its step's completed effect.
    fact_reads_back(
        "LR-3 effect_refused",
        effect_refused(STEP),
        "",
        &json!({
            "kind": "effect_refused",
            "step_index": STEP,
            "operation_label": OPERATION,
            "target_id": "marker/refused",
        }),
        "task.effect_refused",
    );

    // LR-7: a capped operation with its info outcome.
    fact_reads_back(
        "LR-7 operation_capped",
        capped(STEP),
        "",
        &json!({
            "kind": "operation_capped",
            "step_index": STEP,
            "operation_label": OPERATION,
            "count": 4,
            "limit_count": 4,
            "outcome": {
                "code": "contained_task_operation_runs_capped",
                "category": "info",
                "values": {}
            },
        }),
        "task.operation_capped",
    );

    // LR-8: the candidate pages' scores, one entry per page in candidate order, at most 16, the
    // rest counted; and a recognition without them keeps its revision-3 bytes.
    let pages = candidate_pages(18);
    let mut scores = vec![score("page_00", false), score("page_01", true)];
    scores.extend(pages[2..16].iter().map(|page| score(page, false)));
    let mut expected_scores = vec![
        json!({
            "page_id": "page_00",
            "passed": false,
            "best_score_milli": 410,
            "first_failed_target_id": "page_00/title",
        }),
        json!({"page_id": "page_01", "passed": true, "best_score_milli": 930}),
    ];
    expected_scores.extend(pages[2..16].iter().map(|page| {
        json!({
            "page_id": page,
            "passed": false,
            "best_score_milli": 410,
            "first_failed_target_id": format!("{page}/title"),
        })
    }));
    fact_reads_back(
        "LR-8 candidate_scores",
        recognition(pages.clone(), Some("page_01"), scores, 2),
        "",
        &json!({
            "kind": "recognition_completed",
            "candidate_pages": pages,
            "matched_page": "page_01",
            "frame_width": 1280,
            "frame_height": 720,
            "candidate_scores": {"pages": expected_scores, "omitted": 2},
        }),
        "task.recognition_completed",
    );
    fact_reads_back(
        "LR-8 absent",
        TaskSemanticFact::RecognitionCompleted {
            candidate_pages: vec!["page_00".to_owned()],
            matched_page: None,
            frame_width: 1280,
            frame_height: 720,
            targets: Vec::new(),
            candidate_scores: None,
        },
        "",
        &json!({
            "kind": "recognition_completed",
            "candidate_pages": ["page_00"],
            "frame_width": 1280,
            "frame_height": 720,
        }),
        "task.recognition_completed",
    );

    // LR-9: the layer package's declared version on its admission and completion.
    fact_reads_back(
        "LR-9 admitted",
        layer_admitted("2.1.0"),
        "",
        &json!({
            "kind": "entry_recovery_package_admitted",
            "package_label": "neutral.layer",
            "role": "prerequisite",
            "package_version": "2.1.0",
        }),
        "task.entry_preflight",
    );
    fact_reads_back(
        "LR-9 completed",
        layer_completed("2.1.0"),
        "",
        &json!({
            "kind": "entry_recovery_completed",
            "package_label": "neutral.layer",
            "final_page": "home",
            "executed_steps": 2,
            "role": "prerequisite",
            "package_version": "2.1.0",
        }),
        "task.entry_preflight",
    );

    // LR-2: a template row's measured brightness and contrast.
    let template = TaskDiagnosticTemplateData {
        x: 10,
        y: 20,
        width: 30,
        height: 40,
        raw_score: 0.75,
        score: 0.75,
        threshold: 0.5,
        scale_milli: None,
        luma: Some(TaskDiagnosticLumaData {
            mean_ratio_milli: 950,
            contrast_ratio_milli: 1_100,
        }),
    };
    row_reads_back(
        "LR-2 luma",
        &diagnostic_row(
            1,
            target_row(
                "button/start",
                "template",
                true,
                "guard",
                Some(template),
                None,
            ),
        ),
        &row_json(
            1,
            "target",
            json!({
                "id": "button/start",
                "kind": "template",
                "passed": true,
                "template": {
                    "x": 10,
                    "y": 20,
                    "width": 30,
                    "height": 40,
                    "raw_score": 0.75,
                    "score": 0.75,
                    "threshold": 0.5,
                    "luma": {"mean_ratio_milli": 950, "contrast_ratio_milli": 1_100},
                },
                "color": null,
                "source": {"phase": "guard"},
            }),
        ),
    );

    // LR-4: a refusal marker's target row and error row carry the guard phase `refusal`.
    row_reads_back(
        "LR-4 target",
        &diagnostic_row(
            2,
            target_row(
                "marker/refused",
                "template",
                true,
                TASK_DIAGNOSTIC_PHASE_REFUSAL,
                None,
                None,
            ),
        ),
        &row_json(
            2,
            "target",
            json!({
                "id": "marker/refused",
                "kind": "template",
                "passed": true,
                "template": null,
                "color": null,
                "source": {"phase": "refusal"},
            }),
        ),
    );
    let error_outcome = json!({
        "code": "recognition_pack_invalid",
        "category": "error",
        "values": {}
    });
    row_reads_back(
        "LR-4 error",
        &diagnostic_row(
            3,
            TaskDiagnosticPayload::Error(Box::new(TaskDiagnosticErrorData::Recognition {
                phase: TASK_DIAGNOSTIC_PHASE_REFUSAL.to_owned(),
                target_id: Some("marker/refused".to_owned()),
                error: TaskDiagnosticRecognitionError {
                    region: None,
                    timing: None,
                    sample: None,
                    outcome: serde_json::from_value(error_outcome.clone())
                        .expect("the error outcome decodes"),
                },
            })),
        ),
        &row_json(
            3,
            "error",
            json!({
                "phase": "refusal",
                "target_id": "marker/refused",
                "error": {"outcome": error_outcome},
            }),
        ),
    );

    // LR-5: a value test side whose agreement frames did not agree.
    row_reads_back(
        "LR-5 no_agreement",
        &diagnostic_row(
            4,
            target_row(
                "check/count",
                "value_test",
                false,
                "guard",
                None,
                Some(TaskDiagnosticValueTestData {
                    left: TaskDiagnosticValueOperand {
                        target_id: "text/count".to_owned(),
                        raw_text: None,
                        value: None,
                        problem: Some(ValueTestProblem::NoAgreement),
                    },
                    op: crate::ValueTestOp::Ge,
                    right: Some(TaskDiagnosticValueRight::Const { constant: 3 }),
                    passed: false,
                }),
            ),
        ),
        &row_json(
            4,
            "target",
            json!({
                "id": "check/count",
                "kind": "value_test",
                "passed": false,
                "template": null,
                "color": null,
                "source": {"phase": "guard"},
                "value_test": {
                    "left": {"target_id": "text/count", "problem": "no_agreement"},
                    "op": "ge",
                    "right": {"kind": "const", "const": 3},
                    "passed": false,
                },
            }),
        ),
    );

    // LR-6: one agreement frame's value-test row carries the guard phase `agreement_sample`.
    row_reads_back(
        "LR-6 agreement_sample",
        &diagnostic_row(
            5,
            target_row(
                "check/count",
                "value_test",
                true,
                TASK_DIAGNOSTIC_PHASE_AGREEMENT_SAMPLE,
                None,
                Some(TaskDiagnosticValueTestData {
                    left: TaskDiagnosticValueOperand {
                        target_id: "text/count".to_owned(),
                        raw_text: Some("12".to_owned()),
                        value: Some(12),
                        problem: None,
                    },
                    op: crate::ValueTestOp::Parses,
                    right: None,
                    passed: true,
                }),
            ),
        ),
        &row_json(
            5,
            "target",
            json!({
                "id": "check/count",
                "kind": "value_test",
                "passed": true,
                "template": null,
                "color": null,
                "source": {"phase": "agreement_sample"},
                "value_test": {
                    "left": {"target_id": "text/count", "raw_text": "12", "value": 12},
                    "op": "parses",
                    "passed": true,
                },
            }),
        ),
    );
}

// T2: the readers refuse what no writer writes, each row at its own check.
#[test]
fn bt_lr_t2_the_readers_refuse_what_no_writer_writes() {
    let selection_invalid = SanitizationCode::InvalidTaskSelectionRecord.code();
    let seventeen = candidate_pages(17);
    let mut sixteen_scores = vec![score("page_00", true)];
    sixteen_scores.extend(seventeen[1..16].iter().map(|page| score(page, false)));
    // Each stored record below is valid as written; its row then breaks one member of it.
    let records: [(&str, Value, crate::outcome::Code, &str); 5] = [
        // LR-1: the decision hash is not 64 lowercase hex digits.
        (
            "LR-1 decision hash",
            semantic(selected_fact(selection(
                frame_projection(),
                TaskSelectionConfirmation::DecisionMatched {
                    candidate_set_sha256: hex('7'),
                    decision_sha256: hex('a'),
                    projection: None,
                },
            ))),
            selection_invalid,
            "confirmation",
        ),
        // LR-1: a decision confirmation on a scan projection (a scan confirms by identity).
        (
            "LR-1 decision on a scan",
            semantic(selected_fact(selection(
                scan_projection(),
                TaskSelectionConfirmation::IdentityMatched {
                    candidate_id: candidate_id(0),
                    frame_id: frame_id('f'),
                    content_offset_px: 218,
                },
            ))),
            selection_invalid,
            "confirmation",
        ),
        // LR-8: two entries out of candidate order.
        (
            "LR-8 out of order",
            semantic(recognition(
                candidate_pages(2),
                None,
                vec![score("page_00", false), score("page_01", false)],
                0,
            )),
            SanitizationCode::InvalidTaskRecognitionResult.code(),
            "candidate_scores",
        ),
        // LR-8: seventeen entries listed, none omitted.
        (
            "LR-8 over the limit",
            semantic(recognition(
                seventeen.clone(),
                Some("page_00"),
                sixteen_scores,
                1,
            )),
            SanitizationCode::InvalidTaskRecognitionResult.code(),
            "candidate_scores",
        ),
        // LR-9: an empty package version.
        (
            "LR-9 empty version",
            semantic(layer_admitted("2.1.0")),
            SanitizationCode::InvalidTaskSemanticLabel.code(),
            "package_version",
        ),
    ];
    for (label, mut stored, code, field) in records {
        let fact = stored
            .pointer_mut("/payload/data/fact")
            .unwrap_or_else(|| panic!("{label}: the stored fact"));
        match label {
            "LR-1 decision hash" => {
                fact["selection"]["confirmation"] = json!({
                    "kind": "decision_matched",
                    "candidate_set_sha256": hex('7'),
                    "decision_sha256": format!("sha256:{}", hex('a')),
                });
            }
            "LR-1 decision on a scan" => {
                fact["selection"]["confirmation"] = json!({
                    "kind": "decision_matched",
                    "candidate_set_sha256": hex('7'),
                    "decision_sha256": hex('a'),
                });
            }
            "LR-8 out of order" => {
                let pages = fact["candidate_scores"]["pages"]
                    .as_array_mut()
                    .expect("the score entries");
                pages.swap(0, 1);
            }
            "LR-8 over the limit" => {
                fact["candidate_scores"] = json!({
                    "pages": seventeen
                        .iter()
                        .enumerate()
                        .map(|(index, page)| {
                            serde_json::to_value(score(page, index == 0))
                                .expect("the entry encodes")
                        })
                        .collect::<Vec<_>>(),
                    "omitted": 0,
                });
            }
            "LR-9 empty version" => {
                fact["package_version"] = json!("");
            }
            other => panic!("no member to break for {other}"),
        }
        let error = refused(stored);
        assert_eq!(error.code(), code, "{label}: the refusal");
        assert_eq!(error.field(), field, "{label}: the refused member");
    }

    // LR-3 and LR-7: the rules that tie a fact to its run's earlier facts.
    let run_rules: [(&str, TaskSemanticFact, Vec<TaskSemanticFact>, bool); 6] = [
        (
            "LR-3 after its step's completed effect",
            effect_refused(STEP),
            vec![effect_completed(STEP - 1), effect_completed(STEP)],
            true,
        ),
        (
            "LR-3 without its step's completed effect",
            effect_refused(STEP),
            vec![effect_completed(STEP - 1)],
            false,
        ),
        (
            "LR-3 a second refusal of the step",
            effect_refused(STEP),
            vec![effect_completed(STEP), effect_refused(STEP)],
            false,
        ),
        (
            "LR-7 the run's first capped operation",
            capped(STEP),
            vec![effect_completed(STEP)],
            true,
        ),
        (
            "LR-7 a second capped operation in the run",
            capped(STEP + 1),
            vec![capped(STEP), effect_completed(STEP + 1)],
            false,
        ),
        // A fact without run rules is never refused by them.
        (
            "no run rules",
            effect_completed(STEP),
            vec![capped(STEP)],
            true,
        ),
    ];
    for (label, fact, earlier, admitted) in run_rules {
        let result = fact.validate_in_run(&earlier);
        assert_eq!(result.is_ok(), admitted, "{label}: {result:?}");
        if let Err(error) = result {
            let (code, field) = match &fact {
                TaskSemanticFact::EffectRefused { .. } => {
                    (SanitizationCode::InvalidTaskEffect.code(), "effect_refused")
                }
                _ => (
                    SanitizationCode::InvalidTaskSemanticPayload.code(),
                    "operation_capped",
                ),
            };
            assert_eq!(error.code(), code, "{label}: the refusal");
            assert_eq!(error.field(), field, "{label}: the refused member");
        }
    }
}
