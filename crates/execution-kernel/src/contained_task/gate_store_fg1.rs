// SPDX-License-Identifier: AGPL-3.0-only

// STORE-FG-1 (0.12.29; design `rd-1029/design-store-fg.md` §2, §3, §5 rows T11 to T13 and §8;
// register §1.205): a package's `control.json` may declare `store_update`, the steps it runs
// while the store application it names is in front.
//
// - T11: admission refuses each broken shape of the member (`contained_task_control_invalid`,
//   `reason=store_update_invalid`), a store update package of another execution mode, and each
//   broken cross-check (`contained_task_program_invalid` with its `program_problem`, the path
//   `control.json` and the refused member); a valid declaration is admitted.
// - T12: a store update package is no prerequisite, return-home, navigation or interrupt layer
//   (`store_update`, the prerequisite incompatibility every layer check reads) and no startup
//   package or ladder rung; run as a layer it is refused before any capture.
// - T13: in the package's own run, a declared store step's tap goes through `store_input` with
//   the declared store, every other input through `input`, and the run asks `admit_store_foreground`
//   once, before any capture; a layer, a recovery entry and a package without `store_update`
//   never ask and never route.
// - §8.3: a store run waits long on the updating page with a wait step, which makes no input;
//   the tap's own confirmation may hold on that page instead (`expect_after.wait_page`).
//
// A synthetic `neutral` game, server `test`, 2 x 1: a frame's left pixel names its page. The
// store is `test.store.alpha`, a made-up id. The admission fixtures are packages admitted through
// `PreparedContainedTask::load_with_vision_provider` with an OCR provider that only checks models
// (the listing page requires the OCR target `proof/title`, the page `other` the OCR target
// `proof/other`). The run fixtures are prepared from the same declarations without admission, their
// pages told by colour alone, so that no OCR provider reads a frame; the scripted device shows
// `screens[n]` after `n` effects (inputs and application effects) on the scripted clock.

use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::MaintenanceUse;
use actingcommand_recognition_pack::{
    NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult, RecognitionPack,
    VisionProviderError, VisionProviderErrorCode,
};
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::path::PathBuf;
use zip::{ZipWriter, write::FileOptions};

const STORE: &str = "test.store.alpha";
const PACKAGE_ID: &str = "neutral.store.update";
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTROL_INVALID: &str = "contained_task_control_invalid";
const PROGRAM_INVALID: &str = "contained_task_program_invalid";
const PREREQUISITE_INCOMPATIBLE: &str = "contained_task_prerequisite_incompatible";
const TASK_FILE: &str = "resources/operations/task/task.json";
const PAGES: [(&str, [u8; 3]); 6] = [
    ("dialog", [255, 0, 0]),
    ("listing", [0, 255, 0]),
    ("updating", [0, 0, 255]),
    ("done", [255, 255, 0]),
    ("home", [255, 255, 255]),
    ("other", [0, 255, 255]),
];

fn colour(page: &str) -> [u8; 3] {
    PAGES
        .iter()
        .find(|(name, _)| *name == page)
        .map(|(_, colour)| *colour)
        .unwrap_or_else(|| panic!("unknown fixture page {page}"))
}

// The frame of `page`: its colour on the left pixel, black on the right.
fn screen(page: &str) -> ScriptedFrame {
    ScriptedFrame::rgb(2, 1, &[colour(page), [0, 0, 0]])
}

// A device that shows the frames `segments[n]` after `n` effects, one per capture, and performs
// the application effect.
fn device(segments: &[&[&str]]) -> ScriptedRuntime {
    ScriptedRuntime::new(
        segments
            .iter()
            .map(|pages| pages.iter().map(|page| screen(page)).collect())
            .collect(),
    )
    .with_application_effect()
}

fn with(mut operation: Value, extra: Value) -> Value {
    for (member, value) in extra.as_object().expect("extra members") {
        operation[member] = value.clone();
    }
    operation
}

