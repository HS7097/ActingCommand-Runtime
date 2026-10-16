// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 slice SEL-L4b (frozen selector model S0 v2.1 §2.2 `Decide`, `Return` and
// `Confirm`, §3.2-§3.4, §6 row L4b): a select over a scan, driven by a fake source that replays
// synthetic viewport sequences. The list is the neutral grid of the SEL-L3 gate: three columns,
// a row pitch of 100 px, cards of 100 x 80 px, a readable band from y 60 to 540 and a tracking
// area from y 40 to 560 in a 400 x 600 frame; an advance moves 180 to 400 px and a reposition at
// most 480 px. Every expected value comes from the model: the unknown-instance rules of §3.2,
// the bound of §3.3, the path table of §3.4 (each record is written, read back and validated),
// the windows of §2.4 and the return rule of §2.2. Gate exec 79.

use super::scan::{
    ScanDeclaration, ScanDrag, ScanDragMotion, ScanReadInstance, ScanReading, ScanSelectEnd,
    ScanSelection, ScanSession, ScanSource,
};
use super::selection::scan_selection_record;
use super::{ContainedTaskError, ContainedTaskRunError};
use crate::codes::ExecutionKernelCode;
use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateIdentitySource, CandidateRect,
    CandidateUnknownReason, FrameId, ScanCoverageState, TaskSelectionBlocking,
    TaskSelectionConfirmation, TaskSelectionIdentityState, TaskSelectionOutcome,
    TaskSelectionRecord, TaskSelectionRepositionStage, TaskSelectionUpperBound,
};
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition_pack::CandidateLayout;
use actingcommand_selection_policy::{SelectionFactSnapshot, SelectionPolicy};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

const FRAME: (u32, u32) = (400, 600);
const COLUMNS: [i32; 3] = [20, 140, 260];
const POLICY_PATH: &str = "operations/task/policies/select.json";

// One card of the synthetic list, in content coordinates: its top edge, its left edge, its
// identity (read or not), its signature colour, its gate feature and its level (read or not).
#[derive(Debug, Clone)]
struct Card {
    axis: i32,
    cross: i32,
    identity: String,
    readable: bool,
    color: [u8; 3],
    open: bool,
    level: Option<i64>,
}

// `rows` rows of three distinct open cards of level 500, the first row at content y 60.
fn cards(rows: usize) -> Vec<Card> {
    (0..rows * 3)
        .map(|index| Card {
            axis: 60 + 100 * i32::try_from(index / 3).expect("row"),
            cross: COLUMNS[index % 3],
            identity: format!("member-{index:02}"),
            readable: true,
            color: [
                u8::try_from(index % 32).expect("level"),
                u8::try_from((index / 32) % 32).expect("level"),
                9,
            ],
            open: true,
            level: Some(500),
        })
        .collect()
}

// Only the cards with these indices are open; every other one is closed (gated out).
fn open_only(mut list: Vec<Card>, open: &[usize]) -> Vec<Card> {
    for (index, card) in list.iter_mut().enumerate() {
        card.open = open.contains(&index);
    }
    list
}

