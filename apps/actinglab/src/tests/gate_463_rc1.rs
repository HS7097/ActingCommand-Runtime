// SPDX-License-Identifier: AGPL-3.0-only

// #463 RC-1 (contracts/resource-regression-case.md; register §1.450): the regression case of a
// resource repository as a contract of the Runtime, its reader and its offline runner, and the
// operation `resource regression`. One neutral package (game `neutral`, a canvas of 4x1 pixels,
// page `home` recognised by a red pixel at x 0, page `other` by a green pixel at x 1) and
// synthetic cases over scenes of four pixels, written into a temporary checkout; no Runtime, no
// device, no OCR model, no clock read by the tests.
// - `rc1_a_case_whose_page_is_recognised_on_its_scene_passes`: the kernel recognises `home` on a
//   scene whose red pixel is pasted on the black canvas; the verdict `passed`, as answered.
// - `rc1_a_second_page_matching_the_scene_fails_the_case_with_the_conflict`: a green pixel beside
//   it makes `other` match too: `failed`, `page_conflict`, both pages named, none recognised.
// - `rc1_recognize_entries_expected_present_and_absent_pass_when_right_and_fail_when_wrong`: each
//   target expected present and absent, each way right (`passed`) and wrong (`target_mismatch`
//   with what the kernel answered); a case of only the right ones `passed`.
// - `rc1_a_wrong_scene_sha256_canvas_or_rectangle_fails_with_its_word`: `scene_sha256_mismatch`,
//   `canvas_size_mismatch`, `rect_outside_canvas`, each the case's one check: nothing evaluated.
// - `rc1_an_expectation_not_evaluated_is_not_run_and_the_case_partial`: `step` and `entry` beside a
//   page that passes are `not_run` (`not_evaluated`) and the case is `partial`, never `passed`.
// - `rc1_a_case_of_another_schema_is_not_run_with_a_warning_and_fails_nothing`: another and no
//   `schema_version` are `not_run` with `case_schema_unsupported`; a run over a checkout holding
//   one beside a passing case exits 0.
// - `rc1_pixels_outside_the_rectangles_not_of_the_canvas_colour_warn`: one blue pixel outside the
//   rectangles: `resource_regression_scene_outside_rects` with `count` 1, the case still `passed`.
// - `rc1_the_operation_answers_three_verdicts_and_the_counts_and_exits_by_the_failures`: three
//   cases (one passes, one fails, one of another schema): `resource_regression_failed`, exit 1,
//   the three verdicts in order, the counts and the failed path; `--case` of the passing one exits
//   0.
// - `rc1_the_operation_writes_nothing`: the checkout and its directory are the same, file for file
//   and byte for byte, after a run.

use super::*;
use crate::cli_module::own_line;
use actingcommand_lab::{
    LabOcr, RegressionCaseVerdict, RegressionNotRun, RegressionProblem, RegressionResult,
    RegressionVerdict, read_regression_case, run_regression_case,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const PACKAGE: &str = "neutral.regression.a";
const HOME: &str = "neutral/home";
const OTHER: &str = "neutral/other";
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 3] = [0, 0, 0];
const PAGE_AT: &str = "/expect/observe/page";

/// A temporary checkout: the package `packs/<PACKAGE>/` and the cases added under
/// `regression/neutral/`.
struct Checkout {
    temp: TempDir,
}

impl Checkout {
    fn new() -> Self {
        let checkout = Self {
            temp: TempDir::new().expect("a temporary directory"),
        };
        write_package(&checkout.package());
        checkout
    }

    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }

    fn package(&self) -> PathBuf {
        self.repo().join("packs").join(PACKAGE)
    }

    /// The case directory `regression/neutral/<name>/` with `case` as its `case.json` and, when
    /// given, `scene` as its `scene.png`.
    fn add_case(&self, name: &str, case: &Value, scene: Option<&[u8]>) {
        let directory = self.repo().join("regression").join("neutral").join(name);
        fs::create_dir_all(&directory).expect("the case directory");
        fs::write(
            directory.join("case.json"),
            serde_json::to_vec_pretty(case).expect("the case serializes"),
        )
        .expect("case.json written");
        if let Some(scene) = scene {
            fs::write(directory.join("scene.png"), scene).expect("scene.png written");
        }
    }

    /// `resource regression --repo <repo>` with `more` after it, as actinglab runs it.
    fn run(&self, more: &[&str]) -> crate::CliResult {
        let repo = self.repo();
        let mut line = vec![
            "--json",
            "resource",
            "regression",
            "--repo",
            repo.to_str().expect("the temporary path is UTF-8"),
        ];
        line.extend(more);
        crate::run_cli(line, true)
    }
}

