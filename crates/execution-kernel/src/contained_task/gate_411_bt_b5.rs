// SPDX-License-Identifier: AGPL-3.0-only

// #411 B5 + B14 + B20 + B24, slice BT-B5 (0.12.x B train; B-train plan §2.5, test rows T1 and
// T3 to T6 of §5.3; register §1.154; exec keys 120 to 124): a stage of a phased route may list
// its end pages (`end_pages`) and its entry pages (`entry_pages`).
//
// - T1 (§2.5 point 1): a page an operation of a stage confirms that is one of the stage's end
//   pages ends the run Completed there, and no further operation runs.
// - T3 (point 2, B14): a page of the entry page set a stage lists in `entry_pages` starts that
//   stage.
// - T4 (point 3, B20): an entry page a stage lists in `end_pages` ends the run before any step,
//   Completed on that page with no designated effect; a page that is both an end page and an
//   operation's `from` has two starts (`phase_start_ambiguous`).
// - T5 (point 1, B24's shape): the first stage's reached page listed in its `end_pages` ends the
//   run.
// - T6 (point 4): a package with end pages is no prerequisite or return-home layer, reason
//   `phase_end_pages`.
//
// Each package is a synthetic `neutral` package, admitted through `PreparedContainedTask::load`:
// a 2 x 1 frame whose left pixel names the page. The runs play scripted frames on the scripted
// clock (EXEC-I4): the device shows `screens[n]` after `n` inputs. A run's scheduling outcome
// key is the one mapping, under no designated effect, of the page it ends on.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{MaintenanceUse, SchedulingEffectCondition};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 2;
const PAGES: [(&str, [u8; 3]); 6] = [
    ("home", [255, 0, 0]),
    ("panel", [0, 255, 0]),
    ("done", [0, 0, 255]),
    ("full", [255, 255, 0]),
    ("notice", [255, 0, 255]),
    ("result", [0, 255, 255]),
];
const ENTRY_PAGES_INVALID: &str = "contained_task_entry_pages_invalid";

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

// A trusted tap from `from` to `to` (one page or a page set).
fn tap(id: &str, from: &str, to: Value) -> Value {
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

// A phased task back to `home`, entering on `entry`; each mapping `(key, page)` maps the page
// under no designated effect.
fn phased_task(
    entry: Value,
    operations: Vec<Value>,
    phases: Vec<Value>,
    mappings: &[(&str, &str)],
) -> Value {
    let mappings = mappings
        .iter()
        .map(|(key, page)| {
            json!({"outcome_key": key, "effect": "no_designated_effect",
                   "terminal_pages": [page]})
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": entry,
        "target_page": "home",
        "operations": operations,
        "phases": phases,
        "scheduling_outcome": {"mappings": mappings}
    })
}

// Three stages from `home` back to `home`: `reach` opens the panel, where its tap may bring a
// notice instead (its end page); `collect` claims on the panel and comes to `done`, unless the
// panel reports it is full (its end page); `return` goes home.
fn collect_route() -> Value {
    let mut reach = stage("reach", &["open"], &["panel"]);
    reach["end_pages"] = json!(["notice"]);
    let mut collect = stage("collect", &["claim"], &["done"]);
    collect["end_pages"] = json!(["full"]);
    phased_task(
        json!("home"),
        vec![
            tap("open", "home", json!(["panel", "notice"])),
            tap("claim", "panel", json!(["done", "full"])),
            tap("back", "done", json!("home")),
        ],
        vec![reach, collect, stage("return", &["back"], &["home"])],
        &[
            ("returned", "home"),
            ("notice_shown", "notice"),
            ("collect_blocked", "full"),
        ],
    )
}

// Three stages from `home` back to `home`: `depart` opens the panel, `business` submits it to the
// result page, `return` leaves any page for `home`. The entry page set also holds the result
// page, which `return` lists in its `entry_pages`: no operation's `from` infers its stage.
fn resume_route() -> Value {
    let mut back = stage("return", &["leave"], &["home"]);
    back["entry_pages"] = json!(["result"]);
    phased_task(
        json!(["home", "result"]),
        vec![
            tap("open", "home", json!("panel")),
            tap("submit", "panel", json!("result")),
            tap("leave", "any", json!("home")),
        ],
        vec![
            stage("depart", &["open"], &["panel"]),
            stage("business", &["submit"], &["result"]),
            back,
        ],
        &[("returned", "home")],
    )
}

// Two stages from `home` back to `home`: `reach` opens the panel, `return` comes back. The entry
// page set also holds the notice page, which `reach` lists in its `end_pages`.
fn ending_entry_route() -> Value {
    let mut reach = stage("reach", &["open"], &["panel"]);
    reach["end_pages"] = json!(["notice"]);
    phased_task(
        json!(["home", "notice"]),
        vec![
            tap("open", "home", json!("panel")),
            tap("back", "panel", json!("home")),
        ],
        vec![reach, stage("return", &["back"], &["home"])],
        &[("returned", "home"), ("notice_shown", "notice")],
    )
}

// The same two stages entering on `home` alone, without end pages.
fn plain_route() -> Value {
    phased_task(
        json!("home"),
        vec![
            tap("open", "home", json!("panel")),
            tap("back", "panel", json!("home")),
        ],
        vec![
            stage("reach", &["open"], &["panel"]),
            stage("return", &["back"], &["home"]),
        ],
        &[("returned", "home")],
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

// The package of `task`: a `Lab-1y.control.v2` control holding the task's stages, and a
// recognition pack and page set that tell the fixture pages by their colour.
fn package(task: &Value) -> Vec<u8> {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.stage.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 1_000,
        "timeout_ms": 60_000,
        "phases": task["phases"].clone()
    });
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

fn admit(task: &Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task))
}

