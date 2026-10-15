// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #308 SEL-L6a (FCM row SEL-L6a; compromises T-01 and T-03 §1): a whole package
// replayed over a saved frame sequence with the production interpreter. The replay reports the
// entry check, every step with its operation, destinations, guard outcome and the page it
// reached, and the run's end; a multi-destination operation names its destination set and the
// page the sequence reached; a multi-frame consensus takes its samples from the sequence; a
// select step evaluates the given fact snapshot at the instant it was read, or the empty one.
//
// A frame's left pixel names its page; its right pixel is the guard's colour, green when the
// guard target is ready. Every expected value comes from the FCM row or the contracts it names.

use super::*;
use crate::{
    OfflineReplayEnd, OfflineReplayFacts, OfflineReplayResult, OfflineReplayStep,
    replay_contained_task,
};
use actingcommand_contract::{
    FactContent, FactRecord, FactScope, FactValue, InstanceFactContext, InstanceFactSnapshot,
};
use actingcommand_device::CaptureBackendName;
use actingcommand_recognition_pack::RecognitionPack;
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::PathBuf;

const READY: [u8; 3] = [0, 255, 0];
const NOT_READY: [u8; 3] = [0, 0, 0];
// The fact the select step's policy gates on, published at `PUBLISHED_AT` and fresh for a
// minute.
const FACT_KEY: &str = "session.example.enabled";
const PUBLISHED_AT: u64 = 1_000_000;
const MAX_AGE_MS: u64 = 60_000;

fn color(page: &str) -> [u8; 3] {
    match page {
        "home" => [255, 0, 0],
        "middle" => [255, 255, 0],
        "terminal" => [0, 0, 255],
        "alternate" => [0, 255, 255],
        "blank" => [0, 0, 0],
        _ => panic!("unknown fixture page {page}"),
    }
}

fn frame(page: &str, guard: [u8; 3]) -> Frame {
    Frame::from_pixels(
        2,
        1,
        [color(page), guard].concat(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

fn replay(
    task: &PreparedContainedTask,
    frames: Vec<Frame>,
    facts: OfflineReplayFacts,
) -> OfflineReplayResult {
    replay_contained_task(task, frames.into_iter().map(Ok::<_, Infallible>), facts)
        .expect("the replay gives a result")
}

fn no_facts() -> OfflineReplayFacts {
    OfflineReplayFacts::Empty {
        now_unix_ms: PUBLISHED_AT,
    }
}

fn control() -> TaskControl {
    serde_json::from_value(json!({
        "schema_version": "Lab-1y.control.v1",
        "package_id": "neutral.replay.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 10_000,
        "timeout_ms": 60_000
    }))
    .expect("task control")
}

fn guard(page: &str) -> Value {
    json!({
        "page_id": page,
        "target_id": "guard/ready",
        "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
        "color_probe": "guard/ready"
    })
}

// A guarded click on the right pixel from `from` to `to` (one page or a page set).
fn click(id: &str, from: &str, to: Value) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "guard": guard(from)
    })
}

// The recognition pack of every page; with `consensus`, page `home` needs two frames 1 ms
// apart; with `layout`, a candidate layout.
fn pack(consensus: bool, layout: Option<Value>) -> RecognitionPack {
    let page = |id: &str, name: &str| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": color(name)})
    };
    let mut pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            page("page/home", "home"),
            page("page/middle", "middle"),
            page("page/terminal", "terminal"),
            page("page/alternate", "alternate"),
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": READY}
        ]
    });
    if consensus {
        pack["target_consensus"] = json!({
            "page/home": {"samples": [{"frame": 0}, {"frame": 1}], "k": 2,
                          "sample_interval_ms": 1}
        });
    }
    if let Some(layout) = layout {
        pack["candidate_layouts"] = json!([layout]);
    }
    serde_json::from_value(pack).expect("recognition pack")
}