fn domain(list: &[Card]) -> Vec<String> {
    list.iter()
        .map(|card| card.identity.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn digest(color: [u8; 3]) -> ColorDigest {
    ColorDigest::from_hex(
        ColorDigestGrid::new(1, 1).expect("grid"),
        &format!("{:02x}{:02x}{:02x}", color[0], color[1], color[2]),
    )
    .expect("digest")
}

fn features(card: &Card) -> CandidateFeatureMap {
    let identity = if card.readable {
        CandidateFeature::Identity {
            value: card.identity.clone(),
            variant: None,
            source: CandidateIdentitySource::OcrAlias,
            distance: 0,
            confidence: Some(1000),
        }
    } else {
        CandidateFeature::Unknown {
            reason: CandidateUnknownReason::LowConfidence,
            source: Some(CandidateIdentitySource::OcrAlias),
        }
    };
    let level = match card.level {
        Some(value) => CandidateFeature::Integer {
            value,
            confidence: None,
        },
        None => CandidateFeature::Unknown {
            reason: CandidateUnknownReason::Missing,
            source: None,
        },
    };
    BTreeMap::from([
        ("member".to_owned(), identity),
        (
            "open".to_owned(),
            CandidateFeature::Boolean {
                value: card.open,
                confidence: None,
            },
        ),
        ("level".to_owned(), level),
    ])
}

// One captured frame of the list.
#[derive(Debug, Clone)]
struct View {
    instances: Vec<ScanReadInstance>,
    start: bool,
    end: bool,
}

// The list at content offset `offset`: every card that lies in the frame, actionable inside the
// readable band, with its features only then.
fn view(list: &[Card], offset: i32, start: bool, end: bool) -> View {
    let instances = list
        .iter()
        .filter_map(|card| {
            let y = card.axis - offset;
            (y >= 0 && y + 80 <= 600).then(|| {
                let actionable = y >= 60 && y + 80 <= 540;
                ScanReadInstance {
                    rect: CandidateRect {
                        x: card.cross,
                        y,
                        width: 100,
                        height: 80,
                    },
                    signature: Some(digest(card.color)),
                    actionable,
                    features: if actionable {
                        features(card)
                    } else {
                        CandidateFeatureMap::new()
                    },
                }
            })
        })
        .collect();
    View {
        instances,
        start,
        end,
    }
}

// What the fake hands out: the dispatched frame, then its captures in order.
struct Script {
    dispatched: Vec<CandidateRect>,
    captures: VecDeque<Result<View, ContainedTaskError>>,
}

impl Script {
    // `Pin`: the dispatched frame, and one equal capture, so it settles at once.
    fn starting_with(first: View) -> Self {
        Self {
            dispatched: first
                .instances
                .iter()
                .map(|instance| instance.rect)
                .collect(),
            captures: VecDeque::from([Ok(first)]),
        }
    }

    // After a move, or for a fresh F2: the frame captured twice, so it settles at the second.
    fn then(mut self, moved: View) -> Self {
        self.captures.push_back(Ok(moved.clone()));
        self.captures.push_back(Ok(moved));
        self
    }

    // One more capture of a frame (F2 captured again while its identity reads Unknown).
    fn once(mut self, frame: View) -> Self {
        self.captures.push_back(Ok(frame));
        self
    }

    // A capture the source refuses with `error`.
    fn then_refused(mut self, error: ContainedTaskError) -> Self {
        self.captures.push_back(Err(error));
        self
    }
}

// The identity of the n-th capture's frame.
fn frame_id(capture: u32) -> FrameId {
    serde_json::from_value(json!(format!("frame_{capture:032x}"))).expect("frame id")
}

// A scan source that replays a script; every capture costs the settle interval and is the next
// frame, with its own frame identity.
struct Fake {
    script: Script,
    current: Option<View>,
    captures: u32,
    clock: Duration,
    drags: Vec<ScanDragMotion>,
}

impl ScanSource for Fake {
    type Error = String;

    fn elapsed(&mut self) -> Duration {
        self.clock
    }

    fn dispatched(&mut self) -> Result<Vec<CandidateRect>, ContainedTaskRunError<String>> {
        Ok(self.script.dispatched.clone())
    }

    fn capture(
        &mut self,
        interval: Duration,
    ) -> Result<Vec<CandidateRect>, ContainedTaskRunError<String>> {
        self.clock += interval;
        let view =
            self.script.captures.pop_front().ok_or_else(|| {
                ContainedTaskRunError::Boundary("no further capture".to_owned())
            })??;
        self.captures += 1;
        let anchors = view
            .instances
            .iter()
            .map(|instance| instance.rect)
            .collect();
        self.current = Some(view);
        Ok(anchors)
    }

    fn viewport(&mut self) -> Result<ScanReading, ContainedTaskRunError<String>> {
        let view = self
            .current
            .clone()
            .ok_or_else(|| ContainedTaskRunError::Boundary("no capture yet".to_owned()))?;
        Ok(ScanReading {
            instances: view.instances,
            start_check: view.start,
            end_check: view.end,
            frame_id: Some(frame_id(self.captures)),
        })
    }

    fn drag(&mut self, drag: ScanDrag) -> Result<(), ContainedTaskRunError<String>> {
        self.drags.push(drag.motion);
        Ok(())
    }
}

// The declaration every test starts from (the SEL-L3 gate's); `patch` changes it.
fn declaration(patch: impl FnOnce(&mut Value)) -> ScanDeclaration {
    let mut value = json!({
        "identity_feature": "member",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 150, "y": 450, "width": 100, "height": 20},
                    "to": {"x": 150, "y": 150, "width": 100, "height": 20},
                    "duration_ms": 500, "min_content_step_px": 180, "max_content_step_px": 400},
        "reposition": {"kind": "drag",
                       "from": {"x": 150, "y": 500, "width": 100, "height": 20},
                       "to": {"x": 150, "y": 80, "width": 100, "height": 20},
                       "duration_ms": 400, "max_content_step_px": 480},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 40, "width": 400, "height": 520},
                  "edge_margin_px": 12,
                  "signature": {"rect": {"x": 0, "y": 0, "width": 20, "height": 20},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 4,
                  "min_overlap_instances": 2,
                  "spacing": {"max_axis_gap_px": 120, "columns": 3, "max_trailing_gap_px": 200,
                              "max_cross_lead_px": 40, "max_cross_gap_px": 140}},
        "start": {"check": "check/list_start", "rewind": true},
        "end": {"check": "check/list_end"},
        "bounds": {"max_viewports": 32, "max_reposition_viewports": 8, "max_ms": 240000}
    });
    patch(&mut value);
    ScanDeclaration::for_test(value)
}

