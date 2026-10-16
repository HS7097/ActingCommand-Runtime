// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L4a (frozen model S0 v2.1 §1.9, §3.4 and §6 row L4a): the record shapes of a
// scan select (projection v2; the confirmations identity_matched, identity_unconfirmed and
// reposition_failed; unknown_check). Every §3.4 path shape round-trips and validates, for a scan
// select (v2) and for a single-frame select that declares on_empty (v1); each of the seven path
// rules of §1.9 has records that break it; the reader bounds are 256 viewports, 64 members and
// 64 unknown positions.
//
// Workflow #357 DB-3s (frozen model M-357b v1.0 §5, §12.6 T1–T15): the record shape of a
// multi select, below the SEL-L4a tests.

use super::*;
use crate::outcome::RawSource;
use crate::{
    CANDIDATE_PROJECTION_BUDGET_EXCEEDED, CANDIDATE_PROJECTION_MAX_BYTES, CandidateFeature,
    CandidateFeatureMap, CandidateFrame, CandidateIdentitySource, CandidateLayoutKind,
    CandidateRect, INVALID_CANDIDATE_PROJECTION, ProjectedCandidate, ScanAxis, ScanCandidate,
    ScanCandidateProjectionParts, ScanCoverage, ScanEndEvidence, ScanGatedOut, ScanReposition,
    ScanStartEvidence, ScanUnknownCause, ScanUnknownPosition, ScanViewport, ScanViewportMotion,
    TaskSemanticFact,
};
use serde_json::{Value, json};
use std::io::Write as _;

const LAYOUT: &str = "list/rows";
const PAGE: &str = "list_page";
// SHA-256 of no identities: the compact JSON array `[]` (`gated_out.identities_sha256`,
// selection-graph.md "Record per path").
const NO_IDENTITIES: &str = "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";

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

fn content_rect(position: u32) -> CandidateRect {
    CandidateRect {
        x: 40,
        y: 120 + 240 * position as i32,
        width: 200,
        height: 200,
    }
}

fn member(index: u32, fixed_viewport: u32, value: &str) -> ScanCandidate {
    ScanCandidate {
        id: candidate_id(index),
        instance_index: index,
        actionable: true,
        content_rect: content_rect(index),
        fixed_viewport,
        features: identity(value),
    }
}

// A position that never read a Known identity.
fn unread(position_index: u32, first_read_viewport: u32) -> ScanUnknownPosition {
    ScanUnknownPosition {
        position_index,
        content_rect: content_rect(position_index),
        first_read_viewport,
        cause: ScanUnknownCause::IdentityUnread,
        identity: None,
        identity_unknown: Some(CandidateUnknownReason::OutOfDomain),
    }
}

// A position the evaluator dropped as unknown, which keeps its identity.
fn dropped(position_index: u32, first_read_viewport: u32, value: &str) -> ScanUnknownPosition {
    ScanUnknownPosition {
        position_index,
        content_rect: content_rect(position_index),
        first_read_viewport,
        cause: ScanUnknownCause::UnknownDropped,
        identity: Some(value.to_owned()),
        identity_unknown: None,
    }
}

fn coverage(state: ScanCoverageState) -> ScanCoverage {
    match state {
        ScanCoverageState::NotStarted => ScanCoverage {
            state,
            start: None,
            end: None,
            extent_px: None,
        },
        ScanCoverageState::Incomplete => ScanCoverage {
            state,
            start: Some(ScanStartEvidence::StartCheck),
            end: None,
            extent_px: None,
        },
        ScanCoverageState::EarlyStopped => ScanCoverage {
            state,
            start: Some(ScanStartEvidence::RewindCheck),
            end: None,
            extent_px: Some(960),
        },
        ScanCoverageState::Complete => ScanCoverage {
            state,
            start: Some(ScanStartEvidence::StartCheck),
            end: Some(ScanEndEvidence::EndCheck),
            extent_px: Some(1_440),
        },
    }
}

fn viewport(motion: ScanViewportMotion, content_offset_px: i32, frame: char) -> ScanViewport {
    ScanViewport {
        motion,
        content_offset_px,
        tracked_instances: 9,
        readable_instances: 6,
        unknown_instances: 0,
        frame_id: frame_id(frame),
    }
}

// A start, a forward advance and the corrective that closed its gap; none before the start.
fn viewports(state: ScanCoverageState) -> Vec<ScanViewport> {
    if state == ScanCoverageState::NotStarted {
        return Vec::new();
    }
    vec![
        viewport(ScanViewportMotion::Start, 0, 'a'),
        viewport(ScanViewportMotion::Forward, 230, 'b'),
        viewport(ScanViewportMotion::Corrective, 210, 'c'),
    ]
}

fn scan(
    state: ScanCoverageState,
    candidates: Vec<ScanCandidate>,
    unknown: Vec<ScanUnknownPosition>,
) -> ScanCandidateProjection {
    ScanCandidateProjection::new(ScanCandidateProjectionParts {
        page_id: PAGE.to_owned(),
        layout_id: LAYOUT.to_owned(),
        axis: ScanAxis::Vertical,
        frame: CandidateFrame {
            width: 1280,
            height: 720,
        },
        coverage: coverage(state),
        viewports: viewports(state),
        reposition: ScanReposition {
            rewind_viewports: 0,
            return_viewports: 1,
        },
        candidates,
        unknown,
        gated_out: ScanGatedOut {
            count: 0,
            identities_sha256: NO_IDENTITIES.to_owned(),
        },
    })
    .expect("the scan projection is valid")
}

// Two fixed slots; the second reads an out-of-domain identity when `unknown_second` is set.
fn frame_projection(unknown_second: bool) -> CandidateProjection {
    let slot = |index: u32, features: CandidateFeatureMap| ProjectedCandidate {
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
        features,
    };
    let second = if unknown_second {
        CandidateFeatureMap::from([(
            "member".to_owned(),
            CandidateFeature::Unknown {
                reason: CandidateUnknownReason::OutOfDomain,
                source: Some(CandidateIdentitySource::OcrAlias),
            },
        )])
    } else {
        identity("member_b")
    };
    CandidateProjection::new(
        PAGE,
        LAYOUT,
        CandidateLayoutKind::FixedSlots,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        vec![slot(0, identity("member_a")), slot(1, second)],
    )
    .expect("the frame projection is valid")
}

fn ranked(index: u32, score_milli: i64, rank: u32) -> TaskSelectionVerdict {
    TaskSelectionVerdict {
        candidate_id: candidate_id(index),
        status: TaskSelectionCandidateStatus::Ranked,
        score_milli: Some(score_milli),
        rank: Some(rank),
        gates: Vec::new(),
        terms: Vec::new(),
        reasons: Vec::new(),
    }
}

fn record(
    projection: impl Into<TaskSelectionProjection>,
    selection_result: TaskSelectionOutcome,
) -> TaskSelectionRecord {
    let (outcome_key, reason) = match selection_result {
        TaskSelectionOutcome::Selected { .. } => ("selected", "selection_selected"),
        TaskSelectionOutcome::Empty => ("empty", "selection_empty"),
        TaskSelectionOutcome::Insufficient { .. } => ("insufficient", "selection_insufficient"),
        TaskSelectionOutcome::Ambiguous { .. } => ("ambiguous", "selection_ambiguous"),
        TaskSelectionOutcome::Unknown { .. } => ("unknown", "selection_unknown"),
    };
    TaskSelectionRecord {
        layout_id: LAYOUT.to_owned(),
        page_id: PAGE.to_owned(),
        projection: projection.into(),
        policy: TaskSelectionPolicy {
            path: "policy.json".to_owned(),
            policy_sha256: format!("sha256:{}", "d".repeat(64)),
            policy_id: "policy-a".to_owned(),
        },
        fact_snapshot_id: "snapshot-a".to_owned(),
        input_ledger_position: 1,
        input_fact_revisions: None,
        now_unix_ms: 1_752_147_200_000,
        input_sha256: format!("sha256:{}", "e".repeat(64)),
        selection_result,
        outcome_key: outcome_key.to_owned(),
        selected: Vec::new(),
        verdicts: Vec::new(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed(reason))],
        confirmation: TaskSelectionConfirmation::NotAttempted,
        unknown_check: None,
        multi_select: None,
    }
}

// The record selects member `#00`, whose verdict scores `score_milli`.
fn chose_first(mut record: TaskSelectionRecord, score_milli: i64) -> TaskSelectionRecord {
    record.selected = vec![candidate_id(0)];
    record.verdicts.insert(0, ranked(0, score_milli, 1));
    record
}

fn check(
    unknown_instances: u32,
    blocking: TaskSelectionBlocking,
    upper_bound: TaskSelectionUpperBound,
) -> Option<TaskSelectionUnknownCheck> {
    Some(TaskSelectionUnknownCheck {
        unknown_instances,
        blocking,
        upper_bound,
    })
}

fn nothing_unknown() -> Option<TaskSelectionUnknownCheck> {
    check(
        0,
        TaskSelectionBlocking::None,
        TaskSelectionUpperBound::NoneUnknown,
    )
}

const SELECTED: TaskSelectionOutcome = TaskSelectionOutcome::Selected { count: 1 };

// A scan that selected member `#00` (score 1000) with nothing unknown and the given confirmation.
fn scan_selected(
    state: ScanCoverageState,
    confirmation: TaskSelectionConfirmation,
) -> TaskSelectionRecord {
    let mut record = chose_first(
        record(
            scan(state, vec![member(0, 0, "member_a")], Vec::new()),
            SELECTED,
        ),
        1_000,
    );
    record.unknown_check = nothing_unknown();
    record.confirmation = confirmation;
    record
}

fn identity_matched() -> TaskSelectionConfirmation {
    TaskSelectionConfirmation::IdentityMatched {
        candidate_id: candidate_id(0),
        frame_id: frame_id('f'),
        content_offset_px: 218,
    }
}

