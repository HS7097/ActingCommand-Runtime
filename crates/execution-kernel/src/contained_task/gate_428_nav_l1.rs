// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 NAV-L1 (register §1.319; the coordinator's ruling of 2026-11-11, not Alice's
// words): one rule for every run of a phased route. A layer the host runs for another package's
// entry (`run_as_prerequisite`, `run_entry_recovery`, `run_as_host_layer`, each entered as
// `ContainedTaskEntry::BoundRecovery`) whose first observed page is one of the package's entry
// pages enters at the stage that page starts, as an ordinary run does since Workflow #373 NAV-3b;
// on any other first page it starts at the first stage, as before.
//
// - T1: a two-stage route whose entry page set holds `panel_done` (it starts the first stage) and
//   `panel_busy` (it starts the second). A layer first observing `panel_busy` runs the second
//   stage's operation alone and ends on that stage's target; one first observing `panel_done`
//   runs the first stage, then the second.
// - T2: the same route run as an ordinary run from each entry page runs the same operations, in
//   the same order, with the same end, as each layer.
// - T3: a layer whose first observed page is no entry page starts at the first stage.
// - T4: a route without stages and a phased route with a single entry page run as a layer as
//   before.
//
// Each package is a synthetic `neutral` package, admitted through `PreparedContainedTask::load`:
// a 2 x 1 frame whose left pixel names the page. The runs play scripted frames on the scripted
// clock (EXEC-I4): the device shows `screens[n]` after `n` inputs.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
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
// The panel page with a finished item starts `receive` and the one with every slot busy starts
// `browse`: `receive` has no operation from it and does not target it.
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

// The route's entry page set: the panel with a finished item and the panel with every slot busy.
fn entry_page_set() -> Value {
    json!(["panel_done", "panel_busy"])
}

// The same entry page set without stages: each entry page leaves for the list in one step.
fn route_without_stages() -> Value {
    route_task(
        entry_page_set(),
        vec![
            tap("from_done", "panel_done", "list"),
            tap("from_busy", "panel_busy", "list"),
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
        "package_id": "neutral.layer.task",
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

fn succeeded(case: &str, result: RunResult) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success, "{case}");
            outcome
        }
        Err(error) => panic!("{case}: expected the run to end Completed, got {error:?}"),
    }
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

fn expected(steps: &[(&str, Option<u32>)]) -> Vec<(String, Option<u32>)> {
    steps
        .iter()
        .map(|(operation, phase)| ((*operation).to_owned(), *phase))
        .collect()
}

