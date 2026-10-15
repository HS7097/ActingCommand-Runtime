// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 R-10 (forced-compromises mapping R-10; REC-M §8.6, coordinator ruling option 2;
// slice row `r10-forbidden.md` §5, tests R10-T1 to R10-T6). An OCR target located relative to an
// anchor template cannot be evaluated while that anchor does not pass: it did not pass, with
// `failure` `ocr_region_unresolved` and its region evidence (`unresolved`
// `anchor_not_matched`), and no provider call, in a page rule, a check member and a guard. One
// page's missing anchor no longer fails the recognition of every page of the frame. In
// `forbidden` such a target is admitted only when its anchor is directly in the page's
// `required`. An anchor that matched with its region out of the frame stays an error.
//
// Every expected value comes from the slice row, REC-M §8.6 and the coordinator's rulings
// (Q1 option 2, Q2 `anchor_not_matched` only, Q3 guards alike, Q4 the anchor directly in
// `required`). A scripted OCR provider answers on synthetic neutral frames.

use super::*;
use actingcommand_contract::{OcrRegionUnresolvedReason, ResourceDeclarationReason};
use actingcommand_device::CaptureBackendName;
use actingcommand_page_detector::PageTargetEvaluation;
use actingcommand_recognition_pack::{
    AssetResolver, NnProviderRequest, NnProviderResult, OcrProviderRequest, OcrProviderResult,
    OcrProviderTextBlock, RecognitionPackCode, RecognitionPackError, RecognitionPackResult,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 16;
const MODEL_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BADGE_WIDTH: u32 = 8;
const BADGE_HEIGHT: u32 = 6;
// Where the badge shows when it shows; its template searches (16, 0, 16, 12).
const BADGE_X: u32 = 20;
const BADGE_Y: u32 = 2;
// The region beside the badge: the match's top-left corner plus (10, 0), 20 x 8.
const COUNT_ROI: PackRect = PackRect {
    x: 30,
    y: 2,
    width: 20,
    height: 8,
};

// What a frame shows: a marker pixel for a page (`Detail` blue at (0, 0), `Other` green at
// (1, 0); the terminal page's yellow marker at (2, 0) never shows here) and the patterned badge.
#[derive(Debug, Clone, Copy)]
enum Shown {
    Detail,
    Other,
    Badge,
}

// A patterned badge whose normalized correlation with a black frame is zero.
fn badge() -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..BADGE_HEIGHT {
        for x in 0..BADGE_WIDTH {
            pixels.extend_from_slice(&[
                ((x * 17 + y * 7) % 251 + 1) as u8,
                ((x * 11 + y * 19 + 23) % 239) as u8,
                ((x * 5 + y * 29 + 41) % 227) as u8,
            ]);
        }
    }
    pixels
}

fn pixels(shown: &[Shown]) -> Vec<u8> {
    let mut pixels = vec![0_u8; (WIDTH * HEIGHT * 3) as usize];
    let mut put = |x: u32, y: u32, rgb: &[u8]| {
        let start = ((y * WIDTH + x) * 3) as usize;
        pixels[start..start + 3].copy_from_slice(rgb);
    };
    for item in shown {
        match item {
            Shown::Detail => put(0, 0, &[0, 0, 255]),
            Shown::Other => put(1, 0, &[0, 255, 0]),
            Shown::Badge => {
                let icon = badge();
                for y in 0..BADGE_HEIGHT {
                    for x in 0..BADGE_WIDTH {
                        let start = ((y * BADGE_WIDTH + x) * 3) as usize;
                        put(BADGE_X + x, BADGE_Y + y, &icon[start..start + 3]);
                    }
                }
            }
        }
    }
    pixels
}

fn scene(shown: &[Shown]) -> Scene {
    Scene::from_rgb8(WIDTH, HEIGHT, &pixels(shown)).expect("scene")
}

