// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 slice SEL-L3 (frozen selector model S0 v2.1 §2.1–§2.6, §6 row L3): the scan
// session engine, driven by a fake source that replays synthetic viewport sequences. The list is
// a neutral grid of cards: three columns, a row pitch of 100 px, cards of 100 x 80 px, a readable
// band from y 60 to 540 and a tracking area from y 40 to 560 in a 400 x 600 frame. Every value a
// test expects comes from the model: the windows of §2.4, the coverage and spacing rules of §2.5,
// the readings of §2.6 and the codes of §2.7. Gate exec 59.

use super::scan::{
    ScanDeclaration, ScanDragMotion, ScanDryRun, ScanEnd, ScanReadInstance, ScanReading,
    ScanSession, ScanSource,
};
use super::{ContainedTaskError, ContainedTaskRunError};
use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateIdentitySource, CandidateRect,
    CandidateUnknownReason, ScanCoverageState, ScanStartEvidence, ScanUnknownCause,
    ScanViewportMotion,
};
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition_pack::CandidateLayout;
use actingcommand_selection_policy::{SelectionFactSnapshot, SelectionPolicy};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

const FRAME: (u32, u32) = (400, 600);
const COLUMNS: [i32; 3] = [20, 140, 260];

// One card of the synthetic list, in content coordinates: its top edge on the axis, its left
// edge, its identity, its signature color and its gate feature.
#[derive(Debug, Clone)]
struct Card {
    axis: i32,
    cross: i32,
    identity: String,
    color: [u8; 3],
    open: bool,
}

// `rows` rows of three distinct cards, the first row at content y 60.
fn cards(rows: usize) -> Vec<Card> {
    (0..rows * 3)
        .map(|index| Card {
            axis: 60 + 100 * i32::try_from(index / 3).expect("row"),
            cross: COLUMNS[index % 3],
            identity: format!("member-{index:02}"),
            color: [
                u8::try_from(index % 32).expect("level"),
                u8::try_from((index / 32) % 32).expect("level"),
                9,
            ],
            open: true,
        })
        .collect()
}

// The cards with these row indices made one family: the same identity and the same look.
fn family(mut list: Vec<Card>, rows: std::ops::Range<usize>) -> Vec<Card> {
    for card in &mut list {
        let row = usize::try_from((card.axis - 60) / 100).expect("row");
        if rows.contains(&row) {
            card.identity = "member-family".to_owned();
            card.color = [30, 30, 30];
        }
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
    BTreeMap::from([
        (
            "member".to_owned(),
            CandidateFeature::Identity {
                value: card.identity.clone(),
                variant: None,
                source: CandidateIdentitySource::OcrAlias,
                distance: 0,
                confidence: Some(1000),
            },
        ),
        (
            "open".to_owned(),
            CandidateFeature::Boolean {
                value: card.open,
                confidence: None,
            },
        ),
    ])
}

// One captured frame of the list.
#[derive(Debug, Clone)]
struct View {
    instances: Vec<ScanReadInstance>,
    start: bool,
    end: bool,
}

// The list at content offset `offset`: every card that lies in the frame, actionable inside
// the readable band, with its features only then.
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

    // After a drag: the moved frame, captured twice, so it settles at the second capture.
    fn then(mut self, moved: View) -> Self {
        self.captures.push_back(Ok(moved.clone()));
        self.captures.push_back(Ok(moved));
        self
    }

    // A capture the source refuses with `error`, as a runtime refuses a frame of another page.
    fn then_refused(mut self, error: ContainedTaskError) -> Self {
        self.captures.push_back(Err(error));
        self
    }
}

// A scan source that replays a script; every capture costs the settle interval plus `cost`.
struct Fake {
    script: Script,
    current: Option<View>,
    clock: Duration,
    cost: Duration,
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
        self.clock += interval + self.cost;
        let view =
            self.script.captures.pop_front().ok_or_else(|| {
                ContainedTaskRunError::Boundary("no further capture".to_owned())
            })??;
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
            frame_id: None,
        })
    }

    fn drag(&mut self, drag: super::scan::ScanDrag) -> Result<(), ContainedTaskRunError<String>> {
        self.drags.push(drag.motion);
        Ok(())
    }
}

// The declaration every test starts from; `patch` changes it.
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
             "offset": {"x": 80, "y": 0}}
        ]
    }))
    .expect("layout")
}