// A trusted tap from `from` to `to`.
fn tap(id: &str, from: &str, to: &str) -> Value {
    json!({"id": id, "from": from, "to": to, "click": {"kind": "point", "x": 1, "y": 0},
           "unguarded_trusted_coordinate": true, "retryable": false})
}

// §8.3: the wait on the updating page for `done`, with no input, up to the longest a wait takes,
// a frame every 30 s; a frame of neither page runs a 60 s away clock.
fn wait_download() -> Value {
    json!({"id": "wait_download", "from": "updating", "wait": {"max_ms": 1_800_000},
           "expect_after": {"page_id": "done", "timeout_ms": 60_000, "interval_ms": 30_000}})
}

// The way back (design §2.5): the assigned application restarted, back on its home page.
fn restart() -> Value {
    json!({"id": "restart", "from": "done", "to": "home", "application": {"action": "restart"}})
}

// The store run of the design's resource-side sketch: the game's dialog opens the store, the
// store step taps update on the listing, the run waits while the store updates, then restarts
// the game.
fn store_run() -> Vec<Value> {
    vec![
        tap("open_store", "dialog", "listing"),
        tap("tap_update", "listing", "updating"),
        wait_download(),
        restart(),
    ]
}

fn declaration(store_operations: Value) -> Value {
    json!({"store_application_id": STORE, "store_operations": store_operations})
}

// The one store step `tap_update`, proved by `proof`.
fn tap_update(proof: &[&str]) -> Value {
    declaration(json!([{"operation": "tap_update", "proof": proof}]))
}

fn control(mode: &str, store_update: Option<Value>) -> Value {
    let mut control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE_ID,
        "execution_mode": mode,
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1_000,
        "step_timeout_ms": 3_000,
        "timeout_ms": 1_800_000,
        "max_steps": 8
    });
    if let Some(store_update) = store_update {
        control["store_update"] = store_update;
    }
    control
}

// A page-graph task from `dialog` to `home`.
fn task(operations: Vec<Value>) -> Value {
    json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "dialog",
        "target_page": "home",
        "operations": operations
    })
}

fn colour_targets() -> Vec<Value> {
    PAGES
        .iter()
        .map(|(page, colour)| {
            json!({"type": "color", "id": format!("page/{page}"),
                   "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": colour})
        })
        .collect()
}

// An OCR target of the whole frame; §8.2: `expected` is a list matched with `contains`.
fn ocr(id: &str) -> Value {
    json!({"type": "ocr", "id": id, "region": {"x": 0, "y": 0, "width": 2, "height": 1},
           "languages": ["en"], "timeout_ms": 1000, "match_mode": "contains",
           "expected": ["Neutral listing", "Neutral store page"], "case_sensitive": false,
           "minimum_confidence": 0.8, "model_ref": "PP-OCRv6_medium",
           "model_sha256": MODEL_SHA256})
}

// The page set: each page requires its colour; with `proof`, the listing page also requires
// `proof/title` and the page `other` `proof/other`.
fn pages(proof: bool) -> Value {
    let pages = PAGES
        .iter()
        .map(|(page, _)| {
            let mut required = vec![format!("page/{page}")];
            if proof && *page == "listing" {
                required.push("proof/title".to_owned());
            }
            if proof && *page == "other" {
                required.push("proof/other".to_owned());
            }
            json!({"id": format!("neutral/{page}"), "required": required,
                   "optional": [], "forbidden": []})
        })
        .collect::<Vec<_>>();
    json!({"schema_version": "0.3", "pages": pages})
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

// Checks OCR models and reads nothing: the admission fixtures are never run.
#[derive(Debug)]
struct ModelsOnly;

impl VisionProvider for ModelsOnly {
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
        _request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "the admission fixtures read no frame",
        ))
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