fn frame(shown: &[Shown]) -> Frame {
    Frame::from_pixels(
        WIDTH,
        HEIGHT,
        pixels(shown),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("fixture frame")
}

// The badge template's PNG.
#[derive(Debug)]
struct BadgeAsset(Vec<u8>);

impl AssetResolver for BadgeAsset {
    fn read_asset(&self, path: &str) -> RecognitionPackResult<Vec<u8>> {
        if path == "badge.png" {
            Ok(self.0.clone())
        } else {
            Err(RecognitionPackError::fatal("no such fixture asset"))
        }
    }
}

// Reads the configured words of a region, one block each, and counts every call.
#[derive(Debug)]
struct Words {
    regions: Vec<(PackRect, Vec<&'static str>)>,
    calls: AtomicUsize,
}

impl Words {
    fn new(regions: Vec<(PackRect, Vec<&'static str>)>) -> Arc<Self> {
        Arc::new(Self {
            regions,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl VisionProvider for Words {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let words = self
            .regions
            .iter()
            .find(|(region, _)| *region == request.region)
            .map(|(_, words)| words.clone())
            .unwrap_or_default();
        let blocks = words
            .iter()
            .enumerate()
            .map(|(index, word)| OcrProviderTextBlock {
                text: (*word).to_owned(),
                rect: PackRect {
                    x: request.region.x,
                    y: request.region.y + 4 * index as i32,
                    width: 16,
                    height: 4,
                },
                confidence: Some(0.9),
            })
            .collect::<Vec<_>>();
        Ok(OcrProviderResult {
            ppocr_diagnostics: Vec::new(),
            text: words.join("\n"),
            blocks,
            confidence: Some(0.9),
            detection: None,
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

// An OCR target beside the badge, `dx` pixels right of the match's left edge, reading DONE.
fn beside_badge(id: &str, dx: i32, locate: bool) -> Value {
    let mut target = json!({
        "type": "ocr", "id": id,
        "region": {"mode": "template_relative", "anchor_target_id": "icon/badge",
                   "offset": {"x": dx, "y": 0}, "width": 20, "height": 8},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["DONE"],
        "case_sensitive": false, "minimum_confidence": 0.5, "model_ref": "PP-OCRv6_medium",
        "model_sha256": MODEL_SHA256
    });
    if locate {
        target["locate"] = json!({
            "normalize": "identity_v1", "blocks": "detect", "line_merge": {"gap_factor": 0}
        });
    }
    target
}

fn evaluator(provider: Arc<Words>) -> RecognitionEvaluator {
    let marker = |id: &str, x: u32, rgb: [u8; 3]| {
        json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
               "expected": rgb})
    };
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": WIDTH, "height": HEIGHT},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                marker("page/detail", 0, [0, 0, 255]),
                marker("page/other", 1, [0, 255, 0]),
                marker("page/terminal", 2, [255, 255, 0]),
                {"type": "template", "id": "icon/badge", "template_path": "badge.png",
                 "region": {"x": 16, "y": 0, "width": 16, "height": 12}, "threshold": 0.9},
                beside_badge("text/count", 10, false),
                beside_badge("text/find", 10, true),
                // Its region ends 16 pixels past the right edge of the frame.
                beside_badge("text/far", 40, false),
                {"type": "composite", "id": "check/count", "mode": "all_of",
                 "members": ["page/other", "text/count"]},
                {"type": "composite", "id": "check/either", "mode": "any_of",
                 "members": ["page/other", "text/count"]},
                {"type": "composite", "id": "check/badge", "mode": "all_of",
                 "members": ["page/detail", "icon/badge"]}
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    let png = Frame::from_pixels(
        BADGE_WIDTH,
        BADGE_HEIGHT,
        badge(),
        PixelFormat::Rgb8,
        CaptureBackendName::FixtureSimulation,
    )
    .expect("badge frame")
    .png_for_artifact()
    .expect("badge PNG");
    RecognitionEvaluator::with_vision_provider(pack, Arc::new(BadgeAsset(png)), provider)
        .expect("recognition evaluator")
}

fn detector(pages: Value) -> PageDetector {
    PageDetector::new(
        serde_json::from_value(json!({"schema_version": "0.6", "pages": pages})).expect("page set"),
    )
    .expect("page detector")
}

fn matched(pages: &[PageEvaluation]) -> Vec<&str> {
    pages
        .iter()
        .filter(|page| page.matched)
        .map(|page| page.page_id.as_str())
        .collect()
}

fn row<'a>(page: &'a PageEvaluation, target_id: &str) -> &'a PageTargetEvaluation {
    page.target_results
        .iter()
        .find(|target| target.target_id == target_id)
        .expect("the target's row")
}

// The required state of a target evaluation whose anchor `icon/badge` did not pass.
fn assert_anchor_missing(evaluation: &TargetEvaluation) {
    assert!(!evaluation.passed, "{evaluation:?}");
    assert!(
        evaluation
            .failure
            .as_ref()
            .is_some_and(|failure| failure.is(RecognitionPackCode::OcrRegionUnresolved)),
        "{evaluation:?}"
    );
    let region = &evaluation.ocr.as_ref().expect("the OCR evidence").region;
    assert_eq!(
        region.unresolved,
        Some(OcrRegionUnresolvedReason::AnchorNotMatched)
    );
    assert_eq!(region.anchor_target_id.as_deref(), Some("icon/badge"));
    assert_eq!(
        region.anchor_match.as_ref().map(|anchor| anchor.passed),
        Some(false)
    );
    assert_eq!(region.roi, None);
}

fn assert_row_anchor_missing(row: &PageTargetEvaluation) {
    assert!(!row.passed);
    assert_eq!(row.skipped, None, "the target was evaluated");
    assert_anchor_missing(&row.evaluation);
}

// R10-T1: page A requires its header and the count beside the badge, page B its own marker. On
// a frame without the badge page B matches and page A does not; the count did not pass with
// `ocr_region_unresolved` and `anchor_not_matched`, and the provider was not called.
#[test]
fn r10_a_missing_anchor_rules_out_only_its_page_and_calls_no_provider() {
    let provider = Words::new(vec![(COUNT_ROI, vec!["DONE"])]);
    let evaluator = evaluator(provider.clone());
    let detector = detector(json!([
        {"id": "neutral/detail", "required": ["page/detail", "text/count"]},
        {"id": "neutral/other", "required": ["page/other"]}
    ]));
    detector
        .validate(&evaluator)
        .expect("the page set is admitted");

    let pages = detector
        .evaluate_all(&evaluator, &scene(&[Shown::Detail, Shown::Other]))
        .expect("one page's missing anchor does not fail the recognition");

    assert_eq!(matched(&pages), ["neutral/other"]);
    assert!(row(&pages[0], "page/detail").passed);
    assert_row_anchor_missing(row(&pages[0], "text/count"));
    assert_eq!(provider.calls(), 0, "no OCR without the anchor");
}

// R10-T2: in `any_of` the count whose anchor is missing is one member that did not pass;
// another member may pass the group.
#[test]
fn r10_an_any_of_member_without_its_anchor_does_not_pass_and_another_member_may() {
    let provider = Words::new(vec![(COUNT_ROI, vec!["DONE"])]);
    let evaluator = evaluator(provider.clone());
    let detector = detector(json!([
        {"id": "neutral/detail", "required": ["page/detail"],
         "any_of": [["text/count", "page/other"]]}
    ]));

    for (shown, page_matched) in [
        (vec![Shown::Detail, Shown::Other], true),
        (vec![Shown::Detail], false),
    ] {
        let pages = detector
            .evaluate_all(&evaluator, &scene(&shown))
            .expect("the frame is recognized");

        assert_eq!(pages[0].matched, page_matched, "{shown:?}");
        assert_row_anchor_missing(row(&pages[0], "text/count"));
    }
    assert_eq!(provider.calls(), 0);
}

// R10-T3, option 2: in `forbidden`, directly or as a member of a check, an anchored OCR target
// (reading or locating) is refused unless its anchor is directly in the page's `required`.
// Admitted, a missing anchor rules the page out through `required`; with the anchor, the text
// rules the page out and its absence does not.
#[test]
fn r10_an_anchored_ocr_target_in_forbidden_needs_its_anchor_directly_in_required() {
    let admission = evaluator(Words::new(vec![(COUNT_ROI, vec!["DONE"])]));
    for (case, page) in [
        (
            "directly",
            json!({"id": "neutral/detail", "required": ["page/detail"], "forbidden": ["text/count"]}),
        ),
        (
            "a member of a check",
            json!({"id": "neutral/detail", "required": ["page/detail"], "forbidden": ["check/either"]}),
        ),
        (
            "a locating target",
            json!({"id": "neutral/detail", "required": ["page/detail"], "forbidden": ["text/find"]}),
        ),
        (
            "the anchor optional",
            json!({"id": "neutral/detail", "required": ["page/detail"], "optional": ["icon/badge"],
                   "forbidden": ["text/count"]}),
        ),
        (
            "the anchor inside a required check",
            json!({"id": "neutral/detail", "required": ["check/badge"], "forbidden": ["text/count"]}),
        ),
    ] {
        let error = detector(json!([{"id": "neutral/other", "required": ["page/other"]}, page]))
            .validate(&admission)
            .expect_err(case);
        let issue = error.declaration_issue().expect(case);
        assert_eq!(issue.field_path, "/pages/1/forbidden/0", "{case}");
        assert_eq!(
            issue.reason,
            ResourceDeclarationReason::InvalidValue,
            "{case}"
        );
    }

    for forbidden in ["text/count", "check/either", "text/find"] {
        detector(
            json!([{"id": "neutral/detail", "required": ["page/detail", "icon/badge"],
                         "forbidden": [forbidden]}]),
        )
        .validate(&admission)
        .expect(forbidden);
    }

    let pages = json!([{"id": "neutral/detail", "required": ["page/detail", "icon/badge"],
                        "forbidden": ["text/count"]}]);
    for (shown, words, page_matched, calls) in [
        (vec![Shown::Detail], vec!["DONE"], false, 0),
        (vec![Shown::Detail, Shown::Badge], vec!["DONE"], false, 1),
        (vec![Shown::Detail, Shown::Badge], vec![], true, 1),
    ] {
        let provider = Words::new(vec![(COUNT_ROI, words.clone())]);
        let evaluator = evaluator(provider.clone());

        let page = detector(pages.clone())
            .evaluate_page(&evaluator, &scene(&shown), "neutral/detail")
            .expect("the page is evaluated");

        assert_eq!(page.matched, page_matched, "{shown:?} {words:?}");
        assert_eq!(provider.calls(), calls, "{shown:?} {words:?}");
        if calls == 0 {
            assert!(!row(&page, "icon/badge").passed);
            assert_row_anchor_missing(row(&page, "text/count"));
        } else {
            assert!(row(&page, "icon/badge").passed);
            assert_eq!(row(&page, "text/count").passed, !page_matched);
        }
    }
}

// A device that always shows one frame, records every input and keeps the guard's evaluation.
struct ShowRuntime {
    frame: Frame,
    inputs: Vec<InputAction>,
    guard: Vec<RecognitionPackResult<TargetEvaluation>>,
}

impl ShowRuntime {
    fn new(frame: Frame) -> Self {
        Self {
            frame,
            inputs: Vec::new(),
            guard: Vec::new(),
        }
    }
}

impl ContainedTaskRuntime for ShowRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        Ok(self
            .frame
            .try_clone()
            .expect("copy the fixture frame")
            .into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        Ok(())
    }

    fn record_guard_evaluation(
        &mut self,
        _target_id: Option<&str>,
        result: Option<&RecognitionPackResult<TargetEvaluation>>,
        _evaluation: ContainedTaskGuardEvaluation,
    ) -> Result<(), Self::Error> {
        self.guard.extend(result.cloned());
        Ok(())
    }
}

// One operation from `other` to `terminal`, the task's target page.
fn task(pages: Value, operation: Value, provider: Arc<Words>) -> PreparedContainedTask {
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": PHASED_CONTROL_SCHEMA,
        "package_id": "neutral.anchor.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": WIDTH, "height": HEIGHT},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "timeout_ms": 1000
    }))
    .expect("task control");
    let program: TaskProgram = serde_json::from_value(json!({
        "schema_version": "0.9", "task_id": "task", "game": "neutral", "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": HEIGHT}, "target_page": "terminal",
        "operations": [operation]
    }))
    .expect("task program");
    PreparedContainedTask {
        control,
        program,
        evaluator: evaluator(provider),
        detector: detector(pages),
        entry_page: None,
        scheduling_outcome: None,
        post_admission_ocr: None,
        post_admission_fields: None,
        entry_count: 1,
        task_count: 1,
    }
}