/// The neutral package: a source directory of one task on a 4x1 coordinate space whose pages are
/// recognised by one color probe each, `home_red` at x 0 and `other_green` at x 1.
fn write_package(root: &Path) {
    let probe = |id: &str, x: u32, expected: [u8; 3]| {
        json!({
            "id": id,
            "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let files = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": PACKAGE,
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 4, "height": 1},
                "entry_task_id": "task",
                "capture_interval_ms": 1,
                "step_timeout_ms": 10,
                "timeout_ms": 100,
                "max_steps": 2
            }),
        ),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        (
            "resources/operations/task/task.json",
            json!({
                "schema_version": "0.6",
                "task_id": "task",
                "game": "neutral",
                "server_scope": ["test"],
                "locale": "en-US",
                "goal": "neutral regression case fixture",
                "coordinate_space": {"width": 4, "height": 1},
                "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
                "color_probes": [probe("home_red", 0, RED), probe("other_green", 1, GREEN)],
                "page_rules": {
                    "home": {"required": ["home_red"]},
                    "other": {"required": ["other_green"]}
                },
                "entry_page": "home",
                "target_page": "other",
                "operations": [{
                    "id": "open_other",
                    "purpose": "a neutral operation from home to other",
                    "from": "home",
                    "to": "other",
                    "click": {"kind": "point", "x": 3, "y": 0},
                    "unguarded_trusted_coordinate": true,
                    "retryable": false
                }]
            }),
        ),
    ];
    for (path, value) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().expect("a parent")).expect("the package's directories");
        let text = serde_json::to_string_pretty(&value).expect("the file serializes");
        fs::write(file, format!("{text}\n")).expect("a package file written");
    }
}

/// A scene of four pixels in a row, as PNG bytes.
fn scene(pixels: [[u8; 3]; 4]) -> Vec<u8> {
    encode_rgb_png(4, 1, &pixels)
}

/// A case of the contract over `scene`: the black 4x1 canvas with `rects` pasted on it, the
/// scene's SHA-256, and `expect`.
fn case(scene: &[u8], rects: Value, expect: Value) -> Value {
    json!({
        "schema_version": "actingcommand.resource-regression-case.v1",
        "id": "neutral-regression-case",
        "package_id": PACKAGE,
        "scene": "scene.png",
        "scene_construction": {
            "kind": "roi_patch",
            "canvas": {"width": 4, "height": 1, "fill": [0, 0, 0]},
            "rects": rects,
            "scene_sha256": format!("{:x}", Sha256::digest(scene))
        },
        "source": {"page_shown": "a synthetic scene of four pixels"},
        "expect": expect
    })
}

/// One rectangle of `scene_construction.rects` on the 4x1 canvas.
fn rect(x: i64, width: i64) -> Value {
    json!({"x": x, "y": 0, "width": width, "height": 1, "content": "neutral patch"})
}

/// The Lab's runner on `case` and `scene` over the checkout's package, without an OCR model.
fn verdict(checkout: &Checkout, case: &Value, scene: &[u8]) -> RegressionCaseVerdict {
    run_regression_case(
        &checkout.package(),
        &serde_json::to_vec(case).expect("the case serializes"),
        scene,
        &LabOcr::from_installation(None),
    )
}

/// Every file of `root` with its bytes, and every directory (`None`).
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("a directory of the tree") {
            let path = entry.expect("an entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("under the root")
                .to_path_buf();
            if path.is_dir() {
                found.insert(relative, None);
                pending.push(path);
            } else {
                found.insert(relative, Some(fs::read(&path).expect("a file of the tree")));
            }
        }
    }
    found
}

#[test]
fn rc1_a_case_whose_page_is_recognised_on_its_scene_passes() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let case = case(
        &bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    );

    let verdict = verdict(&checkout, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
    assert!(verdict.warnings.is_empty(), "{verdict:?}");
    assert_eq!(
        serde_json::to_value(&verdict).expect("the verdict serializes"),
        json!({
            "id": "neutral-regression-case",
            "package_id": PACKAGE,
            "verdict": "passed",
            "checks": [{
                "at": PAGE_AT,
                "result": "passed",
                "page": {"expected": HOME, "recognized": HOME}
            }]
        })
    );
}