// A member set: every open card is a member.
fn policy(identities: &[String]) -> SelectionPolicy {
    let policy: SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "cards-open",
        "applies_to": {
            "candidate_layout_id": "layout/cards",
            "outcome_keys": {"selected": "card_selected", "empty": "card_empty",
                             "insufficient": "card_insufficient",
                             "ambiguous": "card_ambiguous", "unknown": "card_unknown"}
        },
        "fields": [
            {"name": "member", "value_type": {"type": "enum_string", "allowed": identities}},
            {"name": "open", "value_type": {"type": "boolean"}}
        ],
        "facts": [],
        "gates": [{"gate_id": "open",
                   "predicate": {"kind": "boolean_equals",
                                 "value": {"source": "field", "field": "open"},
                                 "expected": true},
                   "on_unknown": {"kind": "drop_candidate"}}],
        "scoring": [],
        "selection": {"mode": "none_allowed", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("policy");
    policy.validate().expect("valid policy");
    policy
}

struct Run {
    result: Result<ScanEnd, ContainedTaskRunError<String>>,
    summary: ScanDryRun,
    drags: Vec<ScanDragMotion>,
}

fn run_with_cost(scan: &ScanDeclaration, list: &[Card], script: Script, cost: Duration) -> Run {
    let identities = domain(list);
    let layout = layout(&identities);
    let policy = policy(&identities);
    let facts = SelectionFactSnapshot {
        snapshot_id: "empty".into(),
        snapshot_at_unix_ms: 0,
        facts: BTreeMap::new(),
    };
    let mut session = ScanSession::new(
        scan,
        &layout,
        &policy,
        &facts,
        1_000,
        FRAME,
        Duration::from_secs(1_800),
    )
    .expect("session");
    let mut fake = Fake {
        script,
        current: None,
        clock: Duration::ZERO,
        cost,
        drags: Vec::new(),
    };
    let result = session.run(&mut fake);
    let summary = session.summary(result.as_ref().ok());
    Run {
        result,
        summary,
        drags: fake.drags,
    }
}

fn run(scan: &ScanDeclaration, list: &[Card], script: Script) -> Run {
    run_with_cost(scan, list, script, Duration::ZERO)
}

// The task failure a run ended with.
fn failure(run: &Run) -> &ContainedTaskError {
    match &run.result {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("expected a task failure, got {other:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

fn assert_covered(run: &Run) {
    assert!(
        matches!(run.result, Ok(ScanEnd::Covered)),
        "expected complete coverage, got {:?}",
        run.result
    );
    assert_eq!(run.summary.coverage, ScanCoverageState::Complete);
}

fn offsets(run: &Run) -> Vec<i64> {
    run.summary
        .viewports
        .iter()
        .map(|viewport| viewport.content_offset_px)
        .collect()
}

// Twelve rows read from the top in steps of 300 px, the last step short at the list's end.
fn top_to_end(list: &[Card]) -> Script {
    Script::starting_with(view(list, 0, true, false))
        .then(view(list, 300, false, false))
        .then(view(list, 600, false, false))
        .then(view(list, 760, false, true))
}

#[test]
fn l3_a_list_at_its_top_is_covered_to_its_end() {
    let list = cards(12);
    let run = run(&declaration(|_| {}), &list, top_to_end(&list));
    assert_covered(&run);
    assert_eq!(run.summary.start, Some(ScanStartEvidence::StartCheck));
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
    assert_eq!(
        run.summary
            .viewports
            .iter()
            .map(|viewport| viewport.motion)
            .collect::<Vec<_>>(),
        [
            ScanViewportMotion::Start,
            ScanViewportMotion::Forward,
            ScanViewportMotion::Forward,
            ScanViewportMotion::Forward
        ]
    );
    assert_eq!(run.drags, [ScanDragMotion::Advance; 3]);
    assert_eq!(run.summary.positions, 36);
    assert_eq!(run.summary.members.len(), 36);
    assert!(run.summary.unknown.is_empty());
}

#[test]
fn l3_a_mid_list_start_rewinds_until_the_start_check_passes() {
    let list = cards(12);
    let script = Script::starting_with(view(&list, 600, false, false))
        .then(view(&list, 120, false, false))
        .then(view(&list, 0, true, false))
        .then(view(&list, 300, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 760, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(run.summary.start, Some(ScanStartEvidence::RewindCheck));
    assert_eq!(run.summary.rewind_viewports, 2);
    assert_eq!(
        run.drags,
        [
            ScanDragMotion::Rewind,
            ScanDragMotion::Rewind,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance
        ]
    );
    // The rewind viewports are no coverage: the coverage starts at the start check.
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
}

#[test]
fn l3_a_one_screen_list_passes_both_checks_on_viewport_0() {
    let list = cards(4);
    let run = run(
        &declaration(|_| {}),
        &list,
        Script::starting_with(view(&list, 0, true, true)),
    );
    assert_covered(&run);
    assert_eq!(offsets(&run), [0]);
    assert!(run.drags.is_empty());
    assert_eq!(run.summary.members.len(), 12);
}

#[test]
fn l3_an_overshoot_inside_the_band_aligns() {
    // 390 px is beyond the usual step but within the declared largest step (400 px), so the
    // bands still overlap by the card extent.
    let list = cards(12);
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 390, false, false))
        .then(view(&list, 690, false, false))
        .then(view(&list, 760, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 390, 690, 760]);
    assert_eq!(run.drags, [ScanDragMotion::Advance; 3]);
}

#[test]
fn l3_a_gap_is_closed_by_its_corrective_twice_in_one_scan() {
    // A step of 403 px (the largest step plus 3 px of tolerance) leaves the bands 3 px short of
    // overlapping by the card extent; each such gap takes one corrective reverse advance.
    let list = cards(16);
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 403, false, false))
        .then(view(&list, 203, false, false))
        .then(view(&list, 606, false, false))
        .then(view(&list, 1009, false, false))
        .then(view(&list, 809, false, false))
        .then(view(&list, 1100, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 403, 203, 606, 1009, 809, 1100]);
    assert_eq!(
        run.drags,
        [
            ScanDragMotion::Advance,
            ScanDragMotion::Corrective,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance,
            ScanDragMotion::Corrective,
            ScanDragMotion::Advance
        ]
    );
    assert_eq!(
        run.summary
            .viewports
            .iter()
            .filter(|viewport| viewport.motion == ScanViewportMotion::Corrective)
            .count(),
        2
    );
}

#[test]
fn l3_a_gap_still_open_after_its_corrective_fails() {
    // The corrective drag is absorbed: the list stays where the gap opened.
    let list = cards(12);
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 403, false, false))
        .then(view(&list, 403, false, false));
    let run = run(&declaration(|_| {}), &list, script);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_gap_detected");
    assert_eq!(value(error, "stage"), "select_scan_align");
    assert_eq!(value(error, "viewport_index"), 2);
    assert_eq!(value(error, "gap_px"), 3);
}