fn admitted(case: &str, task: &Value) -> PreparedContainedTask {
    admit(task).unwrap_or_else(|error| panic!("{case}: admission refused the package: {error}"))
}

fn succeeded(
    case: &str,
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success, "{case}");
            outcome
        }
        Err(error) => panic!("{case}: expected the run to end Completed, got {error:?}"),
    }
}

// The key of the one mapping of the task's scheduling outcome that holds `page` under no
// designated effect: the key of a run that ends there without a designated effect.
fn mapped_key<'a>(task: &'a PreparedContainedTask, page: &str) -> &'a str {
    let keys = task
        .scheduling_outcome
        .as_ref()
        .expect("a scheduling outcome")
        .mappings()
        .iter()
        .filter(|mapping| {
            mapping.effect() == SchedulingEffectCondition::NoDesignatedEffect
                && mapping
                    .terminal_pages()
                    .iter()
                    .any(|terminal| crate::page_anchor_matches("neutral", page, terminal))
        })
        .map(|mapping| mapping.outcome_key())
        .collect::<Vec<_>>();
    let [key] = keys.as_slice() else {
        panic!("{page}: one mapping, not {keys:?}");
    };
    key
}

fn step_finished(
    step_index: u32,
    operation: &str,
    page: &str,
    phase: (u32, &str, bool),
) -> ContainedTaskTrace {
    ContainedTaskTrace::StepFinished {
        step_index,
        operation_label: operation.to_owned(),
        page_label: page.to_owned(),
        phase: Some(TaskPhaseEvidence {
            index: phase.0,
            id: phase.1.to_owned(),
            completed: phase.2,
        }),
    }
}

fn effects(runtime: &ScriptedRuntime) -> usize {
    runtime
        .traces()
        .iter()
        .filter(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. }))
        .count()
}