#[test]
fn rc1_a_second_page_matching_the_scene_fails_the_case_with_the_conflict() {
    let checkout = Checkout::new();
    let bytes = scene([RED, GREEN, BLACK, BLACK]);
    let case = case(
        &bytes,
        json!([rect(0, 2)]),
        json!({"observe": {"page": HOME}}),
    );

    let verdict = verdict(&checkout, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Failed, "{verdict:?}");
    assert_eq!(verdict.checks.len(), 1, "{verdict:?}");
    let check = &verdict.checks[0];
    assert_eq!(check.at, PAGE_AT);
    assert_eq!(check.result, RegressionResult::Failed);
    assert_eq!(check.problem, Some(RegressionProblem::PageConflict));
    let page = check.page.as_ref().expect("the page expectation");
    assert_eq!(page.expected.as_deref(), Some(HOME));
    assert_eq!(page.recognized, None, "no page is recognised in a conflict");
    let mut matched = page.matched_pages.clone();
    matched.sort();
    assert_eq!(matched, [HOME, OTHER]);
}

#[test]
fn rc1_recognize_entries_expected_present_and_absent_pass_when_right_and_fail_when_wrong() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let rects = json!([rect(0, 1)]);
    let entries = json!([
        {"target": "home_red", "passed": true},
        {"target": "other_green", "passed": false},
        {"target": "home_red", "passed": false},
        {"target": "other_green", "passed": true}
    ]);
    let case_of_four = case(&bytes, rects.clone(), json!({"recognize": entries}));

    let verdict_of_four = verdict(&checkout, &case_of_four, &bytes);

    assert_eq!(
        verdict_of_four.verdict,
        RegressionVerdict::Failed,
        "{verdict_of_four:?}"
    );
    let answered = verdict_of_four
        .checks
        .iter()
        .map(|check| {
            let target = check.target.as_ref().expect("the target expectation");
            (
                check.at.as_str(),
                check.result,
                check.problem,
                target.target.as_str(),
                target.expected,
                target.recognized,
            )
        })
        .collect::<Vec<_>>();
    let (passed, failed, mismatch) = (
        RegressionResult::Passed,
        RegressionResult::Failed,
        Some(RegressionProblem::TargetMismatch),
    );
    assert_eq!(
        answered,
        [
            (
                "/expect/recognize/0",
                passed,
                None,
                "home_red",
                true,
                Some(true)
            ),
            (
                "/expect/recognize/1",
                passed,
                None,
                "other_green",
                false,
                Some(false)
            ),
            (
                "/expect/recognize/2",
                failed,
                mismatch,
                "home_red",
                false,
                Some(true)
            ),
            (
                "/expect/recognize/3",
                failed,
                mismatch,
                "other_green",
                true,
                Some(false)
            ),
        ]
    );

    let right = json!([
        {"target": "home_red", "passed": true},
        {"target": "other_green", "passed": false}
    ]);
    let case_of_two = case(&bytes, rects, json!({"recognize": right}));
    let verdict_of_two = verdict(&checkout, &case_of_two, &bytes);
    assert_eq!(
        verdict_of_two.verdict,
        RegressionVerdict::Passed,
        "{verdict_of_two:?}"
    );
    assert!(
        verdict_of_two
            .checks
            .iter()
            .all(|check| check.result == RegressionResult::Passed)
    );
}