// R10-T4: a guard on an anchored OCR target, as `guard.check` (a check holding it) or as the
// located text of `verify_text`, refuses its input when the anchor is missing: the guard's
// evaluation is a not-passed result (no evaluation error), there is no input and no provider
// call.
#[test]
fn r10_a_guard_on_an_ocr_target_without_its_anchor_refuses_with_zero_input() {
    let click = json!({"x": 50, "y": 4, "width": 8, "height": 8});
    let checked = json!({
        "id": "confirm", "from": "other", "to": "terminal",
        "click": {"kind": "rect", "x": 50, "y": 4, "width": 8, "height": 8},
        "guard": {"page_id": "other", "target_id": "check/count",
                  "expected_rect": click.clone(), "check": "check/count"}
    });
    let located = json!({
        "id": "confirm", "from": "other", "to": "terminal",
        "click": {"kind": "target", "target_id": "text/find"},
        "guard": {"page_id": "other", "target_id": "text/find", "expected_rect": click,
                  "verify_text": "text/find"}
    });
    let pages = json!([
        {"id": "neutral/other", "required": ["page/other"]},
        {"id": "neutral/terminal", "required": ["page/terminal"]}
    ]);
    for (operation, target, detail) in [
        (
            checked,
            "check/count",
            "operation=confirm target=check/count".to_owned(),
        ),
        (
            located,
            "text/find",
            format!(
                "operation=confirm target=text/find cause={}",
                RecognitionPackCode::OcrRegionUnresolved.code().as_str()
            ),
        ),
    ] {
        let provider = Words::new(vec![(COUNT_ROI, vec!["DONE"])]);
        let task = task(pages.clone(), operation, provider.clone());
        let mut runtime = ShowRuntime::new(frame(&[Shown::Other]));

        let Err(ContainedTaskRunError::Task(error)) = task.run(&mut runtime) else {
            panic!("the guard on {target} refuses the input");
        };

        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::ContainedTaskGuardRefused.as_str(),
            "{target}"
        );
        assert_eq!(error.detail(), Some(detail.as_str()));
        assert!(runtime.inputs.is_empty(), "zero inputs");
        assert_eq!(provider.calls(), 0, "no OCR without the anchor");
        let [Ok(guard)] = runtime.guard.as_slice() else {
            panic!("one guard evaluation that concluded: {:?}", runtime.guard);
        };
        assert!(!guard.passed);
        match &guard.composite {
            Some(check) => {
                let member = check
                    .members
                    .iter()
                    .find(|member| member.target_id == "text/count")
                    .expect("the count member");
                assert!(!member.passed);
                assert_anchor_missing(&member.evaluation);
            }
            None => assert_anchor_missing(guard),
        }
    }
}

