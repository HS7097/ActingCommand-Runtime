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
//   Fix round 1 (adapted, R3): the sha256 check names both values, the case's and the bytes'.
// - `rc1_an_expectation_not_evaluated_is_not_run_and_the_case_partial`: `step` and `entry` beside a
//   page that passes are `not_run` (`not_evaluated`) and the case is `partial`, never `passed`.
// - `rc1_a_case_of_another_schema_is_not_run_with_a_warning_and_fails_nothing`: another and no
//   `schema_version` are `not_run` with `case_schema_unsupported`; a run over a checkout holding
//   one beside a passing case exits 0. Fix round 1 (adapted, R2): the run asks for the rows with
//   `--game`, as a success lists them only on request, and answers no `failed_total`.
// - `rc1_pixels_outside_the_rectangles_not_of_the_canvas_colour_warn`: one blue pixel outside the
//   rectangles: `resource_regression_scene_outside_rects` with `count` 1, the case still `passed`.
// - `rc1_the_operation_answers_three_verdicts_and_the_counts_and_exits_by_the_failures`: three
//   cases (one passes, one fails, one of another schema): `resource_regression_failed`, exit 1,
//   the counts of the three verdicts; fix round 1 (adapted, R2): the details list the one case
//   that did not pass as a short row (`failed_total` 1, `listed` 1, no `detail`); `--case` of the
//   passing one exits 0 and lists its row.
// - `rc1_the_operation_writes_nothing`: fix round 1 (adapted, R5): the checkout it ran on is file
//   for file and byte for byte the same after a run, and nothing is written beside it.
// Fix round 1 (coordinator work order rd-1116/brief-463-rc1-fix1.md, rulings R2, R3 and R5):
// - `rc1_fix1_text_recognition_unavailable_is_not_run_and_the_case_partial`: a recognize entry of
//   an OCR target without an OCR model is `not_run` (`ocr_unavailable`), the page beside it passes,
//   the case is `partial`.
// - `rc1_fix1_a_package_whose_target_consensus_reads_several_frames_is_not_run`: a package whose
//   target consensus reads two frames: the page is `not_run` (`frames_required`), the case
//   `not_run`.
// - `rc1_fix1_a_scene_of_another_size_than_its_package_fails_scene_size_unsupported`: a 2x1 scene
//   (and canvas) for the 4x1 package: `failed`, `scene_size_unsupported`, nothing evaluated.
// - `rc1_fix1_a_package_missing_from_the_checkout_fails_its_case`: a case naming a package the
//   checkout lacks: exit 1, its row's one check `package_missing` at `/package_id`.
// - `rc1_fix1_game_and_package_id_select_their_cases`: `--game` keeps one game's cases,
//   `--package-id` the cases naming that package, each run listing exactly their rows.
// - `rc1_fix1_a_filter_that_selects_no_case_is_refused`: `--case` of no case: `validation_failed`,
//   exit 1.
// - `rc1_fix1_an_unknown_member_of_the_top_level_warns_and_fails_nothing`: an unknown top-level
//   member: `passed` with `case_member_unknown` naming it; inside `scene_construction` it stays
//   the failure `member_unknown`.
// - `rc1_fix1_partial_cases_warn_with_their_count_and_exit_0`: two partial cases beside a passing
//   one: exit 0, `resource_regression_partial` with `count` 2, once.
// - `rc1_fix1_a_case_of_another_package_than_its_package_declares_fails`: the case's `package_id`
//   is not the package's: `failed`, `package_id_mismatch` naming both, nothing evaluated.
// - `rc1_fix1_a_failing_run_lists_the_cases_that_did_not_pass_up_to_its_bound`: the bound plus two
//   failing cases, a partial one and a passing one: the refusal names the first failed case, lists
//   the first `ROWS_LISTED_MAX` failed cases (the partial one after them is left out),
//   `failed_total` counts every case that did not pass, each row short.

