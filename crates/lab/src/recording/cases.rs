// SPDX-License-Identifier: AGPL-3.0-only

//! #463 S2-10 (`contracts/lab-recording.md`, "Regression cases written with the package";
//! `contracts/resource-regression-case.md`): the regression cases `record stop` writes beside the
//! package it generated, each run first by the one runner of the case contract.
//!
//! - **What a case is made from.** The recording knows the screens its package must recognise: one
//!   case per effective step with a press and a page, on the frame its press was sent on; one for
//!   the final screen, the last step's primary frame, expecting the terminal page; one per ending,
//!   on the frame it was taken from, expecting the ending's page. A dropped step is no effective
//!   step and has none; the application entry step, which has no page and no frame, has none. A
//!   case expects its page (`expect.observe.page`, as the package's page detection names it) and
//!   every recognition target that page requires to pass (`expect.recognize`).
//! - **The scene** is a black canvas of the recording's size on which are pasted, where they are in
//!   the frame, the regions every recognition target of the whole package reads, as the admitted
//!   package declares them ([`read_regions`]), each rectangle once with the targets that read it:
//!   never a whole frame and nothing outside those regions, so that a page told apart from the
//!   case's page by a target it forbids sees that target's region as on the frame.
//! - **Each case is run before it is written**, by [`run_regression_case_on_bundle`] on the final
//!   package admitted in memory: `failed` is a failure of the self-check `cases` and the case is
//!   not written; `passed` is written; `partial` and `not_run` are written and named with the
//!   runner's reasons. The bare canvas, with no patch at all, is run once per package: a case whose
//!   page it answers too is written and named with the warning `record_case_bare_canvas`.
//!
//! Nothing here writes a file: `record stop` writes the set ([`CaseSet::files`]) beside the package,
//! and `record ready` lists the failures.

use super::crosscheck::{Failures, FrameCache, Locus, evaluator_bundle};
use super::frames::LoadedFrame;
use super::generate::{Plan, PlanStep};
use super::model::{RecordSize, RecordedFrame};
use super::ready::press_frame;
use super::store::{blocked, hex_sha256, invalid, with_details};
use crate::LabOcr;
use crate::codes::{CaseLimit, LabCode, RegressionNotRun, RegressionResult, RegressionVerdict};
use crate::regression_case::{
    REGRESSION_CASE_FILE, REGRESSION_CASE_SCHEMA, RegressionCaseVerdict, RegressionCheck,
    run_regression_case_on_bundle,
};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::page_anchor_matches;
use actingcommand_pack_containment::LoadedBundle;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::{PackRect, PackRegion, RecognitionPack, RecognitionTarget};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The colour of a case's canvas, black, as the repositories' cases fill theirs.
const CANVAS_FILL: [u8; 3] = [0, 0, 0];
/// How a case's scene is made (`scene_construction.kind`): patches of a frame on a canvas.
const SCENE_KIND: &str = "roi_patch";
/// A case's scene file, beside its `case.json`.
const SCENE_FILE: &str = "scene.png";
/// The id of the bare canvas's case, run once per package and never written.
const BARE_CANVAS_ID: &str = "bare_canvas";
/// The id of the case of the final screen.
const FINAL_ID: &str = "final";
/// The text of the limit `self_agreement` (`case_limit`).
const SELF_AGREEMENT_TEXT: &str = "These cases were made from the frames this package was built \
    from: that they pass shows the package recognises the screens it was recorded on and says \
    nothing about screens it did not meet.";
/// The text of the warning `record_case_bare_canvas`.
const BARE_CANVAS_MESSAGE: &str = "The page this case expects is also what the package's page \
    detection answers on the bare canvas, with no patch at all: the case proves nothing of its \
    page. Give the page a mark of a bright or textured area of its own screen to make it prove it.";

/// What a case is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseKind {
    /// An effective step with a press and a page, on the frame its press was sent on.
    Step,
    /// The final screen: the last step, on its primary frame.
    Final,
    /// An ending, on the frame it was taken from.
    Ending,
}