fn layout(identities: &[String]) -> CandidateLayout {
    serde_json::from_value(json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 100, "height": 80},
        "click": {"x": 10, "y": 10, "width": 80, "height": 40},
        "readable_band": {"x": 0, "y": 60, "width": 400, "height": 480},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 50},
             "identity": {
                 "entries": identities.iter().map(|id| json!({"id": id, "aliases": [id]}))
                     .collect::<Vec<_>>(),
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 80, "y": 0}},
            {"name": "level", "value": "measure_milli", "target": "card/level",
             "offset": {"x": 60, "y": 0}}
        ]
    }))
    .expect("layout")
}

const LOWEST_ID: &str = "candidate_id";

// A policy over the cards: the gate `open`, the given scoring terms and tie-break keys, mode
// `none_allowed` with one candidate.
fn policy_with(identities: &[String], scoring: Value, tie_break: Value) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": identities}},
            {"name": "open", "value_type": {"type": "boolean"}},
            {"name": "level", "value_type": {"type": "integer"}}
        ],
        "facts": [],
        "gates": [{"gate_id": "open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "open"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": scoring,
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": tie_break
    }))
    .expect("policy");
    policy.validate().expect("valid policy");
    policy
}

fn lowest_id() -> Value {
    json!([{"kind": LOWEST_ID, "direction": "lowest_first"}])
}

// A member set: every open card is a member, all of equal score.
fn member_set(identities: &[String]) -> SelectionPolicy {
    policy_with(identities, json!([]), lowest_id())
}

// Ranked by level.
fn by_level(identities: &[String]) -> SelectionPolicy {
    policy_with(
        identities,
        json!([{"term_id": "level", "value": {"source": "field", "field": "level"},
                "transform": {"kind": "identity"}, "weight_milli": 1000,
                "on_unknown": {"kind": "drop_candidate"}}]),
        lowest_id(),
    )
}

// A ranked list: `top` ranks 3000, every other identity 1000.
fn ranked(identities: &[String], top: &str, tie_break: Value) -> SelectionPolicy {
    policy_with(
        identities,
        json!([{"term_id": "rank", "value": {"source": "field", "field": "member"},
                "transform": {"kind": "lookup",
                              "entries": [{"key": {"type": "string", "value": top},
                                           "value_milli": 3000}],
                              "default_milli": 1000},
                "weight_milli": 1000, "on_unknown": {"kind": "drop_candidate"}}]),
        tie_break,
    )
}

fn no_facts() -> SelectionFactSnapshot {
    SelectionFactSnapshot {
        snapshot_id: "snapshot-empty".into(),
        snapshot_at_unix_ms: 0,
        facts: BTreeMap::new(),
    }
}

struct Selected {
    selection: Result<ScanSelection<String>, ContainedTaskRunError<String>>,
    record: Option<TaskSelectionRecord>,
    drags: Vec<ScanDragMotion>,
    captures: u32,
    left: usize,
}

// The record of a selection (S0 §1.9, §3.4), written, validated for the ledger, read back and
// validated again.
fn record_of(session: &ScanSession<'_>, selection: &ScanSelection<String>) -> TaskSelectionRecord {
    let projection = session
        .projection(selection.coverage)
        .expect("the scan's projection");
    let record = scan_selection_record(
        POLICY_PATH,
        (1, 1_000),
        &selection.decision,
        projection,
        selection.unknown_check,
        selection.confirmation.clone(),
    )
    .expect("the record");
    record.validate_for_append().expect("an appendable record");
    let read: TaskSelectionRecord =
        serde_json::from_value(serde_json::to_value(&record).expect("encoded")).expect("decoded");
    assert_eq!(read, record);
    read.validate().expect("the record reads back");
    record
}

fn select_with(
    scan: &ScanDeclaration,
    list: &[Card],
    policy: &SelectionPolicy,
    facts: &SelectionFactSnapshot,
    script: Script,
    on_empty: bool,
) -> Selected {
    let identities = domain(list);
    let layout = layout(&identities);
    let mut session = ScanSession::new(
        scan,
        &layout,
        policy,
        facts,
        1_000,
        FRAME,
        Duration::from_secs(1_800),
    )
    .expect("session");
    let mut fake = Fake {
        script,
        current: None,
        captures: 0,
        clock: Duration::ZERO,
        drags: Vec::new(),
    };
    let selection = session.select(&mut fake, on_empty);
    let record = selection
        .as_ref()
        .ok()
        .map(|selection| record_of(&session, selection));
    Selected {
        selection,
        record,
        drags: fake.drags,
        captures: fake.captures,
        left: fake.script.captures.len(),
    }
}

fn select(scan: &ScanDeclaration, list: &[Card], script: Script, on_empty: bool) -> Selected {
    let policy = member_set(&domain(list));
    select_with(scan, list, &policy, &no_facts(), script, on_empty)
}

fn selection(selected: &Selected) -> &ScanSelection<String> {
    match &selected.selection {
        Ok(selection) => selection,
        Err(error) => panic!("expected a decision, got {error:?}"),
    }
}

fn record(selected: &Selected) -> &TaskSelectionRecord {
    selected.record.as_ref().expect("a record")
}