#[test]
fn l3_two_absorbed_drags_mid_list_fail_end_unconfirmed() {
    let list = cards(12);
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 300, false, false))
        .then(view(&list, 300, false, false))
        .then(view(&list, 300, false, false));
    let run = run(&declaration(|_| {}), &list, script);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_end_unconfirmed");
    assert_eq!(value(error, "stage"), "select_scan_end");
    assert_eq!(value(error, "viewport_index"), 3);
}

#[test]
fn l3_a_popup_page_mid_scan_fails() {
    // The source refuses a capture of another page (`selection_page_changed`); the session
    // ends with it and makes no further drag.
    let list = cards(12);
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 300, false, false))
        .then_refused(ContainedTaskError::new(
            crate::codes::ExecutionKernelCode::SelectionPageChanged.code(),
        ));
    let run = run(&declaration(|_| {}), &list, script);
    assert_eq!(failure(&run).code().as_str(), "selection_page_changed");
    assert_eq!(run.drags, [ScanDragMotion::Advance; 2]);
}

#[test]
fn l3_a_missed_anchor_fails_instance_missing() {
    // The middle card of row 2 is never matched: the row holds two positions.
    let list = cards(12)
        .into_iter()
        .filter(|card| card.identity != "member-07")
        .collect::<Vec<_>>();
    let run = run(&declaration(|_| {}), &list, top_to_end(&list));
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_instance_missing");
    assert_eq!(value(error, "stage"), "select_scan_decide");
    assert_eq!(value(error, "scan_spacing_failure"), "row_short");
    assert_eq!(value(error, "count"), 2);
    assert_eq!(value(error, "limit_count"), 3);
}

#[test]
fn l3_a_last_row_that_never_enters_the_band_fails_position_unread() {
    // At the list's end the last row stops at y 470: tracked, but 10 px beyond the band.
    let list = cards(12);
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 300, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 690, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_position_unread");
    assert_eq!(value(error, "stage"), "select_scan_decide");
    assert_eq!(value(error, "position_index"), 33);
    assert_eq!(value(error, "viewport_index"), 3);
}

#[test]
fn l3_one_px_jitter_aligns() {
    let list = cards(12);
    let jittered = list
        .iter()
        .map(|card| Card {
            axis: card.axis + [1, -1, 0][usize::try_from(card.cross / 120).expect("column")],
            ..card.clone()
        })
        .collect::<Vec<_>>();
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&jittered, 300, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 760, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
    assert_eq!(run.summary.positions, 36);
}

