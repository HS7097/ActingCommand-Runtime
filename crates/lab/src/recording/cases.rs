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
//! - **A public recording's scenes hold no pixel of a value reading** (#463 PRIV-3,
//!   [`scene_regions`]; the owner's ruling of 11-18: quantities an account holds, shown on a
//!   screen, are private, and their pixels are never stored in a repository). The one list of the
//!   package's read regions is asked with the reading declarations of the generated entry task.
//!   A reading of a fixed rectangle, its target's own or a layout's read of it, is withheld: not
//!   pasted, and after every patch is pasted its reach is filled with the canvas colour, so that no
//!   other target's patch leaves its pixels; a reading with no fixed rectangle is withheld by the
//!   bound the list confines it to; a layout's shared read that is a reading is withheld as a
//!   whole. A reading nothing confines, or confined only to the whole frame, gives no scene:
//!   `record_case_reading_unbounded`, naming the target. A case that fails where a failed check's
//!   target reads inside a withheld rectangle is `record_case_reading_withheld`; every other
//!   failure is `record_case_failed`. `scene_construction` lists what is pasted, as before; `record
//!   stop` and `record ready` answer the withheld rectangles and their targets. A local
//!   recording's scenes are built as before.
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
use super::model::{RecordDestination, RecordSize, RecordWithheld, RecordedFrame};
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
use actingcommand_contract::outcome::{Detail, Outcome, OutcomeEnvelope};
use actingcommand_contract::{LabError, LabResult};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::page_anchor_matches;
use actingcommand_pack_containment::LoadedBundle;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition_pack::{
    PackReadArea, PackReadRegion, PackReadingDeclarations, PackRect, RecognitionPack,
    RecognitionTarget, TargetKind, ValueTestRight,
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
    /// #463 PRIV-3: of a public recording, the rectangles withheld from its scenes with the
    /// reading targets they belong to; `None` of a local one.
    withheld: Option<Vec<RecordWithheld>>,
}