// The task failure a selection ends with once its record is written.
fn failure(selected: &Selected) -> &ContainedTaskError {
    match &selection(selected).end {
        ScanSelectEnd::Failed(ContainedTaskRunError::Task(error)) => error,
        ScanSelectEnd::Failed(other) => panic!("expected a task failure, got {other:?}"),
        ScanSelectEnd::Confirmed { identity, .. } => panic!("expected a failure, got {identity}"),
        ScanSelectEnd::Empty => panic!("expected a failure, got an empty end"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// The member the selection confirmed, and its rectangle on F2.
fn confirmed(selected: &Selected) -> (&str, CandidateRect) {
    match &selection(selected).end {
        ScanSelectEnd::Confirmed { identity, rect } => (identity.as_str(), *rect),
        ScanSelectEnd::Failed(error) => panic!("expected a confirmed member, got {error:?}"),
        ScanSelectEnd::Empty => panic!("expected a confirmed member, got an empty end"),
    }
}

// The code a record's confirmation keeps for its failure.
fn confirmation_code(confirmation: &TaskSelectionConfirmation) -> String {
    match confirmation {
        TaskSelectionConfirmation::CaptureFailed { outcome }
        | TaskSelectionConfirmation::GuardFailed { outcome }
        | TaskSelectionConfirmation::RepositionFailed { outcome, .. } => {
            outcome.code().as_str().to_owned()
        }
        other => panic!("expected a confirmation with an outcome, got {other:?}"),
    }
}

// Four rows on one screen: the start and the end check both pass on viewport 0.
fn one_screen(list: &[Card]) -> Script {
    Script::starting_with(view(list, 0, true, true))
}

// Twelve rows read from the top in steps of 300 px, the last step short at the list's end.
fn top_to_end(list: &[Card]) -> Script {
    Script::starting_with(view(list, 0, true, false))
        .then(view(list, 300, false, false))
        .then(view(list, 600, false, false))
        .then(view(list, 760, false, true))
}

// Thirty rows read from the top in steps of 400 px, the last one short at the list's end.
fn long_scan(list: &[Card]) -> Script {
    let mut script = Script::starting_with(view(list, 0, true, false));
    for offset in [400, 800, 1200, 1600, 2000, 2400] {
        script = script.then(view(list, offset, false, false));
    }
    script.then(view(list, 2600, false, true))
}

const RETURN_REPOSITION: ScanDragMotion = ScanDragMotion::Return {
    reposition: true,
    toward_start: true,
};
const RETURN_ADVANCE: ScanDragMotion = ScanDragMotion::Return {
    reposition: false,
    toward_start: true,
};

// P4 and P5 (S0 §3.2 rule 1, §1.4): a complete coverage where nobody listed is present and
// nothing is unknown ends the step normally when it declares `on_empty`, with no confirmation;
// without `on_empty` the same decision fails `selection_not_selected`, as today.
#[test]
fn l4b_on_empty_ends_a_complete_scan_with_nobody_listed_and_nothing_unknown() {
    let list = open_only(cards(4), &[]);
    let selected = select(&declaration(|_| {}), &list, one_screen(&list), true);
    let selection = selection(&selected);
    assert!(matches!(selection.end, ScanSelectEnd::Empty));
    assert_eq!(selection.coverage, ScanCoverageState::Complete);
    assert_eq!(selection.unknown_check.unknown_instances, 0);
    assert_eq!(
        selection.unknown_check.blocking,
        TaskSelectionBlocking::None
    );
    assert_eq!(
        selection.unknown_check.upper_bound,
        TaskSelectionUpperBound::NoneUnknown
    );
    let recorded = record(&selected);
    assert_eq!(recorded.selection_result, TaskSelectionOutcome::Empty);
    assert_eq!(
        recorded.confirmation,
        TaskSelectionConfirmation::NotAttempted
    );
    assert_eq!(recorded.outcome_key, "card_empty");
    assert!(selected.drags.is_empty());

    let without = select(&declaration(|_| {}), &list, one_screen(&list), false);
    let error = failure(&without);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionNotSelected.as_str()
    );
    assert_eq!(
        record(&without).confirmation,
        TaskSelectionConfirmation::NotAttempted
    );
}

// P6 (S0 §3.2 rule 2): an empty decision with one card it could not read is blocked, even with
// `on_empty`; the record keeps the evaluator's `empty` and says why it did not stand.
#[test]
fn l4b_an_empty_decision_with_an_unreadable_card_is_blocked() {
    let mut list = open_only(cards(4), &[]);
    list[4].readable = false;
    let selected = select(&declaration(|_| {}), &list, one_screen(&list), true);
    let error = failure(&selected);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionResultBlocked.as_str()
    );
    assert_eq!(value(error, "stage"), json!("select_decide"));
    assert_eq!(value(error, "selection_kind"), json!("empty"));
    assert_eq!(value(error, "outcome_key"), json!("card_empty"));
    assert_eq!(
        value(error, "selection_blocking"),
        json!("unknown_instances")
    );
    assert_eq!(value(error, "count"), json!(1));
    let record = record(&selected);
    assert_eq!(record.selection_result, TaskSelectionOutcome::Empty);
    let check = record.unknown_check.expect("unknown_check");
    assert_eq!(check.unknown_instances, 1);
    assert_eq!(check.blocking, TaskSelectionBlocking::UnknownInstances);
    assert_ne!(check.upper_bound, TaskSelectionUpperBound::NoneUnknown);
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
}

// P7 (S0 §3.2 rule 3, §3.3): a choice that a card of unread level might outrank is blocked: the
// card's identity is Known, so it is bounded at that identity over the level's whole domain
// (0..=1000), above the chosen level 500. No confirmation is attempted and no runner-up is
// chosen.
#[test]
fn l4b_a_choice_an_unread_card_might_outrank_is_blocked() {
    let mut list = open_only(cards(4), &[0, 1]);
    list[1].level = None;
    let policy = by_level(&domain(&list));
    let selected = select_with(
        &declaration(|_| {}),
        &list,
        &policy,
        &no_facts(),
        one_screen(&list),
        false,
    );
    let error = failure(&selected);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionResultBlocked.as_str()
    );
    assert_eq!(value(error, "selection_kind"), json!("selected"));
    assert_eq!(value(error, "outcome_key"), json!("card_selected"));
    assert_eq!(
        value(error, "selection_blocking"),
        json!("outranked_possible")
    );
    assert_eq!(value(error, "count"), json!(1));
    let record = record(&selected);
    assert_eq!(record.selected, ["layout/cards#00"]);
    let check = record.unknown_check.expect("unknown_check");
    assert_eq!(check.blocking, TaskSelectionBlocking::OutrankedPossible);
    let chosen = record.verdicts[0].score_milli.expect("chosen score");
    match check.upper_bound {
        TaskSelectionUpperBound::Max { milli } => assert!(milli > chosen),
        other => panic!("expected a finite bound above the choice, got {other:?}"),
    }
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    assert!(selected.drags.is_empty());
}