#[test]
fn l3_look_alike_rows_with_distinct_identities_align_at_the_true_shift() {
    // Every card looks alike; only the identities tell the rows apart.
    let list = cards(12)
        .into_iter()
        .map(|card| Card {
            color: [12, 12, 12],
            ..card
        })
        .collect::<Vec<_>>();
    let run = run(&declaration(|_| {}), &list, top_to_end(&list));
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
    assert_eq!(run.summary.members.len(), 36);
}

#[test]
fn l3_a_screen_of_one_family_has_two_consistent_shifts_and_fails_ambiguous_shift() {
    // One family fills every row: no move and the true step both pair every card.
    let list = family(cards(12), 0..12);
    let run = run(&declaration(|_| {}), &list, top_to_end(&list));
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_alignment_rejected");
    assert_eq!(value(error, "stage"), "select_scan_align");
    assert_eq!(value(error, "viewport_index"), 1);
    assert_eq!(value(error, "scan_alignment_failure"), "ambiguous_shift");
    assert_eq!(value(error, "limit_count"), 2);
}

#[test]
fn l3_an_overlap_of_identical_rows_aligns_with_the_minimum_step() {
    // Rows 2 to 9 are one family, so each overlap holds only identical rows. A step window
    // (280 to 360 px) narrower than the row pitch keeps the shifts one pitch away outside it.
    let list = family(cards(12), 2..10);
    let scan = declaration(|value| {
        value["advance"]["min_content_step_px"] = json!(280);
        value["advance"]["max_content_step_px"] = json!(360);
    });
    let run = run(&scan, &list, top_to_end(&list));
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
}

#[test]
fn l3_a_flickering_gate_feature_makes_an_unknown_position() {
    // Card 9 reads open on viewport 0 and closed on viewport 1.
    let list = cards(12);
    let flickered = list
        .iter()
        .map(|card| Card {
            open: card.identity != "member-09",
            ..card.clone()
        })
        .collect::<Vec<_>>();
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&flickered, 300, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 760, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(run.summary.members.len(), 35);
    let [unknown] = run.summary.unknown.as_slice() else {
        panic!("one unknown position, got {:?}", run.summary.unknown);
    };
    assert_eq!(unknown.cause, ScanUnknownCause::Contradicted);
    assert_eq!(unknown.identity.as_deref(), Some("member-09"));
    assert_eq!(unknown.first_read_viewport, 0);
}

#[test]
fn l3_the_scan_deadline_is_its_max_ms_not_the_step_timeout() {
    // Every capture takes 20 s: the scan runs 140 s, past any step timeout (at most 60 s), and
    // is bounded by its own max_ms only.
    let list = cards(12);
    let scan = declaration(|value| value["settle"]["timeout_ms"] = json!(30_000));
    let cost = Duration::from_secs(20);
    let run = run_with_cost(&scan, &list, top_to_end(&list), cost);
    assert_covered(&run);

    let scan = declaration(|value| {
        value["settle"]["timeout_ms"] = json!(30_000);
        value["bounds"]["max_ms"] = json!(100_000);
    });
    let run = run_with_cost(&scan, &list, top_to_end(&list), cost);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_bounds_exceeded");
    assert_eq!(value(error, "scan_bound"), "duration");
    assert_eq!(value(error, "timeout_ms"), 100_000);
}

#[test]
fn l3_a_row_half_under_the_toolbar_and_a_card_jittering_at_the_area_edge_align() {
    // At offset 380 the row at content 860 ends at the area's lower edge (y 560) and jitters
    // 1 px past it; at offset 640 the row at content 660 sits half under the toolbar above the
    // area (y 20). Neither breaks an alignment.
    let list = cards(13);
    let jittered = list
        .iter()
        .map(|card| Card {
            axis: card.axis + i32::from(card.axis == 860),
            ..card.clone()
        })
        .collect::<Vec<_>>();
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&jittered, 380, false, false))
        .then(view(&list, 640, false, false))
        .then(view(&list, 800, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 380, 640, 800]);
    assert_eq!(run.summary.positions, 39);
}

#[test]
fn l3_proposals_of_199_200_and_201_px_form_one_shift() {
    let list = cards(12);
    let jittered = list
        .iter()
        .map(|card| Card {
            axis: card.axis + [1, 0, -1][usize::try_from(card.cross / 120).expect("column")],
            ..card.clone()
        })
        .collect::<Vec<_>>();
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&jittered, 200, false, false))
        .then(view(&list, 400, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 760, false, true));
    let run = run(&declaration(|_| {}), &list, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 200, 400, 600, 760]);
}

