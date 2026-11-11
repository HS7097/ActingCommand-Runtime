// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 DIAG-SI (register §1.320; the coordinator's rulings of 2026-11-11, not Alice's
// words): `contained_task_state_invalid` says what was invalid. The code stays, with its category
// and ladder class, at the same step; its outcome gains `state_problem` (a closed vocabulary) and
// the page (`page_id`), the operation (`pack_operation_id`) and the stage (`phase_id`) the place
// knows.
//
// - T1: the incident's kind, `no_move_from_page`. A phased route whose entry page is the panel
//   with a finished item, run as each kind of layer on the panel with every slot busy (a page of
//   the package outside its entry page set and outside the first stage's moves), starts at the
//   first stage, which has no operation from that page: the run ends before any step, naming the
//   page and the stage. An ordinary run is brought to such a page by its own operation in a route
//   without stages (a phased route's static walk refuses such a route at admission,
//   `task_phases_unreachable`): the run ends after that step, naming the page.
// - T2: the run's state machine gives each refused decision its kind (`RunDecisionKind`), with
//   the code it had before, and the kernel names the token by that kind, never by the message.
// - T3: a pacing factor below 1000, an answer the Runtime never gives, is `runtime_invariant`.
// - T4: the codes-only projection keeps the token.
//
// Each package is a synthetic `neutral` package, admitted through `PreparedContainedTask::load`:
// a 2 x 1 frame whose left pixel names the page. The runs play scripted frames on the scripted
// clock (EXEC-I4): the device shows `screens[n]` after `n` inputs.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::outcome::{Category, LadderClass};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 2;
// A panel page with a finished item, a panel page with every slot busy, a panel page with a free
// slot, a list page and a notice page.
const PAGES: [(&str, [u8; 3]); 5] = [
    ("panel_done", [255, 0, 0]),
    ("panel_busy", [0, 255, 0]),
    ("panel_free", [0, 0, 255]),
    ("list", [255, 255, 0]),
    ("notice", [255, 0, 255]),
];

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

// The ways the host runs a package as a layer of another package's entry.
#[derive(Debug, Clone, Copy)]
enum Layer {
    // A prerequisite package, a navigation edge, a shared interrupt's handler, the configured
    // return-home package.
    Prerequisite,
    // A request's own entry recovery package.
    EntryRecovery,
    // A step of a navigation recovery chain, a run the host starts itself.
    RecoveryChain,
}

const LAYERS: [Layer; 3] = [
    Layer::Prerequisite,
    Layer::EntryRecovery,
    Layer::RecoveryChain,
];

impl Layer {
    fn run(self, task: &PreparedContainedTask, runtime: &mut ScriptedRuntime) -> RunResult {
        match self {
            Self::Prerequisite => task.run_as_prerequisite(runtime),
            Self::EntryRecovery => task.run_entry_recovery(runtime),
            Self::RecoveryChain => task.run_as_host_layer(runtime),
        }
    }
}

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// The frame of `page`: its colour on the left pixel, black on the right.
fn screen(page: &str) -> ScriptedFrame {
    ScriptedFrame::rgb(WIDTH, 1, &[colour(page), [0, 0, 0]])
}

// A device that shows `pages[n]` after `n` inputs.
fn device(pages: &[&str]) -> ScriptedRuntime {
    ScriptedRuntime::new(pages.iter().map(|page| vec![screen(page)]).collect())
}

// A trusted tap from `from` to `to`.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({
        "id": id,
        "from": from,
        "to": to,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true,
        "retryable": false
    })
}

fn stage(id: &str, operations: &[&str], target_pages: &[&str]) -> Value {
    json!({"id": id, "operations": operations, "target_pages": target_pages})
}

// A task ending on the list page, entering on `entry`.
fn route_task(entry: Value, operations: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": entry,
        "target_page": "list",
        "operations": operations
    })
}

// Two stages to the list page, entering on `entry`. `receive` takes the finished item on the panel
// (`collect`), after closing a notice when one shows (`close_notice`), and ends on the panel with
// a free slot; `browse` opens the list from the panel with a free slot or with every slot busy.
// `receive` has no operation from the panel with every slot busy and does not target it.
fn two_stage_route(entry: Value) -> Value {
    let mut route = route_task(
        entry,
        vec![
            tap("collect", "panel_done", "panel_free"),
            tap("close_notice", "notice", "panel_done"),
            tap("open_from_free", "panel_free", "list"),
            tap("open_from_busy", "panel_busy", "list"),
        ],
    );
    route["phases"] = Value::Array(vec![
        stage("receive", &["collect", "close_notice"], &["panel_free"]),
        stage("browse", &["open_from_free", "open_from_busy"], &["list"]),
    ]);
    route["scheduling_outcome"] = json!({"mappings": [{
        "outcome_key": "listed",
        "effect": "no_designated_effect",
        "terminal_pages": ["list"]
    }]});
    route
}