// S0 §3.3 for a ranked list: the top-ranked identity is read on viewport 0, so no unseen
// position could score above it and the policy's first tie-break key is `candidate_id
// lowest_first`; the scan stops there, chooses it and confirms it on F2 without a move.
#[test]
fn l4b_a_ranked_list_stops_at_its_top_ranked_member_and_confirms_it() {
    let list = cards(12);
    let policy = ranked(&domain(&list), "member-04", lowest_id());
    let script =
        Script::starting_with(view(&list, 0, true, false)).then(view(&list, 0, true, false));
    let selected = select_with(
        &declaration(|value| value["early_stop"] = json!("upper_bound")),
        &list,
        &policy,
        &no_facts(),
        script,
        false,
    );
    assert_eq!(
        selection(&selected).coverage,
        ScanCoverageState::EarlyStopped
    );
    let (identity, rect) = confirmed(&selected);
    assert_eq!(identity, "member-04");
    assert_eq!(
        rect,
        CandidateRect {
            x: 140,
            y: 160,
            width: 100,
            height: 80
        }
    );
    let record = record(&selected);
    assert_eq!(record.selected, ["layout/cards#04"]);
    assert_eq!(
        record.confirmation,
        TaskSelectionConfirmation::IdentityMatched {
            candidate_id: "layout/cards#04".to_owned(),
            frame_id: frame_id(3),
            content_offset_px: 0,
        }
    );
    assert!(selected.drags.is_empty());
}

// S0 §3.3 for a member set: every score is equal, so the scan stops at its first member when
// nothing is unknown, and the lowest candidate ID wins.
#[test]
fn l4b_a_member_set_stops_at_its_first_member() {
    let list = cards(12);
    let script =
        Script::starting_with(view(&list, 0, true, false)).then(view(&list, 0, true, false));
    let selected = select(
        &declaration(|value| value["early_stop"] = json!("upper_bound")),
        &list,
        script,
        false,
    );
    assert_eq!(
        selection(&selected).coverage,
        ScanCoverageState::EarlyStopped
    );
    assert_eq!(confirmed(&selected).0, "member-00");
    assert_eq!(record(&selected).selected, ["layout/cards#00"]);
}

// S0 §3.3: with a value tie-break key first, an unseen position scoring as much as the best
// member could be preferred, so a tie does not stop the scan: it reads to the end, then
// returns to the top-ranked member (one reposition, then one advance once the distance is at
// most the threshold) and confirms it.
#[test]
fn l4b_a_value_tie_break_first_does_not_stop_on_a_tie() {
    let list = cards(12);
    let policy = ranked(
        &domain(&list),
        "member-04",
        json!([{"kind": "value", "value": {"source": "field", "field": "level"},
                "direction": "highest_first"},
               {"kind": LOWEST_ID, "direction": "lowest_first"}]),
    );
    let script = top_to_end(&list)
        .then(view(&list, 280, false, false))
        .then(view(&list, 80, false, false))
        .then(view(&list, 80, false, false));
    let selected = select_with(
        &declaration(|value| value["early_stop"] = json!("upper_bound")),
        &list,
        &policy,
        &no_facts(),
        script,
        false,
    );
    assert_eq!(selection(&selected).coverage, ScanCoverageState::Complete);
    assert_eq!(confirmed(&selected).0, "member-04");
    assert_eq!(
        selected.drags,
        [
            ScanDragMotion::Advance,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance,
            RETURN_REPOSITION,
            RETURN_ADVANCE
        ]
    );
    assert_eq!(
        record(&selected).confirmation,
        TaskSelectionConfirmation::IdentityMatched {
            candidate_id: "layout/cards#04".to_owned(),
            frame_id: frame_id(selected.captures),
            content_offset_px: 80,
        }
    );
}