// The record passes the writer's checks, survives JSON unchanged, and passes the reader's.
fn round_trip(label: &str, record: &TaskSelectionRecord) -> TaskSelectionRecord {
    record
        .validate_for_append()
        .unwrap_or_else(|error| panic!("{label}: the record is valid: {error}"));
    let text = serde_json::to_string(record).expect("the record encodes");
    let decoded: TaskSelectionRecord = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{label}: the record decodes: {error}"));
    assert_eq!(&decoded, record, "{label}");
    decoded
        .validate()
        .unwrap_or_else(|error| panic!("{label}: the decoded record is valid: {error}"));
    decoded
}

fn refused(label: &str, record: &TaskSelectionRecord, field: &str) {
    let Err(error) = record.validate() else {
        panic!("{label}: the record is refused");
    };
    assert_eq!(error.code(), INVALID, "{label}");
    assert_eq!(error.field(), field, "{label}");
}

fn as_json(record: &TaskSelectionRecord) -> Value {
    serde_json::to_value(record).expect("the record encodes")
}

fn from_json(value: Value) -> TaskSelectionRecord {
    serde_json::from_value(value).expect("the record decodes")
}

#[test]
fn l4a_v1_records_keep_their_bytes_and_single_frame_on_empty_paths_round_trip() {
    // A single-frame select without on_empty: no unknown_check member, and the projection
    // encodes exactly as the v1 projection itself.
    let projection = frame_projection(false);
    let mut plain = chose_first(record(projection.clone(), SELECTED), 1_000);
    plain.confirmation = TaskSelectionConfirmation::Matched {
        candidate_set_sha256: projection.candidate_set_sha256().to_owned(),
        projection: None,
    };
    let value = as_json(&plain);
    assert!(value.get("unknown_check").is_none());
    assert_eq!(
        value["projection"],
        serde_json::to_value(&projection).expect("the v1 projection encodes")
    );
    let decoded = round_trip("v1 without on_empty", &plain);
    assert!(matches!(decoded.projection, TaskSelectionProjection::V1(_)));

    // A single-frame select with on_empty: P4, P6, P7, P8 and the H1 = H2 confirmation.
    let mut p4 = record(frame_projection(false), TaskSelectionOutcome::Empty);
    p4.unknown_check = nothing_unknown();

    let mut p6 = record(frame_projection(true), TaskSelectionOutcome::Empty);
    p6.unknown_check = check(
        1,
        TaskSelectionBlocking::UnknownInstances,
        TaskSelectionUpperBound::Max { milli: 2_000 },
    );

    let mut p7 = chose_first(record(frame_projection(true), SELECTED), 1_000);
    p7.unknown_check = check(
        1,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::Max { milli: 2_000 },
    );

    let mut p8 = chose_first(record(frame_projection(false), SELECTED), 1_000);
    p8.unknown_check = nothing_unknown();
    p8.confirmation = TaskSelectionConfirmation::CaptureFailed {
        outcome: Outcome::relayed("selection_confirmation_capture_failed"),
    };

    let mut matched = chose_first(record(projection.clone(), SELECTED), 1_000);
    matched.unknown_check = nothing_unknown();
    matched.confirmation = TaskSelectionConfirmation::Matched {
        candidate_set_sha256: projection.candidate_set_sha256().to_owned(),
        projection: None,
    };

    for (label, shape) in [
        ("v1 P4", p4),
        ("v1 P6", p6),
        ("v1 P7", p7),
        ("v1 P8", p8),
        ("v1 matched", matched),
    ] {
        let decoded = round_trip(label, &shape);
        assert!(
            matches!(decoded.projection, TaskSelectionProjection::V1(_)),
            "{label}"
        );
    }
}

#[test]
fn l4a_every_scan_path_shape_round_trips() {
    let complete = ScanCoverageState::Complete;

    let mut p2 = record(
        scan(ScanCoverageState::NotStarted, Vec::new(), Vec::new()),
        TaskSelectionOutcome::Unknown {
            reason: TaskSelectionUnknownReason::FactMissing,
        },
    );
    p2.unknown_check = nothing_unknown();

    let mut p3 = record(
        scan(
            ScanCoverageState::Incomplete,
            vec![member(0, 0, "member_a")],
            vec![unread(1, 1)],
        ),
        TaskSelectionOutcome::Unknown {
            reason: TaskSelectionUnknownReason::FieldMissing,
        },
    );
    p3.unknown_check = check(
        1,
        TaskSelectionBlocking::None,
        TaskSelectionUpperBound::Unbounded,
    );

    let mut p4 = record(
        scan(complete, Vec::new(), Vec::new()),
        TaskSelectionOutcome::Empty,
    );
    p4.unknown_check = nothing_unknown();

    let mut p5 = record(
        scan(
            complete,
            vec![member(0, 0, "member_a"), member(1, 1, "member_b")],
            Vec::new(),
        ),
        TaskSelectionOutcome::Ambiguous {
            candidate_ids: vec![candidate_id(0), candidate_id(1)],
        },
    );
    p5.verdicts = vec![ranked(0, 1_000, 1), ranked(1, 1_000, 1)];
    p5.unknown_check = nothing_unknown();

    let mut p6 = record(
        scan(complete, Vec::new(), vec![dropped(0, 1, "member_b")]),
        TaskSelectionOutcome::Empty,
    );
    p6.unknown_check = check(
        1,
        TaskSelectionBlocking::UnknownInstances,
        TaskSelectionUpperBound::Max { milli: 2_000 },
    );

    let mut p7 = chose_first(
        record(
            scan(complete, vec![member(0, 0, "member_a")], vec![unread(1, 2)]),
            SELECTED,
        ),
        1_000,
    );
    p7.unknown_check = check(
        1,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::Max { milli: 3_000 },
    );

    let p8_capture = scan_selected(
        ScanCoverageState::EarlyStopped,
        TaskSelectionConfirmation::CaptureFailed {
            outcome: Outcome::relayed("candidate_sample_page_or_geometry_changed"),
        },
    );
    let p8_page = scan_selected(complete, TaskSelectionConfirmation::PageChanged);
    let p8_guard = scan_selected(
        complete,
        TaskSelectionConfirmation::GuardFailed {
            outcome: Outcome::relayed("contained_task_guard_refused"),
        },
    );
    let p9 = scan_selected(
        complete,
        TaskSelectionConfirmation::RepositionFailed {
            outcome: Outcome::relayed("selection_page_changed"),
            stage: TaskSelectionRepositionStage::Return,
        },
    );
    // L3 registers this code; until then the relay keeps its spelling as `raw_code`.
    let p12 = scan_selected(
        complete,
        TaskSelectionConfirmation::RepositionFailed {
            outcome: Outcome::relayed("selection_scan_alignment_rejected"),
            stage: TaskSelectionRepositionStage::Confirm,
        },
    );
    let p10_different = scan_selected(
        complete,
        TaskSelectionConfirmation::IdentityUnconfirmed {
            identity_state: TaskSelectionIdentityState::Different,
            observed_identity: Some("member_c".to_owned()),
            identity_unknown: None,
        },
    );
    let p10_unknown = scan_selected(
        complete,
        TaskSelectionConfirmation::IdentityUnconfirmed {
            identity_state: TaskSelectionIdentityState::Unknown,
            observed_identity: None,
            identity_unknown: Some(CandidateUnknownReason::LowConfidence),
        },
    );
    let p10_absent = scan_selected(
        complete,
        TaskSelectionConfirmation::IdentityUnconfirmed {
            identity_state: TaskSelectionIdentityState::Absent,
            observed_identity: None,
            identity_unknown: None,
        },
    );
    let p11 = scan_selected(ScanCoverageState::EarlyStopped, identity_matched());

    for (label, shape) in [
        ("P2", p2),
        ("P3", p3),
        ("P4", p4),
        ("P5", p5),
        ("P6", p6),
        ("P7", p7),
        ("P8 capture", p8_capture),
        ("P8 page", p8_page),
        ("P8 guard", p8_guard),
        ("P9", p9),
        ("P12", p12),
        ("P10 different", p10_different),
        ("P10 unknown", p10_unknown),
        ("P10 absent", p10_absent),
        ("P11", p11.clone()),
    ] {
        let decoded = round_trip(label, &shape);
        assert!(
            matches!(decoded.projection, TaskSelectionProjection::V2(_)),
            "{label}"
        );
        // A reposition failure is the record's outcome, as a guard or capture failure is.
        if let TaskSelectionConfirmation::RepositionFailed { outcome, .. } = &shape.confirmation {
            let fact = TaskSemanticFact::SelectionEvaluated {
                step_index: 1,
                operation_label: "select".to_owned(),
                selection: Box::new(decoded),
            };
            assert_eq!(fact.outcome(), Some(outcome), "{label}");
        }
    }

    // The projection is read by its schema version; another version is not read.
    let mut value = as_json(&p11);
    value["projection"]["schema_version"] = Value::from("actingcommand.candidate-projection.v3");
    assert!(serde_json::from_value::<TaskSelectionRecord>(value).is_err());
}

#[test]
fn l4a_path_rule_1_refuses_a_decision_its_coverage_cannot_hold() {
    // `empty` after an incomplete scan.
    let mut empty = record(
        scan(ScanCoverageState::Incomplete, Vec::new(), Vec::new()),
        TaskSelectionOutcome::Empty,
    );
    empty.unknown_check = nothing_unknown();
    refused("empty, incomplete", &empty, "path_rule_1");

    // `unknown` after a complete scan.
    let mut unknown = record(
        scan(ScanCoverageState::Complete, Vec::new(), Vec::new()),
        TaskSelectionOutcome::Unknown {
            reason: TaskSelectionUnknownReason::FactStale,
        },
    );
    unknown.unknown_check = nothing_unknown();
    refused("unknown, complete", &unknown, "path_rule_1");
}