// The package of `control` and `task`, its recognition pack holding the colour of every page and
// the OCR targets `proof/title` and `proof/other`, admitted.
fn admit(control: Value, task: Value) -> Result<PreparedContainedTask, ContainedTaskError> {
    let mut targets = colour_targets();
    targets.push(ocr("proof/title"));
    targets.push(ocr("proof/other"));
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": targets
    });
    let package = zip_entries(&[
        ("control.json", bytes(&control)),
        (
            "resources/manifest.json",
            bytes(&json!({"schema_version": "0.3", "entry_task_id": "task"})),
        ),
        (TASK_FILE, bytes(&task)),
        ("resources/recognition/neutral.test.pack.json", bytes(&pack)),
        (
            "resources/recognition/neutral.test.pages.json",
            bytes(&pages(true)),
        ),
    ]);
    PreparedContainedTask::load_with_vision_provider(
        "neutral.instance",
        &package,
        Arc::new(ModelsOnly),
    )
}

fn admitted(case: &str, control: Value, task: Value) -> PreparedContainedTask {
    admit(control, task)
        .unwrap_or_else(|error| panic!("{case}: admission refused the package: {error}"))
}

// The run fixture: `control` and `task` prepared without admission (see the header), the pages
// told by colour alone.
fn prepared(control: Value, task: Value) -> PreparedContainedTask {
    let control = parse_task_control_declaration(control).expect("control declaration");
    let program: TaskProgram = serde_json::from_value(task).expect("task program");
    let pack: RecognitionPack = serde_json::from_value(json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": colour_targets()
    }))
    .expect("recognition pack");
    let evaluator = RecognitionEvaluator::new(PathBuf::new(), pack).expect("recognition evaluator");
    let page_set: PageSet = serde_json::from_value(pages(false)).expect("page set");
    let detector = PageDetector::new(page_set).expect("page detector");
    detector
        .validate(&evaluator)
        .expect("page detector targets");
    let scheduling_outcome = program.scheduling_outcome.clone();
    PreparedContainedTask {
        control,
        program,
        evaluator,
        detector,
        entry_page: None,
        scheduling_outcome,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

type RunResult = Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>;

fn succeeded(case: &str, result: RunResult) -> ContainedTaskOutcome {
    match result {
        Ok(outcome) => {
            assert_eq!(outcome.outcome, TaskOutcome::Success, "{case}");
            outcome
        }
        Err(error) => panic!("{case}: expected the run to end Completed, got {error:?}"),
    }
}

fn failed(case: &str, result: RunResult) -> ContainedTaskError {
    match result {
        Err(ContainedTaskRunError::Task(error)) => error,
        Err(other) => panic!("{case}: expected a task failure, got {other:?}"),
        Ok(outcome) => panic!("{case}: expected a task failure, the run ended {outcome:?}"),
    }
}

fn value(error: &ContainedTaskError, key: &str) -> Value {
    error.outcome().value(key).cloned().unwrap_or(Value::Null)
}

// A refusal's code, `program_problem`, `path` and `json_pointer`.
fn program_refusal_of(error: &ContainedTaskError) -> (String, Value, Value, Value) {
    (
        error.code().as_str().to_owned(),
        value(error, "program_problem"),
        value(error, "path"),
        value(error, "json_pointer"),
    )
}

// The pages the steps of `operation` finished on, in order.
fn finished_on<'r>(runtime: &'r ScriptedRuntime, operation: &str) -> Vec<&'r str> {
    runtime
        .traces()
        .iter()
        .filter_map(|trace| match trace {
            ContainedTaskTrace::StepFinished {
                operation_label,
                page_label,
                ..
            } if operation_label == operation => Some(page_label.as_str()),
            _ => None,
        })
        .collect()
}

// The effect records (`EffectIntent`, `EffectCompleted`) of `operation`'s steps.
fn effect_records(runtime: &ScriptedRuntime, operation: &str) -> usize {
    runtime
        .traces()
        .iter()
        .filter(|trace| match trace {
            ContainedTaskTrace::EffectIntent {
                operation_label, ..
            }
            | ContainedTaskTrace::EffectCompleted {
                operation_label, ..
            } => operation_label == operation,
            _ => false,
        })
        .count()
}