/// One case to write: its id, what it is made from, its `case.json` and its scene.
#[derive(Debug, Clone)]
pub(crate) struct WrittenCase {
    id: String,
    kind: CaseKind,
    case_json: Vec<u8>,
    scene_png: Vec<u8>,
}

/// The cases of a package that its run did not fail, in the order the recording gives them (the
/// steps in path order, the final screen, the endings in the order taken); the cases among them
/// that did not prove everything (`partial`, `not_run`) with the runner's reasons; and the
/// bare-canvas warnings.
#[derive(Debug, Clone, Default)]
pub(crate) struct CaseSet {
    cases: Vec<WrittenCase>,
    not_proven: Vec<Value>,
    warnings: Vec<Value>,
}

impl CaseSet {
    /// Every file of the cases' directory, `/`-separated below it: `<id>/case.json` and
    /// `<id>/scene.png` of each case.
    pub(crate) fn files(&self) -> Vec<(String, Vec<u8>)> {
        self.cases
            .iter()
            .flat_map(|case| {
                [
                    (
                        format!("{}/{REGRESSION_CASE_FILE}", case.id),
                        case.case_json.clone(),
                    ),
                    (format!("{}/{SCENE_FILE}", case.id), case.scene_png.clone()),
                ]
            })
            .collect()
    }

    /// The member `cases` of `record stop`'s answer: the directory, its `status` (`written`,
    /// `replaced`; under the dry run `to_write`, `to_replace`), the directory an earlier set was
    /// set aside as (`kept`), how many cases there are by kind, their ids, the cases not proven
    /// with the runner's reasons, the bare-canvas warnings, and the limit `self_agreement` with
    /// its text.
    pub(crate) fn answer(&self, directory: String, status: &str, kept: Option<String>) -> Value {
        let count = |kind: CaseKind| self.cases.iter().filter(|case| case.kind == kind).count();
        let limit = CaseLimit::SelfAgreement.as_str();
        let mut texts = Map::new();
        texts.insert(limit.to_string(), json!(SELF_AGREEMENT_TEXT));
        let mut answer = Map::new();
        answer.insert("directory".to_string(), json!(directory));
        answer.insert("status".to_string(), json!(status));
        if let Some(kept) = kept {
            answer.insert("kept".to_string(), json!(kept));
        }
        answer.insert(
            "counts".to_string(),
            json!({
                "steps": count(CaseKind::Step),
                "final": count(CaseKind::Final),
                "endings": count(CaseKind::Ending)
            }),
        );
        answer.insert(
            "ids".to_string(),
            json!(
                self.cases
                    .iter()
                    .map(|case| case.id.as_str())
                    .collect::<Vec<_>>()
            ),
        );
        answer.insert("not_proven".to_string(), json!(self.not_proven));
        answer.insert("warnings".to_string(), json!(self.warnings));
        answer.insert("limits".to_string(), json!([limit]));
        answer.insert("limit_texts".to_string(), Value::Object(texts));
        Value::Object(answer)
    }
}

/// The name of the directory beside the package that holds its cases, `<package_id>.cases`.
pub(crate) fn cases_dir_name(package_id: &str) -> String {
    format!("{package_id}.cases")
}

/// A rectangle of the frame a case's scene holds, in the recording's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Patch {
    y: u32,
    x: u32,
    height: u32,
    width: u32,
}

/// Where a case comes from: its id, what it is made from, the package's page it expects (as the
/// package names it, `step_01`), the targets that page requires, the frame, its `source` member
/// and where a failure of it is.
struct Source<'p> {
    id: String,
    kind: CaseKind,
    page: String,
    targets: Vec<String>,
    frame: &'p RecordedFrame,
    source: Value,
    locus: Locus,
}