#[test]
fn l4a_path_rule_2_refuses_a_scan_that_does_not_begin_with_its_start_viewport() {
    // A started scan whose first viewport is a forward advance. The hash leaves the viewports
    // out, so the reader sees a sealed projection.
    let mut value = as_json(&scan_selected(
        ScanCoverageState::Complete,
        identity_matched(),
    ));
    value["projection"]["viewports"][0]["motion"] = Value::from("forward");
    let error = from_json(value)
        .validate()
        .expect_err("a scan that does not begin with its start is refused");
    assert_eq!(error.code(), INVALID_CANDIDATE_PROJECTION);
    assert_eq!(error.field(), "viewports");

    // A scan that never started but holds a viewport.
    let mut never = record(
        scan(ScanCoverageState::NotStarted, Vec::new(), Vec::new()),
        TaskSelectionOutcome::Unknown {
            reason: TaskSelectionUnknownReason::FactMissing,
        },
    );
    never.unknown_check = nothing_unknown();
    let mut value = as_json(&never);
    value["projection"]["viewports"] =
        serde_json::to_value(viewports(ScanCoverageState::Complete)).expect("the viewports encode");
    let error = from_json(value)
        .validate()
        .expect_err("a scan that never started holds no viewport");
    assert_eq!(error.code(), INVALID_CANDIDATE_PROJECTION);
    assert_eq!(error.field(), "viewports");
}

#[test]
fn l4a_path_rule_3_refuses_an_unknown_count_or_bound_the_unknown_list_does_not_bear() {
    let projection = || {
        scan(
            ScanCoverageState::Complete,
            Vec::new(),
            vec![dropped(0, 1, "member_b")],
        )
    };
    // Two unknown instances counted, one listed.
    let mut count = record(projection(), TaskSelectionOutcome::Empty);
    count.unknown_check = check(
        2,
        TaskSelectionBlocking::UnknownInstances,
        TaskSelectionUpperBound::Max { milli: 2_000 },
    );
    refused("count", &count, "path_rule_3");

    // `none_unknown` with an unknown instance.
    let mut bound = record(projection(), TaskSelectionOutcome::Empty);
    bound.unknown_check = check(
        1,
        TaskSelectionBlocking::UnknownInstances,
        TaskSelectionUpperBound::NoneUnknown,
    );
    refused("bound", &bound, "path_rule_3");
}

#[test]
fn l4a_path_rule_4_ties_unknown_instances_blocking_to_empty_or_insufficient_with_an_unknown() {
    // `empty` with an unknown instance that does not block.
    let mut unblocked = record(
        scan(
            ScanCoverageState::Complete,
            Vec::new(),
            vec![dropped(0, 1, "member_b")],
        ),
        TaskSelectionOutcome::Empty,
    );
    unblocked.unknown_check = check(
        1,
        TaskSelectionBlocking::None,
        TaskSelectionUpperBound::Max { milli: 2_000 },
    );
    refused("empty, unblocked", &unblocked, "path_rule_4");

    // `insufficient` without an unknown instance, blocked by unknown instances.
    let mut blocked = record(
        scan(ScanCoverageState::Complete, Vec::new(), Vec::new()),
        TaskSelectionOutcome::Insufficient {
            surviving: 0,
            required: 1,
        },
    );
    blocked.unknown_check = check(
        0,
        TaskSelectionBlocking::UnknownInstances,
        TaskSelectionUpperBound::NoneUnknown,
    );
    refused("insufficient, blocked", &blocked, "path_rule_4");
}

#[test]
fn l4a_path_rule_5_ties_outranked_possible_to_a_bound_above_the_chosen_score() {
    let projection = || {
        scan(
            ScanCoverageState::Complete,
            vec![member(0, 0, "member_a")],
            vec![unread(1, 2)],
        )
    };
    // `outranked_possible` under a maximum equal to the chosen score.
    let mut at_score = chose_first(record(projection(), SELECTED), 1_000);
    at_score.unknown_check = check(
        1,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::Max { milli: 1_000 },
    );
    refused("maximum at the chosen score", &at_score, "path_rule_5");

    // An unbounded unknown instance that does not block the choice.
    let mut unbounded = chose_first(record(projection(), SELECTED), 1_000);
    unbounded.unknown_check = check(
        1,
        TaskSelectionBlocking::None,
        TaskSelectionUpperBound::Unbounded,
    );
    unbounded.confirmation = identity_matched();
    refused("unbounded, unblocked", &unbounded, "path_rule_5");
}

#[test]
fn l4a_path_rule_6_refuses_verdicts_beside_an_aborted_scan_decision() {
    let mut aborted = record(
        scan(
            ScanCoverageState::Incomplete,
            vec![member(0, 0, "member_a")],
            vec![unread(1, 1)],
        ),
        TaskSelectionOutcome::Unknown {
            reason: TaskSelectionUnknownReason::FieldMissing,
        },
    );
    aborted.verdicts = vec![ranked(0, 1_000, 1)];
    aborted.unknown_check = check(
        1,
        TaskSelectionBlocking::None,
        TaskSelectionUpperBound::Unbounded,
    );
    refused("unknown with a verdict", &aborted, "path_rule_6");
}

#[test]
fn l4a_path_rule_7_ties_the_confirmation_to_an_unblocked_choice_and_its_projection() {
    let complete = ScanCoverageState::Complete;

    // A selected, unblocked scan that did not confirm.
    refused(
        "not attempted",
        &scan_selected(complete, TaskSelectionConfirmation::NotAttempted),
        "confirmation",
    );

    // A blocked choice that confirmed.
    let mut blocked = chose_first(
        record(
            scan(complete, vec![member(0, 0, "member_a")], vec![unread(1, 2)]),
            SELECTED,
        ),
        1_000,
    );
    blocked.unknown_check = check(
        1,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::Max { milli: 3_000 },
    );
    blocked.confirmation = identity_matched();
    refused("blocked, confirmed", &blocked, "confirmation");

    // A scan select with a single-frame comparison.
    let mut compared = scan_selected(complete, TaskSelectionConfirmation::NotAttempted);
    compared.confirmation = TaskSelectionConfirmation::Matched {
        candidate_set_sha256: compared.projection.candidate_set_sha256().to_owned(),
        projection: None,
    };
    refused("v2 matched", &compared, "confirmation");

    // A single-frame select with a scan confirmation.
    let mut single = chose_first(record(frame_projection(false), SELECTED), 1_000);
    single.confirmation = TaskSelectionConfirmation::RepositionFailed {
        outcome: Outcome::relayed("selection_page_changed"),
        stage: TaskSelectionRepositionStage::Return,
    };
    refused("v1 reposition_failed", &single, "confirmation");

    // `identity_matched` that names a member other than the selected one.
    let mut other = chose_first(
        record(
            scan(
                complete,
                vec![member(0, 0, "member_a"), member(1, 1, "member_b")],
                Vec::new(),
            ),
            SELECTED,
        ),
        1_000,
    );
    other.verdicts.push(ranked(1, 500, 2));
    other.unknown_check = nothing_unknown();
    other.confirmation = TaskSelectionConfirmation::IdentityMatched {
        candidate_id: candidate_id(1),
        frame_id: frame_id('f'),
        content_offset_px: 218,
    };
    refused("identity_matched, other member", &other, "confirmation");
}

#[test]
fn l4a_scan_records_keep_the_v2_member_rules() {
    // Every scan record carries unknown_check.
    let mut missing = scan_selected(ScanCoverageState::Complete, identity_matched());
    missing.unknown_check = None;
    refused("no unknown_check", &missing, "unknown_check");

    // The coverage state decides its members: complete has its end.
    let parts =
        |coverage: ScanCoverage, unknown: Vec<ScanUnknownPosition>| ScanCandidateProjectionParts {
            page_id: PAGE.to_owned(),
            layout_id: LAYOUT.to_owned(),
            axis: ScanAxis::Vertical,
            frame: CandidateFrame {
                width: 1280,
                height: 720,
            },
            coverage,
            viewports: viewports(ScanCoverageState::Complete),
            reposition: ScanReposition {
                rewind_viewports: 0,
                return_viewports: 0,
            },
            candidates: Vec::new(),
            unknown,
            gated_out: ScanGatedOut {
                count: 0,
                identities_sha256: NO_IDENTITIES.to_owned(),
            },
        };
    let mut no_end = coverage(ScanCoverageState::Complete);
    no_end.end = None;
    let error = ScanCandidateProjection::new(parts(no_end, Vec::new()))
        .expect_err("complete coverage names its end");
    assert_eq!(error.code(), INVALID_CANDIDATE_PROJECTION);
    assert_eq!(error.item(), "coverage");

    // An unknown position keeps an identity exactly when it read one.
    let complete = coverage(ScanCoverageState::Complete);
    let mut unread_with_identity = unread(0, 1);
    unread_with_identity.identity = Some("member_b".to_owned());
    unread_with_identity.identity_unknown = None;
    let mut dropped_without_identity = dropped(0, 1, "member_b");
    dropped_without_identity.identity = None;
    for position in [unread_with_identity, dropped_without_identity] {
        let error = ScanCandidateProjection::new(parts(complete, vec![position]))
            .expect_err("the identity follows the cause");
        assert_eq!(error.code(), INVALID_CANDIDATE_PROJECTION);
        assert_eq!(error.item(), "unknown_identity");
    }
}

#[test]
fn l4a_reader_bounds_are_256_viewports_64_members_and_64_unknown_positions() {
    let read = |value: Value| -> Result<(), CandidateProjectionError> {
        serde_json::from_value::<TaskSelectionProjection>(value)
            .expect("the projection decodes")
            .validate_decoded()
    };
    let budget = |value: Value, item: &str| {
        let error = read(value).expect_err("above the reader bound");
        assert_eq!(error.code(), CANDIDATE_PROJECTION_BUDGET_EXCEEDED, "{item}");
        assert_eq!(error.item(), item);
    };
    let entries =
        |value: &Value, key: &str| -> Vec<Value> { value[key].as_array().expect("a list").clone() };

    // Viewports: the hash leaves them out, so a sealed projection takes 256 and refuses 257.
    let mut value = serde_json::to_value(scan(ScanCoverageState::Complete, Vec::new(), Vec::new()))
        .expect("the projection encodes");
    let mut list = vec![
        serde_json::to_value(viewport(ScanViewportMotion::Start, 0, 'a'))
            .expect("the viewport encodes"),
    ];
    for index in 1..256 {
        list.push(
            serde_json::to_value(viewport(ScanViewportMotion::Forward, 220 * index, 'b'))
                .expect("the viewport encodes"),
        );
    }
    value["viewports"] = Value::Array(list.clone());
    read(value.clone()).expect("256 viewports are read");
    list.push(list[255].clone());
    value["viewports"] = Value::Array(list);
    budget(value, "scan_viewports");

    // Members: 64 are read; a 65th is refused before anything else is checked.
    let members = (0..64).map(|index| member(index, 0, "member_a")).collect();
    let mut value = serde_json::to_value(scan(ScanCoverageState::Complete, members, Vec::new()))
        .expect("the projection encodes");
    read(value.clone()).expect("64 members are read");
    let mut list = entries(&value, "candidates");
    list.push(list[63].clone());
    value["candidates"] = Value::Array(list);
    budget(value, "scan_candidates");

    // Unknown positions: 64 are read; a 65th is refused.
    let unknown = (0..64).map(|index| unread(index, 1)).collect();
    let mut value = serde_json::to_value(scan(ScanCoverageState::Complete, Vec::new(), unknown))
        .expect("the projection encodes");
    read(value.clone()).expect("64 unknown positions are read");
    let mut list = entries(&value, "unknown");
    let mut extra = list[63].clone();
    extra["position_index"] = Value::from(64);
    list.push(extra);
    value["unknown"] = Value::Array(list);
    budget(value, "scan_unknown");
}

