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
//!   package declares them ([`read_regions`]), each rectangle once with the targets that read it,
//!   and nothing outside those regions, so that a page told apart from the case's page by a target
//!   it forbids sees that target's region as on the frame. Fix round 3: where a target's region or
//!   search area is the whole frame, the scene is the whole frame; each case is then named with the
//!   warning `record_case_whole_frame` and the targets that read it, and nothing is refused.
//! - **Each case is run before it is written**, by [`run_regression_case_on_bundle`] on the final
//!   package admitted in memory: `failed` is a failure of the self-check `cases` and the case is
//!   not written; `passed` is written; `partial` and `not_run` are written and named with the
//!   runner's reasons. The bare canvas, with no patch at all, is run once per package: a case whose
//!   page it answers too is written and named with the warning `record_case_bare_canvas`. Fix round
//!   3: without a text-recognition model for a text target the page detection reads, no case's page
//!   is judged, which the warning `record_cases_ocr_unavailable` says once.
//! - **Ambiguity is no miss** (the coordinator's ruling after the first round, R2's ruling 4 as its
//!   precedent): a press step of a linear run waits for its own page and does not ask which page of
//!   the whole package a screen is (#463 SC-1: a select step's confirmation does, before its press,
//!   and `record stop`'s self-check item 8 judges a select step's page that is not alone on a
//!   recorded frame before the cases are made: on a frame that confirmation can meet a refusal, on a
//!   frame after its press, fix round 1, a warning). When the runner answers `page_conflict` and
//!   the case's page is among
//!   the pages matched, the case is made again without `expect.observe`: every target its page
//!   requires, `passed: true`, and (fix round 3) every target it forbids, `passed: false`, so that
//!   it states that this page holds on this scene; it is run again in that form before it is
//!   written (failing then is a miss) and named with the warning `record_case_page_ambiguous`. A
//!   page with no target to pin gives no case, named with the warning
//!   `record_case_ambiguous_not_written`. Both name the other pages, in `record stop`'s answer and
//!   in `record ready`'s.
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
use crate::codes::{
    CaseLimit, LabCode, RegressionNotRun, RegressionProblem, RegressionResult, RegressionVerdict,
};
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
use actingcommand_recognition_pack::{
    PackReadArea, PackReadRegion, PackReadingDeclarations, PackRect, RecognitionPack, TargetKind,
};
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
/// The text of the warning `record_case_page_ambiguous`, a case written without `observe`. #463
/// SC-1: what is so, a press step waits for its own page and a select step's confirmation asks;
/// fix round 1: what `record stop` does of it.
const AMBIGUOUS_MESSAGE: &str = "This case's page is recognised on its screen, but so are the \
    other pages named here: a press step of a linear run waits for its own page and does not ask \
    which page of the package a screen is, so the case is written without expect.observe. A select \
    step's confirmation does ask, before its press, and fails on a screen where two pages pass; \
    record stop refuses a select step's page that passes beside another page on a frame that \
    confirmation can meet (its own, and those of the step before it), and warns of one on a frame \
    after its press (record_select_page_passes_after_press). This case states that its page holds \
    on this screen (every target the page requires passes, every target it forbids does not), \
    which it passes. To make it prove its page too, tell the screens apart with a mark of this \
    screen or a forbidden mark on the other page.";
/// The text of the warning `record_case_ambiguous_not_written` (fix round 3).
const AMBIGUOUS_UNWRITTEN_MESSAGE: &str = "This case's page is recognised on its screen, but so \
    are the other pages named here, and the page has no recognition target to pin: without \
    expect.observe the case would declare nothing, so it is not written. Mark a target on this \
    page's screen and run record stop again to have it written.";
/// The text of the warning `record_case_whole_frame` (fix round 3).
const WHOLE_FRAME_MESSAGE: &str = "A recognition target of the package reads the whole frame \
    (the targets named here), so this case's scene is the whole frame, not patches of it: it holds \
    everything the screen showed. Nothing is refused for it; a target with a smaller region gives \
    a scene of patches.";
/// The text of the warning `record_cases_ocr_unavailable` (fix round 3).
const PAGES_UNJUDGED_MESSAGE: &str = "No case's page was judged: the Lab has no text-recognition \
    model and the package's page detection reads text, so every case was written without its page \
    judgement (no page was recognised, the bare canvas and the other pages judged nothing). The \
    cases are judged where a model is.";

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
/// that did not prove everything (`partial`, `not_run`) with the runner's reasons; the warnings
/// (fix round 3: `record_cases_ocr_unavailable` first, once, then each case's in that order:
/// `record_case_page_ambiguous` or `record_case_ambiguous_not_written`, `record_case_bare_canvas`,
/// `record_case_whole_frame`); and the two ambiguity warnings alone, which `record ready` names too.
#[derive(Debug, Clone, Default)]
pub(crate) struct CaseSet {
    cases: Vec<WrittenCase>,
    not_proven: Vec<Value>,
    warnings: Vec<Value>,
    ambiguous: Vec<Value>,
}