/// Self-check `cases`, the last of `record stop`'s self-checks: the regression cases of the package
/// `plan` generated, whose final files are `entries` (admitted here as the package `record stop`
/// writes), on the recording's frames `cache`, each run by [`run_regression_case_on_bundle`] with
/// `ocr`. A case whose verdict is `failed` is a failure `failures` meets (the step or the ending,
/// the frame, the page, the case and its failed checks) and is not in the set; under `record ready`
/// the next case is still made and run.
pub(crate) fn regression_cases(
    plan: &Plan,
    entries: &BTreeMap<String, Vec<u8>>,
    cache: &FrameCache,
    ocr: &LabOcr,
    failures: &mut Failures,
) -> LabResult<CaseSet> {
    let admitted = evaluator_bundle(entries, ocr)?;
    let bundle = admitted.loaded_bundle();
    let evaluator = bundle.evaluator().ok_or_else(|| {
        invalid(
            "validation_failed",
            "the generated package gives no recognition evaluator",
        )
    })?;
    let detector = bundle.detector().ok_or_else(|| {
        invalid(
            "validation_failed",
            "the generated package gives no page set",
        )
    })?;
    let regions = read_regions(evaluator.pack(), plan.size)?;
    let bare = bare_canvas_pages(plan, bundle, ocr)?;
    let mut set = CaseSet::default();
    for source in sources(plan)? {
        let page = detected_page(detector, &plan.game, &source.page)?;
        let loaded = cache.get(&source.frame.sha256).ok_or_else(|| {
            invalid(
                "validation_failed",
                format!("frame {} was not loaded", source.frame.frame_id),
            )
        })?;
        let scene = scene_of(loaded, &regions, plan.size)?;
        let mut expect = Map::new();
        expect.insert("observe".to_string(), json!({"page": page}));
        if !source.targets.is_empty() {
            expect.insert(
                "recognize".to_string(),
                Value::Array(
                    source
                        .targets
                        .iter()
                        .map(|target| json!({"target": target, "passed": true}))
                        .collect(),
                ),
            );
        }
        let document = case_document(
            plan,
            &source.id,
            &source.source,
            &regions,
            &scene,
            Value::Object(expect),
        )?;
        let verdict = run_regression_case_on_bundle(bundle, &document, &scene, ocr);
        match verdict.verdict {
            RegressionVerdict::Failed => {
                let failed = verdict
                    .checks
                    .iter()
                    .filter(|check| check.result == RegressionResult::Failed)
                    .cloned()
                    .collect::<Vec<_>>();
                let refusal = case_failed(&source, &failed);
                failures.fail(source.locus.with_case(&source.id, failed), refusal)?;
                continue;
            }
            RegressionVerdict::Passed => {}
            RegressionVerdict::Partial | RegressionVerdict::NotRun => {
                set.not_proven.push(json!({
                    "case": source.id,
                    "verdict": verdict.verdict.as_str(),
                    "reasons": not_run_reasons(&verdict)
                }));
            }
        }
        if bare.contains(&page) {
            set.warnings.push(json!({
                "code": LabCode::RecordCaseBareCanvas.as_str(),
                "case": source.id,
                "page": page,
                "message": BARE_CANVAS_MESSAGE
            }));
        }
        set.cases.push(WrittenCase {
            id: source.id,
            kind: source.kind,
            case_json: document,
            scene_png: scene,
        });
    }
    Ok(set)
}