// Workflow #357 DB-3s (frozen model M-357b v1.0 §5, §12.6; contract gate K24). A step whose
// policy declares more than one candidate records its picks in `multi_select`, each with its
// confirmation before the tap and its check after it, and the record's top-level confirmation
// sums the sequence up. Rules M1–M8 (§5.2) refuse what no writer writes; a single select keeps
// its bytes. Every name below is a neutral synthetic one.

// The layout's mark feature: true once a candidate is picked.
const MARK: &str = "picked_mark";

// The writer's unmarked hash of a frame (M-357b §4.2); a reader checks only its grammar.
fn unmarked(digit: char) -> String {
    digit.to_string().repeat(64)
}

fn hex(index: u32) -> char {
    char::from_digit(index % 16, 16).expect("a hexadecimal digit")
}

// A member's identity and its mark, unmarked.
fn marked_identity(index: u32) -> CandidateFeatureMap {
    let mut features = identity(&format!("member_{index:02}"));
    features.insert(
        MARK.to_owned(),
        CandidateFeature::Boolean {
            value: false,
            confidence: Some(950),
        },
    );
    features
}

// Seven cards on one frame, in two rows.
fn card_list() -> CandidateProjection {
    let card = |index: u32| {
        let (column, row) = ((index % 5) as i32, (index / 5) as i32);
        ProjectedCandidate {
            id: candidate_id(index),
            instance_index: index,
            actionable: true,
            rect: CandidateRect {
                x: 40 + 240 * column,
                y: 120 + 280 * row,
                width: 220,
                height: 260,
            },
            click: CandidateRect {
                x: 50 + 240 * column,
                y: 130 + 280 * row,
                width: 200,
                height: 240,
            },
            features: marked_identity(index),
        }
    };
    CandidateProjection::new(
        PAGE,
        LAYOUT,
        CandidateLayoutKind::RepeatedAnchor,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        (0..7).map(card).collect(),
    )
    .expect("the card list is valid")
}

// A complete scan of ten members, with `unknown` positions the scan could not read.
fn scanned_list(unknown: Vec<ScanUnknownPosition>) -> ScanCandidateProjection {
    let members = (0..10)
        .map(|index| ScanCandidate {
            id: candidate_id(index),
            instance_index: index,
            actionable: true,
            content_rect: content_rect(index),
            fixed_viewport: index / 4,
            features: marked_identity(index),
        })
        .collect();
    scan(ScanCoverageState::Complete, members, unknown)
}

// The decision `chosen` (candidate index and score, best first) over `projection`, with the
// multi-select member of a step whose policy declares `declared` and reads `free_slots`.
fn multi_record(
    projection: impl Into<TaskSelectionProjection>,
    chosen: &[(u32, i64)],
    declared: u32,
    free_slots: Option<u32>,
    order: TaskSelectionPickOrder,
) -> TaskSelectionRecord {
    let projection = projection.into();
    let single_frame = matches!(projection, TaskSelectionProjection::V1(_));
    let mut multi = record(
        projection,
        TaskSelectionOutcome::Selected {
            count: chosen.len() as u32,
        },
    );
    multi.selected = chosen
        .iter()
        .map(|(index, _)| candidate_id(*index))
        .collect();
    multi.verdicts = chosen
        .iter()
        .zip(1..)
        .map(|((index, score), rank)| ranked(*index, *score, rank))
        .collect();
    if !single_frame {
        multi.unknown_check = nothing_unknown();
    }
    multi.multi_select = Some(TaskSelectionMultiSelect {
        declared_count: declared,
        required_count: free_slots.map_or(declared, |free| declared.min(free)),
        free_slots,
        order,
        mark_feature: MARK.to_owned(),
        pick_set_sha256: single_frame.then(|| unmarked('a')),
        picks: Vec::new(),
    });
    multi
}

fn multi_of(record: &mut TaskSelectionRecord) -> &mut TaskSelectionMultiSelect {
    record.multi_select.as_mut().expect("a multi-select record")
}

fn pick_matched(frame: char) -> TaskSelectionConfirmation {
    TaskSelectionConfirmation::PickMatched {
        frame_id: frame_id(frame),
    }
}

fn tap_marked(frame: char) -> TaskSelectionConfirmation {
    TaskSelectionConfirmation::TapMarked {
        frame_id: frame_id(frame),
    }
}

fn mark_unconfirmed(
    index: u32,
    mark_expectation: TaskSelectionMarkExpectation,
    mark_state: TaskSelectionMarkState,
    frame: char,
) -> TaskSelectionConfirmation {
    TaskSelectionConfirmation::MarkUnconfirmed {
        candidate_id: candidate_id(index),
        mark_expectation,
        mark_state,
        frame_id: frame_id(frame),
    }
}

// A single-frame pick: confirmed on frame `on`, its tap checked on frame `checked` when the
// check reached a result.
fn frame_pick(index: u32, on: char, checked: Option<char>) -> TaskSelectionPick {
    TaskSelectionPick {
        candidate_id: candidate_id(index),
        confirmation: pick_matched(on),
        tap_check: checked.map(tap_marked),
    }
}

fn scan_identity(index: u32) -> TaskSelectionConfirmation {
    TaskSelectionConfirmation::IdentityMatched {
        candidate_id: candidate_id(index),
        frame_id: frame_id(hex(index)),
        content_offset_px: 240 * index as i32,
    }
}

// A scan pick: returned to, its identity confirmed on a fresh frame, then its tap checked.
fn scan_pick(index: u32, checked: Option<TaskSelectionConfirmation>) -> TaskSelectionPick {
    TaskSelectionPick {
        candidate_id: candidate_id(index),
        confirmation: scan_identity(index),
        tap_check: checked,
    }
}

fn skipped(index: u32) -> TaskSelectionPick {
    TaskSelectionPick {
        candidate_id: candidate_id(index),
        confirmation: TaskSelectionConfirmation::NotAttempted,
        tap_check: None,
    }
}

// §12.4, the first example: three cards picked on one frame by rank, each confirmed on the
// frame before its tap (F2, then each tap check's frame) and checked as marked.
fn frame_success() -> TaskSelectionRecord {
    let mut success = multi_record(
        card_list(),
        &[(4, 900), (1, 700), (6, 500)],
        3,
        None,
        TaskSelectionPickOrder::Rank,
    );
    multi_of(&mut success).picks = vec![
        frame_pick(4, '1', Some('2')),
        frame_pick(1, '2', Some('3')),
        frame_pick(6, '3', Some('4')),
    ];
    success.confirmation = TaskSelectionConfirmation::PicksConfirmed { count: 3 };
    success
}

// A scan that chose #05, #01 and #09 (rank order) and tapped them in content order.
fn scan_success() -> TaskSelectionRecord {
    let mut success = multi_record(
        scanned_list(Vec::new()),
        &[(5, 900), (1, 700), (9, 500)],
        3,
        None,
        TaskSelectionPickOrder::Content,
    );
    multi_of(&mut success).picks = vec![
        scan_pick(1, Some(tap_marked('a'))),
        scan_pick(5, Some(tap_marked('b'))),
        scan_pick(9, Some(tap_marked('c'))),
    ];
    success.confirmation = TaskSelectionConfirmation::PicksConfirmed { count: 3 };
    success
}

// A reason as the ledger reads it back: its code and values as stored.
fn read_back_reason(code: &str, values: Value) -> PolicyReasonRecord {
    PolicyReasonRecord(
        serde_json::from_value(json!({"code": code, "category": "info", "values": values}))
            .expect("a stored outcome decodes"),
    )
}

// A record whose top-level confirmation copies its last pick's failure, or sums up its picks.
fn with_top(mut shape: TaskSelectionRecord, top: TaskSelectionConfirmation) -> TaskSelectionRecord {
    shape.confirmation = top;
    shape
}

// `shape` whose pick `index` ends the sequence with `confirmation` before its tap: no tap
// check, every later pick not attempted, and the top level copying the end.
fn ends_before_tap(
    mut shape: TaskSelectionRecord,
    index: usize,
    confirmation: TaskSelectionConfirmation,
) -> TaskSelectionRecord {
    let picks = &mut multi_of(&mut shape).picks;
    picks[index].confirmation = confirmation.clone();
    picks[index].tap_check = None;
    for later in picks.iter_mut().skip(index + 1) {
        later.confirmation = TaskSelectionConfirmation::NotAttempted;
        later.tap_check = None;
    }
    with_top(shape, confirmation)
}

// `shape` whose pick `index` ends the sequence with its tap check `tap_check`: every later pick
// not attempted, and the top level copying the end.
fn ends_after_tap(
    mut shape: TaskSelectionRecord,
    index: usize,
    tap_check: TaskSelectionConfirmation,
) -> TaskSelectionRecord {
    let picks = &mut multi_of(&mut shape).picks;
    picks[index].tap_check = Some(tap_check.clone());
    for later in picks.iter_mut().skip(index + 1) {
        later.confirmation = TaskSelectionConfirmation::NotAttempted;
        later.tap_check = None;
    }
    with_top(shape, tap_check)
}