// The stores the taps routed to a store went to, in order.
fn store_taps(runtime: &ScriptedRuntime) -> Vec<&str> {
    runtime
        .store_inputs()
        .iter()
        .map(|(action, store)| {
            assert!(
                matches!(action, InputAction::Tap { .. }),
                "a store input is a tap: {action:?}"
            );
            store.as_str()
        })
        .collect()
}

// T11 (design §3, `reason=store_update_invalid`): each broken shape of the member, and the member
// in a package that is neither a page graph nor a step plan, refuses the control declaration with
// `contained_task_control_invalid` and that reason; the member in a page graph or a step plan
// passes.
#[test]
fn store_fg1_t11_a_broken_store_update_member_is_refused_with_its_reason() {
    let proved = |operation: &str| json!({"operation": operation, "proof": ["proof/title"]});
    let nine = Value::Array(
        (0..9)
            .map(|index| proved(&format!("tap_{index}")))
            .collect(),
    );
    let cases = [
        (
            "a member the declaration does not have",
            json!({"store_application_id": STORE, "store_operations": [proved("tap_update")],
                   "store_label": "alpha"}),
        ),
        (
            "no store application id",
            json!({"store_operations": [proved("tap_update")]}),
        ),
        (
            "a store application id that is no string",
            json!({"store_application_id": 7, "store_operations": [proved("tap_update")]}),
        ),
        (
            "a store application id of one segment",
            json!({"store_application_id": "store", "store_operations": [proved("tap_update")]}),
        ),
        (
            "a store application id with a dash",
            json!({"store_application_id": "test.store-alpha",
                   "store_operations": [proved("tap_update")]}),
        ),
        ("no store operation", declaration(json!([]))),
        ("nine store operations", declaration(nine)),
        (
            "an operation declared twice",
            declaration(json!([proved("tap_update"), proved("tap_update")])),
        ),
        (
            "a store operation with a member it does not have",
            declaration(json!([{"operation": "tap_update", "proof": ["proof/title"],
                                 "click": "point"}])),
        ),
        (
            "a store operation without proof",
            declaration(json!([{"operation": "tap_update", "proof": []}])),
        ),
        (
            "five proof targets",
            declaration(json!([{"operation": "tap_update",
                                 "proof": ["t_1", "t_2", "t_3", "t_4", "t_5"]}])),
        ),
        (
            "a proof target named twice",
            declaration(json!([{"operation": "tap_update",
                                 "proof": ["proof/title", "proof/title"]}])),
        ),
        (
            "a blank proof target",
            declaration(json!([{"operation": "tap_update", "proof": [" "]}])),
        ),
    ];
    for (case, store_update) in cases {
        let error = validate_control_declaration(control("navigable_route", Some(store_update)))
            .expect_err(case);
        assert_eq!(
            (error.code().as_str(), error.detail()),
            (CONTROL_INVALID, Some("reason=store_update_invalid")),
            "{case}"
        );
    }
    for mode in ["recognize_only", "in_page_guard"] {
        let error = validate_control_declaration(control(mode, Some(tap_update(&["proof/title"]))))
            .expect_err(mode);
        assert_eq!(
            (error.code().as_str(), error.detail()),
            (CONTROL_INVALID, Some("reason=store_update_invalid")),
            "{mode}"
        );
    }
    for mode in ["navigable_route", "linear_steps"] {
        validate_control_declaration(control(mode, Some(tap_update(&["proof/title"]))))
            .unwrap_or_else(|error| panic!("{mode}: {error}"));
    }
}