// What one run did: its steps, its final page and its step count.
fn run_record(case: &str, runtime: &ScriptedRuntime, result: RunResult) -> RunRecord {
    let outcome = succeeded(case, result);
    RunRecord {
        steps: steps(runtime),
        final_page: outcome.final_page,
        executed_steps: outcome.executed_steps,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RunRecord {
    steps: Vec<(String, Option<u32>)>,
    final_page: Option<String>,
    executed_steps: u32,
}

// Runs `task` as `layer` on a device that shows `screens[n]` after `n` inputs.
fn layer_run(
    case: &str,
    task: &PreparedContainedTask,
    layer: Layer,
    screens: &[&str],
) -> RunRecord {
    let mut runtime = device(screens);
    let result = layer.run(task, &mut runtime);
    run_record(&format!("{case}, {layer:?}"), &runtime, result)
}

// The incident's shape: a layer first observing the panel with every slot busy starts at
// `browse`, the stage that page starts. Its one step opens the list from there and completes
// `browse`, and the layer ends on the list; no step of `receive` runs. A layer first observing the
// panel with a finished item starts at `receive`, completes it on the panel with a free slot, and
// goes on with `browse` there.
#[test]
fn nav_l1_t1_a_layer_enters_at_the_stage_its_first_entry_page_starts() {
    let route = admitted("two-stage route", &two_stage_route(entry_page_set()));
    for layer in LAYERS {
        let busy = layer_run("busy panel", &route, layer, &["panel_busy", "list"]);
        assert_eq!(
            busy.steps,
            expected(&[("open_from_busy", Some(1))]),
            "{layer:?}"
        );
        assert_eq!(
            busy.final_page.as_deref(),
            Some("neutral/list"),
            "{layer:?}"
        );
        assert_eq!(busy.executed_steps, 1, "{layer:?}");

        let done = layer_run(
            "panel with a finished item",
            &route,
            layer,
            &["panel_done", "panel_free", "list"],
        );
        assert_eq!(
            done.steps,
            expected(&[("collect", Some(0)), ("open_from_free", Some(1))]),
            "{layer:?}"
        );
        assert_eq!(
            done.final_page.as_deref(),
            Some("neutral/list"),
            "{layer:?}"
        );
        assert_eq!(done.executed_steps, 2, "{layer:?}");
    }
}

// The parity that was missing: from each entry page, the ordinary run (the offline simulation's
// kind of run) and every layer run the same operations of the same stages, in the same order, and
// end on the same page after as many steps.
#[test]
fn nav_l1_t2_an_ordinary_run_and_a_layer_run_the_same_operations_from_each_entry_page() {
    let route = admitted("two-stage route", &two_stage_route(entry_page_set()));
    for (case, screens, operations) in [
        (
            "busy panel",
            &["panel_busy", "list"][..],
            expected(&[("open_from_busy", Some(1))]),
        ),
        (
            "panel with a finished item",
            &["panel_done", "panel_free", "list"][..],
            expected(&[("collect", Some(0)), ("open_from_free", Some(1))]),
        ),
    ] {
        let mut runtime = device(screens);
        let result = route.run(&mut runtime);
        let ordinary = run_record(&format!("{case}, ordinary"), &runtime, result);
        assert_eq!(ordinary.steps, operations, "{case}, ordinary");
        for layer in LAYERS {
            assert_eq!(
                layer_run(case, &route, layer, screens),
                ordinary,
                "{case}, {layer:?}"
            );
        }
    }
}

// A layer whose first observed page is a page of the package but no entry page (the notice) starts
// at the first stage, as before: it closes the notice and takes the item in `receive`, then opens
// the list in `browse`.
#[test]
fn nav_l1_t3_a_layer_off_the_entry_pages_starts_at_the_first_stage() {
    let route = admitted("two-stage route", &two_stage_route(entry_page_set()));
    for layer in LAYERS {
        let notice = layer_run(
            "notice",
            &route,
            layer,
            &["notice", "panel_done", "panel_free", "list"],
        );
        assert_eq!(
            notice.steps,
            expected(&[
                ("close_notice", Some(0)),
                ("collect", Some(0)),
                ("open_from_free", Some(1)),
            ]),
            "{layer:?}"
        );
        assert_eq!(
            notice.final_page.as_deref(),
            Some("neutral/list"),
            "{layer:?}"
        );
        assert_eq!(notice.executed_steps, 3, "{layer:?}");
    }
}

// As before: a route without stages runs, as a layer, the operation of the page it first observes;
// a phased route with a single entry page starts at the first stage there.
#[test]
fn nav_l1_t4_a_route_without_stages_and_a_single_entry_page_run_as_a_layer_as_before() {
    let without = admitted("route without stages", &route_without_stages());
    let single = admitted("single entry page", &two_stage_route(json!("panel_done")));
    for layer in LAYERS {
        for (page, operation) in [("panel_busy", "from_busy"), ("panel_done", "from_done")] {
            let run = layer_run("route without stages", &without, layer, &[page, "list"]);
            assert_eq!(
                run.steps,
                expected(&[(operation, None)]),
                "{page}, {layer:?}"
            );
            assert_eq!(
                run.final_page.as_deref(),
                Some("neutral/list"),
                "{page}, {layer:?}"
            );
            assert_eq!(run.executed_steps, 1, "{page}, {layer:?}");
        }

        let run = layer_run(
            "single entry page",
            &single,
            layer,
            &["panel_done", "panel_free", "list"],
        );
        assert_eq!(
            run.steps,
            expected(&[("collect", Some(0)), ("open_from_free", Some(1))]),
            "{layer:?}"
        );
        assert_eq!(run.final_page.as_deref(), Some("neutral/list"), "{layer:?}");
        assert_eq!(run.executed_steps, 2, "{layer:?}");
    }
}