// T1: the single selects of SEL-L4a keep their bytes: no `multi_select`, the same members in
// the same order, and the record ends where it ended.
#[test]
fn db3s_t1_single_select_records_keep_their_bytes() {
    let projection = frame_projection(false);
    let mut single = chose_first(record(projection.clone(), SELECTED), 1_000);
    single.confirmation = TaskSelectionConfirmation::Matched {
        candidate_set_sha256: projection.candidate_set_sha256().to_owned(),
        projection: None,
    };
    let scanned = scan_selected(ScanCoverageState::EarlyStopped, identity_matched());
    let members = [
        "layout_id",
        "page_id",
        "projection",
        "policy",
        "fact_snapshot_id",
        "input_ledger_position",
        "now_unix_ms",
        "input_sha256",
        "selection_result",
        "outcome_key",
        "selected",
        "verdicts",
        "reasons",
        "confirmation",
    ];
    for (label, shape) in [("v1 matched", single), ("v2 identity_matched", scanned)] {
        let text = serde_json::to_string(&shape).expect("the record encodes");
        assert!(!text.contains("multi_select"), "{label}");
        let mut expected = members
            .iter()
            .map(|member| (*member).to_owned())
            .collect::<Vec<_>>();
        let tail = match &shape.unknown_check {
            None => format!(
                "\"confirmation\":{}}}",
                serde_json::to_string(&shape.confirmation).expect("the confirmation encodes")
            ),
            Some(check) => {
                expected.push("unknown_check".to_owned());
                format!(
                    "\"unknown_check\":{}}}",
                    serde_json::to_string(check).expect("the check encodes")
                )
            }
        };
        let Value::Object(object) = as_json(&shape) else {
            panic!("{label}: the record is an object");
        };
        let mut keys = object.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        expected.sort();
        assert_eq!(keys, expected, "{label}");
        assert!(text.ends_with(&tail), "{label}");
        let decoded = round_trip(label, &shape);
        assert_eq!(decoded.multi_select, None, "{label}");
        assert_eq!(
            serde_json::to_string(&decoded).expect("the record encodes"),
            text,
            "{label}"
        );
    }
}

// T2: §12.4's first example round-trips, passes the writer's checks and has the pinned shape.
#[test]
fn db3s_t2_a_single_frame_multi_select_that_picked_three_round_trips() {
    let success = frame_success();
    round_trip("three picks", &success);
    let value = as_json(&success);
    let frame = |digit: char| serde_json::to_value(frame_id(digit)).expect("the frame id encodes");
    assert_eq!(
        value["confirmation"],
        json!({"kind": "picks_confirmed", "count": 3})
    );
    assert_eq!(
        value["multi_select"],
        json!({
            "declared_count": 3,
            "required_count": 3,
            "order": "rank",
            "mark_feature": MARK,
            "pick_set_sha256": unmarked('a'),
            "picks": [
                {"candidate_id": candidate_id(4),
                 "confirmation": {"kind": "pick_matched", "frame_id": frame('1')},
                 "tap_check": {"kind": "tap_marked", "frame_id": frame('2')}},
                {"candidate_id": candidate_id(1),
                 "confirmation": {"kind": "pick_matched", "frame_id": frame('2')},
                 "tap_check": {"kind": "tap_marked", "frame_id": frame('3')}},
                {"candidate_id": candidate_id(6),
                 "confirmation": {"kind": "pick_matched", "frame_id": frame('3')},
                 "tap_check": {"kind": "tap_marked", "frame_id": frame('4')}}
            ]
        })
    );
    // `selected` keeps the evaluator's rank order.
    assert_eq!(
        value["selected"],
        json!([candidate_id(4), candidate_id(1), candidate_id(6)])
    );
}

// T3: a scan taps in content order while `selected` keeps the rank order; tapping in rank order
// under `order: content` is refused.
#[test]
fn db3s_t3_a_scan_multi_select_taps_in_content_order() {
    let success = scan_success();
    let decoded = round_trip("content order", &success);
    assert_eq!(
        decoded.selected,
        vec![candidate_id(5), candidate_id(1), candidate_id(9)]
    );

    let mut rank_taps = scan_success();
    multi_of(&mut rank_taps).picks = vec![
        scan_pick(5, Some(tap_marked('b'))),
        scan_pick(1, Some(tap_marked('a'))),
        scan_pick(9, Some(tap_marked('c'))),
    ];
    refused("rank order under content", &rank_taps, "picks");
}

// T4: N is min(declared, free slots), or the declared count without a free-slot reading.
#[test]
fn db3s_t4_the_resolved_count_follows_the_declared_count_and_the_free_slots() {
    // Declared 3, one free slot: N 1, one pick.
    let mut one = multi_record(
        card_list(),
        &[(4, 900)],
        3,
        Some(1),
        TaskSelectionPickOrder::Rank,
    );
    multi_of(&mut one).picks = vec![frame_pick(4, '1', Some('2'))];
    one.confirmation = TaskSelectionConfirmation::PicksConfirmed { count: 1 };
    assert_eq!(multi_of(&mut one).required_count, 1);
    round_trip("one free slot", &one);

    // Declared 3, five free slots: N 3.
    let mut five = frame_success();
    multi_of(&mut five).free_slots = Some(5);
    round_trip("five free slots", &five);

    // Declared 3, no free-slot reading: N 3.
    round_trip("no free-slot reading", &frame_success());

    type MultiChange = fn(&mut TaskSelectionMultiSelect);
    let refusals: [(&str, MultiChange); 7] = [
        ("N not min(declared, free)", |multi| {
            multi.free_slots = Some(2);
        }),
        ("N 0", |multi| multi.required_count = 0),
        ("N above declared", |multi| multi.required_count = 4),
        ("declared 1", |multi| {
            multi.declared_count = 1;
            multi.required_count = 1;
        }),
        ("declared 17", |multi| {
            multi.declared_count = 17;
            multi.required_count = 17;
        }),
        ("free slots 0", |multi| multi.free_slots = Some(0)),
        ("free slots 65536", |multi| multi.free_slots = Some(65_536)),
    ];
    for (label, change) in refusals {
        let mut shape = frame_success();
        change(multi_of(&mut shape));
        refused(label, &shape, "multi_select");
    }
}

// T5 (R1-B1): a `none_allowed` partial fill of a scan, two of four. Without an unknown
// instance it stands; with one it is blocked as `unknown_instances` and never as
// `outranked_possible`.
#[test]
fn db3s_t5_a_partial_fill_with_an_unknown_instance_is_blocked_by_rule_4_only() {
    let partial = |unknown: Vec<ScanUnknownPosition>| {
        multi_record(
            scanned_list(unknown),
            &[(1, 900), (5, 700)],
            4,
            None,
            TaskSelectionPickOrder::Content,
        )
    };

    let mut stands = partial(Vec::new());
    multi_of(&mut stands).picks = vec![
        scan_pick(1, Some(tap_marked('a'))),
        scan_pick(5, Some(tap_marked('b'))),
    ];
    stands.confirmation = TaskSelectionConfirmation::PicksConfirmed { count: 2 };
    round_trip("partial, nothing unknown", &stands);

    let mut blocked = partial(vec![unread(10, 2)]);
    blocked.unknown_check = check(
        1,
        TaskSelectionBlocking::UnknownInstances,
        TaskSelectionUpperBound::Unbounded,
    );
    round_trip("partial, an unbounded unknown instance", &blocked);

    let mut outranked = blocked.clone();
    outranked.unknown_check = check(
        1,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::Unbounded,
    );
    refused("partial, outranked_possible", &outranked, "path_rule_4");

    let mut unblocked = stands.clone();
    unblocked.projection = scanned_list(vec![unread(10, 2)]).into();
    unblocked.unknown_check = check(
        1,
        TaskSelectionBlocking::None,
        TaskSelectionUpperBound::Unbounded,
    );
    refused(
        "partial, an unknown instance not blocking",
        &unblocked,
        "path_rule_4",
    );
}

// T6: single-frame failures. The first pick's frame moved before its tap; the first pick's tap
// check found the list moved (R1-B2); the second pick's tap check found it unmarked. The
// top-level confirmation copies the failure.
#[test]
fn db3s_t6_single_frame_failures_end_the_sequence_and_the_record_copies_them() {
    let moved = TaskSelectionConfirmation::PickMoved {
        frame_id: frame_id('1'),
        pick_set_sha256: unmarked('c'),
    };
    let mut before_tap = frame_success();
    multi_of(&mut before_tap).picks = vec![
        TaskSelectionPick {
            candidate_id: candidate_id(4),
            confirmation: moved.clone(),
            tap_check: None,
        },
        skipped(1),
        skipped(6),
    ];
    let before_tap = with_top(before_tap, moved);

    let moved_after = TaskSelectionConfirmation::PickMoved {
        frame_id: frame_id('2'),
        pick_set_sha256: unmarked('c'),
    };
    let mut after_tap = frame_success();
    multi_of(&mut after_tap).picks = vec![
        TaskSelectionPick {
            candidate_id: candidate_id(4),
            confirmation: pick_matched('1'),
            tap_check: Some(moved_after.clone()),
        },
        skipped(1),
        skipped(6),
    ];
    let after_tap = with_top(after_tap, moved_after);

    let unmarked_after = mark_unconfirmed(
        1,
        TaskSelectionMarkExpectation::Marked,
        TaskSelectionMarkState::Unmarked,
        '3',
    );
    let mut second = frame_success();
    multi_of(&mut second).picks = vec![
        frame_pick(4, '1', Some('2')),
        TaskSelectionPick {
            candidate_id: candidate_id(1),
            confirmation: pick_matched('2'),
            tap_check: Some(unmarked_after.clone()),
        },
        skipped(6),
    ];
    let second = with_top(second, unmarked_after);

    for (label, shape, other) in [
        (
            "moved before the first tap",
            before_tap,
            TaskSelectionConfirmation::PicksStopped { count: 0 },
        ),
        (
            "moved after the first tap",
            after_tap,
            TaskSelectionConfirmation::PicksConfirmed { count: 3 },
        ),
        (
            "second tap unmarked",
            second,
            mark_unconfirmed(
                1,
                TaskSelectionMarkExpectation::Marked,
                TaskSelectionMarkState::Unknown,
                '3',
            ),
        ),
    ] {
        round_trip(label, &shape);
        refused(label, &with_top(shape, other), "confirmation");
    }
}

