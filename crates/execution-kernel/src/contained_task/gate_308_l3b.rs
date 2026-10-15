// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 slice SEL-L3b (forced compromises E-10, E-11, E-12; the scan session of the frozen
// selector model S0 v2.1 §2): a list along the horizontal axis, a drag that ends with a pause and
// a brake across the axis, and the page flip, aligned by its overlap or by page-order evidence.
// A fake source replays synthetic viewport sequences of a neutral strip of cards: card columns
// 100 px apart along x, three cards across each column (y 20, 140, 260), cards of 80 x 100 px, a
// readable band from x 60 to 540 and a tracking area from x 40 to 560 in a 600 x 400 frame (the
// grid of SEL-L3's gate, turned onto the horizontal axis). The kernel's own path then runs over
// real frames: the drag's input and its effect intent, and page checks judged on captured
// frames. Every value a test expects comes from S0 §2 and the SEL-L3b row of the forced
// compromises mapping. Gate exec 64.

use super::scan::{
    ScanBrake, ScanDeclaration, ScanDrag, ScanDragMotion, ScanDryRun, ScanEnd, ScanInput,
    ScanReadInstance, ScanReading, ScanSession, ScanSource,
};
use super::{ClickRect, ContainedTaskError, ContainedTaskRunError};
use actingcommand_contract::{
    CandidateFeature, CandidateFeatureMap, CandidateIdentitySource, CandidateRect,
    ScanCoverageState, ScanStartEvidence, ScanViewportMotion,
};
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition_pack::CandidateLayout;
use actingcommand_selection_policy::{SelectionFactSnapshot, SelectionPolicy};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

const FRAME: (u32, u32) = (600, 400);
const ACROSS: [i32; 3] = [20, 140, 260];

// One card of the synthetic strip, in content coordinates: its left edge on the axis, its top
// edge across it, its identity and its signature colour.
#[derive(Debug, Clone)]
struct Card {
    axis: i32,
    cross: i32,
    identity: String,
    color: [u8; 3],
}

// `columns` columns of three distinct cards, the first column at content x 60.
fn strip(columns: usize) -> Vec<Card> {
    (0..columns * 3)
        .map(|index| Card {
            axis: 60 + 100 * i32::try_from(index / 3).expect("column"),
            cross: ACROSS[index % 3],
            identity: format!("member-{index:02}"),
            color: [
                u8::try_from(index % 32).expect("level"),
                u8::try_from((index / 32) % 32).expect("level"),
                9,
            ],
        })
        .collect()
}

fn domain(cards: &[Card]) -> Vec<String> {
    cards
        .iter()
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
                value: true,
                confidence: None,
            },
        ),
    ])
}

// One captured frame of the strip, with the page checks that pass on it.
#[derive(Debug, Clone)]
struct View {
    instances: Vec<ScanReadInstance>,
    start: bool,
    end: bool,
    pages: Vec<u32>,
}

