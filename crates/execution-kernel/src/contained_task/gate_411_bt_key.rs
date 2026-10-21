// SPDX-License-Identifier: AGPL-3.0-only

// #411 B16 part 1, B22 (+ B33), slice BT-KEY (0.12.x B train; B-train plan §2.11, test rows T1,
// T2 and T4 to T7 of §5.3; register §1.164; exec keys 152 to 157): an operation may declare a
// per-run cap (`max_runs` with `on_max_runs`), and a guard refusal may end the run on a named key
// (the keyed `on_guard_refused`).
//
// - T1 (§2.11 items 1 and 5): `max_runs 3` with `on_max_runs {outcome_key}` on an operation that
//   stays on its page: after three dispatches the run is Completed on the current page with the
//   key, and its one `task.operation_capped` is exactly the LR-7 shape (register §1.153).
// - T2 (item 1): `on_max_runs: next_candidate`: on the fourth time the page's next operation runs.
// - T4 (item 2, B22): the guard's value test is not met and the operation declares a keyed
//   `on_guard_refused`: the step ends without an input, and the run is Completed with the key, no
//   failure.
// - T5 (item 2): the value test unread (`value_test_unresolved`) keeps the failure,
//   `contained_task_guard_refused`.
// - T6 (item 2): `failed` names the member `flag/a` of the guard's composite; the member
//   `flag/b` failing alone keeps the failure.
// - T7 (item 4): a package with a keyed cap or a keyed guard refusal is no prerequisite or
//   return-home layer, reasons `max_runs_end` and `guard_refused_end`.
//
// Each package is a synthetic `neutral` package admitted through
// `PreparedContainedTask::load_with_vision_provider`: a 4 x 1 frame whose pixel 0 names the page
// (`page_a` or `page_b`), pixels 1 and 3 show the flags `flag/a` and `flag/b` (ready or busy), and
// pixel 2 the code of the count the OCR provider reads. The value test `check/stock` holds when
// that count is at least 3. The runs play scripted frames on the scripted clock (EXEC-I4): the
// device shows `screens[n]` after `n` inputs.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{MaintenanceUse, TaskSemanticFact};
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrExecutionProviderKind, OcrProviderObservation,
    OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock, VisionProviderError,
    VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use zip::{ZipWriter, write::FileOptions};

const WIDTH: u32 = 4;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAGES: [(&str, [u8; 3]); 2] = [("page_a", [255, 0, 0]), ("page_b", [0, 0, 255])];
const READY: [u8; 3] = [0, 255, 0];
const BUSY: [u8; 3] = [128, 128, 128];
// The text of each count code pixel 2 may show: nothing readable, enough (5), too few (1).
const COUNTS: [&str; 3] = ["", "5", "1"];
const UNREAD: u8 = 0;
const ENOUGH: u8 = 1;
const SHORT: u8 = 2;
const GUARD_REFUSED: &str = "contained_task_guard_refused";

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// The frame of `page` with the flags and the count code.
fn screen(page: &str, flag_a: bool, count: u8, flag_b: bool) -> ScriptedFrame {
    let flag = |ready: bool| if ready { READY } else { BUSY };
    ScriptedFrame::rgb(
        WIDTH,
        1,
        &[colour(page), flag(flag_a), [count, 0, 0], flag(flag_b)],
    )
}

// `page` with both flags ready and enough counted.
fn plain(page: &str) -> ScriptedFrame {
    screen(page, true, ENOUGH, true)
}

// A device that shows `screens[n]` after `n` inputs.
fn device(screens: Vec<ScriptedFrame>) -> ScriptedRuntime {
    ScriptedRuntime::new(screens.into_iter().map(|frame| vec![frame]).collect())
}

// Reads the count whose code the pixel its region starts on shows, at confidence 0.99.
#[derive(Debug, Default)]
struct ShownCount;