// S0 §2.6, §2.1: two rows of one family hold the same business ID at six positions; each is its
// own member, numbered in first-seen order, and the last tie-break key makes the first seen win,
// never `ambiguous`.
#[test]
fn l4b_duplicate_identity_rows_are_members_and_the_first_seen_wins() {
    let mut list = open_only(cards(4), &[0, 1, 2, 3, 4, 5]);
    for card in &mut list[..6] {
        card.identity = "member-family".to_owned();
    }
    let script = one_screen(&list).then(view(&list, 0, true, true));
    let selected = select(&declaration(|_| {}), &list, script, false);
    let record = record(&selected);
    assert_eq!(record.verdicts.len(), 6);
    assert_eq!(record.selected, ["layout/cards#00"]);
    assert_eq!(
        record.selection_result,
        TaskSelectionOutcome::Selected { count: 1 }
    );
    let (identity, rect) = confirmed(&selected);
    assert_eq!(identity, "member-family");
    assert_eq!((rect.x, rect.y), (20, 60));
}

// S0 §1.3, §3.4 P10 and P11: F2's instance at the chosen member's content position decides. The
// chosen card (member-00, top left) lies in the area's edge margin (24 px here), so the
// alignment ignores it and only its identity is compared. Present with its identity: matched;
// missing: absent; another Known identity: different, at once; Unknown on every capture until
// the settle timeout: unknown; Unknown on F2 and then rendered: matched on the capture after.
#[test]
fn l4b_f2_confirms_the_chosen_identity_and_types_each_way_it_does_not() {
    let scan = declaration(|value| value["track"]["edge_margin_px"] = json!(24));
    let list = open_only(cards(4), &[0]);
    let run = |f2: Script| select(&scan, &list, f2, false);
    let base = || one_screen(&list);

    let matched = run(base().then(view(&list, 0, true, true)));
    assert_eq!(confirmed(&matched).0, "member-00");

    let mut missing = list.clone();
    missing.remove(0);
    let absent = run(base().then(view(&missing, 0, true, true)));
    let error = failure(&absent);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionIdentityUnconfirmed.as_str()
    );
    assert_eq!(value(error, "stage"), json!("select_confirm"));
    assert_eq!(value(error, "expected_identity"), json!("member-00"));
    assert_eq!(value(error, "identity_state"), json!("absent"));
    assert_eq!(
        record(&absent).confirmation,
        TaskSelectionConfirmation::IdentityUnconfirmed {
            identity_state: TaskSelectionIdentityState::Absent,
            observed_identity: None,
            identity_unknown: None,
        }
    );

    let mut other = list.clone();
    other[0].identity = "member-11".to_owned();
    let different = run(base().then(view(&other, 0, true, true)));
    let error = failure(&different);
    assert_eq!(value(error, "identity_state"), json!("different"));
    assert_eq!(value(error, "observed_identity"), json!("member-11"));
    assert_eq!(
        record(&different).confirmation,
        TaskSelectionConfirmation::IdentityUnconfirmed {
            identity_state: TaskSelectionIdentityState::Different,
            observed_identity: Some("member-11".to_owned()),
            identity_unknown: None,
        }
    );
    // A different identity fails at once: no capture after F2's settle pair.
    assert_eq!(different.captures, 3);

    let mut unread = list.clone();
    unread[0].readable = false;
    let mut script = base().then(view(&unread, 0, true, true));
    for _ in 0..6 {
        script = script.once(view(&unread, 0, true, true));
    }
    let unknown = run(script);
    let error = failure(&unknown);
    assert_eq!(value(error, "identity_state"), json!("unknown"));
    assert_eq!(
        record(&unknown).confirmation,
        TaskSelectionConfirmation::IdentityUnconfirmed {
            identity_state: TaskSelectionIdentityState::Unknown,
            observed_identity: None,
            identity_unknown: Some(CandidateUnknownReason::LowConfidence),
        }
    );
    // F2's settle pair and its captures at the settle interval until the settle timeout
    // (1000 ms at 200 ms): five captures in all, and no more.
    assert_eq!(unknown.captures, 1 + 5);

    let late = run(base()
        .then(view(&unread, 0, true, true))
        .once(view(&list, 0, true, true)));
    assert_eq!(confirmed(&late).0, "member-00");
    assert_eq!(late.captures, 4);
}

