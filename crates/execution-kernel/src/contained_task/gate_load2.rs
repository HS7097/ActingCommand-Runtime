// SPDX-License-Identifier: AGPL-3.0-only

// LOAD-2 (Workflow #407, register §1.258; design `rd-1031/check-speed/model-loading.md` §2.4, §5
// and §7 LOAD-2; coordinator brief `brief-load-2.md`): an operation's `expect_after.loading` is
// admitted against the package's loading pages, in a page graph and in a step plan.
//
// - `"off"` beside a wait page is refused (`loading_conflict`), and so is a `loading.max_ms`
//   beside a wait page's `max_ms`; a `max_ms` outside 1..=1800000 is `wait_max_invalid`.
// - An `except` entry names a loading page the package inherits (`loading_except_unknown`).
// - An `add` entry is one page of the package, and in a page graph not the operation's `from`, a
//   destination or an error page (`loading_page_invalid`).
// - A step plan admits `loading` and a wait page, refuses a wait step (`reason=wait`) and refuses
//   a page its loading adds, or its wait page, that is its own `from`, destination or transition
//   (`reason=loading_page`).
//
// Each package is a synthetic `neutral` assembled package admitted through
// `PreparedContainedTask::load`: a 2 x 1 frame whose left pixel names the page (`page_a` to
// `page_d`) and whose right white pixel is the target `loading/spinner` of the loading page
// `transition`, which its page set carries beside its pages (`loading`, `max_ms` 30000). Nothing
// runs: admission only.

use super::*;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 2;
const PAGES: [(&str, [u8; 3]); 4] = [
    ("page_a", [255, 0, 0]),
    ("page_b", [0, 255, 0]),
    ("page_c", [0, 0, 255]),
    ("page_d", [255, 255, 0]),
];
const PROGRAM_INVALID: &str = "contained_task_program_invalid";
const LINEAR_INVALID: &str = "contained_task_linear_invalid";
const TASK_FILE: &str = "resources/operations/task/task.json";

// A trusted tap `id` from `from` confirmed on `to`, its `expect_after` with `extra` members.
fn tap(id: &str, from: &str, to: &str, extra: Value) -> Value {
    let mut expect_after = json!({"page_id": to, "timeout_ms": 2_000, "interval_ms": 500});
    for (member, value) in extra.as_object().expect("expect_after members") {
        expect_after[member] = value.clone();
    }
    json!({
        "id": id, "from": from,
        "click": {"kind": "point", "x": 1, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false,
        "expect_after": expect_after
    })
}

// A task from `page_a` to `target` of `operations`, with `error_pages`.
fn task(operations: Vec<Value>, target: &str, error_pages: &[&str]) -> Value {
    let mut task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1},
        "entry_page": "page_a",
        "target_page": target,
        "operations": operations
    });
    if !error_pages.is_empty() {
        task["error_pages"] = json!(error_pages);
    }
    task
}

// The page-graph task of the one tap `op_tap` from `page_a` to `page_b`, its `expect_after` with
// `extra`, and the error page `page_c`.
fn page_graph(extra: Value) -> Value {
    task(
        vec![tap("op_tap", "page_a", "page_b", extra)],
        "page_b",
        &["page_c"],
    )
}

// The step plan `page_a` -> `page_b` -> `page_c`, its two taps' `expect_after` with `first` and
// `second`.
fn step_plan(first: Value, second: Value) -> Value {
    let mut plan = task(
        vec![
            tap("op_1", "page_a", "page_b", first),
            tap("op_2", "page_b", "page_c", second),
        ],
        "page_c",
        &[],
    );
    plan["max_steps"] = json!(2);
    plan
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

// The assembled package of `task` under the execution mode `mode`, its page set carrying the
// loading page `transition`.
fn package(task: &Value, mode: &str) -> Vec<u8> {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.loading.task",
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1_000,
        "step_timeout_ms": 3_000,
        "timeout_ms": 30_000
    });
    if let Some(max_steps) = task.get("max_steps") {
        control["max_steps"] = max_steps.clone();
    }
    let mut targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    targets.push(json!({"type": "color", "id": "loading/spinner",
                        "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                        "expected": [255, 255, 255]}));
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
    let page_set = json!({
        "schema_version": "0.3",
        "pages": pages,
        "loading": {"max_ms": 30_000, "pages": [
            {"id": "neutral/transition", "required": ["loading/spinner"],
             "optional": [], "forbidden": []}
        ]}
    });
    zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (TASK_FILE, bytes(task)),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&page_set),
        ),
    ])
}

fn admit(task: &Value, mode: &str) -> Result<PreparedContainedTask, ContainedTaskError> {
    PreparedContainedTask::load("neutral.instance", &package(task, mode))
}

fn admitted(case: &str, task: &Value, mode: &str) -> PreparedContainedTask {
    admit(task, mode).unwrap_or_else(|error| panic!("{case}: admission refused: {error}"))
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// `task` is refused `contained_task_program_invalid` with `problem` at `pointer` in its task file.
fn refused_with(case: &str, task: &Value, mode: &str, problem: &str, pointer: &str) {
    let error = match admit(task, mode) {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), PROGRAM_INVALID, "{case}: {error}");
    assert_eq!(value(&error, "program_problem"), json!(problem), "{case}");
    assert_eq!(value(&error, "json_pointer"), json!(pointer), "{case}");
    assert_eq!(value(&error, "path"), json!(TASK_FILE), "{case}");
}