impl VisionProvider for ShownCount {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.read_text_with_execution_evidence(request)
            .map(|observation| observation.result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let pixel = usize::try_from(request.region.x).expect("a region on the frame") * 3;
        let text = COUNTS[usize::from(request.frame.rgb8_pixels[pixel])].to_owned();
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: "count-ocr".to_owned(),
                session_id: "count-session".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// `op_a`, a trusted tap that stays on `page_a`, with `extra` members.
fn repeat(extra: Value) -> Value {
    with(
        json!({
            "id": "op_a", "from": "page_a", "to": "page_a",
            "click": {"kind": "point", "x": 0, "y": 0},
            "unguarded_trusted_coordinate": true, "retryable": false
        }),
        extra,
    )
}

// `op_b`, a trusted tap from `page_a` to `page_b`.
fn leave() -> Value {
    json!({
        "id": "op_b", "from": "page_a", "to": "page_b",
        "click": {"kind": "point", "x": 2, "y": 0},
        "unguarded_trusted_coordinate": true, "retryable": false
    })
}

// `op_c`, a click from `page_a` to `page_b` guarded by `target`, with `extra` members.
fn guarded(target: &str, extra: Value) -> Value {
    with(
        json!({
            "id": "op_c", "from": "page_a", "to": "page_b",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {"page_id": "page_a", "target_id": target,
                      "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                      "check": target},
            "retryable": false
        }),
        extra,
    )
}

fn with(mut operation: Value, extra: Value) -> Value {
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    operation
}

// A page-graph task from `page_a` to `page_b`; each mapping `(key, page)` maps the page under
// no designated effect.
fn task(operations: Vec<Value>, mappings: &[(&str, &str)]) -> Value {
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
        "entry_page": "page_a",
        "target_page": "page_b",
        "operations": operations,
        "scheduling_outcome": {"mappings": mappings}
    })
}

// T1's task: `op_a` capped at 3 runs, ending on `repeat_capped`.
fn capped_task() -> Value {
    task(
        vec![repeat(
            json!({"max_runs": 3, "on_max_runs": {"outcome_key": "repeat_capped"}}),
        )],
        &[("repeat_capped", "page_a"), ("finished", "page_b")],
    )
}

// T2's task: `op_a` capped at 3 runs, then `op_b`.
fn next_candidate_task() -> Value {
    task(
        vec![
            repeat(json!({"max_runs": 3, "on_max_runs": "next_candidate"})),
            leave(),
        ],
        &[("finished", "page_b")],
    )
}

// T4 and T5's task: `op_c` guarded by the value test, its refusal keyed to `count_short`.
fn stock_task() -> Value {
    task(
        vec![guarded(
            "check/stock",
            json!({"on_guard_refused": {"outcome_key": "count_short"}}),
        )],
        &[("count_short", "page_a"), ("claimed", "page_b")],
    )
}

// T6's task: `op_c` guarded by the composite `check/ready`, its refusal keyed to `a_busy` when
// the member `flag/a` fails.
fn ready_task() -> Value {
    task(
        vec![guarded(
            "check/ready",
            json!({"on_guard_refused": {"outcome_key": "a_busy", "failed": "flag/a"}}),
        )],
        &[("a_busy", "page_a"), ("claimed", "page_b")],
    )
}

fn pack_json() -> Value {
    let mut targets = PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect::<Vec<_>>();
    targets.extend([
        json!({"type": "color", "id": "flag/a",
               "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": READY}),
        json!({"type": "color", "id": "flag/b",
               "region": {"x": 3, "y": 0, "width": 1, "height": 1}, "expected": READY}),
        json!({"type": "composite", "id": "check/ready", "mode": "all_of",
               "members": ["flag/a", "flag/b"]}),
        json!({
            "type": "ocr", "id": "count/stock",
            "region": {"x": 2, "y": 0, "width": 1, "height": 1},
            "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
            "case_sensitive": false, "minimum_confidence": 0.5,
            "model_ref": "PP-OCRv6_medium", "model_sha256": MODEL_SHA256
        }),
        json!({
            "type": "value_test", "id": "check/stock",
            "left": {"target_id": "count/stock", "trim": "whitespace_v1",
                     "value": {"type": "unsigned_integer", "min": 0, "max": 999}},
            "op": "ge",
            "right": {"const": 3}
        }),
    ]);
    json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": WIDTH, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    })
}

fn pages_json() -> Value {
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            json!({"id": format!("neutral/{page}"), "required": [format!("page/{page}")],
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    json!({"schema_version": "0.3", "pages": pages})
}

fn control() -> Value {
    json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.keyed.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 100,
        "step_timeout_ms": 1_000,
        "timeout_ms": 60_000
    })
}

// The package of `task`, admitted through the declaration gate with the count reader.
fn admitted(case: &str, task: Value) -> PreparedContainedTask {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in [
        ("control.json", control()),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack_json()),
        (
            "resources/recognition/neutral.test.pages.json",
            pages_json(),
        ),
    ] {
        zip.start_file(path, options).expect("zip entry");
        serde_json::to_writer(&mut zip, &value).expect("zip JSON");
        zip.write_all(b"\n").expect("zip newline");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &bytes,
        Arc::new(ShownCount),
    )
    .unwrap_or_else(|error| panic!("{case}: admission refused the package: {error}"))
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

fn failed(
    case: &str,
    result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!(
            "{case}: expected a task failure, the run ended {:?} with {:?}",
            outcome.outcome, outcome.selected_scheduling_outcome
        ),
    }
}