// T7: scan failures. The second pick's identity was not confirmed; the first pick's tap check
// did not align. Neither is a single-frame record, and a tap check's stage is never a
// confirmation before a tap.
#[test]
fn db3s_t7_scan_failures_end_the_sequence_and_stay_scan_shapes() {
    let unconfirmed = TaskSelectionConfirmation::IdentityUnconfirmed {
        identity_state: TaskSelectionIdentityState::Different,
        observed_identity: Some("member_77".to_owned()),
        identity_unknown: None,
    };
    let mut identity = scan_success();
    multi_of(&mut identity).picks = vec![
        scan_pick(1, Some(tap_marked('a'))),
        TaskSelectionPick {
            candidate_id: candidate_id(5),
            confirmation: unconfirmed.clone(),
            tap_check: None,
        },
        skipped(9),
    ];
    let identity = with_top(identity, unconfirmed);
    round_trip("second identity unconfirmed", &identity);

    let misaligned = TaskSelectionConfirmation::RepositionFailed {
        outcome: Outcome::relayed("selection_scan_alignment_rejected"),
        stage: TaskSelectionRepositionStage::TapCheck,
    };
    let mut aligned = scan_success();
    multi_of(&mut aligned).picks = vec![
        scan_pick(1, Some(misaligned.clone())),
        skipped(5),
        skipped(9),
    ];
    let aligned = with_top(aligned, misaligned.clone());
    round_trip("first tap check misaligned", &aligned);

    // The same picks over the frame of a single-frame select.
    let mut single_frame = identity.clone();
    single_frame.projection = list_of_ten().into();
    multi_of(&mut single_frame).pick_set_sha256 = Some(unmarked('a'));
    refused("scan picks over one frame", &single_frame, "picks");

    let mut before_tap = scan_success();
    multi_of(&mut before_tap).picks = vec![
        TaskSelectionPick {
            candidate_id: candidate_id(1),
            confirmation: misaligned.clone(),
            tap_check: None,
        },
        skipped(5),
        skipped(9),
    ];
    let before_tap = with_top(before_tap, misaligned);
    refused("tap-check stage before a tap", &before_tap, "picks");
}

// Ten cards on one frame, in two rows: the scan's members as a single frame shows them.
fn list_of_ten() -> CandidateProjection {
    let card = |index: u32| {
        let (column, row) = ((index % 5) as i32, (index / 5) as i32);
        ProjectedCandidate {
            id: candidate_id(index),
            instance_index: index,
            actionable: true,
            rect: CandidateRect {
                x: 40 + 240 * column,
                y: 120 + 280 * row,
                width: 220,
                height: 260,
            },
            click: CandidateRect {
                x: 50 + 240 * column,
                y: 130 + 280 * row,
                width: 200,
                height: 240,
            },
            features: marked_identity(index),
        }
    };
    CandidateProjection::new(
        PAGE,
        LAYOUT,
        CandidateLayoutKind::RepeatedAnchor,
        CandidateFrame {
            width: 1280,
            height: 720,
        },
        (0..10).map(card).collect(),
    )
    .expect("the card list is valid")
}

// T8 (C3): the sequence stops outside the record. §12.4's second example stops after the
// second pick's confirmation; a scan stops on its way back to the second pick; a sequence stops
// before its first pick.
#[test]
fn db3s_t8_a_sequence_stopped_outside_the_record_counts_its_checked_taps() {
    let mut confirmed_then_stopped = multi_record(
        card_list(),
        &[(4, 900), (1, 700)],
        3,
        Some(2),
        TaskSelectionPickOrder::Rank,
    );
    multi_of(&mut confirmed_then_stopped).picks =
        vec![frame_pick(4, '1', Some('2')), frame_pick(1, '2', None)];
    let confirmed_then_stopped = with_top(
        confirmed_then_stopped,
        TaskSelectionConfirmation::PicksStopped { count: 1 },
    );
    let frame = |digit: char| serde_json::to_value(frame_id(digit)).expect("the frame id encodes");
    let value = as_json(&confirmed_then_stopped);
    assert_eq!(
        value["confirmation"],
        json!({"kind": "picks_stopped", "count": 1})
    );
    assert_eq!(
        value["multi_select"],
        json!({
            "declared_count": 3,
            "required_count": 2,
            "free_slots": 2,
            "order": "rank",
            "mark_feature": MARK,
            "pick_set_sha256": unmarked('a'),
            "picks": [
                {"candidate_id": candidate_id(4),
                 "confirmation": {"kind": "pick_matched", "frame_id": frame('1')},
                 "tap_check": {"kind": "tap_marked", "frame_id": frame('2')}},
                {"candidate_id": candidate_id(1),
                 "confirmation": {"kind": "pick_matched", "frame_id": frame('2')}}
            ]
        })
    );

    let mut returning = scan_success();
    multi_of(&mut returning).picks =
        vec![scan_pick(1, Some(tap_marked('a'))), skipped(5), skipped(9)];
    let returning = with_top(
        returning,
        TaskSelectionConfirmation::PicksStopped { count: 1 },
    );

    let mut first = scan_success();
    multi_of(&mut first).picks = vec![skipped(1), skipped(5), skipped(9)];
    let first = with_top(first, TaskSelectionConfirmation::PicksStopped { count: 0 });

    for (label, shape, wrong) in [
        ("stopped after a confirmation", confirmed_then_stopped, 2),
        ("stopped returning", returning, 0),
        ("stopped before the first pick", first, 1),
    ] {
        round_trip(label, &shape);
        refused(
            label,
            &with_top(
                shape,
                TaskSelectionConfirmation::PicksStopped { count: wrong },
            ),
            "confirmation",
        );
    }

    // Every pick succeeded: the sequence did not stop.
    refused(
        "stopped after every pick",
        &with_top(
            scan_success(),
            TaskSelectionConfirmation::PicksStopped { count: 3 },
        ),
        "confirmation",
    );
}

// T9 (rule 5, M7): a full fill compares the bound with its lowest score, both ways; a partial
// fill is never `outranked_possible`.
#[test]
fn db3s_t9_rule_5_compares_a_full_fill_by_its_lowest_score() {
    let full = || {
        multi_record(
            scanned_list(vec![unread(10, 2)]),
            &[(5, 900), (1, 700), (9, 500)],
            3,
            None,
            TaskSelectionPickOrder::Content,
        )
    };
    let blocked = |milli: i64| {
        let mut shape = full();
        shape.unknown_check = check(
            1,
            TaskSelectionBlocking::OutrankedPossible,
            TaskSelectionUpperBound::Max { milli },
        );
        shape
    };
    let confirmed = |milli: i64| {
        let mut shape = scan_success();
        shape.projection = scanned_list(vec![unread(10, 2)]).into();
        shape.unknown_check = check(
            1,
            TaskSelectionBlocking::None,
            TaskSelectionUpperBound::Max { milli },
        );
        shape
    };

    // A bound of 600 lies above the lowest score, 500: outranked_possible, and nothing else.
    round_trip("bound 600, outranked", &blocked(600));
    refused("bound 600, not blocking", &confirmed(600), "path_rule_5");
    // A bound of 500 does not: none, and nothing else.
    round_trip("bound 500, confirmed", &confirmed(500));
    refused("bound 500, outranked", &blocked(500), "path_rule_5");

    // Two of three chosen with nothing unknown: never outranked_possible. With no unknown
    // instance the bound is `none_unknown` (rule 3).
    let mut partial = multi_record(
        scanned_list(Vec::new()),
        &[(5, 900), (1, 700)],
        3,
        None,
        TaskSelectionPickOrder::Content,
    );
    partial.unknown_check = check(
        0,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::NoneUnknown,
    );
    refused("partial, outranked", &partial, "path_rule_5");
}

// T10 (rule 1, M8): a scan stops early only with N chosen and no quota left unfilled.
#[test]
fn db3s_t10_an_early_stopped_multi_select_is_full_with_every_quota_filled() {
    let early = |chosen: &[(u32, i64)]| {
        let members = (0..10)
            .map(|index| ScanCandidate {
                id: candidate_id(index),
                instance_index: index,
                actionable: true,
                content_rect: content_rect(index),
                fixed_viewport: index / 4,
                features: marked_identity(index),
            })
            .collect();
        let mut shape = multi_record(
            scan(ScanCoverageState::EarlyStopped, members, Vec::new()),
            chosen,
            3,
            None,
            TaskSelectionPickOrder::Content,
        );
        let mut indices = chosen.iter().map(|(index, _)| *index).collect::<Vec<_>>();
        indices.sort_unstable();
        let picks = indices
            .iter()
            .zip(['a', 'b', 'c'])
            .map(|(index, frame)| scan_pick(*index, Some(tap_marked(frame))))
            .collect::<Vec<_>>();
        shape.confirmation = TaskSelectionConfirmation::PicksConfirmed {
            count: picks.len() as u32,
        };
        multi_of(&mut shape).picks = picks;
        shape
    };

    round_trip("early stop, full", &early(&[(5, 900), (1, 700), (9, 500)]));
    refused(
        "early stop, two of three",
        &early(&[(5, 900), (1, 700)]),
        "path_rule_1",
    );
    let mut unfilled = early(&[(5, 900), (1, 700), (9, 500)]);
    unfilled.reasons.push(read_back_reason(
        "selection_quota_unfilled",
        json!({"rule_id": "by_group", "count": 1, "limit_count": 2}),
    ));
    refused("early stop, a quota unfilled", &unfilled, "path_rule_1");
}