#[test]
fn l3_a_short_last_step_is_admitted_only_where_the_end_check_passes() {
    let list = cards(12);
    // Where the end check passes, the short last step (160 px) is the list stopping at its end.
    let run_at_end = run(&declaration(|_| {}), &list, top_to_end(&list));
    assert_covered(&run_at_end);
    // The same step elsewhere lies outside the forward window.
    let script = Script::starting_with(view(&list, 0, true, false))
        .then(view(&list, 300, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 760, false, false));
    let run = run(&declaration(|_| {}), &list, script);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_alignment_rejected");
    assert_eq!(value(error, "viewport_index"), 3);
    assert_eq!(
        value(error, "scan_alignment_failure"),
        "unexpected_displacement"
    );
}

#[test]
fn l3_a_missed_middle_card_of_the_last_row_fails_cross_gap() {
    let list = cards(12)
        .into_iter()
        .filter(|card| card.identity != "member-34")
        .collect::<Vec<_>>();
    let run = run(&declaration(|_| {}), &list, top_to_end(&list));
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_instance_missing");
    assert_eq!(value(error, "stage"), "select_scan_decide");
    assert_eq!(value(error, "scan_spacing_failure"), "cross_gap");
    assert_eq!(value(error, "gap_px"), 240);
}

#[test]
fn l3_a_missed_final_row_fails_trailing_gap() {
    // Row 11 is never matched: at the end the last row read starts 240 px before the band's
    // trailing edge.
    let list = cards(12)
        .into_iter()
        .filter(|card| card.axis < 1160)
        .collect::<Vec<_>>();
    let run = run(&declaration(|_| {}), &list, top_to_end(&list));
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_instance_missing");
    assert_eq!(value(error, "stage"), "select_scan_end");
    assert_eq!(value(error, "viewport_index"), 3);
    assert_eq!(value(error, "scan_spacing_failure"), "trailing_gap");
    assert_eq!(value(error, "gap_px"), 240);
}

#[test]
fn l3_a_frame_aligns_against_the_content_map_within_the_return_window() {
    // After complete coverage, a frame at offset 300 aligns directly against the content map:
    // a move toward the start from 760 by at most one reposition step (480 px).
    let list = cards(12);
    let identities = domain(&list);
    let layout = layout(&identities);
    let policy = policy(&identities);
    let facts = SelectionFactSnapshot {
        snapshot_id: "empty".into(),
        snapshot_at_unix_ms: 0,
        facts: BTreeMap::new(),
    };
    let scan = declaration(|_| {});
    let mut session = ScanSession::new(
        &scan,
        &layout,
        &policy,
        &facts,
        1_000,
        FRAME,
        Duration::from_secs(1_800),
    )
    .expect("session");
    let mut fake = Fake {
        script: top_to_end(&list),
        current: None,
        clock: Duration::ZERO,
        cost: Duration::ZERO,
        drags: Vec::new(),
    };
    assert!(matches!(session.run(&mut fake), Ok(ScanEnd::Covered)));
    let frame = view(&list, 300, false, false);
    let reading = ScanReading {
        instances: frame.instances,
        start_check: false,
        end_check: false,
        frame_id: None,
    };
    assert_eq!(
        session
            .align_return_for_test(&reading, 760, true, true)
            .expect("aligned"),
        300
    );
    // Toward the start by one advance step (at most 400 px), offset 300 lies outside.
    let error = session
        .align_return_for_test(&reading, 760, true, false)
        .expect_err("outside the window");
    assert_eq!(
        value(&error, "scan_alignment_failure"),
        "unexpected_displacement"
    );
}

// `view` with the identity of the card `identity` read Unknown (low confidence).
fn identity_unread(mut frame: View, identity: &str) -> View {
    for instance in &mut frame.instances {
        let read = matches!(
            instance.features.get("member"),
            Some(CandidateFeature::Identity { value, .. }) if value == identity
        );
        if read {
            instance.features.insert(
                "member".to_owned(),
                CandidateFeature::Unknown {
                    reason: CandidateUnknownReason::LowConfidence,
                    source: Some(CandidateIdentitySource::OcrAlias),
                },
            );
        }
    }
    frame
}