impl CaseSet {
    /// #463 PRIV-3: the rectangles withheld from a public recording's scenes, with the reading
    /// targets they belong to (empty when it reads no value); `None` for a local recording.
    pub(crate) fn withheld(&self) -> Option<&[RecordWithheld]> {
        self.withheld.as_deref()
    }

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
    // #463 PRIV-3: a public recording's scenes hold no pixel of a value reading; the reading
    // declarations are its generated entry task's.
    let declarations = match plan.destination {
        RecordDestination::Public => reading_declarations(plan, entries)?,
        RecordDestination::Local => PackReadingDeclarations::default(),
    };
    let built = scene_regions(evaluator.pack(), &declarations, plan.size, plan.destination)?;
    let regions = &built.pasted;
    // Fix round 3: the targets whose region or search area is the whole frame, which make every
    // scene the whole frame.
    let whole_frame = regions
        .get(&Patch::whole(plan.size))
        .cloned()
        .unwrap_or_default();
    let bare = bare_canvas_pages(plan, bundle, ocr)?;
    let sources = sources(plan)?;
    let mut set = CaseSet {
        withheld: built.answer(sources.iter().flat_map(|source| &source.forbidden)),
        ..CaseSet::default()
    };
    // Fix round 3: without a text-recognition model for a text target the page detection reads
    // (the runner's own check, `LabOcr::require_pages`), the runner judges no case's page: said
    // once, before every case's warnings.
    if ocr.require_pages(evaluator.pack(), detector).is_err() {
        set.warnings.push(json!({
            "code": LabCode::RecordCasesOcrUnavailable.as_str(),
            "message": PAGES_UNJUDGED_MESSAGE
        }));
    }
    // #463 PRIV-3: a reading nothing confines gives no scene without its pixels: the recording
    // cannot be public, and no case is made.
    if !built.unbounded.is_empty() {
        for target in &built.unbounded {
            failures.fail(Locus::default(), reading_unbounded(target))?;
        }
        return Ok(set);
    }
    for source in sources {
        let page = detected_page(detector, &plan.game, &source.page)?;
        let loaded = cache.get(&source.frame.sha256).ok_or_else(|| {
            invalid(
                "validation_failed",
                format!("frame {} was not loaded", source.frame.frame_id),
            )
        })?;
        let scene = built.scene(loaded, plan.size)?;
        let mut document = case_document(
            plan,
            &source.id,
            &source.source,
            regions,
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
            document = case_document(plan, &source.id, &source.source, regions, &scene, expect)?;
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
                // #463 PRIV-3: a failed check's target that reads inside a withheld rectangle
                // makes the failure the withholding's; every other failure is today's.
                let met = built.met_by(&failed);
                let refusal = if met.is_empty() {
                    case_failed(&source, &page, &failed)
                } else {
                    case_reading_withheld(&source, &page, &failed, met)
                };
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
/// feature it reads at an offset) is not pasted, as a scene never pasted it: a recording generates
/// no feature read at an offset, and a select's shared read (#463 SEL-2L-a) only beside the slot
/// targets the scene pastes. A region that is no fixed rectangle inside the recording's frame
/// (`size`) cannot be pasted where it is read, and is the check's error; as before the list moved,
/// a target's regions are all found fixed before the first of them is placed.
pub(crate) fn read_regions(
    pack: &RecognitionPack,
    size: RecordSize,
) -> LabResult<BTreeMap<Patch, Vec<String>>> {
    let reads = pack.reads(&PackReadingDeclarations::default());
    pasted_regions(
        reads.regions.iter().filter(|region| region.layout.is_none()),
        size,
    )
}

/// #463 PRIV-3: what the scenes of a recording's regression cases are built of.
pub(crate) struct SceneRegions {
    /// Every rectangle pasted, each once with the targets that read it: [`read_regions`]'s, and
    /// of a public recording without the regions that are readings.
    pub(crate) pasted: BTreeMap<Patch, Vec<String>>,
    /// Of a public recording, every rectangle withheld (filled with the canvas colour after the
    /// patches are pasted), each once with the reading targets it belongs to.
    pub(crate) withheld: BTreeMap<Patch, Vec<String>>,
    /// Of a public recording, the reading targets nothing confines (or only the whole frame), in
    /// the order of the list: no scene can be made without their pixels.
    pub(crate) unbounded: Vec<String>,
    /// Of a public recording, the rectangles each target reads (its regions' reaches, or bounds,
    /// and those of a check's members and of a value test's sides), to tell a failure inside a
    /// withheld rectangle.
    reads: BTreeMap<String, Vec<PackRect>>,
    /// Whether the recording is public.
    public: bool,
}

impl SceneRegions {
    /// The scene of the frame `loaded`: the patches pasted ([`scene_of`]), then each withheld
    /// rectangle filled with the canvas colour. Without a withheld rectangle it is [`scene_of`]'s,
    /// byte for byte.
    pub(crate) fn scene(&self, loaded: &LoadedFrame, size: RecordSize) -> LabResult<Vec<u8>> {
        if self.withheld.is_empty() {
            return scene_of(loaded, &self.pasted, size);
        }
        let mut pixels = pasted_pixels(loaded, &self.pasted, size)?;
        let width = size.width as usize;
        for patch in self.withheld.keys() {
            let canvas = CANVAS_FILL.repeat(patch.width as usize);
            for row in patch.y..patch.y + patch.height {
                let start = (row as usize * width + patch.x as usize) * 3;
                pixels[start..start + canvas.len()].copy_from_slice(&canvas);
            }
        }
        png(size, pixels)
    }

    /// For each failed check of `failed` that names a target, the withheld rectangles that
    /// target reads inside (its rectangles meet them: their intersection's area is above zero),
    /// `{target, withheld}`, when there is one; empty of a local recording.
    pub(crate) fn met_by(&self, failed: &[RegressionCheck]) -> Vec<Value> {
        failed
            .iter()
            .filter_map(|check| check.target.as_ref())
            .filter_map(|check| {
                let met = self.withheld_met(&check.target);
                (!met.is_empty()).then(|| {
                    json!({
                        "target": check.target,
                        "withheld": met.iter().map(|patch| patch_json(**patch)).collect::<Vec<_>>()
                    })
                })
            })
            .collect()
    }

    /// The withheld rectangles the target `id` reads inside.
    fn withheld_met(&self, id: &str) -> Vec<&Patch> {
        let Some(rects) = self.reads.get(id) else {
            return Vec::new();
        };
        self.withheld
            .keys()
            .filter(|patch| rects.iter().any(|rect| meets(*rect, **patch)))
            .collect()
    }

    /// The answer's withheld rectangles of a public recording, each with the reading targets it
    /// belongs to and the targets of `forbidden` (the targets the package's pages forbid) that
    /// read inside it; `None` of a local recording.
    pub(crate) fn answer<'f>(
        &self,
        forbidden: impl IntoIterator<Item = &'f String>,
    ) -> Option<Vec<RecordWithheld>> {
        if !self.public {
            return None;
        }
        let forbidden = forbidden.into_iter().collect::<BTreeSet<_>>();
        Some(
            self.withheld
                .iter()
                .map(|(patch, targets)| RecordWithheld {
                    x: patch.x,
                    y: patch.y,
                    width: patch.width,
                    height: patch.height,
                    targets: targets.clone(),
                    forbidden: forbidden
                        .iter()
                        .filter(|target| {
                            self.reads.get(target.as_str()).is_some_and(|rects| {
                                rects.iter().any(|rect| meets(*rect, *patch))
                            })
                        })
                        .map(|target| (*target).clone())
                        .collect(),
                })
                .collect(),
        )
    }
}

/// #463 PRIV-3 (`contracts/lab-recording.md`, "Regression cases written with the package"): what
/// the scenes of a recording of `destination` are built of, of the admitted package `pack` whose
/// entry task declares `task` and the recording's frame of `size`. A local recording's are
/// [`read_regions`]'s, nothing withheld. A public recording's are asked of the one list with
/// `task`'s reading declarations, and each region that is a reading is withheld instead of
/// pasted:
///
/// - a fixed rectangle, its target's own or a layout's read of it (a feature at an offset in a
///   slot, a shared read): its reach, the rectangle that holds it under every shift the pack
///   declares for it;
/// - no fixed rectangle: the bound the list confines it to, unless that bound is the whole frame;
/// - nothing confines it (`full_frame`, a relative region, a `repeated_anchor` row without a band),
///   or only the whole frame: it is `unbounded`, and no scene can be made.
///
/// The patches are the regions [`read_regions`] pastes that are no reading; a withheld rectangle
/// is clipped to the frame.
pub(crate) fn scene_regions(
    pack: &RecognitionPack,
    task: &PackReadingDeclarations,
    size: RecordSize,
    destination: RecordDestination,
) -> LabResult<SceneRegions> {
    if destination == RecordDestination::Local {
        return Ok(SceneRegions {
            pasted: read_regions(pack, size)?,
            withheld: BTreeMap::new(),
            unbounded: Vec::new(),
            reads: BTreeMap::new(),
            public: false,
        });
    }
    let list = pack.reads(task);
    let pasted = pasted_regions(
        list.regions
            .iter()
            .filter(|region| region.layout.is_none() && !region.reading),
        size,
    )?;
    let mut withheld: BTreeMap<Patch, Vec<String>> = BTreeMap::new();
    let mut unbounded = Vec::new();
    for region in list.regions.iter().filter(|region| region.reading) {
        let rect = match region.area {
            PackReadArea::Rect(area) => Some(region.reach.unwrap_or(area)),
            PackReadArea::FullFrame | PackReadArea::Unbounded => region
                .bound
                .filter(|bound| !covers_frame(*bound, size)),
        };
        match rect {
            Some(rect) => {
                if let Some(patch) = clipped(rect, size) {
                    let owners = withheld.entry(patch).or_default();
                    if !owners.contains(&region.target) {
                        owners.push(region.target.clone());
                    }
                }
            }
            None => {
                if !unbounded.contains(&region.target) {
                    unbounded.push(region.target.clone());
                }
            }
        }
    }
    Ok(SceneRegions {
        pasted,
        withheld,
        unbounded,
        reads: target_reads(pack, &list.regions, size),
        public: true,
    })
}

/// #463 PRIV-3: the reading declarations of the generated package's entry task,
/// `resources/operations/<task_id>/task.json` of its final files `entries`.
fn reading_declarations(
    plan: &Plan,
    entries: &BTreeMap<String, Vec<u8>>,
) -> LabResult<PackReadingDeclarations> {
    let name = format!("{}/task.json", plan.task_dir());
    let bytes = entries.get(&name).ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("the generated package holds no {name}"),
        )
    })?;
    serde_json::from_slice::<Value>(bytes)
        .and_then(|task| PackReadingDeclarations::of_task(&task))
        .map_err(|error| {
            invalid(
                "validation_failed",
                format!("the generated package's {name} declares no readable reading: {error}"),
            )
        })
}