// A route without stages whose one operation from the panel with a finished item leads to the
// panel with every slot busy, from which no operation leaves.
fn route_to_a_page_without_a_move() -> Value {
    route_task(
        json!("panel_done"),
        vec![
            tap("collect", "panel_done", "panel_busy"),
            tap("open_from_free", "panel_free", "list"),
        ],
    )
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON bytes")
}

fn zip_entries(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, content) in entries {
        zip.start_file(*path, options).expect("zip entry");
        zip.write_all(content).expect("zip bytes");
    }
    zip.finish().expect("finish zip").into_inner()
}

// The package of `task`: a `Lab-1y.control.v2` control holding the task's stages when it has
// any, and a recognition pack and page set that tell the fixture pages by their colour.
fn package(task: &Value) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.diag.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 1_000,
        "timeout_ms": 60_000
    });
    if let Some(phases) = task.get("phases") {
        control["phases"] = phases.clone();
    }
    let targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        ("resources/operations/task/task.json", bytes(task)),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&json!({"schema_version": "0.3", "pages": pages})),
        ),
    ])
}

fn admitted(case: &str, task: &Value) -> PreparedContainedTask {
    PreparedContainedTask::load("neutral.instance", &package(task))
        .unwrap_or_else(|error| panic!("{case}: admission refused the package: {error}"))
}

// The operation and the stage index of each step the run started, in order.
fn steps(runtime: &ScriptedRuntime) -> Vec<(String, Option<u32>)> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepStarted {
                operation_label,
                phase,
                ..
            } => Some((
                operation_label.clone(),
                phase.as_ref().map(|phase| phase.index),
            )),
            _ => None,
        })
        .collect()
}

// The run ended with `contained_task_state_invalid`, of category error, with values: its outcome.
fn state_invalid_outcome(case: &str, result: RunResult) -> Outcome {
    let code = SharedCode::ContainedTaskStateInvalid;
    match result {
        Err(ContainedTaskRunError::Task(error)) => {
            assert_eq!(error.code().as_str(), code.as_str(), "{case}");
            assert!(error.has_values(), "{case}: the failure carries values");
            let outcome = error.outcome();
            assert_eq!(outcome.code().as_str(), code.as_str(), "{case}");
            assert_eq!(outcome.category(), Category::Error, "{case}");
            outcome
        }
        other => panic!("{case}: expected the run to end with {code}, got {other:?}"),
    }
}

// The text value of `key`, when the outcome carries one.
fn text<'o>(outcome: &'o Outcome, key: &str) -> Option<&'o str> {
    outcome.value(key).and_then(Value::as_str)
}

// T1: the incident's kind. As each kind of layer on the panel with every slot busy, the route
// starts at `receive` (the page is no entry page), which has no operation from that page: the run
// ends before any step and input with `no_move_from_page`, the page and the stage `receive`. The
// ordinary run of a route without stages ends after its one step on the page that step reached,
// from which no operation leaves: `no_move_from_page` and the page, no stage.
#[test]
fn diag_si_t1_a_stage_without_a_move_from_the_observed_page_names_the_page_and_the_stage() {
    let code = SharedCode::ContainedTaskStateInvalid;
    assert_eq!(code.category(), Category::Error);
    assert_eq!(code.ladder_class(), LadderClass::NotApplicable);
    let route = admitted("two-stage route", &two_stage_route(json!("panel_done")));
    for layer in LAYERS {
        let case = format!("{layer:?}");
        let mut runtime = device(&["panel_busy", "list"]);
        let result = layer.run(&route, &mut runtime);
        let outcome = state_invalid_outcome(&case, result);
        assert_eq!(
            text(&outcome, "state_problem"),
            Some(StateProblem::NoMoveFromPage.as_str()),
            "{case}: {outcome:?}"
        );
        assert_eq!(
            text(&outcome, "page_id"),
            Some("neutral/panel_busy"),
            "{case}"
        );
        assert_eq!(text(&outcome, "phase_id"), Some("receive"), "{case}");
        assert_eq!(outcome.value("pack_operation_id"), None, "{case}");
        assert!(steps(&runtime).is_empty(), "{case}: no step started");
        assert!(runtime.inputs().is_empty(), "{case}: nothing was input");
    }

    let route = admitted("route without stages", &route_to_a_page_without_a_move());
    let mut runtime = device(&["panel_done", "panel_busy"]);
    let result = route.run(&mut runtime);
    let outcome = state_invalid_outcome("ordinary run", result);
    assert_eq!(
        text(&outcome, "state_problem"),
        Some(StateProblem::NoMoveFromPage.as_str()),
        "{outcome:?}"
    );
    assert_eq!(text(&outcome, "page_id"), Some("neutral/panel_busy"));
    assert_eq!(outcome.value("phase_id"), None, "a route without stages");
    assert_eq!(steps(&runtime), vec![("collect".to_owned(), None)]);
    assert_eq!(runtime.inputs().len(), 1);
}