// R10-T5: an anchor that matched with its region out of the frame stays the evaluation error of
// today: the page's outcome is that error, and the page recognition fails.
#[test]
fn r10_an_anchor_that_matched_with_its_region_out_of_frame_stays_an_error() {
    let provider = Words::new(Vec::new());
    let evaluator = evaluator(provider.clone());
    let detector = detector(json!([
        {"id": "neutral/detail", "required": ["page/detail", "text/far"]},
        {"id": "neutral/other", "required": ["page/other"]}
    ]));
    let scene = scene(&[Shown::Detail, Shown::Badge, Shown::Other]);

    let outcomes = detector
        .evaluate_all_outcomes(&evaluator, &scene)
        .expect("the batch runs every page");

    let Err(error) = &outcomes[0].result else {
        panic!("the out-of-frame region is an error: {:?}", outcomes[0]);
    };
    let failed = error.failed_target.as_ref().expect("the failed target");
    assert_eq!(failed.target_id, "text/far");
    assert_eq!(
        failed.cause.code(),
        RecognitionPackErrorCode::RegionUnresolved
    );
    assert_eq!(
        failed.cause.region().and_then(|region| region.unresolved),
        Some(OcrRegionUnresolvedReason::OutOfFrame)
    );
    assert!(outcomes[1].result.as_ref().is_ok_and(|page| page.matched));
    detector
        .evaluate_all(&evaluator, &scene)
        .expect_err("the page recognition fails as before");
    assert_eq!(provider.calls(), 0);
}