// The strip at content offset `offset` along x: every card that lies in the frame, actionable
// inside the readable band, with its features only then.
fn view(cards: &[Card], offset: i32, start: bool, end: bool) -> View {
    let instances = cards
        .iter()
        .filter_map(|card| {
            let x = card.axis - offset;
            (x >= 0 && x + 80 <= 600).then(|| {
                let actionable = x >= 60 && x + 80 <= 540;
                ScanReadInstance {
                    rect: CandidateRect {
                        x,
                        y: card.cross,
                        width: 80,
                        height: 100,
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
        pages: Vec::new(),
    }
}

// `frame` with the page checks `pages` passing on it.
fn on_pages(mut frame: View, pages: &[u32]) -> View {
    frame.pages = pages.to_vec();
    frame
}

// What the fake hands out: the dispatched frame, then its captures in order.
struct Script {
    dispatched: Vec<CandidateRect>,
    captures: VecDeque<View>,
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
            captures: VecDeque::from([first]),
        }
    }

    // After a move: the moved frame, captured twice, so it settles at the second capture.
    fn then(mut self, moved: View) -> Self {
        self.captures.push_back(moved.clone());
        self.captures.push_back(moved);
        self
    }
}

// A scan source that replays a script and keeps every move it was asked for.
struct Fake {
    script: Script,
    current: Option<View>,
    clock: Duration,
    drags: Vec<ScanDrag>,
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
        let view = self
            .script
            .captures
            .pop_front()
            .ok_or_else(|| ContainedTaskRunError::Boundary("no further capture".to_owned()))?;
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

    fn drag(&mut self, drag: ScanDrag) -> Result<(), ContainedTaskRunError<String>> {
        self.drags.push(drag);
        Ok(())
    }

    fn page_checks(
        &mut self,
        checks: &[String],
    ) -> Result<Vec<u32>, ContainedTaskRunError<String>> {
        let view = self
            .current
            .as_ref()
            .ok_or_else(|| ContainedTaskRunError::Boundary("no capture yet".to_owned()))?;
        assert_eq!(checks, ["check/page_0", "check/page_1", "check/page_2"]);
        Ok(view.pages.clone())
    }
}

const ADVANCE_FROM: ClickRect = ClickRect {
    x: 450,
    y: 150,
    width: 20,
    height: 100,
};
const ADVANCE_TO: ClickRect = ClickRect {
    x: 150,
    y: 150,
    width: 20,
    height: 100,
};
const NEXT: ClickRect = ClickRect {
    x: 560,
    y: 180,
    width: 30,
    height: 40,
};
const PREVIOUS: ClickRect = ClickRect {
    x: 10,
    y: 180,
    width: 30,
    height: 40,
};
const FIRST_PAGE: ClickRect = ClickRect {
    x: 10,
    y: 330,
    width: 30,
    height: 40,
};

fn rect_json(rect: ClickRect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

// The declaration every test starts from: the strip along x, advanced by a drag of 300 px of
// finger travel; `patch` changes it.
fn declaration(patch: impl FnOnce(&mut Value)) -> ScanDeclaration {
    let mut value = json!({
        "identity_feature": "member",
        "axis": "horizontal",
        "advance": {"kind": "drag", "from": rect_json(ADVANCE_FROM), "to": rect_json(ADVANCE_TO),
                    "duration_ms": 500, "min_content_step_px": 180, "max_content_step_px": 400},
        "reposition": {"kind": "drag",
                       "from": {"x": 500, "y": 150, "width": 20, "height": 100},
                       "to": {"x": 80, "y": 150, "width": 20, "height": 100},
                       "duration_ms": 400, "max_content_step_px": 480},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 40, "y": 0, "width": 520, "height": 400},
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

// The page-flip advance by its overlap: a tap on `NEXT` moves the strip by 180 to 400 px.
fn paged_by_overlap(value: &mut Value) {
    value["advance"] = json!({"kind": "rect", "rect": rect_json(NEXT),
                              "back_rect": rect_json(PREVIOUS),
                              "min_content_step_px": 180, "max_content_step_px": 400});
}

// The page-flip advance by page-order evidence: pages of five columns that do not overlap.
fn paged_by_order(value: &mut Value) {
    value["advance"] = json!({"kind": "rect", "rect": rect_json(NEXT),
                              "back_rect": rect_json(PREVIOUS),
                              "page_order": {"checks": ["check/page_0", "check/page_1",
                                                        "check/page_2"],
                                             "page_step_px": 500}});
}

fn layout(identities: &[String]) -> CandidateLayout {
    serde_json::from_value(json!({
        "id": "layout/cards", "page_id": "neutral/list", "kind": "repeated_anchor",
        "anchor": "ui/card", "max_instances": 64, "order": "left_to_right",
        "instance_rect": {"x": 0, "y": 0, "width": 80, "height": 100},
        "click": {"x": 10, "y": 10, "width": 40, "height": 80},
        "readable_band": {"x": 60, "y": 0, "width": 480, "height": 400},
        "unknown_identity": "reject",
        "features": [
            {"name": "member", "value": "identity", "target": "card/name",
             "offset": {"x": 0, "y": 70},
             "identity": {
                 "entries": identities.iter().map(|id| json!({"id": id, "aliases": [id]}))
                     .collect::<Vec<_>>(),
                 "recognition": {"kind": "ocr_aliases", "max_distance": 1, "minimum_margin": 1,
                                 "minimum_confidence_milli": 800}}},
            {"name": "open", "value": "passed", "target": "card/open",
             "offset": {"x": 60, "y": 0}}
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
    drags: Vec<ScanDrag>,
}

fn run(scan: &ScanDeclaration, cards: &[Card], script: Script) -> Run {
    let identities = domain(cards);
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

fn motions(run: &Run) -> Vec<ScanDragMotion> {
    run.drags.iter().map(|drag| drag.motion).collect()
}

fn inputs(run: &Run) -> Vec<ScanInput> {
    run.drags.iter().map(|drag| drag.input).collect()
}

// FCM E-11: the scan reads a list along the horizontal axis. Twelve columns read from the start
// in steps of 300 px, the last step short at the strip's end; every rule of S0 §2 holds along x.
#[test]
fn l3b_a_horizontal_strip_is_covered_from_its_start_to_its_end() {
    let cards = strip(12);
    let script = Script::starting_with(view(&cards, 0, true, false))
        .then(view(&cards, 300, false, false))
        .then(view(&cards, 600, false, false))
        .then(view(&cards, 760, false, true));
    let run = run(&declaration(|_| {}), &cards, script);
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
    assert_eq!(
        inputs(&run),
        [ScanInput::Drag {
            from: ADVANCE_FROM,
            to: ADVANCE_TO,
            duration_ms: 500,
        }; 3]
    );
    assert_eq!(run.summary.positions, 36);
    assert_eq!(run.summary.members.len(), 36);
    // A member's content position lies along x, its cross position along y.
    let last = run
        .summary
        .members
        .iter()
        .find(|member| member.identity == "member-35")
        .expect("the last card is a member");
    assert_eq!((last.content_axis_px, last.cross_px), (1160, 260));
    assert!(run.summary.unknown.is_empty());
}

// FCM E-11: a drag may end with a pause and a brake across the axis, so the strip keeps no
// fling. Every advance is that drag; a gap's corrective is the same drag reversed, with the same
// pause and brake. On the horizontal axis the brake moves the touch along y.
#[test]
fn l3b_a_drag_with_a_pause_and_a_brake_brakes_across_the_axis_and_its_corrective_reverses_it() {
    let cards = strip(16);
    let scan = declaration(|value| {
        value["advance"]["pause_ms"] = json!(150);
        value["advance"]["brake"] = json!({"cross_px": -100, "duration_ms": 200});
    });
    // A step of 403 px leaves the bands 3 px short of overlapping by the card extent; each such
    // gap takes one corrective reverse move (S0 §2.5).
    let script = Script::starting_with(view(&cards, 0, true, false))
        .then(view(&cards, 403, false, false))
        .then(view(&cards, 203, false, false))
        .then(view(&cards, 606, false, false))
        .then(view(&cards, 1009, false, false))
        .then(view(&cards, 809, false, false))
        .then(view(&cards, 1100, false, true));
    let run = run(&scan, &cards, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 403, 203, 606, 1009, 809, 1100]);
    assert_eq!(
        motions(&run),
        [
            ScanDragMotion::Advance,
            ScanDragMotion::Corrective,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance,
            ScanDragMotion::Corrective,
            ScanDragMotion::Advance
        ]
    );
    let brake = Some(ScanBrake {
        displacement: (0, -100),
        duration_ms: 200,
    });
    let advance = ScanInput::HeldDrag {
        from: ADVANCE_FROM,
        to: ADVANCE_TO,
        duration_ms: 500,
        pause_ms: 150,
        brake,
    };
    let corrective = ScanInput::HeldDrag {
        from: ADVANCE_TO,
        to: ADVANCE_FROM,
        duration_ms: 500,
        pause_ms: 150,
        brake,
    };
    assert_eq!(
        inputs(&run),
        [advance, corrective, advance, advance, corrective, advance]
    );
}

// FCM E-11: a page flip is a tap. Pages that overlap align by their overlap within the declared
// steps, as drags do (S0 §2.4); a scan that starts mid-list rewinds by the reposition's reverse
// tap until the start check passes.
#[test]
fn l3b_a_page_flip_aligns_by_its_overlap_and_rewinds_by_its_reverse_tap() {
    let cards = strip(12);
    let scan = declaration(|value| {
        paged_by_overlap(value);
        value["reposition"] = json!({"kind": "rect",
                                     "rect": {"x": 560, "y": 330, "width": 30, "height": 40},
                                     "back_rect": rect_json(FIRST_PAGE),
                                     "max_content_step_px": 480});
    });
    let script = Script::starting_with(view(&cards, 600, false, false))
        .then(view(&cards, 120, false, false))
        .then(view(&cards, 0, true, false))
        .then(view(&cards, 300, false, false))
        .then(view(&cards, 600, false, false))
        .then(view(&cards, 760, false, true));
    let run = run(&scan, &cards, script);
    assert_covered(&run);
    assert_eq!(run.summary.start, Some(ScanStartEvidence::RewindCheck));
    assert_eq!(run.summary.rewind_viewports, 2);
    assert_eq!(offsets(&run), [0, 300, 600, 760]);
    assert_eq!(
        motions(&run),
        [
            ScanDragMotion::Rewind,
            ScanDragMotion::Rewind,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance
        ]
    );
    let back = ScanInput::Tap { rect: FIRST_PAGE };
    let next = ScanInput::Tap { rect: NEXT };
    assert_eq!(inputs(&run), [back, back, next, next, next]);
    assert_eq!(run.summary.members.len(), 36);
}

// S0 §2.5 under a page flip: a gap between overlapping pages is closed by one corrective, the
// tap on `back_rect`, aligned in the corrective window.
#[test]
fn l3b_a_page_flip_gap_is_closed_by_one_tap_on_its_back_rect() {
    let cards = strip(12);
    let scan = declaration(paged_by_overlap);
    let script = Script::starting_with(view(&cards, 0, true, false))
        .then(view(&cards, 403, false, false))
        .then(view(&cards, 203, false, false))
        .then(view(&cards, 606, false, false))
        .then(view(&cards, 760, false, true));
    let run = run(&scan, &cards, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 403, 203, 606, 760]);
    let next = ScanInput::Tap { rect: NEXT };
    let back = ScanInput::Tap { rect: PREVIOUS };
    assert_eq!(inputs(&run), [next, back, next, next]);
    assert_eq!(
        motions(&run),
        [
            ScanDragMotion::Advance,
            ScanDragMotion::Corrective,
            ScanDragMotion::Advance,
            ScanDragMotion::Advance
        ]
    );
    assert_eq!(run.summary.positions, 36);
}

// FCM E-11: pages that do not overlap align by page-order evidence. Each viewport passes exactly
// one page check; page k lies at k times the page step, so the pages tile the strip and no
// corrective is made. Page 2 ends the strip.
#[test]
fn l3b_pages_without_overlap_are_placed_by_page_order_evidence() {
    let cards = strip(15);
    let scan = declaration(paged_by_order);
    let script = Script::starting_with(on_pages(view(&cards, 0, true, false), &[0]))
        .then(on_pages(view(&cards, 500, false, false), &[1]))
        .then(on_pages(view(&cards, 1000, false, true), &[2]));
    let run = run(&scan, &cards, script);
    assert_covered(&run);
    assert_eq!(offsets(&run), [0, 500, 1000]);
    let next = ScanInput::Tap { rect: NEXT };
    assert_eq!(inputs(&run), [next, next]);
    assert_eq!(motions(&run), [ScanDragMotion::Advance; 2]);
    assert_eq!(run.summary.positions, 45);
    assert_eq!(run.summary.members.len(), 45);
}

// A page flip that skips a page fails: the page after page 0 is page 0 (an absorbed tap) or
// page 1, never page 2 (S0 §2.4 step 5, `unexpected_displacement`; one page check passed of the
// one required).
#[test]
fn l3b_a_skipped_page_fails_unexpected_displacement() {
    let cards = strip(15);
    let scan = declaration(paged_by_order);
    let script = Script::starting_with(on_pages(view(&cards, 0, true, false), &[0]))
        .then(on_pages(view(&cards, 1000, false, true), &[2]));
    let run = run(&scan, &cards, script);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_alignment_rejected");
    assert_eq!(value(error, "stage"), "select_scan_align");
    assert_eq!(value(error, "viewport_index"), 1);
    assert_eq!(
        value(error, "scan_alignment_failure"),
        "unexpected_displacement"
    );
    assert_eq!(value(error, "count"), 1);
    assert_eq!(value(error, "limit_count"), 1);
}

// A viewport that passes no page check, or two, has no single page: the scan fails, never
// guessing (S0 §2.4 step 5: `no_consistent_shift`, `ambiguous_shift`).
#[test]
fn l3b_a_viewport_with_no_page_check_or_with_two_fails() {
    let cards = strip(15);
    let scan = declaration(paged_by_order);
    // The page after page 0 shows the page checks `pages`.
    let after_page_0 = |pages: &[u32]| {
        let script = Script::starting_with(on_pages(view(&cards, 0, true, false), &[0]))
            .then(on_pages(view(&cards, 500, false, false), pages));
        run(&scan, &cards, script)
    };

    let none = after_page_0(&[]);
    let error = failure(&none);
    assert_eq!(error.code().as_str(), "selection_scan_alignment_rejected");
    assert_eq!(value(error, "viewport_index"), 1);
    assert_eq!(
        value(error, "scan_alignment_failure"),
        "no_consistent_shift"
    );
    assert_eq!(value(error, "count"), 0);
    assert_eq!(value(error, "limit_count"), 1);

    let two = after_page_0(&[1, 2]);
    let error = failure(&two);
    assert_eq!(error.code().as_str(), "selection_scan_alignment_rejected");
    assert_eq!(value(error, "viewport_index"), 1);
    assert_eq!(value(error, "scan_alignment_failure"), "ambiguous_shift");
    assert_eq!(value(error, "count"), 2);
    assert_eq!(value(error, "limit_count"), 1);
}

// Two absorbed page flips in a row show page 0 three times: the scan is stuck, not at its end
// (S0 §2.2 `EndTest`, `selection_scan_end_unconfirmed`).
#[test]
fn l3b_two_absorbed_page_flips_fail_end_unconfirmed() {
    let cards = strip(15);
    let scan = declaration(paged_by_order);
    let script = Script::starting_with(on_pages(view(&cards, 0, true, false), &[0]))
        .then(on_pages(view(&cards, 0, false, false), &[0]))
        .then(on_pages(view(&cards, 0, false, false), &[0]));
    let run = run(&scan, &cards, script);
    let error = failure(&run);
    assert_eq!(error.code().as_str(), "selection_scan_end_unconfirmed");
    assert_eq!(value(error, "stage"), "select_scan_end");
    assert_eq!(value(error, "viewport_index"), 2);
    assert_eq!(motions(&run), [ScanDragMotion::Advance; 2]);
}

// The kernel's own path over real frames (S0 §6: one test per new emission path). A 64 x 96 list
// holds one column of cards along y: card k is a noise marker (the anchor template, 8 x 6 px)
// at x 4 and content y 10 + 12 k, with a 4 x 4 patch of its own colour to its right (its
// signature). Pixel (0, 0) is red on the list page, pixel (1, 0) green at the list's start,
// pixel (2, 0) blue at its end, and pixel (3 + k, 0) yellow on page k. The readable band lies
// below every card, so no card is actionable and no text is read.
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

    // The list at content offset `offset`, with its start and end marks and its page mark.
    fn list(offset: u32, start: bool, end: bool, page: Option<u32>) -> Frame {
        image(WIDTH, HEIGHT, |x, y| {
            match (x, y) {
                (0, 0) => return [255, 0, 0],
                (1, 0) if start => return [0, 255, 0],
                (2, 0) if end => return [0, 0, 255],
                (_, 0) if page.is_some_and(|page| x == 3 + page) => return [255, 255, 0],
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

    fn page_check(page: u32) -> serde_json::Value {
        json!({"type": "color", "id": format!("check/page_{page}"),
               "region": {"x": 3 + page, "y": 0, "width": 1, "height": 1},
               "expected": [255, 255, 0]})
    }

    // A prepared task whose one select step `pick` runs on the list page over the card layout.
    fn scan_task() -> PreparedContainedTask {
        let mut targets = vec![
            json!({"type": "color", "id": "page/list",
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]}),
            json!({"type": "color", "id": "check/list_start",
                   "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]}),
            json!({"type": "color", "id": "check/list_end",
                   "region": {"x": 2, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]}),
            json!({"type": "template", "id": "ui/card", "template_path": "assets/card.png",
                   "region": "full_frame", "threshold": 0.95}),
            json!({"type": "ocr", "id": "card/name",
                   "region": {"x": 0, "y": 0, "width": 8, "height": 6},
                   "languages": ["en"], "timeout_ms": 2000, "match_mode": "exact",
                   "expected": ["a"], "case_sensitive": true, "minimum_confidence": 0.8,
                   "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)}),
        ];
        targets.extend((0..3).map(page_check));
        let pack: RecognitionPack = serde_json::from_value(json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
            "targets": targets,
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
            entry_count: 1,
            task_count: 1,
        }
    }

    // The scan over the card list with the advance `advance`.
    fn card_scan(advance: serde_json::Value) -> ScanDeclaration {
        ScanDeclaration::for_test(json!({
            "identity_feature": "member",
            "axis": "vertical",
            "advance": advance,
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
            "start": {"check": "check/list_start", "rewind": false},
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

    fn task_failure(error: ContainedTaskRunError<ContainedTaskError>) -> ContainedTaskError {
        match error {
            ContainedTaskRunError::Task(error) => error,
            other => panic!("expected a task failure, got {other:?}"),
        }
    }

    fn effect_intents(runtime: &Recording) -> Vec<(u32, &str, InputAction)> {
        runtime
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
            .collect()
    }

    // FCM E-11, register §1.58: a drag with a pause and a brake is one input, the primitive
    // `single_touch_drag_with_pause_and_brake_v1`, and one `task.effect_intent` of the select
    // step with the same action; it completes no effect (register L3-1). On the vertical axis the
    // brake moves the touch along x: from the drag's end (30, 25) by the declared 20 px. The list
    // absorbs the drag and shows its end; no card is readable, so the scan then fails at `Decide`
    // on the first unread position.
    #[test]
    fn l3b_a_drag_with_a_pause_and_a_brake_is_one_effect_intent_and_one_input() {
        let task = scan_task();
        let scan = card_scan(json!({
            "kind": "drag",
            "from": {"x": 20, "y": 60, "width": 20, "height": 10},
            "to": {"x": 20, "y": 20, "width": 20, "height": 10},
            "duration_ms": 300, "pause_ms": 150,
            "brake": {"cross_px": 20, "duration_ms": 200},
            "min_content_step_px": 12, "max_content_step_px": 40
        }));
        let mut runtime = Recording {
            frames: VecDeque::from([
                list(0, true, false, None),
                list(0, true, false, None),
                list(0, false, true, None),
                list(0, false, true, None),
            ]),
            traces: Vec::new(),
            inputs: Vec::new(),
        };
        let error = task_failure(
            run_scan(
                &task,
                &mut runtime,
                ("pick", &scan),
                (&no_facts(), 1_000),
                false,
            )
            .expect_err("no card is readable"),
        );
        assert_eq!(error.code().as_str(), "selection_scan_position_unread");
        let drag = InputAction::SingleTouchDragWithPauseAndBrakeV1 {
            x1: 30,
            y1: 65,
            x2: 30,
            y2: 25,
            drag_duration_ms: 300,
            pause_ms: 150,
            x3: 50,
            y3: 25,
            brake_duration_ms: 200,
        };
        assert_eq!(runtime.inputs, std::slice::from_ref(&drag));
        assert_eq!(effect_intents(&runtime), [(0, "pick", drag)]);
        assert!(
            !runtime
                .traces
                .iter()
                .any(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. })),
            "a scan move completes no effect"
        );
    }

    // FCM E-11: a page flip is one tap, an effect intent of the select step, and its page is the
    // declared page check that passes on the frame captured after it. The tap shows page 2 after
    // page 0, so the scan fails `unexpected_displacement` with no further input.
    #[test]
    fn l3b_a_page_flip_is_one_tap_and_its_page_is_read_from_the_captured_frame() {
        let task = scan_task();
        let scan = card_scan(json!({
            "kind": "rect",
            "rect": {"x": 50, "y": 40, "width": 10, "height": 10},
            "back_rect": {"x": 2, "y": 40, "width": 10, "height": 10},
            "page_order": {"checks": ["check/page_0", "check/page_1", "check/page_2"],
                           "page_step_px": 60}
        }));
        let mut runtime = Recording {
            frames: VecDeque::from([
                list(0, true, false, Some(0)),
                list(0, true, false, Some(0)),
                list(60, false, false, Some(2)),
                list(60, false, false, Some(2)),
            ]),
            traces: Vec::new(),
            inputs: Vec::new(),
        };
        let error = task_failure(
            run_scan(
                &task,
                &mut runtime,
                ("pick", &scan),
                (&no_facts(), 1_000),
                false,
            )
            .expect_err("a skipped page"),
        );
        assert_eq!(error.code().as_str(), "selection_scan_alignment_rejected");
        assert_eq!(
            error.outcome().value("scan_alignment_failure").cloned(),
            Some(json!("unexpected_displacement"))
        );
        assert_eq!(
            error.outcome().value("viewport_index").cloned(),
            Some(json!(1))
        );
        let tap = InputAction::Tap { x: 55, y: 45 };
        assert_eq!(runtime.inputs, std::slice::from_ref(&tap));
        assert_eq!(effect_intents(&runtime), [(0, "pick", tap)]);
    }
}