/// The cases the recording gives, in its order: each step before the last with a page (every one
/// has a press: the pre-checks hold it), on the frame its press was sent on; the last step, the
/// final screen, on its primary frame; each ending, on the frame it was taken from. Each case's
/// `source` names the recording and the step (its serial number), `final` or the ending (its key),
/// nothing else.
fn sources(plan: &Plan) -> LabResult<Vec<Source<'_>>> {
    let (last, before) = plan
        .steps
        .split_last()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    let targets = |ids: &[String]| ids.iter().map(|id| plan.target_id(id)).collect::<Vec<_>>();
    let mut sources = Vec::new();
    for step in before {
        // The application entry step has no page and no frame.
        let Some(page) = &step.page else {
            continue;
        };
        let frame = pressed_on(step)?;
        sources.push(Source {
            id: format!("step_{}", plan.numbered(step.number)),
            kind: CaseKind::Step,
            page: page.clone(),
            targets: targets(step.required.as_slice()),
            frame,
            source: json!({"record_id": plan.record_id, "step": step.step.index}),
            locus: Locus::on_step(step).with_frame(frame).with_page(page),
        });
    }
    let page = last.page_id()?;
    let frame = last.primary().ok_or_else(|| frame_missing(last))?;
    sources.push(Source {
        id: FINAL_ID.to_string(),
        kind: CaseKind::Final,
        page: page.to_string(),
        targets: targets(last.required.as_slice()),
        frame,
        source: json!({"record_id": plan.record_id, "final": true}),
        locus: Locus::on_step(last).with_frame(frame).with_page(page),
    });
    for ending in &plan.endings {
        let key = &ending.ending.outcome_key;
        let frames = ending.live_frames();
        let frame = frames
            .iter()
            .copied()
            .find(|frame| frame.role == "primary")
            .or_else(|| frames.first().copied())
            .ok_or_else(|| {
                blocked(
                    "record_step_frame_missing",
                    format!("the ending '{key}' has no live frame"),
                )
            })?;
        sources.push(Source {
            id: format!("ending_{key}"),
            kind: CaseKind::Ending,
            page: ending.page.clone(),
            targets: targets(ending.required.as_slice()),
            frame,
            source: json!({"record_id": plan.record_id, "ending": key}),
            locus: Locus::on_ending(ending).with_frame(frame),
        });
    }
    Ok(sources)
}

/// The frame `step`'s press was sent on: the stored frame its click's or its application
/// operation's `before_frame` names (`ready::press_frame`) while it is one of the step's live
/// frames, else (a press declared offline, or a step whose frame was replaced since) the step's
/// primary frame.
fn pressed_on(step: &PlanStep) -> LabResult<&RecordedFrame> {
    let live = step.live_frames();
    press_frame(&step.step)
        .and_then(|id| live.iter().copied().find(|frame| frame.frame_id == id))
        .or_else(|| step.primary())
        .ok_or_else(|| frame_missing(step))
}

fn frame_missing(step: &PlanStep) -> LabError {
    blocked(
        "record_step_frame_missing",
        format!("step {} has no live frame", step.step.index),
    )
}

/// The page of the package's page set that the package's page `page` is, as the page detection
/// names it (`<game>/<page>`), found by the kernel's own match of a page anchor.
fn detected_page(detector: &PageDetector, game: &str, page: &str) -> LabResult<String> {
    let found = detector
        .page_ids()
        .filter(|id| page_anchor_matches(game, id, page))
        .collect::<Vec<_>>();
    match found.as_slice() {
        [one] => Ok((*one).to_string()),
        _ => Err(invalid(
            "validation_failed",
            format!("the generated package's page set holds no one page {page}"),
        )),
    }
}

/// Every rectangle the recognition targets of the admitted package `pack` read, each once with the
/// targets that read it in the package's order: a template's search region (and its colour check's
/// region), a colour or colour digest probe's region, an OCR or NN target's region. A check
/// (`all_of`, `any_of`), a value test and a click-only target read nothing of their own: a check's
/// members and a value test's sides are targets of the package themselves. A region that is no
/// fixed rectangle inside the recording's frame (`size`) cannot be pasted where it is read, and is
/// the check's error.
fn read_regions(
    pack: &RecognitionPack,
    size: RecordSize,
) -> LabResult<BTreeMap<Patch, Vec<String>>> {
    let mut regions: BTreeMap<Patch, Vec<String>> = BTreeMap::new();
    for target in &pack.targets {
        let read = match target {
            RecognitionTarget::Template(template) => {
                let mut read = vec![(&template.id, fixed(&template.id, &template.region)?)];
                if let Some(check) = &template.color_check {
                    read.push((&template.id, fixed(&template.id, &check.region)?));
                }
                read
            }
            RecognitionTarget::Color(color) => vec![(&color.id, color.region)],
            RecognitionTarget::ColorDigest(digest) => vec![(&digest.id, digest.region)],
            RecognitionTarget::Ocr(ocr) => vec![(&ocr.id, fixed(&ocr.id, &ocr.region)?)],
            RecognitionTarget::Nn(nn) => vec![(&nn.id, fixed(&nn.id, &nn.region)?)],
            RecognitionTarget::ClickOnly(_)
            | RecognitionTarget::Composite(_)
            | RecognitionTarget::ValueTest(_) => Vec::new(),
        };
        for (id, rect) in read {
            let readers = regions.entry(patch(id, rect, size)?).or_default();
            if !readers.contains(id) {
                readers.push(id.clone());
            }
        }
    }
    Ok(regions)
}