#[test]
fn l3_early_stop_ends_a_member_set_at_its_first_viewport_when_nothing_is_unknown() {
    // S0 §3.3: in a member set every member scores alike, so S equals the bound over the
    // unseen positions; with `candidate_id lowest_first` as the first tie-break key a later
    // member would lose the tie, and the scan stops at its first viewport.
    let list = cards(12);
    let scan = declaration(|value| value["early_stop"] = json!("upper_bound"));
    let run = run(&scan, &list, top_to_end(&list));
    assert!(
        matches!(run.result, Ok(ScanEnd::EarlyStopped)),
        "expected an early stop, got {:?}",
        run.result
    );
    assert_eq!(run.summary.coverage, ScanCoverageState::EarlyStopped);
    assert_eq!(offsets(&run), [0]);
    assert!(run.drags.is_empty());
    assert_eq!(run.summary.members.len(), 15);
}

#[test]
fn l3_early_stop_does_not_hold_while_an_unknown_position_might_tie() {
    // A card whose identity reads Unknown is bounded over the whole domain: it could score as
    // much as any member, so S > U fails and the scan reads to the end of the list.
    let list = cards(12);
    let scan = declaration(|value| value["early_stop"] = json!("upper_bound"));
    let script = Script::starting_with(identity_unread(view(&list, 0, true, false), "member-04"))
        .then(view(&list, 300, false, false))
        .then(view(&list, 600, false, false))
        .then(view(&list, 760, false, true));
    let run = run(&scan, &list, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
    let [unknown] = run.summary.unknown.as_slice() else {
        panic!("one unknown position, got {:?}", run.summary.unknown);
    };
    assert_eq!(unknown.cause, ScanUnknownCause::IdentityUnread);
    assert_eq!(
        unknown.identity_unknown,
        Some(CandidateUnknownReason::LowConfidence)
    );
}

// The kernel's own path over real frames (S0 §6 row L3: the kernel's `dry_run_scan`, and the
// drags of a scan as a runtime records them). A 64 x 96 list holds one column of cards: card k
// is a noise marker (the anchor template, 8 x 6 px) at x 4 and content y 10 + 12 k, with a
// 4 x 4 patch of its own colour to its right (its signature). Pixel (0, 0) is red on the list
// page, pixel (1, 0) green at the list's start and pixel (2, 0) blue at its end. The readable
// band lies below every card, so no card is actionable and no text is read.
mod kernel_path {
    use super::super::scan::run_scan;
    use super::super::*;
    use actingcommand_device::CaptureBackendName;
    use actingcommand_recognition_pack::{AssetResolver, RecognitionPack, RecognitionPackResult};
    use actingcommand_selection_policy::SelectionFactSnapshot;
    use serde_json::json;

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 96;
    const CARDS: u32 = 6;

    // Deterministic pixel noise: a shifted copy of it correlates with it near zero.
    fn noise(x: u32, y: u32) -> [u8; 3] {
        let mut state = u64::from(y * 64 + x).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491;
        let mut next = || {
            state ^= state >> 33;
            state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
            state ^= state >> 33;
            (state & 0xff) as u8
        };
        [next(), next(), next()]
    }

    fn image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
        let pixel = &pixel;
        Frame::from_pixels(
            width,
            height,
            (0..height)
                .flat_map(|y| (0..width).flat_map(move |x| pixel(x, y)))
                .collect(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
    }

    // The list at content offset `offset`, with its start and end marks.
    fn list(offset: u32, start: bool, end: bool) -> Frame {
        image(WIDTH, HEIGHT, |x, y| {
            match (x, y) {
                (0, 0) => return [255, 0, 0],
                (1, 0) if start => return [0, 255, 0],
                (2, 0) if end => return [0, 0, 255],
                _ => {}
            }
            let content = y + offset;
            for card in 0..CARDS {
                let top = 10 + 12 * card;
                if (top..top + 6).contains(&content) && (4..12).contains(&x) {
                    return noise(x - 4, content - top);
                }
                if (top..top + 4).contains(&content) && (12..16).contains(&x) {
                    let level = u8::try_from(30 + 36 * card).expect("level");
                    return [level, 220 - level / 2, 90];
                }
            }
            [30, 31, 32]
        })
    }

    #[derive(Debug)]
    struct Marker(Vec<u8>);

    impl AssetResolver for Marker {
        fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
            assert_eq!(path, "assets/card.png");
            Ok(self.0.clone())
        }
    }

    // A prepared task whose one select step `pick` runs on the list page over the card layout.
    fn scan_task() -> PreparedContainedTask {
        let pack: RecognitionPack = serde_json::from_value(json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
            "targets": [
                {"type": "color", "id": "page/list",
                 "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
                {"type": "color", "id": "check/list_start",
                 "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
                {"type": "color", "id": "check/list_end",
                 "region": {"x": 2, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
                {"type": "template", "id": "ui/card", "template_path": "assets/card.png",
                 "region": "full_frame", "threshold": 0.95},
                {"type": "ocr", "id": "card/name",
                 "region": {"x": 0, "y": 0, "width": 8, "height": 6},
                 "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
                 "expected": ["a"], "case_sensitive": true, "minimum_confidence": 0.8,
                 "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}
            ],
            "candidate_layouts": [{
                "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
                "anchor": "ui/card", "max_instances": 16, "order": "top_to_bottom",
                "instance_rect": {"x": 0, "y": 0, "width": 16, "height": 8},
                "click": {"x": 2, "y": 1, "width": 4, "height": 4},
                "readable_band": {"x": 0, "y": 88, "width": 64, "height": 8},
                "unknown_identity": "reject",
                "features": [{"name": "member", "value": "identity", "target": "card/name",
                              "offset": {"x": 0, "y": 0},
                              "identity": {
                                  "entries": [{"id": "member-a", "aliases": ["a"]}],
                                  "recognition": {"kind": "ocr_aliases", "max_distance": 1,
                                                  "minimum_margin": 1,
                                                  "minimum_confidence_milli": 800}}}]
            }]
        }))
        .expect("recognition pack");
        let marker = image(8, 6, noise).png_for_artifact().expect("marker PNG");
        let evaluator = RecognitionEvaluator::with_asset_resolver(pack, Arc::new(Marker(marker)))
            .expect("recognition evaluator");
        let control: TaskControl = serde_json::from_value(json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.scan.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": WIDTH, "height": HEIGHT},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 10_000,
            "timeout_ms": 60_000
        }))
        .expect("task control");
        let mut program: TaskProgram = serde_json::from_value(json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "target_page": "list",
            "operations": [{
                "id": "pick", "from": "list", "unguarded_trusted_coordinate": true,
                "select": {"layout_id": "layout/cards",
                           "policy": {"path": "policies/select.json", "sha256": "b".repeat(64)}}
            }]
        }))
        .expect("task program");
        program.operations[0].prepared_select =
            Some(Box::new(selection::PreparedSelect::for_test(
                "layout/cards",
                super::policy(&["member-a".to_owned()]),
            )));
        let page_set: PageSet = serde_json::from_value(json!({
            "schema_version": "0.3",
            "pages": [{"id": "neutral/list", "required": ["page/list"]}]
        }))
        .expect("page set");
        let detector = PageDetector::new(page_set).expect("page detector");
        detector
            .validate(&evaluator)
            .expect("page detector targets");
        PreparedContainedTask {
            control,
            program,
            evaluator,
            detector,
            entry_page: None,
            scheduling_outcome: None,
            post_admission_ocr: None,
            post_admission_fields: None,
            package_sha256: "fixture-sha256".into(),
            entry_count: 1,
            task_count: 1,
        }
    }

    // The scan over the card list; `rewind` lets it reposition toward the start.
    fn card_scan(rewind: bool) -> ScanDeclaration {
        ScanDeclaration::for_test(json!({
            "identity_feature": "member",
            "axis": "vertical",
            "advance": {"kind": "drag",
                        "from": {"x": 20, "y": 60, "width": 20, "height": 10},
                        "to": {"x": 20, "y": 20, "width": 20, "height": 10},
                        "duration_ms": 300, "min_content_step_px": 12,
                        "max_content_step_px": 40},
            "reposition": {"kind": "drag",
                           "from": {"x": 20, "y": 70, "width": 20, "height": 10},
                           "to": {"x": 20, "y": 10, "width": 20, "height": 10},
                           "duration_ms": 300, "max_content_step_px": 60},
            "settle": {"interval_ms": 1, "timeout_ms": 1000},
            "track": {"area": {"x": 0, "y": 4, "width": WIDTH, "height": 92},
                      "edge_margin_px": 2,
                      "signature": {"rect": {"x": 8, "y": 0, "width": 4, "height": 4},
                                    "columns": 1, "rows": 1},
                      "signature_max_distance_milli": 500,
                      "position_tolerance_px": 1,
                      "min_overlap_instances": 2,
                      "spacing": {"max_axis_gap_px": 20, "columns": 1,
                                  "max_trailing_gap_px": 100, "max_cross_lead_px": 10,
                                  "max_cross_gap_px": 10}},
            "start": {"check": "check/list_start", "rewind": rewind},
            "end": {"check": "check/list_end"},
            "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 60_000}
        }))
    }

    fn no_facts() -> SelectionFactSnapshot {
        SelectionFactSnapshot {
            snapshot_id: "empty".into(),
            snapshot_at_unix_ms: 0,
            facts: BTreeMap::new(),
        }
    }

    // S0 §6 row L3: a replay that needs a frame it does not hold ends with a typed failure,
    // never as a pass. The saved sequence starts mid-list; the scan would rewind, and a saved
    // forward run holds no frame of a move toward the start.
    #[test]
    fn l3_a_dry_run_that_needs_a_rewind_ends_with_a_typed_failure() {
        let task = scan_task();
        let error = dry_run_scan(
            &task,
            "pick",
            &card_scan(true),
            vec![list(12, false, false), list(12, false, false)],
            &no_facts(),
            1_000,
        )
        .expect_err("a dry run cannot rewind");
        assert_eq!(error.code().as_str(), "selection_scan_replay_frame_missing");
        assert_eq!(
            error.outcome().value("stage").cloned(),
            Some(json!("select_scan_start"))
        );
    }

    // A runtime that hands out frames in order and keeps every trace and input.
    struct Recording {
        frames: VecDeque<Frame>,
        traces: Vec<ContainedTaskTrace>,
        inputs: Vec<InputAction>,
    }

    impl ContainedTaskRuntime for Recording {
        type Error = ContainedTaskError;

        fn capture(&mut self) -> Result<ObservedFrame, ContainedTaskError> {
            self.frames
                .pop_front()
                .map(ObservedFrame::from)
                .ok_or_else(|| {
                    ContainedTaskError::new(
                        ExecutionKernelCode::SelectionScanReplayFrameMissing.code(),
                    )
                })
        }

        fn input(
            &mut self,
            action: InputAction,
            _frame: Option<InputFrameContext>,
        ) -> Result<(), ContainedTaskError> {
            self.inputs.push(action);
            Ok(())
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), ContainedTaskError> {
            self.traces.push(trace);
            Ok(())
        }
    }

    // Register row L3-1: every drag of a scan is a `task.effect_intent` of the select step,
    // followed by its input, and none is followed by `task.effect_completed`; the step's one
    // completed effect belongs to its selecting tap (SEL-L4b). The list here shows the end
    // mark only after the drag, which the list absorbed; no card is readable, so the scan then
    // fails at `Decide` on the first unread position.
    #[test]
    fn l3_each_scan_drag_is_an_effect_intent_of_the_select_step_and_completes_no_effect() {
        let task = scan_task();
        let mut runtime = Recording {
            frames: VecDeque::from([
                list(0, true, false),
                list(0, true, false),
                list(0, false, true),
                list(0, false, true),
            ]),
            traces: Vec::new(),
            inputs: Vec::new(),
        };
        let error = run_scan(
            &task,
            &mut runtime,
            ("pick", &card_scan(false)),
            (&no_facts(), 1_000),
            false,
        )
        .expect_err("no card is readable");
        let error = match error {
            ContainedTaskRunError::Task(error) => error,
            other => panic!("expected a task failure, got {other:?}"),
        };
        assert_eq!(error.code().as_str(), "selection_scan_position_unread");
        assert_eq!(
            error.outcome().value("position_index").cloned(),
            Some(json!(0))
        );
        let swipe = InputAction::Swipe {
            x1: 30,
            y1: 65,
            x2: 30,
            y2: 25,
            duration_ms: 300,
        };
        assert_eq!(runtime.inputs, [swipe.clone()]);
        let intents = runtime
            .traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::EffectIntent {
                    step_index,
                    operation_label,
                    action,
                    ..
                } => Some((*step_index, operation_label.as_str(), action.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(intents, [(0, "pick", swipe)]);
        assert!(
            !runtime
                .traces
                .iter()
                .any(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. })),
            "a scan drag completes no effect"
        );
    }

    // S0 §2.3: every capture of a scan must be the step's page. A popup covers the list after
    // the first drag; the scan fails `selection_page_changed` and makes no further input.
    #[test]
    fn l3_a_frame_of_another_page_mid_scan_fails_selection_page_changed() {
        let task = scan_task();
        let popup = image(WIDTH, HEIGHT, |_, _| [200, 200, 200]);
        let mut runtime = Recording {
            frames: VecDeque::from([list(0, true, false), list(0, true, false), popup]),
            traces: Vec::new(),
            inputs: Vec::new(),
        };
        let error = run_scan(
            &task,
            &mut runtime,
            ("pick", &card_scan(false)),
            (&no_facts(), 1_000),
            false,
        )
        .expect_err("the popup ends the scan");
        match error {
            ContainedTaskRunError::Task(error) => {
                assert_eq!(error.code().as_str(), "selection_page_changed");
            }
            other => panic!("expected a task failure, got {other:?}"),
        }
        assert_eq!(runtime.inputs.len(), 1);
    }
}