// A prepared task of `operations` towards `target_page`, each operation admitted as a package
// admits it.
fn task(
    operations: Vec<TaskOperation>,
    target_page: Value,
    pack: RecognitionPack,
) -> PreparedContainedTask {
    let control = control();
    for operation in &operations {
        operation
            .validate(&control, TaskOperationDefaults::default(), "0.6")
            .expect("valid operation");
    }
    let mut program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "target_page": target_page,
        "operations": []
    }))
    .expect("task program");
    program.operations = operations;
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack).expect("recognition evaluator");
    let pages = ["home", "middle", "terminal", "alternate"]
        .map(|page| json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")]}));
    let page_set: PageSet =
        serde_json::from_value(json!({"schema_version": "0.3", "pages": pages})).expect("page set");
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

fn operation(value: Value) -> TaskOperation {
    serde_json::from_value(value).expect("task operation")
}

fn refusal(end: &OfflineReplayEnd) -> &str {
    match end {
        OfflineReplayEnd::Refused { outcome } => outcome.code().as_str(),
        OfflineReplayEnd::Completed { .. } => panic!("expected a refused end, got {end:?}"),
    }
}

fn completed(final_page: &str, executed_steps: u32) -> OfflineReplayEnd {
    OfflineReplayEnd::Completed {
        final_page: Some(final_page.to_owned()),
        executed_steps,
        scheduling_outcome_key: None,
    }
}

// The one tap the step would have sent, under a passed guard on `guard/ready`.
fn single_tap(step: &OfflineReplayStep) -> (i32, i32) {
    let [effect] = step.effects.as_slice() else {
        panic!("expected one effect, got {:?}", step.effects);
    };
    assert!(
        matches!(&effect.guard, ContainedTaskGuardOutcome::Passed { target_id, .. }
            if target_id == "guard/ready"),
        "{:?}",
        effect.guard
    );
    match effect.action {
        InputAction::Tap { x, y } => (x, y),
        ref other => panic!("expected a tap, got {other:?}"),
    }
}

fn labels(result: &OfflineReplayResult) -> Vec<&str> {
    result
        .steps
        .iter()
        .map(|step| step.operation_label.as_str())
        .collect()
}

// T-01: the replay runs the whole package, not only its first decision. It reports the entry
// check, each step's operation, its guarded input and the page it reached, and the end: the run
// completes; a guard that does not hold on the frame the sequence shows ends the run with the
// guard's refusal; a run that needs a frame after the last one ends with
// offline_fixture_exhausted. Nothing is executed.
#[test]
fn l6a_t1_a_whole_package_replays_its_entry_steps_guards_and_end() {
    let mut two_steps = task(
        vec![
            operation(click("open_middle", "home", json!("middle"))),
            operation(click("open_terminal", "middle", json!("terminal"))),
        ],
        json!("terminal"),
        pack(false, None),
    );
    two_steps.entry_page = Some(RequiredEntryPages::single("neutral/home".to_owned()));

    let result = replay(
        &two_steps,
        vec![
            frame("home", READY),
            frame("middle", READY),
            frame("terminal", NOT_READY),
        ],
        no_facts(),
    );
    assert_eq!(result.mode, "offline_replay");
    assert!(!result.executed);
    assert_eq!(result.package_id, "neutral.replay.task");
    let entry = result.entry.as_ref().expect("the entry check");
    assert_eq!(entry.required_pages, ["neutral/home"]);
    assert_eq!(entry.matched_page.as_deref(), Some("neutral/home"));
    assert_eq!(labels(&result), ["open_middle", "open_terminal"]);
    let [first, second] = result.steps.as_slice() else {
        panic!("expected two steps");
    };
    assert_eq!(first.destinations, ["middle"]);
    assert_eq!(single_tap(first), (1, 0));
    assert_eq!(first.reached_page.as_deref(), Some("neutral/middle"));
    assert_eq!(second.destinations, ["terminal"]);
    assert_eq!(single_tap(second), (1, 0));
    assert_eq!(second.reached_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(result.end, completed("neutral/terminal", 2));
    assert_eq!(result.frames_read, 3);
    assert_eq!(result.recognition.len(), 3);
    assert_eq!(result.selection_now_unix_ms, None);

    let guarded = replay(
        &two_steps,
        vec![frame("home", READY), frame("middle", NOT_READY)],
        no_facts(),
    );
    assert_eq!(labels(&guarded), ["open_middle", "open_terminal"]);
    assert_eq!(
        guarded.steps[0].reached_page.as_deref(),
        Some("neutral/middle")
    );
    assert!(guarded.steps[1].effects.is_empty());
    assert_eq!(guarded.steps[1].reached_page, None);
    assert_eq!(
        refusal(&guarded.end),
        ExecutionKernelCode::ContainedTaskGuardRefused.as_str()
    );
    assert_eq!(guarded.frames_read, 2);

    let short = replay(&two_steps, vec![frame("home", READY)], no_facts());
    assert_eq!(labels(&short), ["open_middle"]);
    assert_eq!(single_tap(&short.steps[0]), (1, 0));
    assert_eq!(short.steps[0].reached_page, None);
    assert_eq!(
        refusal(&short.end),
        ExecutionKernelCode::OfflineFixtureExhausted.as_str()
    );
    assert_eq!(short.frames_read, 1);
}

// T-03: an operation whose `to` names more than one page is replayed like any other; its step
// names the whole destination set and the one page the sequence reached.
#[test]
fn l6a_t2_a_multi_destination_operation_names_its_destinations_and_the_page_reached() {
    let destinations = json!(["terminal", "alternate"]);
    let branch = task(
        vec![operation(click("branch", "home", destinations.clone()))],
        destinations,
        pack(false, None),
    );
    for reached in ["alternate", "terminal"] {
        let result = replay(
            &branch,
            vec![frame("home", READY), frame(reached, NOT_READY)],
            no_facts(),
        );
        let [step] = result.steps.as_slice() else {
            panic!("expected one step");
        };
        assert_eq!(step.operation_label, "branch");
        assert_eq!(step.destinations, ["alternate", "terminal"]);
        assert_eq!(single_tap(step), (1, 0));
        let page = format!("neutral/{reached}");
        assert_eq!(step.reached_page.as_deref(), Some(page.as_str()));
        assert_eq!(result.end, completed(&page, 1));
    }
}

// T-03: a recognition pack with a multi-frame consensus is replayed: every observation takes
// its samples from the sequence, two frames here. Two agreeing frames decide page `home`; one
// frame of it followed by another page does not, and no input is decided.
#[test]
fn l6a_t3_a_multi_frame_consensus_takes_its_samples_from_the_sequence() {
    let sampled = task(
        vec![operation(click("open_terminal", "home", json!("terminal")))],
        json!("terminal"),
        pack(true, None),
    );
    let agreed = replay(
        &sampled,
        vec![
            frame("home", READY),
            frame("home", READY),
            frame("terminal", NOT_READY),
            frame("terminal", NOT_READY),
        ],
        no_facts(),
    );
    assert_eq!(labels(&agreed), ["open_terminal"]);
    assert_eq!(single_tap(&agreed.steps[0]), (1, 0));
    assert_eq!(agreed.end, completed("neutral/terminal", 1));
    assert_eq!(agreed.frames_read, 4);

    let disagreed = replay(
        &sampled,
        vec![frame("home", READY), frame("blank", READY)],
        no_facts(),
    );
    assert!(disagreed.steps.is_empty());
    assert_eq!(
        refusal(&disagreed.end),
        ExecutionKernelCode::OfflineFixtureExhausted.as_str()
    );
    assert_eq!(disagreed.frames_read, 2);
}

// A select step from `home` to `terminal` over one slot; its policy keeps the slot only when
// its `ready` feature holds and the instance fact `FACT_KEY` is true and at most a minute old.
fn select_task() -> PreparedContainedTask {
    let layout = json!({
        "id": "choices",
        "page_id": "neutral/home",
        "kind": "fixed_slots",
        "features": [{"name": "ready", "value": "passed"}],
        "slots": [{
            "rect": {"x": 0, "y": 0, "width": 2, "height": 1},
            "click": {"x": 0, "y": 0, "width": 2, "height": 1},
            "targets": {"ready": "guard/ready"}
        }]
    });
    let policy: actingcommand_selection_policy::SelectionPolicy = serde_json::from_value(json!({
        "schema_version": "actingcommand.selection-policy.v1",
        "policy_id": "policy.enabled",
        "applies_to": {
            "candidate_layout_id": "choices",
            "outcome_keys": {"selected": "enabled_selected", "empty": "enabled_empty",
                             "insufficient": "enabled_insufficient",
                             "ambiguous": "enabled_ambiguous", "unknown": "enabled_unknown"}
        },
        "fields": [{"name": "ready", "value_type": {"type": "boolean"}}],
        "facts": [{"fact_key": FACT_KEY, "value_type": {"type": "boolean"},
                   "max_age_ms": MAX_AGE_MS, "minimum_confidence_milli": 1_000}],
        "gates": [
            {"gate_id": "ready",
             "predicate": {"kind": "boolean_equals",
                           "value": {"source": "field", "field": "ready"}, "expected": true},
             "on_unknown": {"kind": "drop_candidate"}},
            {"gate_id": "enabled",
             "predicate": {"kind": "boolean_equals",
                           "value": {"source": "fact", "fact_key": FACT_KEY}, "expected": true},
             "on_unknown": {"kind": "drop_candidate"}}
        ],
        "scoring": [],
        "selection": {"mode": "exactly_one", "required_count": 1},
        "tie_break": [{"kind": "candidate_id", "direction": "lowest_first"}]
    }))
    .expect("selection policy");
    policy.validate().expect("valid selection policy");
    let mut pick = operation(json!({
        "id": "pick",
        "from": "home",
        "to": "terminal",
        "expect_after": {"page_id": "terminal"},
        "guard": guard("home"),
        "select": {"layout_id": "choices",
                   "policy": {"path": "policies/select.json", "sha256": "b".repeat(64)}}
    }));
    pick.prepared_select = Some(Box::new(selection::PreparedSelect::for_test(
        "choices", policy,
    )));
    task(vec![pick], json!("terminal"), pack(false, Some(layout)))
}

// The fact snapshot a Runtime answered for the instance, read at `read_at_unix_ms`.
fn snapshot_facts(read_at_unix_ms: u64) -> OfflineReplayFacts {
    let record = FactRecord {
        scope: FactScope::Instance {
            instance_id: "instance-a".to_owned(),
        },
        key: FACT_KEY.to_owned(),
        content: FactContent::Inline {
            value: FactValue::Boolean(true),
        },
        observed_at_unix_ms: PUBLISHED_AT,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "decision-state".to_owned(),
        source_snapshot_id: "decision-state:enabled".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "0".repeat(64),
        invalidate_on: Vec::new(),
    };
    OfflineReplayFacts::Snapshot {
        snapshot: InstanceFactSnapshot {
            snapshot_id: "snapshot-a".to_owned(),
            ledger_position: 7,
            context: InstanceFactContext {
                instance_id: "instance-a".to_owned(),
                server_id: "test".to_owned(),
                game_id: "neutral".to_owned(),
            },
            records: vec![record],
        },
        read_at_unix_ms,
    }
}

// T-01 (`--facts`): a select step evaluates the given snapshot at the instant the Runtime read
// it. Read while the fact was fresh, the slot is selected, confirmed on the next frame and
// tapped; read after the fact's age ran out, or without a snapshot, nothing is selected and no
// input is decided. Each records the step's decision and the instant it evaluated at.
#[test]
fn l6a_t4_a_select_step_evaluates_the_facts_snapshot_at_the_instant_it_was_read() {
    let select = select_task();
    let frames = || {
        vec![
            frame("home", READY),
            frame("home", READY),
            frame("terminal", NOT_READY),
        ]
    };

    let fresh = replay(&select, frames(), snapshot_facts(PUBLISHED_AT + 500));
    assert_eq!(labels(&fresh), ["pick"]);
    let step = &fresh.steps[0];
    assert!(step.selection.is_some());
    let (x, y) = single_tap(step);
    assert!(
        (0..2).contains(&x) && y == 0,
        "tap ({x}, {y}) outside the slot"
    );
    assert_eq!(step.reached_page.as_deref(), Some("neutral/terminal"));
    assert_eq!(fresh.end, completed("neutral/terminal", 1));
    assert_eq!(fresh.selection_now_unix_ms, Some(PUBLISHED_AT + 500));
    assert_eq!(fresh.frames_read, 3);

    for (facts, instant) in [
        (
            snapshot_facts(PUBLISHED_AT + MAX_AGE_MS + 1),
            PUBLISHED_AT + MAX_AGE_MS + 1,
        ),
        (no_facts(), PUBLISHED_AT),
    ] {
        let unselected = replay(&select, frames(), facts);
        assert_eq!(labels(&unselected), ["pick"]);
        assert!(unselected.steps[0].selection.is_some());
        assert!(unselected.steps[0].effects.is_empty());
        assert_eq!(
            refusal(&unselected.end),
            ExecutionKernelCode::SelectionNotSelected.as_str()
        );
        assert_eq!(unselected.selection_now_unix_ms, Some(instant));
        assert_eq!(unselected.frames_read, 1);
    }
}