// The operations whose steps started, in order.
fn started(runtime: &ScriptedRuntime) -> Vec<&str> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepStarted {
                operation_label, ..
            } => Some(operation_label.as_str()),
            _ => None,
        })
        .collect()
}

// Every `task.operation_capped` the run wrote, as the ledger's fact (the host moves each member
// of the trace into it unchanged).
fn capped_facts(runtime: &ScriptedRuntime) -> Vec<TaskSemanticFact> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::OperationCapped {
                step_index,
                operation_label,
                count,
                limit_count,
                outcome,
            } => Some(TaskSemanticFact::OperationCapped {
                step_index: *step_index,
                operation_label: operation_label.clone(),
                count: *count,
                limit_count: *limit_count,
                outcome: outcome.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn effect_intents(runtime: &ScriptedRuntime) -> usize {
    runtime
        .traces()
        .iter()
        .filter(|trace| matches!(trace, ContainedTaskTrace::EffectIntent { .. }))
        .count()
}

// §2.11 items 1 and 5: `op_a` stays on `page_a` and is capped at 3 runs with a key. It runs three
// times; when the page would dispatch it a fourth time, no step starts and the run is Completed on
// `page_a` with `repeat_capped`. Its one `task.operation_capped` is the LR-7 shape: the step index
// that dispatch would have taken (3), the operation, its 3 dispatches of at most 3, and the info
// outcome `contained_task_operation_runs_capped` with `pack_operation_id`, `count`,
// `limit_count`, `page_id` and `outcome_key`.
#[test]
fn bt_key_t1_a_keyed_cap_ends_the_run_on_its_page_and_writes_the_capped_record() {
    let task = admitted("keyed cap", capped_task());
    let mut runtime = device(vec![plain("page_a")]);

    let outcome = succeeded("keyed cap", task.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/page_a"));
    assert_eq!(outcome.executed_steps, 3);
    assert_eq!(
        outcome.selected_scheduling_outcome.as_deref(),
        Some("repeat_capped")
    );
    assert_eq!(runtime.inputs().len(), 3);
    assert_eq!(started(&runtime), ["op_a", "op_a", "op_a"]);
    let facts = capped_facts(&runtime);
    let [fact] = facts.as_slice() else {
        panic!("one task.operation_capped, got {facts:?}");
    };
    assert_eq!(
        serde_json::to_value(fact).expect("the fact encodes"),
        json!({
            "kind": "operation_capped",
            "step_index": 3,
            "operation_label": "op_a",
            "count": 3,
            "limit_count": 3,
            "outcome": {
                "code": "contained_task_operation_runs_capped",
                "category": "info",
                "values": {
                    "pack_operation_id": "op_a",
                    "count": 3,
                    "limit_count": 3,
                    "page_id": "neutral/page_a",
                    "outcome_key": "repeat_capped"
                }
            }
        })
    );
}

// §2.11 item 1: `op_a` is capped at 3 runs with `next_candidate`. It runs three times; on the
// fourth time the page's next operation, `op_b`, runs and the run is Completed on `page_b`.
#[test]
fn bt_key_t2_a_capped_operation_with_next_candidate_gives_way_to_the_next_operation() {
    let task = admitted("next candidate", next_candidate_task());
    let mut runtime = device(vec![
        plain("page_a"),
        plain("page_a"),
        plain("page_a"),
        plain("page_a"),
        plain("page_b"),
    ]);

    let outcome = succeeded("next candidate", task.run(&mut runtime));

    assert_eq!(started(&runtime), ["op_a", "op_a", "op_a", "op_b"]);
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/page_b"));
    assert_eq!(outcome.executed_steps, 4);
    assert_eq!(runtime.inputs().len(), 4);
}

// §2.11 item 2 (B22): the count on `page_a` is 1, so the guard's value test `count >= 3` is read
// and not met (`value_test_not_met`), and `op_c` declares `on_guard_refused: {"outcome_key":
// "count_short"}`. The step finishes on `page_a` without an input (no effect intent), and the run
// is Completed there with `count_short` after that one step; no failure is counted.
#[test]
fn bt_key_t4_a_guard_value_test_not_met_ends_the_run_on_the_keyed_refusal() {
    let task = admitted("keyed refusal", stock_task());
    let mut runtime = device(vec![screen("page_a", true, SHORT, true)]);

    let outcome = succeeded("count not met", task.run(&mut runtime));

    assert_eq!(outcome.final_page.as_deref(), Some("neutral/page_a"));
    assert_eq!(outcome.executed_steps, 1);
    assert_eq!(
        outcome.selected_scheduling_outcome.as_deref(),
        Some("count_short")
    );
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
    assert_eq!(effect_intents(&runtime), 0);
    assert!(
        runtime
            .traces()
            .contains(&ContainedTaskTrace::StepFinished {
                step_index: 0,
                operation_label: "op_c".to_owned(),
                page_label: "neutral/page_a".to_owned(),
                phase: None,
            }),
        "{:?}",
        runtime.traces()
    );
}

// §2.11 item 2: the count on `page_a` cannot be read, so the value test is unresolved
// (`value_test_unresolved`), not a definite fail. The keyed `on_guard_refused` does not apply:
// the run fails with `contained_task_guard_refused`, without an input.
#[test]
fn bt_key_t5_an_unread_value_test_keeps_the_guard_refusal_a_failure() {
    let task = admitted("keyed refusal", stock_task());
    let mut runtime = device(vec![screen("page_a", true, UNREAD, true)]);

    let error = failed("count unread", task.run(&mut runtime));

    assert_eq!(error.code().as_str(), GUARD_REFUSED, "{error}");
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
}

// §2.11 item 2: `op_c`'s keyed refusal names the member `flag/a` of its guard `check/ready`
// (all of `flag/a` and `flag/b`). On a frame where `flag/a` is ready and `flag/b` busy, the guard
// refuses because of `flag/b` alone: the run fails with `contained_task_guard_refused`, without an
// input.
#[test]
fn bt_key_t6_another_member_failing_alone_keeps_the_guard_refusal_a_failure() {
    let task = admitted("named member", ready_task());
    let mut runtime = device(vec![screen("page_a", true, ENOUGH, false)]);

    let error = failed("flag/b busy", task.run(&mut runtime));

    assert_eq!(error.code().as_str(), GUARD_REFUSED, "{error}");
    assert!(runtime.inputs().is_empty(), "{:?}", runtime.inputs());
}

// §2.11 item 4: a keyed end stops the run on the operation's page, short of its target pages, so
// a package with a keyed `on_max_runs` is neither a prerequisite package nor a return-home package
// (`max_runs_end`), nor is one with a keyed `on_guard_refused` (`guard_refused_end`).
#[test]
fn bt_key_t7_a_package_with_a_keyed_end_is_no_prerequisite_or_return_home_layer() {
    for (case, task, reason) in [
        ("keyed cap", capped_task(), "max_runs_end"),
        ("keyed refusal", stock_task(), "guard_refused_end"),
    ] {
        let package = admitted(case, task);
        assert_eq!(
            package.prerequisite_incompatibility(),
            Some(reason),
            "{case}"
        );
        assert_eq!(
            package
                .package_descriptor()
                .use_incompatibility(MaintenanceUse::ReturnHome),
            Some(reason),
            "{case}"
        );
    }
}
