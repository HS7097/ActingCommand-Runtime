// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #404 FS-3 (0.12.43; register §1.284; ledger revision 7; design
// `rd-1031/perf-407/model-gpu0-404.md` §2.3, §2.4 and §3 row FS-3). A single-frame select over a
// `repeated_anchor` layout that declares `confirm: "track"` with `track_px` decides its
// confirmation frame F2 again with the same fact snapshot and instant, and finds its choice
// again there: the choice c2 of F2's decision stands for the choice c1 of the step's frame F1
// when the Chebyshev distance between their anchor match origins is at most `track_px`. Then the
// record's confirmation is `tracked {candidate_set_sha256, decision_sha256, shift_px}` and the one
// tap lands inside c2's click box, bound to F2. Otherwise it is `track_lost` and the step fails
// `selection_decision_mismatch`, with `confirm` `track` and `shift_px`, or `selection_kind` when
// F2's decision chose nobody, before any input. Admission takes `confirm: "track"` and `track_px`
// (1..=64) only together, on such a layout and without `scan`.
//
// Every package is a synthetic `neutral` package of 96 x 72 frames, admitted through
// `PreparedContainedTask::load`: the page on the first pixel (red `home`, blue `terminal`), the
// step's guard on the second (green), and at most one 8 x 6 noise marker, the layout's anchor,
// wherever a case puts it. The policy chooses exactly one candidate by its `anchor_milli` match
// score, so each frame's decision chooses the marker that frame shows, and nobody on a frame
// without one. The runtime's clock is the scripted runtime's (EXEC-I3), which only its pauses
// advance.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    CandidateProjection, CandidateRect, InstanceFactContext, InstanceFactSnapshot,
    TaskSelectionRecord,
};
use actingcommand_device::CaptureBackendName;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{Cursor, Write};
use std::time::{Duration, Instant};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 96;
const HEIGHT: u32 = 72;
const TRACK_PX: u32 = 8;
const LAYOUT: &str = "layout/markers";
const CHOICE: &str = "layout/markers#00";

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GREEN: [u8; 3] = [0, 255, 0];
const BACKGROUND: [u8; 3] = [30, 31, 32];

// One screen: its page and the top-left corner of its marker, when it shows one.
#[derive(Debug, Clone, Copy)]
struct Screen {
    page: [u8; 3],
    marker: Option<(u32, u32)>,
}

const fn home(marker: Option<(u32, u32)>) -> Screen {
    Screen { page: RED, marker }
}

const TERMINAL: Screen = Screen {
    page: BLUE,
    marker: None,
};

// F1 of every run: the marker at (40, 30), its click box at (42, 31), 4 x 4.
const FIRST: Screen = home(Some((40, 30)));

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

fn pixels(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Frame {
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

fn frame(screen: Screen) -> Frame {
    pixels(WIDTH, HEIGHT, |x, y| match (x, y) {
        (0, 0) => screen.page,
        (1, 0) => GREEN,
        _ => match screen.marker {
            Some((left, top)) if (left..left + 8).contains(&x) && (top..top + 6).contains(&y) => {
                noise(x - left, y - top)
            }
            _ => BACKGROUND,
        },
    })
}

#[derive(Debug)]
enum Event {
    Selection(Box<TaskSelectionRecord>),
    Intent,
    Input(InputAction),
}

// The Runtime as the kernel sees it: it shows F1 until the select step reads its fact snapshot,
// then F2 until the first input, then `terminal`. Its clock is a scripted runtime's, which plays
// no frame of this run.
struct Device {
    first: Screen,
    confirmation: Screen,
    state_read: bool,
    inputs: usize,
    clock: ScriptedRuntime,
    events: Vec<Event>,
}

impl Device {
    fn new(first: Screen, confirmation: Screen) -> Self {
        Self {
            first,
            confirmation,
            state_read: false,
            inputs: 0,
            clock: ScriptedRuntime::new(vec![vec![ScriptedFrame::rgb(1, 1, &[BACKGROUND])]]),
            events: Vec::new(),
        }
    }

    // The one decision record of the run.
    fn decision_record(&self) -> &TaskSelectionRecord {
        let records = self
            .events
            .iter()
            .filter_map(|event| match event {
                Event::Selection(record) => Some(record.as_ref()),
                _ => None,
            })
            .collect::<Vec<_>>();
        match records.as_slice() {
            [record] => record,
            _ => panic!("one decision record: {:?}", self.events),
        }
    }

    fn taps(&self) -> Vec<&InputAction> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::Input(action) => Some(action),
                _ => None,
            })
            .collect()
    }

    fn intents(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, Event::Intent))
            .count()
    }
}