// T11 (design §2.3 to §2.5, §3): admission takes a store update package whose store step taps
// from the listing page that requires its OCR proof, and whose way back is a `restart` or a
// `launch`; it refuses each broken cross-check with `contained_task_program_invalid`, its
// `program_problem`, the path `control.json` and the refused member.
#[test]
fn store_fg1_t11_admission_checks_the_store_steps_their_proof_and_the_way_back() {
    admitted(
        "a store step proved on its page, restarted back",
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(store_run()),
    );
    let launched = vec![
        tap("open_store", "dialog", "listing"),
        tap("tap_update", "listing", "updating"),
        wait_download(),
        with(restart(), json!({"application": {"action": "launch"}})),
    ];
    admitted(
        "a store step proved on its page, launched back",
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(launched),
    );

    let long_press = vec![
        tap("open_store", "dialog", "listing"),
        with(
            tap("tap_update", "listing", "updating"),
            json!({"click": {"kind": "long_press", "x": 1, "y": 0, "duration_ms": 500}}),
        ),
        wait_download(),
        restart(),
    ];
    let stopped = vec![
        tap("open_store", "dialog", "listing"),
        tap("tap_update", "listing", "updating"),
        wait_download(),
        with(restart(), json!({"application": {"action": "stop"}})),
    ];
    let operation =
        |index: usize, member: &str| format!("/store_update/store_operations/{index}/{member}");
    let cases: [(&str, Value, Vec<Value>, &str, String); 11] = [
        (
            "an operation the task does not declare",
            declaration(json!([{"operation": "tap_missing", "proof": ["proof/title"]}])),
            store_run(),
            "store_operation_invalid",
            operation(0, "operation"),
        ),
        (
            "a wait step",
            declaration(json!([{"operation": "wait_download", "proof": ["proof/title"]}])),
            store_run(),
            "store_operation_invalid",
            operation(0, "operation"),
        ),
        (
            "an application step",
            declaration(json!([{"operation": "restart", "proof": ["proof/title"]}])),
            store_run(),
            "store_operation_invalid",
            operation(0, "operation"),
        ),
        (
            "a long press",
            tap_update(&["proof/title"]),
            long_press,
            "store_operation_invalid",
            operation(0, "operation"),
        ),
        (
            "a step whose page does not require the proof",
            declaration(json!([{"operation": "open_store", "proof": ["proof/title"]}])),
            store_run(),
            "store_proof_invalid",
            operation(0, "proof/0"),
        ),
        (
            "a proof that is no OCR target",
            tap_update(&["page/listing"]),
            store_run(),
            "store_proof_invalid",
            operation(0, "proof/0"),
        ),
        (
            "an OCR proof another page requires",
            tap_update(&["proof/other"]),
            store_run(),
            "store_proof_invalid",
            operation(0, "proof/0"),
        ),
        (
            "a proof the pack does not have",
            tap_update(&["proof/none"]),
            store_run(),
            "store_proof_invalid",
            operation(0, "proof/0"),
        ),
        (
            "a second proof target that is not on the page",
            tap_update(&["proof/title", "proof/other"]),
            store_run(),
            "store_proof_invalid",
            operation(0, "proof/1"),
        ),
        (
            "a second store step that is not proved",
            declaration(json!([
                {"operation": "tap_update", "proof": ["proof/title"]},
                {"operation": "open_store", "proof": ["proof/title"]}
            ])),
            store_run(),
            "store_proof_invalid",
            operation(1, "proof/0"),
        ),
        (
            "no launch or restart back",
            tap_update(&["proof/title"]),
            stopped,
            "store_way_back_missing",
            "/store_update".to_owned(),
        ),
    ];
    for (case, store_update, operations, problem, pointer) in cases {
        let error = admit(
            control("navigable_route", Some(store_update)),
            task(operations),
        )
        .err()
        .unwrap_or_else(|| panic!("{case}: admission took the package"));
        assert_eq!(
            program_refusal_of(&error),
            (
                PROGRAM_INVALID.to_owned(),
                Value::from(problem),
                Value::from("control.json"),
                Value::from(pointer.as_str())
            ),
            "{case}: {error}"
        );
    }
}