/// The fixed rectangle `region` of the target `id`.
fn fixed(id: &str, region: &PackRegion) -> LabResult<PackRect> {
    match region {
        PackRegion::Rect(rect) => Ok(*rect),
        PackRegion::TemplateRelative(_) | PackRegion::Keyword(_) => Err(invalid(
            "validation_failed",
            format!(
                "target '{id}' reads a region that is no fixed rectangle: a case's scene pastes \
                 fixed regions only"
            ),
        )),
    }
}

/// `rect`, which the target `id` reads, as a patch inside the recording's frame of `size`.
fn patch(id: &str, rect: PackRect, size: RecordSize) -> LabResult<Patch> {
    let inside = |start: i32, length: i32, bound: u32| {
        u32::try_from(start)
            .ok()
            .zip(u32::try_from(length).ok())
            .filter(|&(start, length)| {
                length >= 1 && u64::from(start) + u64::from(length) <= u64::from(bound)
            })
    };
    match (
        inside(rect.x, rect.width, size.width),
        inside(rect.y, rect.height, size.height),
    ) {
        (Some((x, width)), Some((y, height))) => Ok(Patch {
            y,
            x,
            height,
            width,
        }),
        _ => Err(invalid(
            "validation_failed",
            format!(
                "target '{id}' reads ({}, {}, {}, {}), which does not lie inside the {}x{} frame",
                rect.x, rect.y, rect.width, rect.height, size.width, size.height
            ),
        )),
    }
}

/// The scene of the frame `loaded`: a canvas of the recording's `size` in [`CANVAS_FILL`] with
/// every rectangle of `regions` pasted from the frame where it is, and nothing else of the frame.
fn scene_of(
    loaded: &LoadedFrame,
    regions: &BTreeMap<Patch, Vec<String>>,
    size: RecordSize,
) -> LabResult<Vec<u8>> {
    if (loaded.scene.width(), loaded.scene.height()) != (size.width, size.height) {
        return Err(invalid(
            "validation_failed",
            format!(
                "a frame of {}x{} is not of the recording's size {}x{}",
                loaded.scene.width(),
                loaded.scene.height(),
                size.width,
                size.height
            ),
        ));
    }
    let width = size.width as usize;
    let frame = loaded.scene.rgb8_pixels();
    let mut pixels = CANVAS_FILL.repeat(width * size.height as usize);
    for patch in regions.keys() {
        for row in patch.y..patch.y + patch.height {
            let start = (row as usize * width + patch.x as usize) * 3;
            let end = start + patch.width as usize * 3;
            pixels[start..end].copy_from_slice(&frame[start..end]);
        }
    }
    png(size, pixels)
}

/// The PNG of `pixels`, RGB rows of a frame of `size`.
fn png(size: RecordSize, pixels: Vec<u8>) -> LabResult<Vec<u8>> {
    Frame::from_pixels(
        size.width,
        size.height,
        pixels,
        PixelFormat::Rgb8,
        CaptureBackendName::AdbScreencap,
    )
    .and_then(|frame| frame.encode_png_fast())
    .map_err(|error| {
        invalid(
            "validation_failed",
            format!("failed to encode a case's scene: {error}"),
        )
    })
}