impl CaseSet {
    /// The warnings `record_case_page_ambiguous` and `record_case_ambiguous_not_written`, in the
    /// order of the cases, which `record ready` names as `record stop` does.
    pub(crate) fn ambiguity_warnings(&self) -> &[Value] {
        &self.ambiguous
    }

    /// The ids of the cases, in their order.
    pub(crate) fn ids(&self) -> Vec<&str> {
        self.cases.iter().map(|case| case.id.as_str()).collect()
    }

    /// The coordinator's ruling after the first round, fix round 3: the `expect` of the case `id`,
    /// whose page `page` is recognised on its scene beside the pages `others`, made without the
    /// page judgement: every target its page requires (`required`), `passed: true`, and every
    /// target it forbids that a scene can evaluate (`forbidden`), `passed: false`, so that the case
    /// states that this page holds on this scene. `None` when there is no such target: the case
    /// would declare nothing and is not written, and the warning
    /// `record_case_ambiguous_not_written` names it.
    pub(crate) fn without_page_judgement(
        &mut self,
        id: &str,
        page: &str,
        others: &[String],
        required: &[String],
        forbidden: &[String],
    ) -> Option<Value> {
        if required.is_empty() && forbidden.is_empty() {
            self.warn_ambiguity(json!({
                "code": LabCode::RecordCaseAmbiguousNotWritten.as_str(),
                "case": id,
                "page": page,
                "other_pages": others,
                "written": false,
                "message": AMBIGUOUS_UNWRITTEN_MESSAGE
            }));
            return None;
        }
        Some(expectation(None, required, forbidden))
    }

    /// The warning `record_case_page_ambiguous` of the case `id`, written without the page
    /// judgement, whose page `page` is recognised on its scene beside the pages `others`; fix round
    /// 3: `left_out` the targets its page forbids that a scene cannot evaluate, named when there
    /// is one.
    fn warn_ambiguous(&mut self, id: &str, page: &str, others: &[String], left_out: &[String]) {
        let mut warning = json!({
            "code": LabCode::RecordCasePageAmbiguous.as_str(),
            "case": id,
            "page": page,
            "other_pages": others,
            "written": true
        });
        if !left_out.is_empty() {
            warning["forbidden_left_out"] = json!(left_out);
        }
        warning["message"] = json!(AMBIGUOUS_MESSAGE);
        self.warn_ambiguity(warning);
    }

    /// The ambiguity warning `warning`, among the warnings and among those `record ready` names.
    fn warn_ambiguity(&mut self, warning: Value) {
        self.ambiguous.push(warning.clone());
        self.warnings.push(warning);
    }

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
    /// with the runner's reasons, the warnings, and the limit `self_agreement` with its text.
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
        answer.insert("ids".to_string(), json!(self.ids()));
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
pub(crate) struct Patch {
    pub(crate) y: u32,
    pub(crate) x: u32,
    pub(crate) height: u32,
    pub(crate) width: u32,
}

impl Patch {
    /// Fix round 3: the whole frame of the recording's `size`.
    fn whole(size: RecordSize) -> Self {
        Self {
            y: 0,
            x: 0,
            height: size.height,
            width: size.width,
        }
    }
}

/// Where a case comes from: its id, what it is made from, the package's page it expects (as the
/// package names it, `step_01`), the targets that page requires and (fix round 3) those it forbids,
/// the frame, its `source` member and where a failure of it is.
struct Source<'p> {
    id: String,
    kind: CaseKind,
    page: String,
    targets: Vec<String>,
    forbidden: Vec<String>,
    frame: &'p RecordedFrame,
    source: Value,
    locus: Locus,
}