// T12 (design §2.3): a store update package answers `store_update` to every layer check (the
// prerequisite incompatibility of the prerequisite chain, the return-home package, the navigation
// edge, recovery chain, interrupt handler and `then.run` checks) and to the startup check (startup
// package and ladder rung); run as a prerequisite or a host layer it is refused before any
// capture and asks for no store. The same package without the member answers none.
#[test]
fn store_fg1_t12_a_store_update_package_is_no_layer_and_no_startup_package() {
    let store = admitted(
        "store update package",
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(store_run()),
    );
    assert_eq!(store.prerequisite_incompatibility(), Some("store_update"));
    assert_eq!(store.startup_incompatibility(), Some("store_update"));
    let descriptor = store.package_descriptor();
    for configured_return_home in [false, true] {
        assert_eq!(
            descriptor.prerequisite_incompatibility_with(
                "neutral",
                "test",
                (2, 1),
                configured_return_home
            ),
            Some("store_update"),
            "configured return home {configured_return_home}"
        );
    }
    assert_eq!(
        descriptor.use_incompatibility(MaintenanceUse::ReturnHome),
        Some("store_update")
    );
    assert_eq!(
        descriptor.use_incompatibility(MaintenanceUse::Startup),
        Some("store_update")
    );

    let mut runtime = device(&[&["dialog"]]);
    let error = failed("as a prerequisite", store.run_as_prerequisite(&mut runtime));
    assert_eq!(error.code().as_str(), PREREQUISITE_INCOMPATIBLE, "{error}");
    let mut layer = device(&[&["dialog"]]);
    let error = failed("as a host layer", store.run_as_host_layer(&mut layer));
    assert_eq!(error.code().as_str(), PREREQUISITE_INCOMPATIBLE, "{error}");
    for runtime in [&runtime, &layer] {
        assert_eq!(runtime.captures(), 0);
        assert!(runtime.store_admissions().is_empty());
        assert!(runtime.inputs().is_empty());
    }

    let ordinary = admitted(
        "the same package without store_update",
        control("navigable_route", None),
        task(store_run()),
    );
    assert_eq!(ordinary.prerequisite_incompatibility(), None);
    assert_eq!(ordinary.startup_incompatibility(), None);
}

// T13 (design §2.7): in the package's own run, the store step `tap_update` taps through
// `store_input` with the declared store, the game's dialog tap `open_store` through `input`, and
// the run asks `admit_store_foreground` once; on the page graph and on the step plan alike.
#[test]
fn store_fg1_t13_a_declared_store_step_taps_through_store_input_and_every_other_through_input() {
    let graph = prepared(
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(store_run()),
    );
    let mut runtime = device(&[
        &["dialog"],
        &["listing"],
        &["updating", "updating", "done"],
        &["home"],
    ]);
    let outcome = succeeded("page graph", graph.run(&mut runtime));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/home"));
    assert_eq!(runtime.store_admissions(), [STORE]);
    assert_eq!(store_taps(&runtime), [STORE]);
    assert_eq!(runtime.inputs().len(), 2, "open_store and tap_update");
    assert_eq!(
        runtime.applications(),
        [ApplicationLifecycleAction::Restart]
    );

    let mut linear = control("linear_steps", Some(tap_update(&["proof/title"])));
    linear["max_steps"] = json!(2);
    let steps = prepared(
        linear,
        json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 2, "height": 1},
            "entry_page": "dialog",
            "target_page": "updating",
            "max_steps": 2,
            "operations": [
                {"id": "open_store", "from": "dialog", "to": "listing",
                 "click": {"kind": "point", "x": 1, "y": 0}, "unguarded_trusted_coordinate": true},
                {"id": "tap_update", "from": "listing", "to": "updating",
                 "click": {"kind": "point", "x": 1, "y": 0}, "unguarded_trusted_coordinate": true}
            ]
        }),
    );
    let mut runtime = device(&[&["dialog"], &["listing"], &["updating"]]);
    let outcome = succeeded("step plan", steps.run(&mut runtime));
    assert_eq!(outcome.final_page.as_deref(), Some("neutral/updating"));
    assert_eq!(runtime.store_admissions(), [STORE]);
    assert_eq!(store_taps(&runtime), [STORE]);
    assert_eq!(runtime.inputs().len(), 2, "open_store and tap_update");
}