// S0 §3.4 P12: F2 aligns against the content map only within the latest offset plus or minus
// the tolerance; a frame 100 px off has its only consistent shift outside that window.
#[test]
fn l4b_a_confirmation_frame_that_does_not_align_records_reposition_failed_at_confirm() {
    let list = open_only(cards(4), &[4]);
    let selected = select(
        &declaration(|_| {}),
        &list,
        one_screen(&list).then(view(&list, 100, false, false)),
        false,
    );
    let error = failure(&selected);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionScanAlignmentRejected.as_str()
    );
    assert_eq!(value(error, "stage"), json!("select_confirm"));
    assert_eq!(
        value(error, "scan_alignment_failure"),
        json!("unexpected_displacement")
    );
    let confirmation = &record(&selected).confirmation;
    assert!(matches!(
        confirmation,
        TaskSelectionConfirmation::RepositionFailed {
            stage: TaskSelectionRepositionStage::Confirm,
            ..
        }
    ));
    assert_eq!(
        confirmation_code(confirmation),
        ExecutionKernelCode::SelectionScanAlignmentRejected.as_str()
    );
}

// S0 §3.4 P8: F2's capture and settle failures are `capture_failed` with their code: anchors
// that never settle within the settle timeout, and a capture whose geometry changed.
#[test]
fn l4b_f2_settle_timeout_and_geometry_change_record_capture_failed() {
    let list = open_only(cards(4), &[4]);
    let mut script = one_screen(&list);
    for offset in [0, 50, 0, 50, 0, 50] {
        script = script.once(view(&list, offset, false, false));
    }
    let unsettled = select(&declaration(|_| {}), &list, script, false);
    let error = failure(&unsettled);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::SelectionScanSettleIncomplete.as_str()
    );
    assert_eq!(
        confirmation_code(&record(&unsettled).confirmation),
        ExecutionKernelCode::SelectionScanSettleIncomplete.as_str()
    );
    assert!(matches!(
        record(&unsettled).confirmation,
        TaskSelectionConfirmation::CaptureFailed { .. }
    ));

    let moved = select(
        &declaration(|_| {}),
        &list,
        one_screen(&list).then_refused(ContainedTaskError::new(
            ExecutionKernelCode::CandidateSamplePageOrGeometryChanged.code(),
        )),
        false,
    );
    let error = failure(&moved);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::CandidateSamplePageOrGeometryChanged.as_str()
    );
    assert!(matches!(
        record(&moved).confirmation,
        TaskSelectionConfirmation::CaptureFailed { .. }
    ));
    assert_eq!(
        confirmation_code(&record(&moved).confirmation),
        ExecutionKernelCode::CandidateSamplePageOrGeometryChanged.as_str()
    );
}

// S0 §2.2 `Return` (review-2 H2, M1): from the end of a long list back to its first row takes
// five repositions toward the start while the distance exceeds the threshold, then one reversed
// advance into the band; no move goes toward the end, so the return never bounces. Every
// return frame aligns against the content map and counts as a return viewport.
#[test]
fn l4b_a_return_over_several_repositions_then_an_advance_lands_without_bouncing() {
    let list = open_only(cards(30), &[0]);
    let script = long_scan(&list)
        .then(view(&list, 2120, false, false))
        .then(view(&list, 1640, false, false))
        .then(view(&list, 1160, false, false))
        .then(view(&list, 680, false, false))
        .then(view(&list, 200, false, false))
        .then(view(&list, 0, true, false))
        .then(view(&list, 0, true, false));
    let selected = select(&declaration(|_| {}), &list, script, false);
    assert_eq!(confirmed(&selected).0, "member-00");
    let returns = selected
        .drags
        .iter()
        .filter(|motion| matches!(motion, ScanDragMotion::Return { .. }))
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        returns,
        [
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_ADVANCE
        ]
    );
    let record = record(&selected);
    assert_eq!(
        serde_json::to_value(&record.projection).expect("projection")["reposition"],
        json!({"rewind_viewports": 0, "return_viewports": 6})
    );
    assert!(matches!(
        record.confirmation,
        TaskSelectionConfirmation::IdentityMatched {
            content_offset_px: 0,
            ..
        }
    ));
    assert_eq!(selected.left, 0);
}

// S0 §2.2 `Return`: *T* = max(`advance.max`, `reposition.max` - (*b* - *e*)) = max(400, 480 -
// 400) = 400. The chosen card's band interval is [300, 700]; at a distance of 700 the return
// still repositions, at a distance of exactly 400 it switches to one advance, which ends inside
// the interval.
#[test]
fn l4b_a_return_switches_to_an_advance_once_the_distance_is_at_most_the_threshold() {
    let list = open_only(cards(30), &[21]);
    let script = long_scan(&list)
        .then(view(&list, 2200, false, false))
        .then(view(&list, 1800, false, false))
        .then(view(&list, 1400, false, false))
        .then(view(&list, 1100, false, false))
        .then(view(&list, 700, false, false))
        .then(view(&list, 700, false, false));
    let selected = select(&declaration(|_| {}), &list, script, false);
    assert_eq!(confirmed(&selected).0, "member-21");
    let returns = selected
        .drags
        .iter()
        .filter(|motion| matches!(motion, ScanDragMotion::Return { .. }))
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        returns,
        [
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_REPOSITION,
            RETURN_ADVANCE
        ]
    );
    assert!(matches!(
        record(&selected).confirmation,
        TaskSelectionConfirmation::IdentityMatched {
            content_offset_px: 700,
            ..
        }
    ));
}