use super::*;
use crate::cli_module::own_line;
use crate::resource_verify::regression::ROWS_LISTED_MAX;
use actingcommand_lab::{
    LabOcr, RegressionCaseVerdict, RegressionMismatch, RegressionNotRun, RegressionProblem,
    RegressionResult, RegressionVerdict, read_regression_case, run_regression_case,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const PACKAGE: &str = "neutral.regression.a";
/// Fix round 1: a second package of the same content, for the filters and the packages a test
/// changes.
const PACKAGE_B: &str = "neutral.regression.b";
const HOME: &str = "neutral/home";
const OTHER: &str = "neutral/other";
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const BLACK: [u8; 3] = [0, 0, 0];
const PAGE_AT: &str = "/expect/observe/page";

/// A temporary checkout: the package `packs/<PACKAGE>/`, the packages added, and the cases added
/// under `regression/<game>/`.
struct Checkout {
    temp: TempDir,
}

impl Checkout {
    fn new() -> Self {
        let checkout = Self {
            temp: TempDir::new().expect("a temporary directory"),
        };
        write_package(&checkout.package(), PACKAGE, |_| {});
        checkout
    }

    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }

    fn package(&self) -> PathBuf {
        self.package_of(PACKAGE)
    }

    fn package_of(&self, package_id: &str) -> PathBuf {
        self.repo().join("packs").join(package_id)
    }

    /// The package `packs/<package_id>/`, declaring `package_id`, its task edited by `edit`.
    fn add_package(&self, package_id: &str, edit: impl FnOnce(&mut Value)) -> PathBuf {
        let root = self.package_of(package_id);
        write_package(&root, package_id, edit);
        root
    }

    /// The case directory `regression/neutral/<name>/` ([`Self::add_case_in`]).
    fn add_case(&self, name: &str, case: &Value, scene: Option<&[u8]>) {
        self.add_case_in("neutral", name, case, scene);
    }

    /// The case directory `regression/<game>/<name>/` with `case` as its `case.json` and, when
    /// given, `scene` as its `scene.png`.
    fn add_case_in(&self, game: &str, name: &str, case: &Value, scene: Option<&[u8]>) {
        let directory = self.repo().join("regression").join(game).join(name);
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

/// The neutral package at `root`, declaring `package_id`: a source directory of one task on a
/// 4x1 coordinate space whose pages are recognised by one color probe each, `home_red` at x 0 and
/// `other_green` at x 1; its task edited by `edit`.
fn write_package(root: &Path, package_id: &str, edit: impl FnOnce(&mut Value)) {
    let probe = |id: &str, x: u32, expected: [u8; 3]| {
        json!({
            "id": id,
            "region": {"mode": "rect", "rect": {"x": x, "y": 0, "width": 1, "height": 1}},
            "expected": expected
        })
    };
    let mut task = json!({
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
    });
    edit(&mut task);
    let files = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": package_id,
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
        ("resources/operations/task/task.json", task),
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

/// A case of the contract over `scene` of the package `PACKAGE` ([`case_of`]).
fn case(scene: &[u8], rects: Value, expect: Value) -> Value {
    case_of(PACKAGE, scene, rects, expect)
}

/// A case of the contract of the package `package_id` over `scene`: the black 4x1 canvas with
/// `rects` pasted on it, the scene's SHA-256, and `expect`.
fn case_of(package_id: &str, scene: &[u8], rects: Value, expect: Value) -> Value {
    json!({
        "schema_version": "actingcommand.resource-regression-case.v1",
        "id": "neutral-regression-case",
        "package_id": package_id,
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

/// The Lab's runner on `case` and `scene` over the checkout's package `PACKAGE`.
fn verdict(checkout: &Checkout, case: &Value, scene: &[u8]) -> RegressionCaseVerdict {
    verdict_on(&checkout.package(), case, scene)
}

/// The Lab's runner on `case` and `scene` over the package at `package`, without an OCR model.
fn verdict_on(package: &Path, case: &Value, scene: &[u8]) -> RegressionCaseVerdict {
    run_regression_case(
        package,
        &serde_json::to_vec(case).expect("the case serializes"),
        scene,
        &LabOcr::from_installation(None),
    )
}

/// Each check of `verdict` as its place, its result and why it did not run.
fn not_run_of(
    verdict: &RegressionCaseVerdict,
) -> Vec<(&str, RegressionResult, Option<RegressionNotRun>)> {
    verdict
        .checks
        .iter()
        .map(|check| (check.at.as_str(), check.result, check.reason))
        .collect()
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
    let wrong = verdict(&checkout, &wrong_sha256, &bytes);
    assert_eq!(
        only_check(&wrong),
        (
            "/scene_construction/scene_sha256".to_owned(),
            Some(RegressionProblem::SceneSha256Mismatch)
        )
    );
    // Fix round 1 (R3): the check names both values, the case's and the scene's bytes'.
    assert_eq!(
        wrong.checks[0].mismatch,
        Some(RegressionMismatch {
            case: format!("{:x}", Sha256::digest(other_scene.as_slice())),
            found: Some(format!("{:x}", Sha256::digest(bytes.as_slice()))),
        }),
        "{wrong:?}"
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

    // Fix round 1 (R2): a success lists the rows only when a filter asks for them.
    let result = checkout.run(&["--game", "neutral"]);

    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the answer");
    assert_eq!(
        data["counts"],
        json!({"cases": 2, "passed": 1, "failed": 0, "partial": 0, "not_run": 1})
    );
    assert_eq!(data.get("failed_total"), None, "{data}");
    assert_eq!(data["cases"][1]["path"], "regression/neutral/b-frames");
    assert_eq!(data["cases"][1]["verdict"], "not_run");
    assert_eq!(data["cases"][1]["checks"], json!([]));
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
    // Fix round 1 (R2): only the case that did not pass, as a short row.
    assert_eq!(details["failed_total"], 1, "{details}");
    assert_eq!(details["listed"], 1, "{details}");
    let rows = details["cases"].as_array().expect("the rows");
    assert_eq!(rows.len(), 1, "{details}");
    let row = &rows[0];
    assert_eq!(row["path"], "regression/neutral/b-conflict");
    assert_eq!(row["id"], "neutral-regression-case");
    assert_eq!(row["package_id"], PACKAGE);
    assert_eq!(row["verdict"], "failed");
    assert_eq!(row["checks"][0]["at"], PAGE_AT);
    assert_eq!(row["checks"][0]["problem"], "page_conflict");
    assert_eq!(row["checks"][0].get("detail"), None, "{row}");
    assert_eq!(
        row.get("scene_construction"),
        None,
        "no case document: {row}"
    );

    let one = checkout.run(&["--case", "a-passes"]);
    let exit = own_line::exit(&one);
    assert_eq!(exit.code, 0, "{}", one.envelope_json());
    let data = one.envelope.data.as_ref().expect("the answer");
    assert_eq!(
        data["counts"],
        json!({"cases": 1, "passed": 1, "failed": 0, "partial": 0, "not_run": 0})
    );
    assert_eq!(data["cases"][0]["path"], "regression/neutral/a-passes");
    assert_eq!(data["cases"][0]["verdict"], "passed");
}

#[test]
fn rc1_the_operation_writes_nothing() {
    let _guard = env_lock();
    let checkout = three_cases();
    let repo = checkout.repo();
    let before = tree(&repo);
    let around = tree(checkout.temp.path());

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
    // Fix round 1 (R5): the checkout it ran on, file for file and byte for byte.
    assert_eq!(tree(&repo), before, "the checkout it ran on as it was");
    assert_eq!(
        tree(checkout.temp.path()),
        around,
        "nothing written beside the checkout"
    );
}

#[test]
fn rc1_fix1_text_recognition_unavailable_is_not_run_and_the_case_partial() {
    let checkout = Checkout::new();
    let package = checkout.add_package(PACKAGE_B, |task| {
        task["ocr_targets"] = json!([{
            "id": "ocr/label",
            "region": {"mode": "rect", "rect": {"x": 3, "y": 0, "width": 1, "height": 1}},
            "languages": ["en"],
            "timeout_ms": 1000,
            "match_mode": "exact",
            "expected": ["unused"],
            "case_sensitive": false,
            "minimum_confidence": 0.0,
            "model_ref": "neutral-ocr-model-not-installed",
            "model_sha256": "a".repeat(64)
        }]);
    });
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let case = case_of(
        PACKAGE_B,
        &bytes,
        json!([rect(0, 1)]),
        json!({
            "observe": {"page": HOME},
            "recognize": [{"target": "ocr/label", "passed": true}]
        }),
    );

    let verdict = verdict_on(&package, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Partial, "{verdict:?}");
    assert_eq!(
        not_run_of(&verdict),
        [
            (PAGE_AT, RegressionResult::Passed, None),
            (
                "/expect/recognize/0",
                RegressionResult::NotRun,
                Some(RegressionNotRun::OcrUnavailable)
            ),
        ]
    );
    let target = verdict.checks[1].target.as_ref().expect("the target");
    assert_eq!(
        (target.target.as_str(), target.expected, target.recognized),
        ("ocr/label", true, None)
    );
}

#[test]
fn rc1_fix1_a_package_whose_target_consensus_reads_several_frames_is_not_run() {
    let checkout = Checkout::new();
    let package = checkout.add_package(PACKAGE_B, |task| {
        task["target_consensus"] = json!({"home_red": {
            "samples": [{"frame": 0}, {"frame": 1}],
            "k": 2,
            "sample_interval_ms": 1
        }});
    });
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let case = case_of(
        PACKAGE_B,
        &bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    );

    let verdict = verdict_on(&package, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::NotRun, "{verdict:?}");
    assert_eq!(
        not_run_of(&verdict),
        [(
            PAGE_AT,
            RegressionResult::NotRun,
            Some(RegressionNotRun::FramesRequired)
        )]
    );
}

#[test]
fn rc1_fix1_a_scene_of_another_size_than_its_package_fails_scene_size_unsupported() {
    let checkout = Checkout::new();
    // A 2x1 scene on a 2x1 canvas, for the 4x1 package: smaller than its coordinate space.
    let bytes = encode_rgb_png(2, 1, &[RED, BLACK]);
    let mut case = case(
        &bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    );
    case["scene_construction"]["canvas"]["width"] = json!(2);

    let verdict = verdict(&checkout, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Failed, "{verdict:?}");
    let answered = verdict
        .checks
        .iter()
        .map(|check| (check.at.as_str(), check.problem))
        .collect::<Vec<_>>();
    assert_eq!(
        answered,
        [("/scene", Some(RegressionProblem::SceneSizeUnsupported))],
        "nothing evaluated: {verdict:?}"
    );
}

#[test]
fn rc1_fix1_a_package_missing_from_the_checkout_fails_its_case() {
    let _guard = env_lock();
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    checkout.add_case(
        "a-absent",
        &case_of(
            "neutral.regression.absent",
            &bytes,
            json!([rect(0, 1)]),
            json!({"observe": {"page": HOME}}),
        ),
        Some(&bytes),
    );

    let result = checkout.run(&[]);

    assert_eq!(
        own_line::exit(&result).code,
        1,
        "{}",
        result.envelope_json()
    );
    let details = result
        .envelope
        .error
        .as_ref()
        .and_then(|error| error.details.as_ref())
        .expect("the refusal's details");
    assert_eq!(details["cases"][0]["path"], "regression/neutral/a-absent");
    assert_eq!(details["cases"][0]["verdict"], "failed");
    assert_eq!(
        details["cases"][0]["checks"],
        json!([{"at": "/package_id", "result": "failed", "problem": "package_missing"}])
    );
}

#[test]
fn rc1_fix1_game_and_package_id_select_their_cases() {
    let _guard = env_lock();
    let checkout = Checkout::new();
    checkout.add_package(PACKAGE_B, |_| {});
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let rects = json!([rect(0, 1)]);
    let expect = json!({"observe": {"page": HOME}});
    checkout.add_case(
        "a",
        &case(&bytes, rects.clone(), expect.clone()),
        Some(&bytes),
    );
    checkout.add_case(
        "b",
        &case_of(PACKAGE_B, &bytes, rects.clone(), expect.clone()),
        Some(&bytes),
    );
    checkout.add_case_in("second", "c", &case(&bytes, rects, expect), Some(&bytes));
    let paths = |more: &[&str]| -> Vec<String> {
        let result = checkout.run(more);
        assert_eq!(
            own_line::exit(&result).code,
            0,
            "{more:?}: {}",
            result.envelope_json()
        );
        let data = result.envelope.data.as_ref().expect("the answer");
        data["cases"]
            .as_array()
            .expect("the rows asked for")
            .iter()
            .map(|row| row["path"].as_str().unwrap_or_default().to_owned())
            .collect()
    };

    assert_eq!(paths(&["--game", "second"]), ["regression/second/c"]);
    assert_eq!(
        paths(&["--package-id", PACKAGE_B]),
        ["regression/neutral/b"]
    );
}

#[test]
fn rc1_fix1_a_filter_that_selects_no_case_is_refused() {
    let _guard = env_lock();
    let checkout = three_cases();

    let result = checkout.run(&["--case", "absent"]);

    assert_eq!(
        own_line::exit(&result).code,
        1,
        "{}",
        result.envelope_json()
    );
    assert_eq!(
        result
            .envelope
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("validation_failed"),
        "{}",
        result.envelope_json()
    );
}

#[test]
fn rc1_fix1_an_unknown_member_of_the_top_level_warns_and_fails_nothing() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let base = case(
        &bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    );
    let mut top = base.clone();
    top["note"] = json!("a member of the repository's own");

    let read_past = verdict(&checkout, &top, &bytes);

    assert_eq!(
        read_past.verdict,
        RegressionVerdict::Passed,
        "{read_past:?}"
    );
    assert_eq!(read_past.warnings.len(), 1, "{read_past:?}");
    let outcome = &read_past.warnings[0].outcome;
    assert_eq!(outcome.code().as_str(), "case_member_unknown");
    assert_eq!(outcome.value("member"), Some(&json!("note")));

    // Inside a part the runner evaluates it stays a failure.
    let mut inside = base;
    inside["scene_construction"]["note"] = json!("a member the contract does not name");
    let refused = verdict(&checkout, &inside, &bytes);
    assert_eq!(refused.verdict, RegressionVerdict::Failed, "{refused:?}");
    let answered = refused
        .checks
        .iter()
        .map(|check| (check.at.as_str(), check.problem))
        .collect::<Vec<_>>();
    assert_eq!(
        answered,
        [(
            "/scene_construction/note",
            Some(RegressionProblem::MemberUnknown)
        )]
    );
}

#[test]
fn rc1_fix1_partial_cases_warn_with_their_count_and_exit_0() {
    let _guard = env_lock();
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let rects = json!([rect(0, 1)]);
    checkout.add_case(
        "a-passes",
        &case(&bytes, rects.clone(), json!({"observe": {"page": HOME}})),
        Some(&bytes),
    );
    let partial = case(
        &bytes,
        rects,
        json!({"observe": {"page": HOME}, "step": {"operation": "open_other"}}),
    );
    checkout.add_case("b-partial", &partial, Some(&bytes));
    checkout.add_case("c-partial", &partial, Some(&bytes));

    let result = checkout.run(&[]);

    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the answer");
    assert_eq!(
        data["counts"],
        json!({"cases": 3, "passed": 1, "failed": 0, "partial": 2, "not_run": 0})
    );
    assert_eq!(data["warnings"].as_array().map(Vec::len), Some(1), "{data}");
    assert_eq!(
        data["warnings"][0]["outcome"]["code"],
        "resource_regression_partial"
    );
    assert_eq!(data["warnings"][0]["outcome"]["values"]["count"], 2);
    let line = exit.line.expect("the success line");
    assert_eq!(
        line["warnings"][0]["outcome"]["code"],
        "resource_regression_partial"
    );
}

#[test]
fn rc1_fix1_a_case_of_another_package_than_its_package_declares_fails() {
    let checkout = Checkout::new();
    let bytes = scene([RED, BLACK, BLACK, BLACK]);
    let case = case_of(
        PACKAGE_B,
        &bytes,
        json!([rect(0, 1)]),
        json!({"observe": {"page": HOME}}),
    );

    // The checkout's package `PACKAGE` declares its own id, not the case's.
    let verdict = verdict(&checkout, &case, &bytes);

    assert_eq!(verdict.verdict, RegressionVerdict::Failed, "{verdict:?}");
    assert_eq!(verdict.checks.len(), 1, "nothing evaluated: {verdict:?}");
    let check = &verdict.checks[0];
    assert_eq!(
        (check.at.as_str(), check.problem),
        ("/package_id", Some(RegressionProblem::PackageIdMismatch))
    );
    assert_eq!(
        check.mismatch,
        Some(RegressionMismatch {
            case: PACKAGE_B.to_owned(),
            found: Some(PACKAGE.to_owned()),
        })
    );
}

#[test]
fn rc1_fix1_a_failing_run_lists_the_cases_that_did_not_pass_up_to_its_bound() {
    let _guard = env_lock();
    let checkout = Checkout::new();
    let passes = scene([RED, BLACK, BLACK, BLACK]);
    let conflict = scene([RED, GREEN, BLACK, BLACK]);
    let page = json!({"observe": {"page": HOME}});
    // A partial case first in the order of the paths, then a passing one, then the failing ones.
    checkout.add_case(
        "a-partial",
        &case(
            &passes,
            json!([rect(0, 1)]),
            json!({"observe": {"page": HOME}, "entry": {"accepted": true}}),
        ),
        Some(&passes),
    );
    checkout.add_case(
        "b-passes",
        &case(&passes, json!([rect(0, 1)]), page.clone()),
        Some(&passes),
    );
    let failing = ROWS_LISTED_MAX + 2;
    let conflicting = case(&conflict, json!([rect(0, 2)]), page);
    for index in 0..failing {
        checkout.add_case(&format!("f-{index:02}"), &conflicting, Some(&conflict));
    }

    let result = checkout.run(&[]);

    let exit = own_line::exit(&result);
    assert_eq!(exit.code, 1, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().expect("the refusal");
    assert_eq!(error.code, "resource_regression_failed");
    assert!(
        error.message.contains("regression/neutral/f-00"),
        "the first failed case named: {}",
        error.message
    );
    let details = error.details.as_ref().expect("the details");
    assert_eq!(
        details["counts"],
        json!({
            "cases": failing + 2,
            "passed": 1,
            "failed": failing,
            "partial": 1,
            "not_run": 0
        })
    );
    assert_eq!(details["failed_total"], json!(failing + 1), "{details}");
    assert_eq!(details["listed"], json!(ROWS_LISTED_MAX), "{details}");
    let rows = details["cases"].as_array().expect("the rows");
    let listed = rows
        .iter()
        .map(|row| {
            (
                row["path"].as_str().unwrap_or_default().to_owned(),
                row["verdict"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let expected = (0..ROWS_LISTED_MAX)
        .map(|index| {
            (
                format!("regression/neutral/f-{index:02}"),
                "failed".to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(listed, expected, "the failed cases first, up to the bound");
    for row in rows {
        assert_eq!(row["checks"].as_array().map(Vec::len), Some(1), "{row}");
        assert_eq!(row["checks"][0]["problem"], "page_conflict", "{row}");
        assert_eq!(row["checks"][0].get("detail"), None, "a short row: {row}");
    }
    assert_eq!(
        details["warnings"][0]["outcome"]["code"],
        "resource_regression_partial"
    );
    assert_eq!(details["warnings"][0]["outcome"]["values"]["count"], 1);
    let line = exit.line.expect("the failure line");
    assert_eq!(line["details"]["listed"], json!(ROWS_LISTED_MAX));
}