/// #463 PRIV-3: the rectangles each target of `pack` reads, of the list's `regions`: each of its
/// regions by its reach (a fixed rectangle), the whole frame of `size` (`full_frame`) or its bound
/// (no fixed rectangle, when it has one); and, of a check, its members', of a value test, its
/// sides'.
fn target_reads(
    pack: &RecognitionPack,
    regions: &[PackReadRegion],
    size: RecordSize,
) -> BTreeMap<String, Vec<PackRect>> {
    let whole = PackRect {
        x: 0,
        y: 0,
        width: i32::try_from(size.width).unwrap_or(i32::MAX),
        height: i32::try_from(size.height).unwrap_or(i32::MAX),
    };
    let mut own: BTreeMap<&str, Vec<PackRect>> = BTreeMap::new();
    for region in regions {
        let rect = match region.area {
            PackReadArea::Rect(area) => Some(region.reach.unwrap_or(area)),
            PackReadArea::FullFrame => Some(whole),
            PackReadArea::Unbounded => region.bound,
        };
        if let Some(rect) = rect {
            own.entry(region.target.as_str()).or_default().push(rect);
        }
    }
    pack.targets
        .iter()
        .map(|target| {
            let mut rects = Vec::new();
            gather(pack, &own, target.id(), &mut BTreeSet::new(), &mut rects);
            (target.id().to_owned(), rects)
        })
        .collect()
}