impl ContainedTaskRuntime for Device {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let screen = if self.inputs > 0 {
            TERMINAL
        } else if self.state_read {
            self.confirmation
        } else {
            self.first
        };
        Ok(frame(screen).into())
    }

    fn selection_state(
        &mut self,
        request: SelectionStateRequest,
    ) -> Result<SelectionState, Self::Error> {
        self.state_read = true;
        Ok(SelectionState::Snapshot {
            snapshot: InstanceFactSnapshot {
                snapshot_id: "snapshot:test:empty".to_owned(),
                ledger_position: 1,
                context: InstanceFactContext {
                    instance_id: "neutral.instance".to_owned(),
                    server_id: request.server,
                    game_id: request.game,
                },
                records: Vec::new(),
            },
            now_unix_ms: 1_000_000,
        })
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs += 1;
        self.events.push(Event::Input(action));
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        let event = match trace {
            ContainedTaskTrace::SelectionEvaluated { selection, .. } => Event::Selection(selection),
            ContainedTaskTrace::EffectIntent { .. } => Event::Intent,
            _ => return Ok(()),
        };
        self.events.push(event);
        Ok(())
    }

    fn monotonic_now(&mut self) -> Instant {
        self.clock.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.clock.pause(duration);
    }
}

// The markers of the home page: each match of the 8 x 6 marker is an instance of its size, with
// a 4 x 4 click box inside it, and the match's score in milli as its feature `match`.
fn marker_layout() -> Value {
    json!({
        "id": LAYOUT, "page_id": "neutral/home", "kind": "repeated_anchor",
        "anchor": "ui/marker", "max_instances": 4, "order": "top_to_bottom",
        "instance_rect": {"x": 0, "y": 0, "width": 8, "height": 6},
        "click": {"x": 2, "y": 1, "width": 4, "height": 4},
        "features": [{"name": "match", "value": "anchor_milli"}]
    })
}

// One fixed slot of the home page with an integer feature `match`.
fn slot_layout() -> Value {
    json!({
        "id": "layout/slots", "page_id": "neutral/home", "kind": "fixed_slots",
        "features": [{"name": "match", "value": "measure_milli"}],
        "slots": [{"rect": {"x": 40, "y": 30, "width": 8, "height": 6},
                   "click": {"x": 42, "y": 31, "width": 4, "height": 4},
                   "targets": {"match": "slot/mark"}}]
    })
}

// The select step over the markers, tracking its choice within `TRACK_PX`.
fn tracking() -> Value {
    json!({"layout_id": LAYOUT, "policy": {"path": "policies/select.json"},
           "confirm": "track", "track_px": TRACK_PX})
}

// The scan of the SEL-L4b admission gate, over the markers: a select over a scan.
fn scan_declaration() -> Value {
    json!({
        "identity_feature": "match",
        "axis": "vertical",
        "advance": {"kind": "drag",
                    "from": {"x": 40, "y": 60, "width": 10, "height": 4},
                    "to": {"x": 40, "y": 20, "width": 10, "height": 4},
                    "duration_ms": 500, "min_content_step_px": 20, "max_content_step_px": 40},
        "reposition": {"kind": "drag",
                       "from": {"x": 40, "y": 64, "width": 10, "height": 4},
                       "to": {"x": 40, "y": 8, "width": 10, "height": 4},
                       "duration_ms": 400, "max_content_step_px": 56},
        "settle": {"interval_ms": 200, "timeout_ms": 1000},
        "track": {"area": {"x": 0, "y": 4, "width": 96, "height": 64},
                  "edge_margin_px": 2,
                  "signature": {"rect": {"x": 0, "y": 0, "width": 8, "height": 6},
                                "columns": 1, "rows": 1},
                  "signature_max_distance_milli": 500,
                  "position_tolerance_px": 2,
                  "min_overlap_instances": 1,
                  "spacing": {"max_axis_gap_px": 30, "columns": 1, "max_trailing_gap_px": 40,
                              "max_cross_lead_px": 10, "max_cross_gap_px": 40}},
        "start": {"check": "guard/ready", "rewind": true},
        "end": {"check": "guard/ready"},
        "bounds": {"max_viewports": 8, "max_reposition_viewports": 4, "max_ms": 20_000}
    })
}

