// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L4a (frozen model S0 v2.1 §1.9, §3.4 and §6 row L4a): the record shapes of a
// scan select (projection v2; the confirmations identity_matched, identity_unconfirmed and
// reposition_failed; unknown_check). Every §3.4 path shape round-trips and validates, for a scan
// select (v2) and for a single-frame select that declares on_empty (v1); each of the seven path
// rules of §1.9 has records that break it; the reader bounds are 256 viewports, 64 members and
// 64 unknown positions.

use super::*;
use crate::{
    CANDIDATE_PROJECTION_BUDGET_EXCEEDED, CandidateFeature, CandidateFeatureMap, CandidateFrame,
    CandidateIdentitySource, CandidateLayoutKind, CandidateRect, INVALID_CANDIDATE_PROJECTION,
    ProjectedCandidate, ScanAxis, ScanCandidate, ScanCandidateProjectionParts, ScanCoverage,
    ScanEndEvidence, ScanGatedOut, ScanReposition, ScanStartEvidence, ScanUnknownCause,
    ScanUnknownPosition, ScanViewport, ScanViewportMotion, TaskSemanticFact,
};
use serde_json::Value;

const LAYOUT: &str = "list/rows";
const PAGE: &str = "list_page";
// SHA-256 of no identities.
const NO_IDENTITIES: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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