// T11 (R1-S3): a single select never carries a multi-select confirmation or the tap-check
// stage at its top level.
#[test]
fn db3s_t11_a_single_select_never_carries_a_multi_select_confirmation() {
    for (label, top) in [
        ("pick_matched", pick_matched('1')),
        (
            "pick_moved",
            TaskSelectionConfirmation::PickMoved {
                frame_id: frame_id('1'),
                pick_set_sha256: unmarked('c'),
            },
        ),
        (
            "mark_unconfirmed",
            mark_unconfirmed(
                0,
                TaskSelectionMarkExpectation::Marked,
                TaskSelectionMarkState::Unmarked,
                '2',
            ),
        ),
        ("tap_marked", tap_marked('2')),
        (
            "picks_confirmed",
            TaskSelectionConfirmation::PicksConfirmed { count: 1 },
        ),
        (
            "picks_stopped",
            TaskSelectionConfirmation::PicksStopped { count: 0 },
        ),
    ] {
        let mut single = chose_first(record(frame_projection(false), SELECTED), 1_000);
        single.confirmation = top;
        refused(label, &single, "confirmation");
    }

    let tap_check_stage = scan_selected(
        ScanCoverageState::Complete,
        TaskSelectionConfirmation::RepositionFailed {
            outcome: Outcome::relayed("selection_scan_alignment_rejected"),
            stage: TaskSelectionRepositionStage::TapCheck,
        },
    );
    refused(
        "reposition_failed at tap_check",
        &tap_check_stage,
        "confirmation",
    );
}

// T12: every other refusal of M1–M5, one each, by the field §12.5 names.
#[test]
fn db3s_t12_each_rule_refuses_what_no_writer_writes() {
    let mut cases: Vec<(&str, TaskSelectionRecord, &str)> = Vec::new();

    // M1.
    let mut ambiguous = multi_record(card_list(), &[], 3, None, TaskSelectionPickOrder::Rank);
    ambiguous.selection_result = TaskSelectionOutcome::Ambiguous {
        candidate_ids: vec![candidate_id(4), candidate_id(1)],
    };
    ambiguous.outcome_key = "ambiguous".to_owned();
    ambiguous.verdicts = vec![ranked(4, 900, 1), ranked(1, 900, 1)];
    cases.push(("ambiguous", ambiguous, "multi_select"));

    let mut above_n = multi_record(card_list(), &[], 3, None, TaskSelectionPickOrder::Rank);
    above_n.selection_result = TaskSelectionOutcome::Insufficient {
        surviving: 1,
        required: 4,
    };
    above_n.outcome_key = "insufficient".to_owned();
    above_n.verdicts = vec![ranked(4, 900, 1)];
    cases.push(("insufficient above N", above_n, "multi_select"));

    let mut over = frame_success();
    multi_of(&mut over).free_slots = Some(2);
    multi_of(&mut over).required_count = 2;
    cases.push(("selected above N", over, "multi_select"));

    let mut v2_hash = scan_success();
    multi_of(&mut v2_hash).pick_set_sha256 = Some(unmarked('a'));
    cases.push(("v2 unmarked hash", v2_hash, "multi_select"));

    let mut v1_no_hash = frame_success();
    multi_of(&mut v1_no_hash).pick_set_sha256 = None;
    cases.push(("v1 without the unmarked hash", v1_no_hash, "multi_select"));

    let mut v1_bad_hash = frame_success();
    multi_of(&mut v1_bad_hash).pick_set_sha256 = Some("A".repeat(64));
    cases.push(("v1 unmarked hash grammar", v1_bad_hash, "multi_select"));

    let mut mark_name = frame_success();
    multi_of(&mut mark_name).mark_feature = "Picked-Mark".to_owned();
    cases.push(("mark feature grammar", mark_name, "multi_select"));

    // M2.
    let mut fewer = frame_success();
    multi_of(&mut fewer).picks.truncate(2);
    cases.push(("attempted, fewer picks", fewer, "picks"));

    let mut stranger = frame_success();
    multi_of(&mut stranger).picks[2].candidate_id = candidate_id(5);
    cases.push(("a pick not selected", stranger, "picks"));

    let mut twice = frame_success();
    multi_of(&mut twice).picks[2].candidate_id = candidate_id(4);
    cases.push(("a pick twice", twice, "picks"));

    // M5: blocked, so not attempted, yet holding picks.
    let mut blocked = scan_success();
    blocked.projection = scanned_list(vec![unread(10, 2)]).into();
    blocked.unknown_check = check(
        1,
        TaskSelectionBlocking::OutrankedPossible,
        TaskSelectionUpperBound::Max { milli: 2_000 },
    );
    blocked.confirmation = TaskSelectionConfirmation::NotAttempted;
    cases.push(("not attempted with picks", blocked, "confirmation"));

    // M3: each case breaks only the pick's own shape; the picks after it were not attempted
    // and the top level copies its end.
    cases.push((
        "v1 second pick moved before its tap",
        ends_before_tap(
            frame_success(),
            1,
            TaskSelectionConfirmation::PickMoved {
                frame_id: frame_id('2'),
                pick_set_sha256: unmarked('c'),
            },
        ),
        "picks",
    ));

    let mut other_frame = frame_success();
    multi_of(&mut other_frame).picks[1].confirmation = pick_matched('9');
    cases.push(("v1 second pick on another frame", other_frame, "picks"));

    let mut v1_identity = frame_success();
    multi_of(&mut v1_identity).picks[0].confirmation = TaskSelectionConfirmation::IdentityMatched {
        candidate_id: candidate_id(4),
        frame_id: frame_id('1'),
        content_offset_px: 0,
    };
    cases.push(("v1 identity_matched", v1_identity, "picks"));

    cases.push((
        "v2 pick_moved",
        ends_before_tap(
            scan_success(),
            0,
            TaskSelectionConfirmation::PickMoved {
                frame_id: frame_id('1'),
                pick_set_sha256: unmarked('c'),
            },
        ),
        "picks",
    ));

    let mut v2_other = scan_success();
    multi_of(&mut v2_other).picks[0].confirmation = scan_identity(5);
    cases.push(("v2 identity of another member", v2_other, "picks"));

    for (label, inside) in [
        (
            "picks_confirmed inside",
            TaskSelectionConfirmation::PicksConfirmed { count: 3 },
        ),
        (
            "picks_stopped inside",
            TaskSelectionConfirmation::PicksStopped { count: 0 },
        ),
        (
            "matched inside",
            TaskSelectionConfirmation::Matched {
                candidate_set_sha256: card_list().candidate_set_sha256().to_owned(),
                projection: None,
            },
        ),
        ("tap_marked before a tap", tap_marked('1')),
    ] {
        cases.push((label, ends_before_tap(frame_success(), 0, inside), "picks"));
    }

    // M4.
    let unconfirmed = TaskSelectionConfirmation::IdentityUnconfirmed {
        identity_state: TaskSelectionIdentityState::Absent,
        observed_identity: None,
        identity_unknown: None,
    };
    let mut after_failure = scan_success();
    multi_of(&mut after_failure).picks[1].confirmation = unconfirmed.clone();
    multi_of(&mut after_failure).picks[1].tap_check = None;
    let after_failure = with_top(after_failure, unconfirmed.clone());
    cases.push(("a success after a failure", after_failure, "picks"));

    let mut checked_failure = ends_before_tap(scan_success(), 0, unconfirmed);
    multi_of(&mut checked_failure).picks[0].tap_check = Some(tap_marked('a'));
    cases.push(("a failed confirmation checked", checked_failure, "picks"));

    // M3, mark_unconfirmed.
    cases.push((
        "mark read as required",
        ends_after_tap(
            frame_success(),
            1,
            mark_unconfirmed(
                1,
                TaskSelectionMarkExpectation::Marked,
                TaskSelectionMarkState::Marked,
                '3',
            ),
        ),
        "picks",
    ));
    cases.push((
        "v1 mark of a card not on the frame",
        ends_after_tap(
            frame_success(),
            1,
            mark_unconfirmed(
                9,
                TaskSelectionMarkExpectation::Unmarked,
                TaskSelectionMarkState::Marked,
                '3',
            ),
        ),
        "picks",
    ));
    cases.push((
        "v2 mark of a member not selected",
        ends_after_tap(
            scan_success(),
            1,
            mark_unconfirmed(
                3,
                TaskSelectionMarkExpectation::Marked,
                TaskSelectionMarkState::Unmarked,
                'b',
            ),
        ),
        "picks",
    ));

    for (label, shape, field) in cases {
        refused(label, &shape, field);
    }
}

// T13 (MS4): the public projection keeps only codes inside the picks, as at the top level, and
// `outcome()` still reads the record's failure.
#[test]
fn db3s_t13_the_public_projection_keeps_only_codes_inside_the_picks() {
    let failure = Outcome::relayed("selection_confirmation_capture_failed").caused_by(
        Outcome::new(crate::codes::ContractCode::ForeignAdbError)
            .raw_source(RawSource::Adb)
            .raw_text("error: closed")
            .exit_code(1),
    );
    let failed = TaskSelectionConfirmation::CaptureFailed {
        outcome: failure.clone(),
    };
    let mut shape = frame_success();
    multi_of(&mut shape).picks = vec![
        frame_pick(4, '1', Some('2')),
        TaskSelectionPick {
            candidate_id: candidate_id(1),
            confirmation: pick_matched('2'),
            tap_check: Some(failed.clone()),
        },
        skipped(6),
    ];
    let shape = with_top(shape, failed);
    let decoded = round_trip("second tap check not captured", &shape);
    let fact = TaskSemanticFact::SelectionEvaluated {
        step_index: 2,
        operation_label: "pick_members".to_owned(),
        selection: Box::new(decoded),
    };
    assert_eq!(fact.outcome(), Some(&failure));

    let public = fact.public_summary();
    let TaskSemanticFact::SelectionEvaluated { selection, .. } = &public else {
        panic!("the projection keeps the fact");
    };
    let picks = &selection
        .multi_select
        .as_ref()
        .expect("the projection keeps the picks")
        .picks;
    let Some(TaskSelectionConfirmation::CaptureFailed { outcome }) = &picks[1].tap_check else {
        panic!("the projection keeps the tap check");
    };
    assert_eq!(outcome, &failure.codes_only());
    assert_eq!(public.outcome(), Some(&failure.codes_only()));
    let text = serde_json::to_string(&public).expect("the projection encodes");
    assert!(text.contains("foreign_adb_error"));
    for kept_out in ["raw_text", "error: closed", "exit_code"] {
        assert!(!text.contains(kept_out), "{kept_out}");
    }
}