// One select step from `home` to `terminal` over `layout`, its `select` being `select`.
fn task(layout: Value, select: Value) -> Value {
    json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "target_page": "terminal",
        "candidate_layouts": [layout],
        "operations": [{
            "id": "pick", "from": "home", "to": "terminal",
            "expect_after": {"page_id": "terminal", "timeout_ms": 5000, "interval_ms": 100},
            "guard": {"page_id": "home", "target_id": "guard/ready",
                      "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                      "color_probe": "guard/ready"},
            "select": select
        }]
    })
}

// The policy over `layout_id`: exactly one actionable candidate, ranked as the design's §2.2
// writes it for interchangeable markers: the match score `match` descending (an `anchor_milli`
// feature on the markers; Workflow #404 FS-2b admits a document that reads it), then the
// candidate ID. The anchor's own threshold decides which positions are markers at all.
fn policy(layout_id: &str) -> Value {
    json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "markers",
        "applies_to": {
            "candidate_layout_id": layout_id,
            "outcome_keys": {"selected": "marker_selected", "empty": "marker_empty",
                             "insufficient": "marker_insufficient",
                             "ambiguous": "marker_ambiguous", "unknown": "marker_unknown"}
        },
        "fields": [{"name": "match", "value_type": {"type": "integer"}}],
        "facts": [],
        "gates": [],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [
            {"kind": "value", "value": {"source": "field", "field": "match"},
             "direction": "highest_first"},
            {"kind": "candidate_id", "direction": "lowest_first"}
        ]
    })
}

fn color(id: &str, x: u32, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id,
           "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

// The package of `task` with `policy` as its select step's document; the recognition pack
// declares the task's layout, the pages, the guard, the slot's mark and the marker template.
fn package(task: &Value, policy: &Value) -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.markers.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 5_000,
        "timeout_ms": 30_000
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": HEIGHT},
        "defaults": {"match_metric": "ccoeff_normed", "color_max_distance": 0.0},
        "targets": [
            color("page/home", 0, RED),
            color("page/terminal", 0, BLUE),
            color("guard/ready", 1, GREEN),
            color("slot/mark", 2, GREEN),
            {"type": "template", "id": "ui/marker", "template_path": "assets/marker.png",
             "region": "full_frame", "threshold": 0.95}
        ],
        "candidate_layouts": task["candidate_layouts"].clone()
    });
    let pages = ["home", "terminal"].map(|page| {
        json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
               "optional": [], "forbidden": []})
    });
    let pages = json!({"schema_version": "0.3", "pages": pages});
    let marker = pixels(8, 6, noise).png_for_artifact().expect("marker PNG");
    let entries: [(&str, Vec<u8>); 7] = [
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        (
            "resources/operations/task/policies/select.json",
            bytes(policy),
        ),
        ("resources/assets/marker.png", marker),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

fn admit(task: &Value, policy: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task, policy))
}

// The tracking step over the markers, admitted.
fn tracking_task() -> PreparedContainedTask {
    admit(&task(marker_layout(), tracking()), &policy(LAYOUT))
        .unwrap_or_else(|error| panic!("the tracking step is refused: {error}"))
}