/// The rectangles the target `id` reads into `rects`: its own (`own`), and a check's members' or
/// a value test's sides', each target once (`seen`).
fn gather<'p>(
    pack: &'p RecognitionPack,
    own: &BTreeMap<&str, Vec<PackRect>>,
    id: &'p str,
    seen: &mut BTreeSet<&'p str>,
    rects: &mut Vec<PackRect>,
) {
    if !seen.insert(id) {
        return;
    }
    if let Some(found) = own.get(id) {
        rects.extend(found.iter().copied());
    }
    match pack.targets.iter().find(|target| target.id() == id) {
        Some(RecognitionTarget::Composite(check)) => {
            for member in &check.members {
                gather(pack, own, member, seen, rects);
            }
        }
        Some(RecognitionTarget::ValueTest(test)) => {
            gather(pack, own, &test.left.target_id, seen, rects);
            if let Some(ValueTestRight::Operand(operand)) = &test.right {
                gather(pack, own, &operand.target_id, seen, rects);
            }
        }
        _ => {}
    }
}

/// Whether `rect` covers the whole frame of `size`.
fn covers_frame(rect: PackRect, size: RecordSize) -> bool {
    rect.x <= 0
        && rect.y <= 0
        && i64::from(rect.x) + i64::from(rect.width) >= i64::from(size.width)
        && i64::from(rect.y) + i64::from(rect.height) >= i64::from(size.height)
}