/// The `case.json` of the case `id` of `plan`'s package, its scene `scene_png` built of `regions`,
/// its `source` and its `expect`, pretty-printed with a final newline.
fn case_document(
    plan: &Plan,
    id: &str,
    source: &Value,
    regions: &BTreeMap<Patch, Vec<String>>,
    scene_png: &[u8],
    expect: Value,
) -> LabResult<Vec<u8>> {
    let rects = regions
        .iter()
        .map(|(patch, readers)| {
            json!({
                "x": patch.x,
                "y": patch.y,
                "width": patch.width,
                "height": patch.height,
                "content": readers.join(", ")
            })
        })
        .collect::<Vec<_>>();
    let document = json!({
        "schema_version": REGRESSION_CASE_SCHEMA,
        "id": id,
        "package_id": plan.package_id,
        "scene": SCENE_FILE,
        "scene_construction": {
            "kind": SCENE_KIND,
            "canvas": {"width": plan.size.width, "height": plan.size.height, "fill": CANVAS_FILL},
            "rects": rects,
            "scene_sha256": hex_sha256(scene_png)
        },
        "source": source,
        "expect": expect
    });
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(|error| {
        invalid(
            "validation_failed",
            format!("failed to encode a case: {error}"),
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The pages the package's page detection answers on the bare canvas, the canvas of the recording's
/// size with no patch at all, run once per package by the one runner as a case that expects no page:
/// the page it recognised, or every page of a conflict; none when the run could not evaluate the
/// page (text recognition without a model).
fn bare_canvas_pages(
    plan: &Plan,
    bundle: &LoadedBundle,
    ocr: &LabOcr,
) -> LabResult<BTreeSet<String>> {
    let size = plan.size;
    let scene = png(
        size,
        CANVAS_FILL.repeat(size.width as usize * size.height as usize),
    )?;
    let document = case_document(
        plan,
        BARE_CANVAS_ID,
        &json!({"record_id": plan.record_id, "bare_canvas": true}),
        &BTreeMap::new(),
        &scene,
        json!({"observe": {"page": null}}),
    )?;
    let verdict = run_regression_case_on_bundle(bundle, &document, &scene, ocr);
    Ok(verdict
        .checks
        .iter()
        .filter_map(|check| check.page.as_ref())
        .flat_map(|page| page.recognized.iter().chain(&page.matched_pages).cloned())
        .collect())
}

/// The runner's reasons, each once, for the checks of `verdict` that did not run.
fn not_run_reasons(verdict: &RegressionCaseVerdict) -> Vec<&'static str> {
    verdict
        .checks
        .iter()
        .filter_map(|check| check.reason)
        .map(RegressionNotRun::as_str)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// `record_case_failed`: the case of `source` failed the checks `failed` on the package; its
/// `regression_problem` is the word of the first.
fn case_failed(source: &Source<'_>, failed: &[RegressionCheck]) -> LabError {
    let Some(problem) = failed.iter().find_map(|check| check.problem) else {
        return invalid(
            "validation_failed",
            format!(
                "the regression case {} failed without a failed check",
                source.id
            ),
        );
    };
    let mut details = json!({
        "case": source.id,
        "page": source.page,
        "frame_id": source.frame.frame_id,
        "verdict": RegressionVerdict::Failed.as_str(),
        "checks": failed,
        "outcome": Outcome::new(LabCode::RecordCaseFailed)
            .regression_problem(problem)
            .envelope(Detail::Full)
    });
    if let Some(object) = details.as_object_mut() {
        if let Some((step, artifact_step)) = source.locus.step {
            object.insert("step".to_string(), json!(step));
            object.insert("artifact_step".to_string(), json!(artifact_step));
        }
        if let Some(key) = &source.locus.outcome_key {
            object.insert("outcome_key".to_string(), json!(key));
        }
    }
    with_details(
        blocked(
            LabCode::RecordCaseFailed.as_str(),
            format!(
                "the regression case {} that record stop made from frame {} for page {} does not \
                 pass on the package it generated: it is not written, and nothing was written; \
                 record ready lists every such case with the runner's checks",
                source.id, source.frame.frame_id, source.page
            ),
        ),
        details,
    )
}