// T13 (design §2.3, §2.7): only the package's own run asks for the store and routes to it. Its
// recovery entry run (a bound recovery package, entered as a layer) asks for none and taps
// through `input` alone, as does a run of the same package without `store_update`; a runtime that
// refuses the store ends the run before any capture or input.
#[test]
fn store_fg1_t13_only_the_package_s_own_run_asks_for_the_store_and_routes_to_it() {
    let screens: [&[&str]; 4] = [&["dialog"], &["listing"], &["updating", "done"], &["home"]];
    let store = prepared(
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(store_run()),
    );
    let mut runtime = device(&screens);
    succeeded("recovery entry", store.run_entry_recovery(&mut runtime));
    assert!(runtime.store_admissions().is_empty());
    assert!(runtime.store_inputs().is_empty());
    assert_eq!(runtime.inputs().len(), 2);

    let ordinary = prepared(control("navigable_route", None), task(store_run()));
    let mut runtime = device(&screens);
    succeeded("without store_update", ordinary.run(&mut runtime));
    assert!(runtime.store_admissions().is_empty());
    assert!(runtime.store_inputs().is_empty());
    assert_eq!(runtime.inputs().len(), 2);

    let mut runtime = device(&screens).refusing_store("store not admitted");
    match store.run(&mut runtime) {
        Err(ContainedTaskRunError::Boundary(error)) => assert_eq!(error, "store not admitted"),
        other => panic!("expected the runtime's refusal, got {other:?}"),
    }
    assert_eq!(runtime.store_admissions(), [STORE]);
    assert_eq!(runtime.captures(), 0);
    assert!(runtime.inputs().is_empty());
}

// §8.3 (the resource side's field note 3): after the store tap, the run waits on the updating
// page with a wait step, which makes no input, a frame every 30 s for as long as the page shows
// (up to `max_ms` 1 800 000, the longest a wait takes); the tap's own confirmation may instead
// hold on that page (`expect_after.wait_page`, `max_ms` up to 1 800 000). Both are admitted in a
// store update package, and the run makes no input between the store tap and the restart.
#[test]
fn store_fg1_s8_3_a_store_run_waits_long_on_the_updating_page_without_an_input() {
    admitted(
        "a wait step on the updating page",
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(store_run()),
    );
    let held = vec![
        tap("open_store", "dialog", "listing"),
        json!({"id": "tap_update", "from": "listing",
               "click": {"kind": "point", "x": 1, "y": 0},
               "unguarded_trusted_coordinate": true, "retryable": false,
               "expect_after": {"page_id": "done", "timeout_ms": 60_000, "interval_ms": 30_000,
                                "wait_page": {"page_id": "updating", "max_ms": 1_800_000}}}),
        restart(),
    ];
    admitted(
        "the store tap's confirmation holding on the updating page",
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(held),
    );

    let store = prepared(
        control("navigable_route", Some(tap_update(&["proof/title"]))),
        task(store_run()),
    );
    let mut runtime = device(&[
        &["dialog"],
        &["listing"],
        &[
            "updating", "updating", "updating", "updating", "updating", "done",
        ],
        &["home"],
    ]);
    succeeded("long wait", store.run(&mut runtime));
    assert_eq!(finished_on(&runtime, "wait_download"), ["neutral/done"]);
    assert_eq!(effect_records(&runtime, "wait_download"), 0);
    assert_eq!(
        runtime.inputs().len(),
        2,
        "no input while the store updates"
    );
    assert_eq!(store_taps(&runtime), [STORE]);
    assert!(
        runtime.pauses().iter().sum::<Duration>() >= Duration::from_millis(90_000),
        "the wait spans the updating frames, 30 s apart: {:?}",
        runtime.pauses()
    );
}