#[test]
fn rc1_a_wrong_scene_sha256_canvas_or_rectangle_fails_with_its_word() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let expect = json!({"observe": {"page": HOME}});
    let only_check = |verdict: &RegressionCaseVerdict| {
        assert_eq!(verdict.verdict, RegressionVerdict::Failed, "{verdict:?}");
        assert_eq!(
            verdict.checks.len(),
            1,
            "nothing else evaluated: {verdict:?}"
        );
        let check = &verdict.checks[0];
        assert_eq!(check.result, RegressionResult::Failed, "{verdict:?}");
        (check.at.clone(), check.problem)
    };

    // The scene's bytes are another scene's than the one scene_sha256 names.
    let other_scene = scene([BLACK, BLACK, BLACK, RED]);
    let wrong_sha256 = case(&other_scene, json!([rect(0, 1)]), expect.clone());
    assert_eq!(
        only_check(&verdict(&checkout, &wrong_sha256, &bytes)),
        (
            "/scene_construction/scene_sha256".to_owned(),
            Some(RegressionProblem::SceneSha256Mismatch)
        )
    );

    // The canvas is 5x1, the scene 4x1.
    let mut wrong_canvas = case(&bytes, json!([rect(0, 1)]), expect.clone());
    wrong_canvas["scene_construction"]["canvas"]["width"] = json!(5);
    assert_eq!(
        only_check(&verdict(&checkout, &wrong_canvas, &bytes)),
        (
            "/scene_construction/canvas".to_owned(),
            Some(RegressionProblem::CanvasSizeMismatch)
        )
    );

    // A rectangle from x 3 two pixels wide leaves the 4x1 canvas: the reader refuses the case.
    let outside = case(&bytes, json!([rect(0, 1), rect(3, 2)]), expect);
    let expected = (
        "/scene_construction/rects/1".to_owned(),
        Some(RegressionProblem::RectOutsideCanvas),
    );
    assert_eq!(
        only_check(&verdict(&checkout, &outside, &bytes)),
        expected.clone()
    );
    let read = read_regression_case(&serde_json::to_vec(&outside).expect("the case serializes"))
        .expect_err("the reader refuses the rectangle");
    assert_eq!(only_check(&read), expected);
}

#[test]
fn rc1_an_expectation_not_evaluated_is_not_run_and_the_case_partial() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let case = case(
        &bytes,
        json!([rect(0, 1)]),
        json!({
            "observe": {"page": HOME},
            "entry": {"accepted": true},
            "step": {"operation": "open_other", "guard_passed": true}
        }),
    );

    let verdict = verdict(&checkout, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Partial, "{verdict:?}");
    assert_ne!(verdict.verdict, RegressionVerdict::Passed);
    let answered = verdict
        .checks
        .iter()
        .map(|check| (check.at.as_str(), check.result, check.reason))
        .collect::<Vec<_>>();
    let not_evaluated = Some(RegressionNotRun::NotEvaluated);
    assert_eq!(
        answered,
        [
            (PAGE_AT, RegressionResult::Passed, None),
            ("/expect/entry", RegressionResult::NotRun, not_evaluated),
            ("/expect/step", RegressionResult::NotRun, not_evaluated),
        ]
    );
}

#[test]
fn rc1_a_case_of_another_schema_is_not_run_with_a_warning_and_fails_nothing() {
    let unsupported_code = |verdict: &RegressionCaseVerdict| {
        assert_eq!(verdict.verdict, RegressionVerdict::NotRun, "{verdict:?}");
        assert!(verdict.checks.is_empty(), "{verdict:?}");
        assert_eq!(verdict.warnings.len(), 1, "{verdict:?}");
        let outcome = &verdict.warnings[0].outcome;
        (
            outcome.code().as_str().to_owned(),
            outcome.value("schema_version").cloned(),
        )
    };
    let frames = read_regression_case(
        br#"{"schema_version": "actingcommand.resource-frame-regression.v1", "id": "neutral-frames"}"#,
    )
    .expect_err("another schema is not read as a case");
    assert_eq!(frames.id.as_deref(), Some("neutral-frames"));
    assert_eq!(
        unsupported_code(&frames),
        (
            "case_schema_unsupported".to_owned(),
            Some(json!("actingcommand.resource-frame-regression.v1"))
        )
    );
    let patch = read_regression_case(br#"{"case": "neutral-patch", "scene": "scene.png"}"#)
        .expect_err("a case without a schema is not read as a case");
    assert_eq!(
        unsupported_code(&patch),
        ("case_schema_unsupported".to_owned(), None)
    );

    let _guard = env_lock();
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    checkout.add_case(
        "a-passes",
        &case(
            &bytes,
            json!([rect(0, 1)]),
            json!({"observe": {"page": HOME}}),
        ),
        Some(&bytes),
    );
    checkout.add_case(
        "b-frames",
        &json!({"schema_version": "actingcommand.resource-frame-regression.v1", "id": "neutral-frames"}),
        None,
    );

    let result = checkout.run(&[]);

    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the answer");
    assert_eq!(
        data["counts"],
        json!({"cases": 2, "passed": 1, "failed": 0, "partial": 0, "not_run": 1})
    );
    assert_eq!(data["failed"], json!([]));
    assert_eq!(data["cases"][1]["path"], "regression/neutral/b-frames");
    assert_eq!(data["cases"][1]["verdict"], "not_run");
    assert_eq!(
        data["cases"][1]["warnings"][0]["outcome"]["code"],
        "case_schema_unsupported"
    );
}

#[test]
fn rc1_pixels_outside_the_rectangles_not_of_the_canvas_colour_warn() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLUE, BLACK]);
    let case = case(
        &bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    );

    let verdict = verdict(&checkout, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Passed, "{verdict:?}");
    assert_eq!(verdict.warnings.len(), 1, "{verdict:?}");
    let outcome = &verdict.warnings[0].outcome;
    assert_eq!(
        outcome.code().as_str(),
        "resource_regression_scene_outside_rects"
    );
    assert_eq!(outcome.value("count"), Some(&json!(1)));
    let answered = serde_json::to_value(&verdict).expect("the verdict serializes");
    assert_eq!(
        answered["warnings"][0]["outcome"]["code"],
        "resource_regression_scene_outside_rects"
    );
    assert_eq!(answered["warnings"][0]["outcome"]["category"], "warning");
    assert_eq!(answered["warnings"][0]["outcome"]["values"]["count"], 1);
}