/// The part of `rect` inside the frame of `size`, as a patch; `None` when none is.
fn clipped(rect: PackRect, size: RecordSize) -> Option<Patch> {
    let left = i64::from(rect.x).max(0);
    let top = i64::from(rect.y).max(0);
    let right = (i64::from(rect.x) + i64::from(rect.width)).min(i64::from(size.width));
    let bottom = (i64::from(rect.y) + i64::from(rect.height)).min(i64::from(size.height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(Patch {
        y: u32::try_from(top).ok()?,
        x: u32::try_from(left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
        width: u32::try_from(right - left).ok()?,
    })
}

/// Whether `rect` and `patch` meet: their intersection's area is above zero.
fn meets(rect: PackRect, patch: Patch) -> bool {
    let left = i64::from(rect.x).max(i64::from(patch.x));
    let top = i64::from(rect.y).max(i64::from(patch.y));
    let right = (i64::from(rect.x) + i64::from(rect.width))
        .min(i64::from(patch.x) + i64::from(patch.width));
    let bottom = (i64::from(rect.y) + i64::from(rect.height))
        .min(i64::from(patch.y) + i64::from(patch.height));
    right > left && bottom > top
}

/// A patch as an answer names it, `{x, y, width, height}`.
fn patch_json(patch: Patch) -> Value {
    json!({"x": patch.x, "y": patch.y, "width": patch.width, "height": patch.height})
}

/// The patches of `regions`, each rectangle once with the targets that read it, in the order the
/// list gives them; a target's regions are all found fixed before the first of them is placed.
fn pasted_regions<'r>(
    regions: impl Iterator<Item = &'r PackReadRegion>,
    size: RecordSize,
) -> LabResult<BTreeMap<Patch, Vec<String>>> {
    let pasted = regions.collect::<Vec<_>>();
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
    png(size, pasted_pixels(loaded, regions, size)?)
}

/// The RGB rows of [`scene_of`]'s scene, before it is encoded.
fn pasted_pixels(
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
    Ok(pixels)
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
    let details = case_details(
        source,
        page,
        failed,
        Outcome::new(LabCode::RecordCaseFailed)
            .regression_problem(problem)
            .envelope(Detail::Full),
    );
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

/// The details of a failed case of `source`, which expects the page `page`: the case, its page,
/// frame and verdict, the checks `failed`, the outcome `outcome`, and its step or ending.
fn case_details(
    source: &Source<'_>,
    page: &str,
    failed: &[RegressionCheck],
    outcome: OutcomeEnvelope,
) -> Value {
    let mut details = json!({
        "case": source.id,
        "page": page,
        "frame_id": source.frame.frame_id,
        "verdict": RegressionVerdict::Failed.as_str(),
        "checks": failed,
        "outcome": outcome
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
    details
}

/// #463 PRIV-3: `record_case_reading_withheld`: the case of `source` of a public recording, which
/// expects the page `page`, failed the checks `failed`, and the target of at least one of them
/// reads inside a rectangle withheld from the scene (`met`, each `{target, withheld}`): a page of a
/// public recording is recognised by a target whose region holds a value reading.
fn case_reading_withheld(
    source: &Source<'_>,
    page: &str,
    failed: &[RegressionCheck],
    met: Vec<Value>,
) -> LabError {
    let mut details = case_details(
        source,
        page,
        failed,
        Outcome::new(LabCode::RecordCaseReadingWithheld).envelope(Detail::Full),
    );
    if let Some(object) = details.as_object_mut() {
        object.insert("withheld".to_string(), Value::Array(met));
    }
    with_details(
        blocked(
            LabCode::RecordCaseReadingWithheld.as_str(),
            format!(
                "the regression case {} that record stop made from frame {} for page {} does not \
                 pass, and a target its checks name reads inside a rectangle withheld from the \
                 scene of this public recording, which holds a value reading (details.withheld): a \
                 page of a public recording is recognised by a target whose region holds a value \
                 reading. Anchor the page on another part of its screen, narrow that target's \
                 region off the reading, or record with --destination local; nothing was written",
                source.id, source.frame.frame_id, page
            ),
        ),
        details,
    )
}

/// #463 PRIV-3: `record_case_reading_unbounded`: the package of a public recording reads a value
/// at the target `target` where nothing confines it to a rectangle smaller than the frame, so no
/// scene can be made without its pixels and the recording cannot be public.
fn reading_unbounded(target: &str) -> LabError {
    with_details(
        blocked(
            LabCode::RecordCaseReadingUnbounded.as_str(),
            format!(
                "the package of this public recording reads a value at '{target}' where nothing \
                 confines it to a rectangle smaller than the frame (a full_frame region, a region \
                 placed relative to a template, a repeated_anchor layout without a readable band, \
                 or a bound that is the whole frame): no regression case's scene can be made \
                 without its pixels, so the recording cannot be public. Bound the reading to a \
                 rectangle, or record with --destination local; no case was made and nothing was \
                 written"
            ),
        ),
        json!({
            "target": target,
            "outcome": Outcome::new(LabCode::RecordCaseReadingUnbounded).envelope(Detail::Full)
        }),
    )
}