fn failed(
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("expected a task failure, the run ended {outcome:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// The layout's projection on `screen`, as the admitted task's evaluator takes it.
fn projection_on(prepared: &PreparedContainedTask, screen: Screen) -> CandidateProjection {
    let scene = scene_from_frame(&frame(screen)).expect("fixture scene");
    prepared
        .evaluator
        .scene_context(&scene)
        .project_candidates(LAYOUT)
        .expect("fixture projection")
}

fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// The record's confirmation as §1.284 registers it: `kind` and the members `expected` (the
// optional `projection` aside, F2's projection when it carries recognition evidence), both hashes
// 64 lowercase hex digits, the candidate-set hash F2's own; the top-level confirmation of a v1
// record without `multi_select` that keeps F1's decision, validates for append and reads back to
// itself. Returns the serialized confirmation.
fn track_confirmation(
    record: &TaskSelectionRecord,
    kind: &str,
    expected: &[&str],
    second: CandidateProjection,
) -> Value {
    let confirmation = serde_json::to_value(&record.confirmation).expect("confirmation json");
    assert_eq!(confirmation["kind"], kind, "{confirmation}");
    assert_eq!(
        confirmation
            .as_object()
            .expect("confirmation object")
            .keys()
            .map(String::as_str)
            .filter(|member| *member != "projection")
            .collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>(),
        "{confirmation}"
    );
    let candidate_set = confirmation["candidate_set_sha256"]
        .as_str()
        .expect("candidate_set_sha256");
    let decision = confirmation["decision_sha256"]
        .as_str()
        .expect("decision_sha256");
    assert!(
        is_hash(candidate_set) && is_hash(decision),
        "{confirmation}"
    );
    assert_eq!(candidate_set, second.candidate_set_sha256(), "F2's hash");
    assert_ne!(
        candidate_set,
        record.projection.candidate_set_sha256(),
        "the marker moved, so F2's candidate set differs from F1's"
    );
    assert_eq!(record.selected, [CHOICE], "the record keeps F1's decision");
    assert!(record.multi_select.is_none());
    record
        .validate_for_append()
        .expect("the record validates for append");
    let stored = serde_json::to_value(record).expect("record json");
    let read: TaskSelectionRecord =
        serde_json::from_value(stored.clone()).expect("the record reads back");
    assert_eq!(&read, record, "{stored}");
    confirmation
}

// F2 shows the marker moved by exactly `TRACK_PX` to the right and 3 px down, so the Chebyshev
// distance between the two anchor match origins is `TRACK_PX`: F2's decision chooses it, the
// record is `tracked` with that shift and F2's hashes, and the one tap lands inside its click box
// on F2, which does not overlap the click box the marker had on F1. The same move under the
// default confirmation (no `confirm`) is refused as today: `mismatched`, no tap.
#[test]
fn a_moved_choice_within_track_px_taps_its_f2_click() {
    let moved = home(Some((40 + TRACK_PX, 33)));
    let prepared = tracking_task();
    let mut device = Device::new(FIRST, moved);

    let result = prepared.run(&mut device);

    let outcome = result.unwrap_or_else(|error| panic!("the tracked run fails: {error:?}"));
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    let record = device.decision_record();
    let first = projection_on(&prepared, FIRST);
    let second = projection_on(&prepared, moved);
    assert_eq!(
        record.projection.candidate_set_sha256(),
        first.candidate_set_sha256()
    );
    let confirmation = track_confirmation(
        record,
        "tracked",
        &[
            "kind",
            "candidate_set_sha256",
            "decision_sha256",
            "shift_px",
        ],
        second.clone(),
    );
    assert_eq!(confirmation["shift_px"], TRACK_PX, "{confirmation}");
    let evaluated = first.candidate(CHOICE).expect("c1").click;
    let tracked = second.candidate(CHOICE).expect("c2").click;
    assert_eq!(
        (evaluated.x, evaluated.y, tracked.x, tracked.y),
        (42, 31, 50, 34),
        "the click boxes of c1 on F1 and c2 on F2"
    );
    assert_eq!(device.intents(), 1, "{:?}", device.events);
    let taps = device.taps();
    let [InputAction::Tap { x, y }] = taps.as_slice() else {
        panic!("one tap: {:?}", device.events);
    };
    let inside = |rect: CandidateRect| {
        (rect.x..rect.x + rect.width).contains(x) && (rect.y..rect.y + rect.height).contains(y)
    };
    assert!(
        inside(tracked),
        "the tap ({x}, {y}) lies in c2's click box {tracked:?}"
    );
    assert!(!inside(evaluated), "the tap ({x}, {y}) is not where c1 was");

    let mut default = Device::new(FIRST, moved);
    let mut select = tracking();
    let select_object = select.as_object_mut().expect("select object");
    select_object.remove("confirm");
    select_object.remove("track_px");
    let untracked = admit(&task(marker_layout(), select), &policy(LAYOUT))
        .unwrap_or_else(|error| panic!("the default step is refused: {error}"));
    let error = failed(untracked.run(&mut default));
    assert_eq!(
        error.code().as_str(),
        "selection_projection_mismatch",
        "{error}"
    );
    let confirmation =
        serde_json::to_value(&default.decision_record().confirmation).expect("confirmation json");
    assert_eq!(confirmation["kind"], "mismatched", "{confirmation}");
    assert!(default.taps().is_empty(), "{:?}", default.events);
}

// F2 shows the marker one pixel farther than `TRACK_PX` (`shift_px` `TRACK_PX` + 1), or shows no
// marker, so that F2's decision chooses nobody (`insufficient`). Each records `track_lost` (with
// the shift, or without one), keeps F1's decision, fails `selection_decision_mismatch` with
// `confirm` `track` and `shift_px` or `selection_kind`, and makes no intent and no input.
#[test]
fn a_choice_beyond_track_px_fails_before_input() {
    let prepared = tracking_task();
    let beyond = home(Some((40 + TRACK_PX + 1, 30)));
    let gone = home(None);
    for (case, second, shift, kind) in [
        ("beyond", beyond, json!(TRACK_PX + 1), Value::Null),
        ("gone", gone, Value::Null, json!("insufficient")),
    ] {
        let mut device = Device::new(FIRST, second);

        let error = failed(prepared.run(&mut device));

        assert_eq!(
            error.code().as_str(),
            "selection_decision_mismatch",
            "{case}: {error}"
        );
        assert_eq!(value(&error, "confirm"), json!("track"), "{case}");
        assert_eq!(value(&error, "shift_px"), shift, "{case}");
        assert_eq!(value(&error, "selection_kind"), kind, "{case}");
        let record = device.decision_record();
        let members: &[&str] = if shift.is_null() {
            &["kind", "candidate_set_sha256", "decision_sha256"]
        } else {
            &[
                "kind",
                "candidate_set_sha256",
                "decision_sha256",
                "shift_px",
            ]
        };
        let confirmation = track_confirmation(
            record,
            "track_lost",
            members,
            projection_on(&prepared, second),
        );
        assert_eq!(
            confirmation.get("shift_px").cloned().unwrap_or(Value::Null),
            shift,
            "{case}: {confirmation}"
        );
        assert_eq!(device.intents(), 0, "{case}: {:?}", device.events);
        assert!(device.taps().is_empty(), "{case}: {:?}", device.events);
    }
}

// Admission takes `confirm: "track"` with `track_px` 1..=64 on a single-frame select over a
// `repeated_anchor` layout, and refuses with `contained_task_select_invalid`, naming the rule:
// either one on a `fixed_slots` layout, `track` beside `scan`, `track_px` 0 or 65, `track_px`
// without `track` (no `confirm`, or `decision`) and `track` without `track_px`.
#[test]
fn track_refused_on_fixed_slots_and_scan() {
    let with = |change: &dyn Fn(&mut Value)| {
        let mut select = tracking();
        change(&mut select);
        select
    };
    for (case, track_px) in [("lowest", 1), ("highest", 64)] {
        let select = with(&|select| select["track_px"] = json!(track_px));
        admit(&task(marker_layout(), select), &policy(LAYOUT))
            .unwrap_or_else(|error| panic!("{case}: refused: {error}"));
    }
    let mut slots = tracking();
    slots["layout_id"] = json!("layout/slots");
    let refusals = [
        (
            "fixed slots",
            task(slot_layout(), slots),
            policy("layout/slots"),
            "confirm=track layout=not_repeated_anchor",
        ),
        (
            "scan",
            task(
                marker_layout(),
                with(&|select| select["scan"] = scan_declaration()),
            ),
            policy(LAYOUT),
            "confirm=track scan=declared",
        ),
        (
            "track_px 0",
            task(
                marker_layout(),
                with(&|select| select["track_px"] = json!(0)),
            ),
            policy(LAYOUT),
            "confirm=track track_px=0 range=1..=64",
        ),
        (
            "track_px 65",
            task(
                marker_layout(),
                with(&|select| select["track_px"] = json!(65)),
            ),
            policy(LAYOUT),
            "confirm=track track_px=65 range=1..=64",
        ),
        (
            "track_px without confirm",
            task(
                marker_layout(),
                with(&|select| {
                    select.as_object_mut().expect("select").remove("confirm");
                }),
            ),
            policy(LAYOUT),
            "track_px=declared confirm=not_track",
        ),
        (
            "track_px with decision",
            task(
                marker_layout(),
                with(&|select| select["confirm"] = json!("decision")),
            ),
            policy(LAYOUT),
            "track_px=declared confirm=not_track",
        ),
        (
            "track without track_px",
            task(
                marker_layout(),
                with(&|select| {
                    select.as_object_mut().expect("select").remove("track_px");
                }),
            ),
            policy(LAYOUT),
            "confirm=track track_px=absent",
        ),
    ];
    for (case, task, policy, detail) in refusals {
        let error = match admit(&task, &policy) {
            Ok(_) => panic!("{case}: admitted"),
            Err(error) => error,
        };
        assert_eq!(
            error.code().as_str(),
            "contained_task_select_invalid",
            "{case}: {error}"
        );
        let written = error.detail().unwrap_or_default();
        assert!(
            written.contains(detail),
            "{case}: the detail {written:?} does not name {detail:?}"
        );
    }
}