/// The checkout of the operation's tests: `a-passes`, `b-conflict` (two pages match) and
/// `c-frames` (another schema).
fn three_cases() -> Checkout {
    let checkout = Checkout::new();
    let passes = scene([RED, BLACK, BLACK, BLACK]);
    checkout.add_case(
        "a-passes",
        &case(
            &passes,
            json!([rect(0, 1)]),
            json!({"observe": {"page": HOME}}),
        ),
        Some(&passes),
    );
    let conflict = scene([RED, GREEN, BLACK, BLACK]);
    checkout.add_case(
        "b-conflict",
        &case(
            &conflict,
            json!([rect(0, 2)]),
            json!({"observe": {"page": HOME}}),
        ),
        Some(&conflict),
    );
    checkout.add_case(
        "c-frames",
        &json!({"schema_version": "actingcommand.resource-frame-regression.v1", "id": "neutral-frames"}),
        None,
    );
    checkout
}

#[test]
fn rc1_the_operation_answers_three_verdicts_and_the_counts_and_exits_by_the_failures() {
    let _guard = env_lock();
    let checkout = three_cases();

    let result = checkout.run(&[]);

    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 1, "{}", result.envelope_json());
    let line = exit.line.expect("the failure line");
    assert_eq!(line["outcome"]["code"], "resource_regression_failed");
    let error = result.envelope.error.as_ref().expect("the refusal");
    assert_eq!(error.code, "resource_regression_failed");
    let details = error.details.as_ref().expect("the answer as the details");
    assert_eq!(
        details["counts"],
        json!({"cases": 3, "passed": 1, "failed": 1, "partial": 0, "not_run": 1})
    );
    assert_eq!(details["failed"], json!(["regression/neutral/b-conflict"]));
    let cases = details["cases"].as_array().expect("the cases");
    let verdicts = cases
        .iter()
        .map(|case| {
            (
                case["path"].as_str().unwrap_or_default(),
                case["case"].as_str().unwrap_or_default(),
                case["verdict"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        verdicts,
        [
            ("regression/neutral/a-passes", "a-passes", "passed"),
            ("regression/neutral/b-conflict", "b-conflict", "failed"),
            ("regression/neutral/c-frames", "c-frames", "not_run"),
        ]
    );
    assert_eq!(cases[1]["checks"][0]["problem"], "page_conflict");
    assert_eq!(cases[0]["package_id"], PACKAGE);

    let one = checkout.run(&["--case", "a-passes"]);
    let exit = own_line::exit(&one);
    assert_eq!(exit.code, 0, "{}", one.envelope_json());
    let data = one.envelope.data.as_ref().expect("the answer");
    assert_eq!(
        data["counts"],
        json!({"cases": 1, "passed": 1, "failed": 0, "partial": 0, "not_run": 0})
    );
    assert_eq!(data["cases"][0]["verdict"], "passed");
}

#[test]
fn rc1_the_operation_writes_nothing() {
    let _guard = env_lock();
    let checkout = three_cases();
    let before = tree(checkout.temp.path());

    let result = checkout.run(&[]);

    assert_eq!(
        result
            .envelope
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("resource_regression_failed"),
        "{}",
        result.envelope_json()
    );
    assert_eq!(
        tree(checkout.temp.path()),
        before,
        "the checkout and its directory as they were"
    );
}