// R10-T6: the observation's page recognition (actinglab observe, `ReadonlyRecognitionEngine`)
// and the package dry run (`simulate_contained_task`) share the evaluator, so on a frame where
// one page's anchor is missing both decide the same page, and the dry run goes on to its click.
#[test]
fn r10_the_observation_and_the_dry_run_decide_the_same_page_without_the_anchor() {
    let provider = Words::new(vec![(COUNT_ROI, vec!["DONE"])]);
    let click = json!({"x": 50, "y": 4, "width": 8, "height": 8});
    let task = task(
        json!([
            {"id": "neutral/detail", "required": ["page/detail", "text/count"]},
            {"id": "neutral/other", "required": ["page/other"]},
            {"id": "neutral/terminal", "required": ["page/terminal"]}
        ]),
        json!({
            "id": "confirm", "from": "other", "to": "terminal",
            "click": {"kind": "rect", "x": 50, "y": 4, "width": 8, "height": 8},
            "guard": {"page_id": "other", "target_id": "page/other", "expected_rect": click,
                      "color_probe": "page/other"}
        }),
        provider.clone(),
    );
    let shown = [Shown::Detail, Shown::Other];

    let observed = crate::ReadonlyRecognitionEngine::new(task.evaluator.clone(), Vec::new())
        .current_page(&task.detector, &scene(&shown))
        .expect("the observation recognizes the frame");
    let simulated = crate::simulate_contained_task(&task, vec![frame(&shown)])
        .expect("the dry run simulates the frame");

    assert!(observed.matched);
    assert_eq!(observed.page, "neutral/other");
    assert_eq!(
        simulated
            .recognition
            .first()
            .and_then(|recognition| recognition.matched_page.as_deref()),
        Some(observed.page.as_str())
    );
    assert!(
        matches!(
            &simulated.decision,
            crate::OfflineDecision::WouldClick { operation_label, .. } if operation_label == "confirm"
        ),
        "{:?}",
        simulated.decision
    );
    assert_eq!(provider.calls(), 0);
}