// §2.5 point 1: `reach` advances to `collect` on the panel; `claim` confirms `full`, an end page
// of `collect`. The run is Completed on `full` after those two steps, the step's stage not
// completed, with the key that maps `full`; `back` never runs.
#[test]
fn bt_b5_t1_a_stage_that_confirms_its_end_page_ends_the_run_there() {
    let task = admitted("collect route", &collect_route());
    let mut runtime = device(&["home", "panel", "full"]);

    let outcome = succeeded("full", task.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/full"));
    assert_eq!(outcome.executed_steps, 2);
    assert_eq!(runtime.inputs().len(), 2, "no operation after the end page");
    assert!(
        runtime.traces().contains(&step_finished(
            1,
            "claim",
            "neutral/full",
            (1, "collect", false)
        )),
        "{:?}",
        runtime.traces()
    );
    assert!(
        !runtime.traces().iter().any(|trace| matches!(
            trace,
            ContainedTaskTrace::StepStarted { operation_label, .. } if operation_label == "back"
        )),
        "{:?}",
        runtime.traces()
    );
    assert_eq!(mapped_key(&task, "neutral/full"), "collect_blocked");
}

// §2.5 point 2 (B14): the run enters on the result page, which only `return`'s `entry_pages`
// names (its one operation leaves any page). The run starts at `return`: its one step taps from
// the result page home and completes that stage, and the run is Completed at home.
#[test]
fn bt_b5_t3_an_entry_page_a_stage_lists_starts_that_stage() {
    let task = admitted("resume route", &resume_route());
    let mut runtime = device(&["result", "home"]);

    let outcome = succeeded("resume", task.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(runtime.inputs().len(), 1);
    assert!(
        runtime.traces().contains(&step_finished(
            0,
            "leave",
            "neutral/home",
            (2, "return", true)
        )),
        "{:?}",
        runtime.traces()
    );
}

// §2.5 point 3 (B20, B38's fixture): the run enters on the notice page, an end page of `reach`.
// It is Completed there before any step: no step, no input, no designated effect, final page the
// notice, whose mapping under no designated effect gives the key. A page that is both an end
// page and an operation's `from` has two starts: admission refuses the entry page set with
// `phase_start_ambiguous` at that page.
#[test]
fn bt_b5_t4_an_entry_page_in_end_pages_ends_the_run_before_any_step() {
    let task = admitted("ending entry route", &ending_entry_route());
    let mut runtime = device(&["notice"]);

    let outcome = succeeded("notice", task.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/notice"));
    assert_eq!(outcome.executed_steps, 0);
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
    assert!(
        !runtime
            .traces()
            .iter()
            .any(|trace| matches!(trace, ContainedTaskTrace::StepStarted { .. })),
        "{:?}",
        runtime.traces()
    );
    assert_eq!(effects(&runtime), 0);
    assert_eq!(mapped_key(&task, "neutral/notice"), "notice_shown");

    let mut ambiguous = ending_entry_route();
    ambiguous["operations"]
        .as_array_mut()
        .expect("operations")
        .push(tap("dismiss", "notice", json!("home")));
    ambiguous["phases"][0]["operations"] = json!(["open", "dismiss"]);
    let error = match admit(&ambiguous) {
        Ok(_) => panic!("an end page that is also an operation's from was admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), ENTRY_PAGES_INVALID, "{error}");
    assert_eq!(
        error.outcome().value("entry_problem").cloned(),
        Some(json!("phase_start_ambiguous")),
        "{error}"
    );
    assert_eq!(
        error.outcome().value("page_id").cloned(),
        Some(json!("neutral/notice")),
        "{error}"
    );
}

// §2.5 point 1 in B24's shape: `reach`'s tap from home reaches the notice, an end page of the
// first stage. The run is Completed on the notice after that one step, with the key that maps
// the notice; nothing else runs.
#[test]
fn bt_b5_t5_the_first_stage_ends_on_its_reached_end_page() {
    let task = admitted("collect route", &collect_route());
    let mut runtime = device(&["home", "notice"]);

    let outcome = succeeded("notice", task.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/notice"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(runtime.inputs().len(), 1, "no operation after the end page");
    assert!(
        runtime.traces().contains(&step_finished(
            0,
            "open",
            "neutral/notice",
            (0, "reach", false)
        )),
        "{:?}",
        runtime.traces()
    );
    assert_eq!(mapped_key(&task, "neutral/notice"), "notice_shown");
}

// §2.5 point 4: a package whose stage lists end pages may end short of its target pages, so it
// is neither a prerequisite package nor a return-home package: reason `phase_end_pages` for both
// uses. A phased package without end pages (and without any other reason) is either.
#[test]
fn bt_b5_t6_a_package_with_end_pages_is_no_prerequisite_or_return_home_layer() {
    let ending = admitted("collect route", &collect_route());
    assert_eq!(
        ending.prerequisite_incompatibility(),
        Some("phase_end_pages")
    );
    assert_eq!(
        ending
            .package_descriptor()
            .use_incompatibility(MaintenanceUse::ReturnHome),
        Some("phase_end_pages")
    );

    let without = admitted("plain route", &plain_route());
    assert_eq!(without.prerequisite_incompatibility(), None);
    assert_eq!(
        without
            .package_descriptor()
            .use_incompatibility(MaintenanceUse::ReturnHome),
        None
    );
}