// `task`, a step plan, is refused `contained_task_linear_invalid` with `reason`.
fn linear_refused_with(case: &str, task: &Value, reason: &str) {
    let error = match admit(task, "linear_steps") {
        Ok(_) => panic!("{case}: admitted"),
        Err(error) => error,
    };
    assert_eq!(error.code().as_str(), LINEAR_INVALID, "{case}: {error}");
    let expected = format!("reason={reason}");
    assert!(
        error
            .detail()
            .is_some_and(|detail| detail.split(' ').any(|token| token == expected)),
        "{case}: {error}"
    );
}

fn wait_page(page: &str) -> Value {
    json!({"page_id": page, "max_ms": 10_000})
}

#[test]
fn operation_loading_off_with_wait_page_is_refused() {
    let prepared = admitted(
        "off alone",
        &page_graph(json!({"loading": "off"})),
        "navigable_route",
    );
    assert_eq!(
        prepared.loading_set(),
        Some(crate::PackageLoadingSet {
            pages: vec!["transition".to_owned()],
            max_ms: 30_000
        })
    );
    admitted(
        "a wait page alone",
        &page_graph(json!({"wait_page": wait_page("page_d")})),
        "navigable_route",
    );
    refused_with(
        "off beside a wait page",
        &page_graph(json!({"loading": "off", "wait_page": wait_page("page_d")})),
        "navigable_route",
        "loading_conflict",
        "/operations/0/expect_after/loading",
    );
}

#[test]
fn operation_loading_except_must_name_an_inherited_page() {
    admitted(
        "an inherited page",
        &page_graph(json!({"loading": {"except": ["transition"]}})),
        "navigable_route",
    );
    for (case, except, pointer) in [
        (
            "a page of the package, no loading page",
            json!(["page_d"]),
            "/operations/0/expect_after/loading/except/0",
        ),
        (
            "a page of nothing",
            json!(["nowhere"]),
            "/operations/0/expect_after/loading/except/0",
        ),
        (
            "an inherited page twice",
            json!(["transition", "transition"]),
            "/operations/0/expect_after/loading/except/1",
        ),
    ] {
        refused_with(
            case,
            &page_graph(json!({"loading": {"except": except}})),
            "navigable_route",
            "loading_except_unknown",
            pointer,
        );
    }
}

#[test]
fn operation_loading_add_refuses_from_destination_and_error_pages() {
    admitted(
        "another page of the package",
        &page_graph(json!({"loading": {"add": ["page_d"]}})),
        "navigable_route",
    );
    for (case, add, pointer) in [
        (
            "the operation's from page",
            json!(["page_a"]),
            "/operations/0/expect_after/loading/add/0",
        ),
        (
            "its destination",
            json!(["page_d", "page_b"]),
            "/operations/0/expect_after/loading/add/1",
        ),
        (
            "an error page",
            json!(["page_c"]),
            "/operations/0/expect_after/loading/add/0",
        ),
        (
            "a loading page, no page of the package",
            json!(["transition"]),
            "/operations/0/expect_after/loading/add/0",
        ),
        (
            "a page added twice",
            json!(["page_d", "page_d"]),
            "/operations/0/expect_after/loading/add/1",
        ),
    ] {
        refused_with(
            case,
            &page_graph(json!({"loading": {"add": add}})),
            "navigable_route",
            "loading_page_invalid",
            pointer,
        );
    }
}

#[test]
fn operation_loading_max_and_wait_page_max_together_are_refused() {
    admitted(
        "an allowance of its own",
        &page_graph(json!({"loading": {"max_ms": 300_000}})),
        "navigable_route",
    );
    admitted(
        "pages added beside a wait page",
        &page_graph(json!({"loading": {"add": ["page_d"]}, "wait_page": wait_page("page_d")})),
        "navigable_route",
    );
    refused_with(
        "two allowances",
        &page_graph(json!({"loading": {"max_ms": 300_000}, "wait_page": wait_page("page_d")})),
        "navigable_route",
        "loading_conflict",
        "/operations/0/expect_after/loading/max_ms",
    );
    for max_ms in [0, 1_800_001] {
        refused_with(
            &format!("max_ms {max_ms}"),
            &page_graph(json!({"loading": {"max_ms": max_ms}})),
            "navigable_route",
            "wait_max_invalid",
            "/operations/0/expect_after/loading/max_ms",
        );
    }
}

#[test]
fn linear_admits_loading_and_wait_page_and_refuses_wait_steps() {
    admitted(
        "a step plan with loading and a wait page",
        &step_plan(
            json!({"loading": {"except": ["transition"], "add": ["page_d"], "max_ms": 60_000}}),
            json!({"wait_page": wait_page("page_d")}),
        ),
        "linear_steps",
    );
    admitted(
        "a step plan with loading off",
        &step_plan(json!({"loading": "off"}), json!({})),
        "linear_steps",
    );
    linear_refused_with(
        "a page its loading adds is its own from page",
        &step_plan(json!({"loading": {"add": ["page_a"]}}), json!({})),
        "loading_page",
    );
    linear_refused_with(
        "its wait page is its destination",
        &step_plan(json!({}), json!({"wait_page": wait_page("page_c")})),
        "loading_page",
    );
    refused_with(
        "an except of no inherited page",
        &step_plan(json!({"loading": {"except": ["page_d"]}}), json!({})),
        "linear_steps",
        "loading_except_unknown",
        "/operations/0/expect_after/loading/except/0",
    );
    let mut waiting = step_plan(json!({}), json!({}));
    waiting["operations"][1] = json!({
        "id": "op_2", "from": "page_b", "wait": {"max_ms": 5_000},
        "expect_after": {"page_id": "page_c", "timeout_ms": 2_000, "interval_ms": 500}
    });
    linear_refused_with("a wait step", &waiting, "wait");
}