// T2: the kind of each refused decision, from the state machine itself, with the code it had
// before; the kernel's token follows the kind, and its values name the operation the kernel asked
// for and the page the machine is on.
#[test]
fn diag_si_t2_the_state_machine_names_the_kind_of_a_refused_decision() {
    let config = RunStateConfig::new("neutral", Some("list".to_owned()), true, 1, 4)
        .expect("run configuration");
    let mut machine = RunStateMachine::new(config, 0).expect("run state machine");
    machine
        .observe_page(Some("neutral/panel_busy".to_owned()))
        .expect("observed page");
    let operations = [RunOperationCandidate::new("collect", "panel_done").expect("candidate")];

    // No operation leaves the page.
    let error = machine
        .next_directive(&operations)
        .expect_err("no operation from the page");
    assert_eq!(error.kind(), RunDecisionKind::NoOperationFromPage);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RunDecisionInvalid.as_str()
    );
    let outcome = machine_refusal(&error, &machine, None).outcome();
    assert_eq!(
        text(&outcome, "state_problem"),
        Some(StateProblem::NoMoveFromPage.as_str())
    );
    assert_eq!(text(&outcome, "page_id"), Some("neutral/panel_busy"));
    assert_eq!(outcome.value("phase_id"), None, "a machine without stages");

    // A transition refused in the machine's state: an outcome of an operation never dispatched.
    let error = machine
        .operation_succeeded("collect", Some("neutral/panel_free".to_owned()))
        .expect_err("no operation was dispatched");
    assert_eq!(error.kind(), RunDecisionKind::TransitionRefused);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RunTransitionInvalid.as_str()
    );
    let outcome = machine_refusal(&error, &machine, Some("collect")).outcome();
    assert_eq!(
        text(&outcome, "state_problem"),
        Some(StateProblem::TransitionRefused.as_str())
    );
    assert_eq!(text(&outcome, "pack_operation_id"), Some("collect"));
    assert_eq!(text(&outcome, "page_id"), Some("neutral/panel_busy"));

    // A value the machine cannot take: a page that matches two of its target pages, which
    // admission never lets a package declare.
    let config = RunStateConfig::new_with_target_pages(
        "neutral",
        vec!["list".to_owned(), "neutral/list".to_owned()],
        true,
        1,
        4,
    )
    .expect("run configuration");
    let mut machine = RunStateMachine::new(config, 0).expect("run state machine");
    machine
        .observe_page(Some("neutral/list".to_owned()))
        .expect("observed page");
    let error = machine.next_directive(&[]).expect_err("two target pages");
    assert_eq!(error.kind(), RunDecisionKind::Invalid);
    assert_eq!(
        error.code().as_str(),
        ExecutionKernelCode::RunDecisionInvalid.as_str()
    );
    let outcome = machine_refusal(&error, &machine, None).outcome();
    assert_eq!(
        text(&outcome, "state_problem"),
        Some(StateProblem::RuntimeInvariant.as_str())
    );
}

// T3: the runtime answers the run's first `step_pacing()` with the factor 500, below the minimum
// 1000 it never goes under: the run ends before any frame of its entry is judged, with
// `runtime_invariant`.
#[test]
fn diag_si_t3_a_pacing_factor_below_the_minimum_is_a_runtime_invariant() {
    let route = admitted("two-stage route", &two_stage_route(json!("panel_done")));
    let mut runtime = device(&["panel_done", "panel_free", "list"]).with_step_factors(vec![500]);
    let result = route.run(&mut runtime);
    let outcome = state_invalid_outcome("factor 500", result);
    assert_eq!(
        text(&outcome, "state_problem"),
        Some(StateProblem::RuntimeInvariant.as_str()),
        "{outcome:?}"
    );
    assert!(steps(&runtime).is_empty(), "no step started");
    assert!(runtime.inputs().is_empty(), "nothing was input");
}

// T4: the codes-only projection (`detail: codes`, a client of class (a)) keeps the code, its
// category and the token; the page and the stage are the full outcome's.
#[test]
fn diag_si_t4_the_codes_only_projection_keeps_the_token() {
    let route = admitted("two-stage route", &two_stage_route(json!("panel_done")));
    let mut runtime = device(&["panel_busy"]);
    let result = route.run_as_prerequisite(&mut runtime);
    let outcome = state_invalid_outcome("prerequisite layer", result);
    let codes = outcome.codes_only();
    assert_eq!(
        codes.code().as_str(),
        SharedCode::ContainedTaskStateInvalid.as_str()
    );
    assert_eq!(codes.category(), Category::Error);
    assert_eq!(
        text(&codes, "state_problem"),
        Some(StateProblem::NoMoveFromPage.as_str()),
        "{codes:?}"
    );
    assert_eq!(text(&outcome, "page_id"), Some("neutral/panel_busy"));
    assert_eq!(text(&outcome, "phase_id"), Some("receive"));
}