// S0 §3.4 P2: a declared record list that cannot be read makes the decision at `Pin`: `unknown`
// with no viewport (`not_started`), no verdict, nothing confirmed, and the step fails
// `selection_not_selected`.
#[test]
fn l4b_an_unreadable_list_at_pin_is_recorded_as_not_started() {
    let list = cards(4);
    let identities = domain(&list);
    let keyed = json!({"source": "keyed_fact", "fact_key": "session.example.list.targets",
                       "key_field": "member", "value_column": "rank",
                       "absent": {"type": "integer", "value": 0}});
    let policy: SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-listed",
        "applies_to": {"candidate_layout_id": "layout/cards", "outcome_keys": {
            "selected": "card_selected", "empty": "card_empty",
            "insufficient": "card_insufficient", "ambiguous": "card_ambiguous",
            "unknown": "card_unknown"}},
        "fields": [{"name": "member", "value_type": {"type": "enum_string",
                                                     "allowed": identities}}],
        "facts": [{"fact_key": "session.example.list.targets",
                   "value_type": {"type": "record_list", "key_column": "id",
                                  "columns": {"rank": {"type": "integer"}}},
                   "max_age_ms": 3_600_000, "minimum_confidence_milli": 1_000}],
        "gates": [{"gate_id": "listed",
                   "predicate": {"kind": "integer_at_least", "value": keyed, "threshold": 1},
                   "on_unknown": {"kind": "abort_evaluation"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": LOWEST_ID, "direction": "lowest_first"}]
    }))
    .expect("keyed policy");
    policy.validate().expect("valid keyed policy");
    let facts = SelectionFactSnapshot::from_fact_records("snapshot-a", 1_000, &[]);
    let selected = select_with(
        &declaration(|_| {}),
        &list,
        &policy,
        &facts,
        one_screen(&list),
        true,
    );
    let selection = selection(&selected);
    assert_eq!(selection.coverage, ScanCoverageState::NotStarted);
    assert_eq!(
        failure(&selected).code().as_str(),
        ExecutionKernelCode::SelectionNotSelected.as_str()
    );
    let record = record(&selected);
    assert!(matches!(
        record.selection_result,
        TaskSelectionOutcome::Unknown { .. }
    ));
    assert!(record.verdicts.is_empty());
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    assert_eq!(
        serde_json::to_value(&record.projection).expect("projection")["viewports"],
        json!([])
    );
    // Nothing was captured: the decision came before the dispatched frame settled.
    assert_eq!(selected.captures, 0);
}

// S0 §3.4 P3: an evaluation that aborts in a viewport (a scalar fact the score reads is
// missing) is the decision: `unknown`, coverage `incomplete`, no verdict, nothing blocked and
// nothing confirmed; the positions that viewport read are its unknown positions.
#[test]
fn l4b_an_abort_in_a_viewport_is_recorded_as_incomplete() {
    let list = cards(4);
    // The term and its fact are patched in together: the document reads only declared facts.
    let mut policy_value = serde_json::to_value(member_set(&domain(&list))).expect("policy");
    policy_value["scoring"] = json!([{"term_id": "weight",
                                      "value": {"source": "fact",
                                                "fact_key": "session.example.weight"},
                                      "transform": {"kind": "identity"}, "weight_milli": 1000,
                                      "on_unknown": {"kind": "abort_evaluation"}}]);
    policy_value["facts"] = json!([{"fact_key": "session.example.weight",
                                    "value_type": {"type": "integer"},
                                    "max_age_ms": 3_600_000,
                                    "minimum_confidence_milli": 1_000}]);
    let policy: SelectionPolicy = serde_json::from_value(policy_value).expect("fact policy");
    policy.validate().expect("valid fact policy");
    let selected = select_with(
        &declaration(|_| {}),
        &list,
        &policy,
        &no_facts(),
        one_screen(&list),
        false,
    );
    let selection = selection(&selected);
    assert_eq!(selection.coverage, ScanCoverageState::Incomplete);
    assert_eq!(
        selection.unknown_check.blocking,
        TaskSelectionBlocking::None
    );
    assert_eq!(
        failure(&selected).code().as_str(),
        ExecutionKernelCode::SelectionNotSelected.as_str()
    );
    let record = record(&selected);
    assert!(matches!(
        record.selection_result,
        TaskSelectionOutcome::Unknown { .. }
    ));
    assert!(record.verdicts.is_empty());
    assert_eq!(record.confirmation, TaskSelectionConfirmation::NotAttempted);
    let check = record.unknown_check.expect("unknown_check");
    assert_eq!(check.unknown_instances, 12);
}