// T14 (R1-S5): the largest record a real multi select writes: 64 verdicts, each with two
// document gates, two constraint gates and three terms; ten combination skips; a v2 projection
// of 30 KiB; sixteen picks, the last one's tap check failing with the longest shape. The bytes
// go to the CI log; within the write bound the record is appended, beyond it the writer refuses
// it as `selection_record_too_large` (M-357b Q14), and a reader always reads it.
#[test]
fn db3s_t14_the_largest_multi_select_record_is_measured_against_the_write_bound() {
    const MEMBERS: u32 = 64;
    const VIEWPORTS: u32 = 14;
    let members = (0..MEMBERS)
        .map(|index| ScanCandidate {
            id: candidate_id(index),
            instance_index: index,
            actionable: true,
            content_rect: CandidateRect {
                x: 40,
                y: 100 + 145 * index as i32,
                width: 1200,
                height: 140,
            },
            fixed_viewport: index * VIEWPORTS / MEMBERS,
            features: CandidateFeatureMap::from([
                (
                    "member".to_owned(),
                    CandidateFeature::Identity {
                        value: format!("member_{index:02}"),
                        variant: None,
                        source: CandidateIdentitySource::OcrAlias,
                        distance: 0,
                        confidence: Some(950),
                    },
                ),
                (
                    MARK.to_owned(),
                    CandidateFeature::Boolean {
                        value: false,
                        confidence: Some(950),
                    },
                ),
                (
                    "level".to_owned(),
                    CandidateFeature::Integer {
                        value: 90 - i64::from(index),
                        confidence: Some(950),
                    },
                ),
                (
                    "group".to_owned(),
                    CandidateFeature::Integer {
                        value: i64::from(index % 4),
                        confidence: Some(950),
                    },
                ),
            ]),
        })
        .collect();
    let viewports = (0..VIEWPORTS)
        .map(|index| ScanViewport {
            motion: if index == 0 {
                ScanViewportMotion::Start
            } else {
                ScanViewportMotion::Forward
            },
            content_offset_px: 680 * index as i32,
            tracked_instances: 9,
            readable_instances: 6,
            unknown_instances: 0,
            frame_id: frame_id(hex(index)),
        })
        .collect();
    let projection = ScanCandidateProjection::new(ScanCandidateProjectionParts {
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
            extent_px: Some(9_380),
        },
        viewports,
        reposition: ScanReposition {
            rewind_viewports: 0,
            return_viewports: 0,
        },
        candidates: members,
        unknown: Vec::new(),
        gated_out: ScanGatedOut {
            count: 0,
            identities_sha256: NO_IDENTITIES.to_owned(),
        },
    })
    .expect("the projection is within its budget");
    let projection_bytes = serde_json::to_vec(&projection)
        .expect("the projection encodes")
        .len();
    assert!(
        (30 * 1024..=CANDIDATE_PROJECTION_MAX_BYTES).contains(&projection_bytes),
        "{projection_bytes}"
    );

    // Ranks 2, 4, … 20 are skipped by the combination layer; the first sixteen others are
    // chosen.
    let skipped_ranks = (1..=10).map(|step| 2 * step).collect::<Vec<u32>>();
    let chosen_ranks = (1..=MEMBERS)
        .filter(|rank| !skipped_ranks.contains(rank))
        .take(16)
        .collect::<Vec<_>>();
    let verdicts = (0..MEMBERS)
        .map(|index| {
            let rank = index + 1;
            let step = i64::from(index);
            TaskSelectionVerdict {
                candidate_id: candidate_id(index),
                status: TaskSelectionCandidateStatus::Ranked,
                score_milli: Some(2_700_000 - 21_000 * step),
                rank: Some(rank),
                gates: ["level_gate", "mark_gate", "by_group", "same_unit"]
                    .into_iter()
                    .map(|gate_id| TaskSelectionGateResult {
                        gate_id: gate_id.to_owned(),
                        selection_result: TaskSelectionGateOutcome::Passed,
                    })
                    .collect(),
                terms: ["level_term", "power_term", "tier_term"]
                    .into_iter()
                    .map(|term_id| TaskSelectionTermResult {
                        term_id: term_id.to_owned(),
                        selection_result: TaskSelectionTermOutcome::Scored {
                            transformed_milli: 900_000 - 7_000 * step,
                        },
                        weight_milli: 333,
                        contribution_milli: 299_700 - 2_331 * step,
                    })
                    .collect(),
                reasons: if skipped_ranks.contains(&rank) {
                    vec![read_back_reason(
                        "candidate_combination_skipped",
                        json!({"rule_id": "same_unit", "constraint_kind": "quota_reserve"}),
                    )]
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    let picks = chosen_ranks
        .iter()
        .zip(0..)
        .map(|(rank, order)| {
            let index = rank - 1;
            let tap_check = if order < 15 {
                tap_marked(hex(order + 1))
            } else {
                mark_unconfirmed(
                    index,
                    TaskSelectionMarkExpectation::Marked,
                    TaskSelectionMarkState::Unknown,
                    hex(order + 1),
                )
            };
            TaskSelectionPick {
                candidate_id: candidate_id(index),
                confirmation: TaskSelectionConfirmation::IdentityMatched {
                    candidate_id: candidate_id(index),
                    frame_id: frame_id(hex(order)),
                    content_offset_px: 145 * index as i32,
                },
                tap_check: Some(tap_check),
            }
        })
        .collect::<Vec<_>>();

    let mut largest = record(projection, TaskSelectionOutcome::Selected { count: 16 });
    largest.selected = chosen_ranks
        .iter()
        .map(|rank| candidate_id(rank - 1))
        .collect();
    largest.verdicts = verdicts;
    largest.reasons.push(read_back_reason(
        "selection_combination_applied",
        json!({"count": 16, "limit_count": 16}),
    ));
    largest.unknown_check = nothing_unknown();
    largest.confirmation = picks[15].tap_check.clone().expect("the last tap check");
    largest.multi_select = Some(TaskSelectionMultiSelect {
        declared_count: 16,
        required_count: 16,
        free_slots: Some(16),
        order: TaskSelectionPickOrder::Rank,
        mark_feature: MARK.to_owned(),
        pick_set_sha256: None,
        picks,
    });

    largest
        .validate()
        .expect("a reader reads the largest record");
    let bytes = serde_json::to_vec(&largest)
        .expect("the record encodes")
        .len();
    // Written to stdout past the test harness's capture, so that the CI log shows it.
    std::io::stdout()
        .write_all(
            format!(
                "DB-3s T14: multi-select record {bytes} bytes (projection {projection_bytes} bytes; write bound {TASK_SELECTION_RECORD_MAX_BYTES})\n"
            )
            .as_bytes(),
        )
        .expect("the measurement is written");
    let appended = largest.validate_for_append();
    if bytes <= TASK_SELECTION_RECORD_MAX_BYTES {
        appended.expect("within the write bound the record is appended");
    } else {
        let error = appended.expect_err("beyond the write bound the writer refuses the record");
        assert_eq!(error.code(), SELECTION_RECORD_TOO_LARGE);
        assert_eq!(error.field(), "selection");
    }
}

// T15: 0.12.x adds no member to these shapes (revision 3): one unknown member in
// `multi_select`, in a pick, or in any new confirmation variant makes the record unreadable.
#[test]
fn db3s_t15_an_unknown_member_in_a_multi_select_shape_is_not_read() {
    let moved = TaskSelectionConfirmation::PickMoved {
        frame_id: frame_id('1'),
        pick_set_sha256: unmarked('c'),
    };
    let mut moved_record = frame_success();
    multi_of(&mut moved_record).picks = vec![
        TaskSelectionPick {
            candidate_id: candidate_id(4),
            confirmation: moved.clone(),
            tap_check: None,
        },
        skipped(1),
        skipped(6),
    ];
    let moved_record = with_top(moved_record, moved);

    let unmarked_after = mark_unconfirmed(
        1,
        TaskSelectionMarkExpectation::Marked,
        TaskSelectionMarkState::Unmarked,
        '3',
    );
    let mut unmarked_record = frame_success();
    multi_of(&mut unmarked_record).picks = vec![
        frame_pick(4, '1', Some('2')),
        TaskSelectionPick {
            candidate_id: candidate_id(1),
            confirmation: pick_matched('2'),
            tap_check: Some(unmarked_after.clone()),
        },
        skipped(6),
    ];
    let unmarked_record = with_top(unmarked_record, unmarked_after);

    let mut stopped_record = frame_success();
    multi_of(&mut stopped_record).picks = vec![
        frame_pick(4, '1', Some('2')),
        frame_pick(1, '2', None),
        skipped(6),
    ];
    let stopped_record = with_top(
        stopped_record,
        TaskSelectionConfirmation::PicksStopped { count: 1 },
    );

    // The object each pointer names gains a member.
    let cases = [
        ("multi_select", frame_success(), "/multi_select"),
        ("a pick", frame_success(), "/multi_select/picks/0"),
        (
            "pick_matched",
            frame_success(),
            "/multi_select/picks/0/confirmation",
        ),
        (
            "tap_marked",
            frame_success(),
            "/multi_select/picks/0/tap_check",
        ),
        ("picks_confirmed", frame_success(), "/confirmation"),
        (
            "pick_moved",
            moved_record,
            "/multi_select/picks/0/confirmation",
        ),
        (
            "mark_unconfirmed",
            unmarked_record,
            "/multi_select/picks/1/tap_check",
        ),
        ("picks_stopped", stopped_record, "/confirmation"),
    ];
    for (label, shape, pointer) in cases {
        let mut value = as_json(&shape);
        serde_json::from_value::<TaskSelectionRecord>(value.clone())
            .unwrap_or_else(|error| panic!("{label}: the record as written is read: {error}"));
        value
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("{label}: {pointer} is an object"))
            .insert("unexpected".to_owned(), Value::from(1));
        assert!(
            serde_json::from_value::<TaskSelectionRecord>(value).is_err(),
            "{label}: a member 0.12.x added is not read"
        );
    }
}