/// Self-check `cases`, the last of `record stop`'s self-checks: the regression cases of the package
/// `plan` generated, whose final files are `entries` (admitted here as the package `record stop`
/// writes), on the recording's frames `cache`, each run by [`run_regression_case_on_bundle`] with
/// `ocr`. A case whose verdict is `failed` is a failure `failures` meets (the step or the ending,
/// the frame, the page, the case and its failed checks) and is not in the set; under `record ready`
/// the next case is still made and run. A page recognised beside another page of the package is no
/// failure ([`others_matched`]): the case is made again without `observe`
/// ([`CaseSet::without_page_judgement`]) and run again, and the warning
/// `record_case_page_ambiguous` names it. Fix round 3: a failed case's `page` is the page
/// detection's name, as its warnings', `<game>/<page>`; the warnings `record_case_whole_frame` and
/// `record_cases_ocr_unavailable`.
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
    // Fix round 3: the targets whose region or search area is the whole frame, which make every
    // scene the whole frame.
    let whole_frame = regions
        .get(&Patch::whole(plan.size))
        .cloned()
        .unwrap_or_default();
    let bare = bare_canvas_pages(plan, bundle, ocr)?;
    let mut set = CaseSet::default();
    // Fix round 3: without a text-recognition model for a text target the page detection reads
    // (the runner's own check, `LabOcr::require_pages`), the runner judges no case's page: said
    // once, before every case's warnings.
    if ocr.require_pages(evaluator.pack(), detector).is_err() {
        set.warnings.push(json!({
            "code": LabCode::RecordCasesOcrUnavailable.as_str(),
            "message": PAGES_UNJUDGED_MESSAGE
        }));
    }
    for source in sources(plan)? {
        let page = detected_page(detector, &plan.game, &source.page)?;
        let loaded = cache.get(&source.frame.sha256).ok_or_else(|| {
            invalid(
                "validation_failed",
                format!("frame {} was not loaded", source.frame.frame_id),
            )
        })?;
        let scene = scene_of(loaded, &regions, plan.size)?;
        let mut document = case_document(
            plan,
            &source.id,
            &source.source,
            &regions,
            &scene,
            expectation(Some(page.as_str()), &source.targets, &[]),
        )?;
        let mut verdict = run_regression_case_on_bundle(bundle, &document, &scene, ocr);
        // Fix round 3: a target the page forbids that a scene cannot evaluate (a click-only
        // target) is left out of a case made without the page judgement, and named.
        let (forbidden, left_out): (Vec<String>, Vec<String>) =
            source.forbidden.iter().cloned().partition(|target| {
                evaluator.target_kind(target).ok() != Some(TargetKind::ClickOnly)
            });
        // The coordinator's ruling (R2's ruling 4 as precedent): another page matching beside the
        // case's own is no miss. The case is made again without the page judgement and run again
        // in that form; a page with no target to pin gives no case.
        let others = others_matched(&verdict, &page);
        if let Some(others) = &others {
            let Some(expect) =
                set.without_page_judgement(&source.id, &page, others, &source.targets, &forbidden)
            else {
                continue;
            };
            document = case_document(plan, &source.id, &source.source, &regions, &scene, expect)?;
            verdict = run_regression_case_on_bundle(bundle, &document, &scene, ocr);
        }
        match verdict.verdict {
            RegressionVerdict::Failed => {
                let failed = verdict
                    .checks
                    .iter()
                    .filter(|check| check.result == RegressionResult::Failed)
                    .cloned()
                    .collect::<Vec<_>>();
                let refusal = case_failed(&source, &page, &failed);
                // Fix round 3: the item names the page as the warnings do.
                failures.fail(
                    source.locus.with_page(&page).with_case(&source.id, failed),
                    refusal,
                )?;
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
        if let Some(others) = &others {
            set.warn_ambiguous(&source.id, &page, others, &left_out);
        }
        if bare.contains(&page) {
            set.warnings.push(json!({
                "code": LabCode::RecordCaseBareCanvas.as_str(),
                "case": source.id,
                "page": page,
                "message": BARE_CANVAS_MESSAGE
            }));
        }
        // Fix round 3: a scene that is the whole frame is named, with the targets that read it.
        if !whole_frame.is_empty() {
            set.warnings.push(json!({
                "code": LabCode::RecordCaseWholeFrame.as_str(),
                "case": source.id,
                "page": page,
                "targets": whole_frame,
                "message": WHOLE_FRAME_MESSAGE
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
        // #463 S2-9a: a scrolling select's case is on the list's first screen, where a run meets
        // its page; its other screens are the offline entry's, in the self-check `selects`.
        let frame = match super::scan::first_screen(step) {
            Some(frame) => frame,
            None => pressed_on(step)?,
        };
        sources.push(Source {
            id: format!("step_{}", plan.numbered(step.number)),
            kind: CaseKind::Step,
            page: page.clone(),
            targets: targets(step.required.as_slice()),
            forbidden: targets(step.forbidden.as_slice()),
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
        forbidden: targets(last.forbidden.as_slice()),
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
            // An ending's page forbids nothing.
            forbidden: Vec::new(),
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
/// targets that read it in the package's order: of the one list of a package's read regions (#463
/// PRIV-1, `RecognitionPack::reads`, which marks no reading here), the regions a target declares
/// itself: a template's search region (and its colour check's region), a colour or colour digest
/// probe's region, an OCR or NN target's region (a select's name region or quantity column among
/// them). A check (`all_of`, `any_of`), a value test and a click-only target read nothing of their
/// own: a check's members and a value test's sides are targets of the package themselves. What a
/// candidate layout reads itself (the list's regions with a `layout`: its shared OCR read, a
/// feature it reads at an offset) is not pasted: no recording generates one, and a scene never
/// pasted it. A region that is no fixed rectangle inside the recording's frame (`size`) cannot be
/// pasted where it is read, and is the check's error; as before the list moved, a target's regions
/// are all found fixed before the first of them is placed.
pub(crate) fn read_regions(
    pack: &RecognitionPack,
    size: RecordSize,
) -> LabResult<BTreeMap<Patch, Vec<String>>> {
    let reads = pack.reads(&PackReadingDeclarations::default());
    let pasted = reads
        .regions
        .iter()
        .filter(|region| region.layout.is_none())
        .collect::<Vec<_>>();
    let mut regions: BTreeMap<Patch, Vec<String>> = BTreeMap::new();
    for target in pasted.chunk_by(|one, other| one.target == other.target) {
        let rects = target
            .iter()
            .copied()
            .map(fixed)
            .collect::<LabResult<Vec<_>>>()?;
        for (region, rect) in target.iter().zip(rects) {
            let readers = regions
                .entry(patch(&region.target, rect, size)?)
                .or_default();
            if !readers.contains(&region.target) {
                readers.push(region.target.clone());
            }
        }
    }
    Ok(regions)
}

/// The fixed rectangle of the read region `region`.
fn fixed(region: &PackReadRegion) -> LabResult<PackRect> {
    match region.area {
        PackReadArea::Rect(rect) => Ok(rect),
        PackReadArea::FullFrame | PackReadArea::Unbounded => Err(invalid(
            "validation_failed",
            format!(
                "target '{}' reads a region that is no fixed rectangle: a case's scene pastes \
                 fixed regions only",
                region.target
            ),
        )),
    }
}

/// `rect`, which the target `id` reads, as a patch inside the recording's frame of `size`.
pub(crate) fn patch(id: &str, rect: PackRect, size: RecordSize) -> LabResult<Patch> {
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
pub(crate) fn scene_of(
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

/// `expect` of a case: its page (`observe.page`), when given, and (`recognize`, when there is one)
/// every target its page requires to pass (`required`) and, fix round 3, every target in
/// `forbidden` to fail.
fn expectation(page: Option<&str>, required: &[String], forbidden: &[String]) -> Value {
    let mut expect = Map::new();
    if let Some(page) = page {
        expect.insert("observe".to_string(), json!({"page": page}));
    }
    let recognize = required
        .iter()
        .map(|target| (target, true))
        .chain(forbidden.iter().map(|target| (target, false)))
        .map(|(target, passed)| json!({"target": target, "passed": passed}))
        .collect::<Vec<_>>();
    if !recognize.is_empty() {
        expect.insert("recognize".to_string(), Value::Array(recognize));
    }
    Value::Object(expect)
}

/// The coordinator's ruling after the first round: the other pages the runner matched beside the
/// case's page `page`, when its page check failed by a conflict (`page_conflict`, a word of
/// `regression_problem`) among whose pages `page` is; `None` otherwise (the page not recognised is a
/// miss).
fn others_matched(verdict: &RegressionCaseVerdict, page: &str) -> Option<Vec<String>> {
    verdict.checks.iter().find_map(|check| {
        let matched = &check.page.as_ref()?.matched_pages;
        (check.problem == Some(RegressionProblem::PageConflict)
            && matched.iter().any(|other| other == page))
        .then(|| {
            matched
                .iter()
                .filter(|other| *other != page)
                .cloned()
                .collect()
        })
    })
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

/// `record_case_failed`: the case of `source`, which expects the page `page` (fix round 3: as the
/// page detection names it, as the warnings do), failed the checks `failed` on the package; its
/// `regression_problem` is the word of the first.
fn case_failed(source: &Source<'_>, page: &str, failed: &[RegressionCheck]) -> LabError {
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
        "page": page,
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
                source.id, source.frame.frame_id, page
            ),
        ),
        details,
    )
}
