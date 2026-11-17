// SPDX-License-Identifier: AGPL-3.0-only

//! #463 RC-1 (`contracts/resource-regression-case.md`): a resource repository's regression case as
//! a contract of the Runtime, its reader and its runner.
//!
//! - **The reader** ([`read_regression_case`]) reads one `case.json` of the format
//!   [`REGRESSION_CASE_SCHEMA`]. It is strict on what the runner evaluates (the scene and its
//!   construction, `expect.observe.page`, `expect.recognize[]`): each problem is a check `failed`
//!   with its word, and a case with a problem evaluates nothing. Every other expectation of the
//!   format is read and answered `not_run` (`not_evaluated`), never passed. A case of another
//!   `schema_version` is not run, with the warning `case_schema_unsupported`; it is no failure.
//!   Fix round 1: the case's top level is open: a member of it the contract does not name is read
//!   past and named once in the case's warning `case_member_unknown`, never a failure. #463 RC-2:
//!   so are `scene_construction`, its `canvas` and each rectangle (notes on how the scene was
//!   made), the warning naming the member's place (`json_pointer`) beside its name; `expect`,
//!   `expect.observe` and an entry of `expect.recognize` stay closed.
//! - **The runner** ([`RegressionPackage::run`]) checks that the case is of the package (its
//!   `package_id` is the one the package's control declares), then the scene against its
//!   construction, and evaluates the expectations with the kernel's own read-only recognition
//!   engine, the one the offline `current-page` and `recognize` use:
//!   [`ReadonlyRecognitionEngine::current_page`] (the kernel's `detect_current_page_frames`, which
//!   answers a conflict when several pages match) and [`ReadonlyRecognitionEngine::recognize`]
//!   (the kernel's `SceneEvaluation::evaluate_target`). Nothing is evaluated a second way here. A
//!   page or a target that needs text recognition where no model is available, or several frames
//!   of target consensus, is `not_run` with its word.
//! - **One call** ([`run_regression_case_on_bundle`], fix round 1): a package already loaded (what
//!   `record stop`'s self-checks hold), a case's `case.json` and its scene as bytes, and the Lab's
//!   OCR; it answers the case's verdict. [`run_regression_case`] admits a package directory and
//!   calls it. `actinglab resource regression` runs a checkout's cases with the same parts; nothing
//!   here reads a command line or writes a file.

use crate::codes::{
    LabCode, RegressionNotRun, RegressionProblem, RegressionResult, RegressionVerdict,
};
use crate::ocr::LabOcr;
use crate::{package_base, scene_in_base};
use actingcommand_contract::outcome::{Detail, Outcome, VocabularyToken};
use actingcommand_contract::{ContainedTaskRequest, LabErrorClass, LabResult};
use actingcommand_execution_kernel::{
    ExternallyVerifiedBundle, ReadonlyRecognitionEngine, RecognizeResponse,
};
use actingcommand_pack_containment::LoadedBundle;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::RecognitionEvaluator;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{Duration, Instant};

/// The `schema_version` of a case of this contract.
pub const REGRESSION_CASE_SCHEMA: &str = "actingcommand.resource-regression-case.v1";
/// The file of a case in its directory `regression/<game>/<case>/`.
pub const REGRESSION_CASE_FILE: &str = "case.json";
/// The one `scene_construction.kind` of this version: the canvas with patches pasted on it.
const SCENE_KIND: &str = "roi_patch";
/// The largest canvas, in pixels, as the kernel bounds a saved frame.
const MAX_CANVAS_PIXELS: u64 = 16 * 1024 * 1024;
/// The label the package is admitted under, as the offline commands admit theirs.
const PACKAGE_LABEL: &str = "resource_regression";
/// The longest text of a key of type name.
const NAME_MAX_BYTES: usize = 256;
/// #463 RC-2: the longest text of a key of type pointer.
const POINTER_MAX_BYTES: usize = 512;
/// Where the page expectation and the scene's parts are in a case.
const OBSERVE_PAGE_AT: &str = "/expect/observe/page";
const SCENE_AT: &str = "/scene";
const SCENE_SHA256_AT: &str = "/scene_construction/scene_sha256";
const CANVAS_AT: &str = "/scene_construction/canvas";
const PACKAGE_AT: &str = "/package_id";

/// The members of a case's top level the contract names. Fix round 1: the top level is open; any
/// other member is read past with the warning `case_member_unknown`.
const TOP_MEMBERS: [&str; 9] = [
    "schema_version",
    "id",
    "package_id",
    "scene",
    "scene_construction",
    "source",
    "incident",
    "issue",
    "expect",
];
/// The members of `scene_construction`, `canvas` and a rectangle (#463 RC-2: each an open object,
/// as the top level is), and of `expect`, `expect.observe` and an entry of `expect.recognize`
/// (each a closed object).
const CONSTRUCTION_MEMBERS: [&str; 4] = ["kind", "canvas", "rects", "scene_sha256"];
const CANVAS_MEMBERS: [&str; 3] = ["width", "height", "fill"];
const RECT_MEMBERS: [&str; 5] = ["x", "y", "width", "height", "content"];
const EXPECT_MEMBERS: [&str; 9] = [
    "observe",
    "recognize",
    "entry",
    "prerequisite",
    "step",
    "terminal_outcome",
    "terminal_effect",
    "error_page",
    "after",
];
const OBSERVE_MEMBERS: [&str; 2] = ["page", "candidates"];
const RECOGNIZE_MEMBERS: [&str; 2] = ["target", "passed"];
/// The expectations of `expect` this version reads and does not evaluate, in the order answered
/// (after `expect.observe.candidates`).
const NOT_EVALUATED: [&str; 7] = [
    "entry",
    "prerequisite",
    "step",
    "terminal_outcome",
    "terminal_effect",
    "error_page",
    "after",
];

/// A case of this contract as read from its `case.json`: what the runner evaluates, checked, and
/// the warnings of reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegressionCase {
    id: String,
    package_id: String,
    scene: String,
    construction: Construction,
    expectations: Expectations,
    /// Fix round 1: `case_member_unknown` for each member of its top level the contract does not
    /// name (#463 RC-2: and of `scene_construction`, its `canvas` and each rectangle, once per
    /// place and name, in the order read), first among the warnings of its verdict.
    warnings: Vec<RegressionWarning>,
}

/// `scene_construction`, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Construction {
    canvas: Canvas,
    rects: Vec<PatchRect>,
    scene_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Canvas {
    width: u32,
    height: u32,
    fill: [u8; 3],
}

/// A rectangle of `scene_construction.rects`, inside the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PatchRect {
    x: u64,
    y: u64,
    width: u64,
    height: u64,
}

/// The expectations of `expect`: the two evaluated, and where each one that is not evaluated is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Expectations {
    page: Option<ExpectedPage>,
    targets: Vec<ExpectedTarget>,
    not_evaluated: Vec<String>,
}

/// `expect.observe.page`: the page the scene is recognised as, or none (`null`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedPage {
    page: Option<String>,
}

/// One entry of `expect.recognize`, with where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedTarget {
    at: String,
    target: String,
    passed: bool,
}

/// A package admitted as the offline commands admit theirs, with what the runner evaluates by.
pub struct RegressionPackage {
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
    ocr: LabOcr,
    /// Fix round 1: the `package_id` the package's control declares, `None` when it declares none.
    package_id: Option<String>,
}

/// One check of a case: where in the case (a JSON pointer, `""` the whole document), its result,
/// and why when it did not pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegressionCheck {
    pub at: String,
    #[serde(serialize_with = "token")]
    pub result: RegressionResult,
    /// Why it failed (`regression_problem`).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "optional_token"
    )]
    pub problem: Option<RegressionProblem>,
    /// Why it did not run (`regression_not_run`).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "optional_token"
    )]
    pub reason: Option<RegressionNotRun>,
    /// The page expectation: what was expected and what the kernel recognised.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<RegressionPageCheck>,
    /// A target expectation: what was expected and what the kernel answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<RegressionTargetCheck>,
    /// Fix round 1: what the case states and what was found, for a check that compares the two
    /// (`scene_sha256_mismatch`, `package_id_mismatch`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mismatch: Option<RegressionMismatch>,
    /// The text of the reader, the loader or the kernel, for people.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Fix round 1: the two values a check compared: `case` what the case states (its
/// `scene_construction.scene_sha256`, its `package_id`), `found` what was found (the SHA-256 of
/// the scene file's bytes; the `package_id` the package's control declares, `null` when it declares
/// none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegressionMismatch {
    pub case: String,
    pub found: Option<String>,
}

/// The page expectation of a check: `expected` the page (`null`: no page), `recognized` the page
/// the kernel recognised (`null`: none, or not evaluated), `matched_pages` every page that matched
/// when several did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegressionPageCheck {
    pub expected: Option<String>,
    pub recognized: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matched_pages: Vec<String>,
}

/// A target expectation of a check: `recognized` is the kernel's `passed` (`null`: not evaluated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegressionTargetCheck {
    pub target: String,
    pub expected: bool,
    pub recognized: Option<bool>,
}

/// A warning of a case, `{outcome}` as the Lab's answers carry one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegressionWarning {
    #[serde(serialize_with = "envelope")]
    pub outcome: Outcome,
}

/// The verdict of one case: `failed` when a check failed; else `passed` when every check passed;
/// `partial` when what ran passed and something did not run; `not_run` when nothing ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegressionCaseVerdict {
    /// The case's `id`, when its `case.json` gives one as text.
    pub id: Option<String>,
    /// The case's `package_id`, when its `case.json` gives one as text.
    pub package_id: Option<String>,
    #[serde(serialize_with = "token")]
    pub verdict: RegressionVerdict,
    pub checks: Vec<RegressionCheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<RegressionWarning>,
}

/// How many cases a run answered, and how many of each verdict.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RegressionCounts {
    pub cases: usize,
    pub passed: usize,
    pub failed: usize,
    pub partial: usize,
    pub not_run: usize,
}

/// The verdict of the case `case_json` with its scene `scene_png` on the package at `package_dir`,
/// admitted and evaluated as `resource regression` does each of a checkout's cases: the package is
/// admitted ([`RegressionPackage::load`]'s admission) and the case run on it by
/// [`run_regression_case_on_bundle`]; a package that is missing or refused answers the case's own
/// verdict when the reader refuses it or it is of another schema, else the case refused by the
/// package's check. Nothing is written.
pub fn run_regression_case(
    package_dir: &Path,
    case_json: &[u8],
    scene_png: &[u8],
    ocr: &LabOcr,
) -> RegressionCaseVerdict {
    match admit(package_dir, ocr) {
        Ok(admitted) => {
            run_regression_case_on_bundle(admitted.loaded_bundle(), case_json, scene_png, ocr)
        }
        Err(check) => match read_regression_case(case_json) {
            Ok(case) => case.refused(check),
            Err(verdict) => verdict,
        },
    }
}

/// Fix round 1 (for `record stop`, which has its package loaded in memory): the verdict of the case
/// `case_json` with its scene `scene_png` on the package `bundle` already loaded (the kernel's
/// admission of it, as `record stop`'s self-checks hold it), evaluated with `ocr`. The case is read
/// first (a case of another schema or one the reader refuses answers so, and nothing else is done),
/// then the package's evaluator and page set taken ([`RegressionPackage::from_bundle`]), then the
/// case run ([`RegressionPackage::run`]). Nothing is written and no file is read.
pub fn run_regression_case_on_bundle(
    bundle: &LoadedBundle,
    case_json: &[u8],
    scene_png: &[u8],
    ocr: &LabOcr,
) -> RegressionCaseVerdict {
    let case = match read_regression_case(case_json) {
        Ok(case) => case,
        Err(verdict) => return verdict,
    };
    match RegressionPackage::from_bundle(bundle, ocr) {
        Ok(package) => package.run(&case, scene_png),
        Err(check) => case.refused(check),
    }
}

/// Reads one `case.json`: the case, or the verdict it already has (`not_run` with the warning
/// `case_schema_unsupported` for another `schema_version` or none; `failed` with every problem
/// the reader found, each a check with its word).
pub fn read_regression_case(case_json: &[u8]) -> Result<RegressionCase, RegressionCaseVerdict> {
    let document = match serde_json::from_slice::<Value>(case_json) {
        Ok(Value::Object(document)) => document,
        Ok(_) => {
            return Err(RegressionCaseVerdict::refused(
                None,
                None,
                RegressionCheck::failed(
                    "",
                    RegressionProblem::CaseUnreadable,
                    Some("case.json is no JSON object".to_owned()),
                ),
            ));
        }
        Err(error) => {
            return Err(RegressionCaseVerdict::refused(
                None,
                None,
                RegressionCheck::failed(
                    "",
                    RegressionProblem::CaseUnreadable,
                    Some(format!("case.json does not read as JSON: {error}")),
                ),
            ));
        }
    };
    let text_of = |name: &str| {
        document
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match document.get("schema_version") {
        Some(Value::String(version)) if version == REGRESSION_CASE_SCHEMA => {}
        version => {
            let mut warning = Outcome::new(LabCode::CaseSchemaUnsupported);
            if let Some(Value::String(version)) = version
                && is_key_name(version)
            {
                warning = warning.schema_version(version.as_str());
            }
            return Err(RegressionCaseVerdict {
                id: text_of("id"),
                package_id: text_of("package_id"),
                verdict: RegressionVerdict::NotRun,
                checks: Vec::new(),
                warnings: vec![RegressionWarning { outcome: warning }],
            });
        }
    }
    let mut reader = Reader::default();
    reader.open(&document, "", &TOP_MEMBERS);
    let id = reader.string_member(&document, "", "id");
    let package_id = reader.file_name(&document, "", "package_id");
    let scene = reader.file_name(&document, "", "scene");
    let construction = reader.construction(&document);
    let expectations = reader.expectations(&document);
    match (id, package_id, scene, construction, expectations) {
        (Some(id), Some(package_id), Some(scene), Some(construction), Some(expectations))
            if reader.problems.is_empty() =>
        {
            Ok(RegressionCase {
                id,
                package_id,
                scene,
                construction,
                expectations,
                warnings: reader.warnings,
            })
        }
        _ => Err(RegressionCaseVerdict {
            id: text_of("id"),
            package_id: text_of("package_id"),
            verdict: RegressionVerdict::Failed,
            checks: reader.problems,
            warnings: reader.warnings,
        }),
    }
}

impl RegressionCase {
    /// The case's `id`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The `package_id` the case is of: its package is `packs/<package_id>/` of the checkout.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// The scene's file name, beside `case.json`.
    pub fn scene(&self) -> &str {
        &self.scene
    }

    /// The verdict of this case when `check` refused it before its scene was evaluated: its scene
    /// file is missing, or its package is missing or refused (`failed`), with the warnings of
    /// reading it.
    pub fn refused(&self, check: RegressionCheck) -> RegressionCaseVerdict {
        RegressionCaseVerdict {
            warnings: self.warnings.clone(),
            ..RegressionCaseVerdict::refused(
                Some(self.id.clone()),
                Some(self.package_id.clone()),
                check,
            )
        }
    }

    /// The verdict of this case from its checks and the warnings of running it, after the
    /// warnings of reading it.
    fn verdict(
        &self,
        checks: Vec<RegressionCheck>,
        warnings: Vec<RegressionWarning>,
    ) -> RegressionCaseVerdict {
        let mut all = self.warnings.clone();
        all.extend(warnings);
        RegressionCaseVerdict {
            id: Some(self.id.clone()),
            package_id: Some(self.package_id.clone()),
            verdict: verdict_of(&checks),
            checks,
            warnings: all,
        }
    }
}

/// The admission of the package at `package_dir` as the offline commands admit theirs
/// (`ExternallyVerifiedBundle::load_path`, the vision provider `ocr` gives, the admission deadline
/// of a contained task). A directory that is not there is `package_missing`; a refusal
/// `package_refused`.
fn admit(package_dir: &Path, ocr: &LabOcr) -> Result<ExternallyVerifiedBundle, RegressionCheck> {
    if !package_dir.is_dir() {
        return Err(RegressionCheck::failed(
            PACKAGE_AT,
            RegressionProblem::PackageMissing,
            Some(format!("{} is no directory", package_dir.display())),
        ));
    }
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| package_refused("the admission deadline overflows the clock".to_owned()))?;
    ExternallyVerifiedBundle::load_path(PACKAGE_LABEL, package_dir, false, ocr.provider(), deadline)
        .map_err(|error| package_refused(error.to_string()))
}

/// The check `package_refused` with the loader's text `detail`.
fn package_refused(detail: String) -> RegressionCheck {
    RegressionCheck::failed(PACKAGE_AT, RegressionProblem::PackageRefused, Some(detail))
}

impl RegressionPackage {
    /// Admits the package at `package_dir` as the offline commands admit theirs
    /// (`ExternallyVerifiedBundle::load_path`, the vision provider `ocr` gives, the admission
    /// deadline of a contained task) and takes it as [`Self::from_bundle`] does. A directory that
    /// is not there is `package_missing`; any other refusal `package_refused`.
    pub fn load(package_dir: &Path, ocr: &LabOcr) -> Result<Self, RegressionCheck> {
        let admitted = admit(package_dir, ocr)?;
        Self::from_bundle(admitted.loaded_bundle(), ocr)
    }

    /// Fix round 1: the package `bundle` already loaded, with its evaluator and its page set, which
    /// must agree, the `package_id` its control declares, and `ocr` for the expectations that need
    /// text recognition. A package without an evaluator or a page set, or whose two do not agree,
    /// is `package_refused`.
    pub fn from_bundle(bundle: &LoadedBundle, ocr: &LabOcr) -> Result<Self, RegressionCheck> {
        let evaluator = bundle.evaluator().cloned().ok_or_else(|| {
            package_refused("the package gives no recognition evaluator".to_owned())
        })?;
        let detector = bundle
            .detector()
            .cloned()
            .ok_or_else(|| package_refused("the package gives no page set".to_owned()))?;
        detector
            .validate(&evaluator)
            .map_err(|error| package_refused(error.to_string()))?;
        let package_id = bundle
            .control()
            .and_then(|control| control.get("package_id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(Self {
            evaluator,
            detector,
            ocr: ocr.clone(),
            package_id,
        })
    }

    /// The verdict of `case` with its scene `scene_png` on this package: fix round 1, the case's
    /// `package_id` against the one the package declares (a case of another package evaluates
    /// nothing); the scene against its construction (a scene that does not hold evaluates
    /// nothing), the warning `resource_regression_scene_outside_rects` when pixels outside the
    /// rectangles are not the canvas colour, then each expectation in the order of the case: the
    /// page, the targets, and the expectations not evaluated.
    pub fn run(&self, case: &RegressionCase, scene_png: &[u8]) -> RegressionCaseVerdict {
        if self.package_id.as_deref() != Some(case.package_id.as_str()) {
            let declared = self
                .package_id
                .as_deref()
                .map_or_else(|| "none".to_owned(), |declared| format!("{declared:?}"));
            let refused = RegressionCheck::failed(
                PACKAGE_AT,
                RegressionProblem::PackageIdMismatch,
                Some(format!(
                    "the case names the package {:?}; the package's control declares {declared}",
                    case.package_id
                )),
            )
            .with_mismatch(case.package_id.clone(), self.package_id.clone());
            return case.verdict(vec![refused], Vec::new());
        }
        let scene = match case.construction.scene(scene_png) {
            Ok(scene) => scene,
            Err(problems) => return case.verdict(problems, Vec::new()),
        };
        let mut warnings = Vec::new();
        let outside = case.construction.outside_rects(&scene);
        if outside > 0 {
            warnings.push(RegressionWarning {
                outcome: Outcome::new(LabCode::ResourceRegressionSceneOutsideRects).count(outside),
            });
        }
        // Workflow #392 DA-5cd: the scene in its package's coordinate space, brought there as the
        // offline commands bring a stored image (`scene_in_base`); one that cannot be evaluates
        // nothing. Fix round 1, one cause per word, told apart by the refusal's class: the frame
        // gate refuses a scene's size as a usage refusal (`lab_scene_size_unsupported`, with the
        // gate's word), and a frame it could not read at all is relayed as a device error.
        let scene = match scene_in_base(package_base(&self.evaluator), scene) {
            Ok((scene, _)) => scene,
            Err(error) => {
                let problem = if matches!(error.class, LabErrorClass::UsageValidation) {
                    RegressionProblem::SceneSizeUnsupported
                } else {
                    RegressionProblem::SceneGateUnreadable
                };
                let refused = RegressionCheck::failed(SCENE_AT, problem, Some(error.message));
                return case.verdict(vec![refused], warnings);
            }
        };
        let engine = ReadonlyRecognitionEngine::new(self.evaluator.clone(), Vec::new());
        let mut checks = Vec::new();
        if let Some(expected) = &case.expectations.page {
            checks.push(self.page_check(&engine, expected, &scene));
        }
        for expected in &case.expectations.targets {
            checks.push(self.target_check(&engine, expected, &scene));
        }
        checks.extend(
            case.expectations.not_evaluated.iter().map(|at| {
                RegressionCheck::not_run(at.as_str(), RegressionNotRun::NotEvaluated, None)
            }),
        );
        case.verdict(checks, warnings)
    }

    /// Why one scene cannot be evaluated by this package, as the offline commands refuse it:
    /// `ocr_unavailable` when the Lab's OCR check (`LabOcr::require_pages` or `LabOcr::require`,
    /// the one the offline commands call) refused, `frames_required` when the package's target
    /// consensus reads more than one frame.
    fn not_evaluable(&self, ocr_check: LabResult<()>) -> Option<(RegressionNotRun, String)> {
        if let Err(error) = ocr_check {
            return Some((RegressionNotRun::OcrUnavailable, error.message));
        }
        let frames = self.evaluator.target_sample_frames();
        (frames > 1).then(|| {
            (
                RegressionNotRun::FramesRequired,
                format!(
                    "the package's target consensus reads {frames} frames; a case has one scene"
                ),
            )
        })
    }

    /// `expect.observe.page`: the kernel's page detection over the scene, which evaluates every
    /// target of every page and answers a conflict when several pages match.
    fn page_check(
        &self,
        engine: &ReadonlyRecognitionEngine,
        expected: &ExpectedPage,
        scene: &Scene,
    ) -> RegressionCheck {
        let mut page = RegressionPageCheck {
            expected: expected.page.clone(),
            recognized: None,
            matched_pages: Vec::new(),
        };
        let ocr_check = self
            .ocr
            .require_pages(self.evaluator.pack(), &self.detector);
        if let Some((reason, detail)) = self.not_evaluable(ocr_check) {
            return RegressionCheck::not_run(OBSERVE_PAGE_AT, reason, Some(detail)).with_page(page);
        }
        let check = match engine.current_page(&self.detector, scene) {
            Err(error) => match error.conflicting_pages() {
                Some(pages) => {
                    page.matched_pages = pages.to_vec();
                    RegressionCheck::failed(
                        OBSERVE_PAGE_AT,
                        RegressionProblem::PageConflict,
                        Some(error.message().to_owned()),
                    )
                }
                None => RegressionCheck::failed(
                    OBSERVE_PAGE_AT,
                    RegressionProblem::PageDetectionRefused,
                    Some(error.message().to_owned()),
                ),
            },
            Ok(response) => {
                page.recognized = response.matched.then_some(response.page);
                if page.recognized == page.expected {
                    RegressionCheck::passed(OBSERVE_PAGE_AT)
                } else {
                    RegressionCheck::failed(OBSERVE_PAGE_AT, RegressionProblem::PageMismatch, None)
                }
            }
        };
        check.with_page(page)
    }

    /// One entry of `expect.recognize`: the kernel's evaluation of the target on the scene.
    fn target_check(
        &self,
        engine: &ReadonlyRecognitionEngine,
        expected: &ExpectedTarget,
        scene: &Scene,
    ) -> RegressionCheck {
        let at = expected.at.as_str();
        let mut target = RegressionTargetCheck {
            target: expected.target.clone(),
            expected: expected.passed,
            recognized: None,
        };
        let ocr_check = self
            .ocr
            .require(self.evaluator.pack(), [expected.target.as_str()]);
        if let Some((reason, detail)) = self.not_evaluable(ocr_check) {
            return RegressionCheck::not_run(at, reason, Some(detail)).with_target(target);
        }
        let check = match engine.recognize(&expected.target, Some(scene)) {
            Err(error) => RegressionCheck::failed(
                at,
                RegressionProblem::TargetRefused,
                Some(error.message().to_owned()),
            ),
            Ok(RecognizeResponse::ClickOnly(_)) => RegressionCheck::failed(
                at,
                RegressionProblem::TargetClickOnly,
                Some("a click-only target is never evaluated on a scene".to_owned()),
            ),
            Ok(RecognizeResponse::Evaluated(evaluated)) => {
                target.recognized = Some(evaluated.passed);
                if evaluated.passed == expected.passed {
                    RegressionCheck::passed(at)
                } else {
                    RegressionCheck::failed(
                        at,
                        RegressionProblem::TargetMismatch,
                        Some(evaluated.message.clone()),
                    )
                }
            }
        };
        check.with_target(target)
    }
}

impl Construction {
    /// The scene of `png`, once it holds against this construction: its bytes' SHA-256 is
    /// `scene_sha256`, it is a PNG, and its size is the canvas's; else every problem found.
    fn scene(&self, png: &[u8]) -> Result<Scene, Vec<RegressionCheck>> {
        let mut problems = Vec::new();
        let actual = format!("{:x}", Sha256::digest(png));
        if actual != self.scene_sha256 {
            // Fix round 1: both values, so a reader sees at once what kind of difference it is.
            let detail = format!("the scene's bytes have the sha256 {actual}");
            problems.push(
                RegressionCheck::failed(
                    SCENE_SHA256_AT,
                    RegressionProblem::SceneSha256Mismatch,
                    Some(detail),
                )
                .with_mismatch(self.scene_sha256.clone(), Some(actual)),
            );
        }
        match actingcommand_device::capture::parse_png_dimensions(png) {
            Err(error) => problems.push(RegressionCheck::failed(
                SCENE_AT,
                RegressionProblem::SceneUnreadable,
                Some(error.to_string()),
            )),
            Ok((width, height)) if (width, height) != (self.canvas.width, self.canvas.height) => {
                problems.push(RegressionCheck::failed(
                    CANVAS_AT,
                    RegressionProblem::CanvasSizeMismatch,
                    Some(format!(
                        "the scene is {width}x{height}, the canvas {}x{}",
                        self.canvas.width, self.canvas.height
                    )),
                ));
            }
            Ok(_) => {}
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        Scene::from_png(png).map_err(|error| {
            vec![RegressionCheck::failed(
                SCENE_AT,
                RegressionProblem::SceneUnreadable,
                Some(error.to_string()),
            )]
        })
    }

    /// How many pixels of `scene` outside every rectangle are not the canvas colour.
    fn outside_rects(&self, scene: &Scene) -> usize {
        let width = u64::from(self.canvas.width);
        let mut outside = 0;
        let (pixels, _) = scene.rgb8_pixels().as_chunks::<3>();
        for (index, pixel) in (0_u64..).zip(pixels) {
            if *pixel == self.canvas.fill {
                continue;
            }
            let (x, y) = (index % width, index / width);
            if !self.rects.iter().any(|rect| rect.holds(x, y)) {
                outside += 1;
            }
        }
        outside
    }
}

impl PatchRect {
    fn holds(&self, x: u64, y: u64) -> bool {
        (self.x..self.x + self.width).contains(&x) && (self.y..self.y + self.height).contains(&y)
    }
}

impl RegressionCheck {
    fn new(at: &str, result: RegressionResult) -> Self {
        Self {
            at: at.to_owned(),
            result,
            problem: None,
            reason: None,
            page: None,
            target: None,
            mismatch: None,
            detail: None,
        }
    }

    /// A check that passed.
    fn passed(at: &str) -> Self {
        Self::new(at, RegressionResult::Passed)
    }

    /// A check at `at` that failed for `problem`.
    pub fn failed(at: &str, problem: RegressionProblem, detail: Option<String>) -> Self {
        Self {
            problem: Some(problem),
            detail,
            ..Self::new(at, RegressionResult::Failed)
        }
    }

    /// A check at `at` that did not run for `reason`.
    pub fn not_run(at: &str, reason: RegressionNotRun, detail: Option<String>) -> Self {
        Self {
            reason: Some(reason),
            detail,
            ..Self::new(at, RegressionResult::NotRun)
        }
    }

    fn with_page(self, page: RegressionPageCheck) -> Self {
        Self {
            page: Some(page),
            ..self
        }
    }

    fn with_target(self, target: RegressionTargetCheck) -> Self {
        Self {
            target: Some(target),
            ..self
        }
    }

    fn with_mismatch(self, case: String, found: Option<String>) -> Self {
        Self {
            mismatch: Some(RegressionMismatch { case, found }),
            ..self
        }
    }
}

impl RegressionCaseVerdict {
    /// The verdict `failed` of a case `check` refused before anything was evaluated, with the
    /// case's `id` and `package_id` when they are known.
    pub fn refused(id: Option<String>, package_id: Option<String>, check: RegressionCheck) -> Self {
        Self {
            id,
            package_id,
            verdict: RegressionVerdict::Failed,
            checks: vec![check],
            warnings: Vec::new(),
        }
    }
}

impl RegressionCounts {
    /// The counts of `verdicts`.
    pub fn of<'a>(verdicts: impl IntoIterator<Item = &'a RegressionCaseVerdict>) -> Self {
        let mut counts = Self::default();
        for verdict in verdicts {
            counts.cases += 1;
            match verdict.verdict {
                RegressionVerdict::Passed => counts.passed += 1,
                RegressionVerdict::Failed => counts.failed += 1,
                RegressionVerdict::Partial => counts.partial += 1,
                RegressionVerdict::NotRun => counts.not_run += 1,
            }
        }
        counts
    }
}

/// The verdict of a case's checks: `failed` when one failed; `not_run` when none passed; `partial`
/// when one passed and one did not run; `passed` when every check passed.
fn verdict_of(checks: &[RegressionCheck]) -> RegressionVerdict {
    let any = |result: RegressionResult| checks.iter().any(|check| check.result == result);
    if any(RegressionResult::Failed) {
        RegressionVerdict::Failed
    } else if !any(RegressionResult::Passed) {
        RegressionVerdict::NotRun
    } else if any(RegressionResult::NotRun) {
        RegressionVerdict::Partial
    } else {
        RegressionVerdict::Passed
    }
}

/// What the reader found wrong, each a failed check, and (fix round 1) the warnings of reading.
#[derive(Default)]
struct Reader {
    problems: Vec<RegressionCheck>,
    warnings: Vec<RegressionWarning>,
}

impl Reader {
    fn fail(&mut self, at: &str, problem: RegressionProblem, detail: String) {
        self.problems
            .push(RegressionCheck::failed(at, problem, Some(detail)));
    }

    /// The open object `object` at `at`: the case's top level (fix round 1, `at` `""`) and (#463
    /// RC-2) `scene_construction`, its `canvas` and each rectangle. Each member of it that is not
    /// one of `members` is read past and named once in the warning `case_member_unknown`, never a
    /// failure: `json_pointer` its place, the JSON pointer a failure there would name (`/note` at
    /// the top level, `/scene_construction/note` inside), when the key table takes it, and
    /// `member` its name, when the key table takes it. The repositories' own runners read past
    /// such a member too, and nothing the runner evaluates is read from one.
    fn open(&mut self, object: &Map<String, Value>, at: &str, members: &[&str]) {
        for name in object.keys() {
            if members.contains(&name.as_str()) {
                continue;
            }
            let mut warning = Outcome::new(LabCode::CaseMemberUnknown);
            let place = pointer(at, name);
            if is_key_pointer(&place) {
                warning = warning.json_pointer(place);
            }
            if is_key_name(name) {
                warning = warning.member(name.as_str());
            }
            self.warnings.push(RegressionWarning { outcome: warning });
        }
    }

    /// Every member of the closed object `object` at `at` that is not one of `members` is
    /// `member_unknown`.
    fn closed(&mut self, object: &Map<String, Value>, at: &str, members: &[&str]) {
        for name in object.keys() {
            if !members.contains(&name.as_str()) {
                self.fail(
                    &pointer(at, name),
                    RegressionProblem::MemberUnknown,
                    format!("{name} is no member of this object in {REGRESSION_CASE_SCHEMA}"),
                );
            }
        }
    }

    /// The member `name` of `object` at `at`, which must be there (`member_missing`).
    fn required<'a>(
        &mut self,
        object: &'a Map<String, Value>,
        at: &str,
        name: &str,
    ) -> Option<&'a Value> {
        let value = object.get(name);
        if value.is_none() {
            self.fail(
                &pointer(at, name),
                RegressionProblem::MemberMissing,
                format!("{name} is missing"),
            );
        }
        value
    }

    /// The member `name` of `object`, an object.
    fn object<'a>(
        &mut self,
        object: &'a Map<String, Value>,
        at: &str,
        name: &str,
    ) -> Option<&'a Map<String, Value>> {
        match self.required(object, at, name)? {
            Value::Object(member) => Some(member),
            _ => {
                self.fail(
                    &pointer(at, name),
                    RegressionProblem::MemberInvalid,
                    format!("{name} is no object"),
                );
                None
            }
        }
    }

    /// The member `name` of `object`, a text that is not empty.
    fn string_member(
        &mut self,
        object: &Map<String, Value>,
        at: &str,
        name: &str,
    ) -> Option<String> {
        match self.required(object, at, name)? {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            _ => {
                self.fail(
                    &pointer(at, name),
                    RegressionProblem::MemberInvalid,
                    format!("{name} is no text that is not empty"),
                );
                None
            }
        }
    }

    /// The member `name` of `object`, one name of a file or a directory: no separator, no drive,
    /// not `.` or `..`.
    fn file_name(&mut self, object: &Map<String, Value>, at: &str, name: &str) -> Option<String> {
        let text = self.string_member(object, at, name)?;
        let single = text != "."
            && text != ".."
            && !text.contains(['/', '\\', ':'])
            && !text.chars().any(char::is_control);
        if single {
            return Some(text);
        }
        self.fail(
            &pointer(at, name),
            RegressionProblem::MemberInvalid,
            format!("{name} {text:?} is no single file name"),
        );
        None
    }

    /// The member `name` of `object`, an integer from `least` to `most`.
    fn integer(
        &mut self,
        object: &Map<String, Value>,
        at: &str,
        name: &str,
        least: i64,
        most: i64,
    ) -> Option<i64> {
        let value = self.required(object, at, name)?;
        match value.as_i64() {
            Some(number) if (least..=most).contains(&number) => Some(number),
            _ => {
                self.fail(
                    &pointer(at, name),
                    RegressionProblem::MemberInvalid,
                    format!("{name} is no integer from {least} to {most}"),
                );
                None
            }
        }
    }

    /// `scene_construction`: its kind, its canvas, its rectangles (each inside the canvas) and
    /// the scene's SHA-256.
    fn construction(&mut self, document: &Map<String, Value>) -> Option<Construction> {
        let at = "/scene_construction";
        let construction = self.object(document, "", "scene_construction")?;
        self.open(construction, at, &CONSTRUCTION_MEMBERS);
        if let Some(kind) = self.string_member(construction, at, "kind")
            && kind != SCENE_KIND
        {
            self.fail(
                &pointer(at, "kind"),
                RegressionProblem::MemberInvalid,
                format!("kind {kind:?} is not {SCENE_KIND}"),
            );
        }
        let canvas = self.canvas(construction, at);
        let rects = self.rects(construction, at, canvas);
        let scene_sha256 = match self.string_member(construction, at, "scene_sha256") {
            Some(hex)
                if hex.len() == 64
                    && hex
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
            {
                Some(hex)
            }
            Some(hex) => {
                self.fail(
                    &pointer(at, "scene_sha256"),
                    RegressionProblem::MemberInvalid,
                    format!("scene_sha256 {hex:?} is no 64 lowercase hexadecimal characters"),
                );
                None
            }
            None => None,
        };
        Some(Construction {
            canvas: canvas?,
            rects: rects?,
            scene_sha256: scene_sha256?,
        })
    }

    /// `scene_construction.canvas`: its size (at most [`MAX_CANVAS_PIXELS`] pixels) and its fill.
    fn canvas(&mut self, construction: &Map<String, Value>, at: &str) -> Option<Canvas> {
        let canvas = self.object(construction, at, "canvas")?;
        let canvas_at = pointer(at, "canvas");
        self.open(canvas, &canvas_at, &CANVAS_MEMBERS);
        let width = self.integer(canvas, &canvas_at, "width", 1, i64::from(u32::MAX));
        let height = self.integer(canvas, &canvas_at, "height", 1, i64::from(u32::MAX));
        let fill = match self.required(canvas, &canvas_at, "fill") {
            Some(value) => {
                let channels = value.as_array().and_then(|channels| {
                    channels
                        .iter()
                        .map(|channel| channel.as_u64().and_then(|level| u8::try_from(level).ok()))
                        .collect::<Option<Vec<u8>>>()
                        .and_then(|levels| <[u8; 3]>::try_from(levels).ok())
                });
                if channels.is_none() {
                    self.fail(
                        &pointer(&canvas_at, "fill"),
                        RegressionProblem::MemberInvalid,
                        "fill is no list of three levels from 0 to 255".to_owned(),
                    );
                }
                channels
            }
            None => None,
        };
        let (width, height) = (u32::try_from(width?).ok()?, u32::try_from(height?).ok()?);
        if u64::from(width) * u64::from(height) > MAX_CANVAS_PIXELS {
            self.fail(
                &canvas_at,
                RegressionProblem::MemberInvalid,
                format!("a canvas of {width}x{height} is larger than {MAX_CANVAS_PIXELS} pixels"),
            );
            return None;
        }
        Some(Canvas {
            width,
            height,
            fill: fill?,
        })
    }

    /// `scene_construction.rects`: every rectangle, each inside `canvas` (`rect_outside_canvas`).
    fn rects(
        &mut self,
        construction: &Map<String, Value>,
        at: &str,
        canvas: Option<Canvas>,
    ) -> Option<Vec<PatchRect>> {
        let rects_at = pointer(at, "rects");
        let Some(items) = self.required(construction, at, "rects")?.as_array() else {
            self.fail(
                &rects_at,
                RegressionProblem::MemberInvalid,
                "rects is no list".to_owned(),
            );
            return None;
        };
        let mut rects = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let item_at = format!("{rects_at}/{index}");
            let Some(rect) = item.as_object() else {
                self.fail(
                    &item_at,
                    RegressionProblem::MemberInvalid,
                    "a rectangle is no object".to_owned(),
                );
                continue;
            };
            self.open(rect, &item_at, &RECT_MEMBERS);
            let x = self.integer(
                rect,
                &item_at,
                "x",
                i64::from(i32::MIN),
                i64::from(i32::MAX),
            );
            let y = self.integer(
                rect,
                &item_at,
                "y",
                i64::from(i32::MIN),
                i64::from(i32::MAX),
            );
            let width = self.integer(rect, &item_at, "width", 1, i64::from(u32::MAX));
            let height = self.integer(rect, &item_at, "height", 1, i64::from(u32::MAX));
            if rect
                .get("content")
                .is_some_and(|content| !content.is_string())
            {
                self.fail(
                    &pointer(&item_at, "content"),
                    RegressionProblem::MemberInvalid,
                    "content is no text".to_owned(),
                );
            }
            let (Some(x), Some(y), Some(width), Some(height), Some(bounds)) =
                (x, y, width, height, canvas)
            else {
                continue;
            };
            match (
                u64::try_from(x),
                u64::try_from(y),
                u64::try_from(width),
                u64::try_from(height),
            ) {
                (Ok(left), Ok(top), Ok(across), Ok(down))
                    if left + across <= u64::from(bounds.width)
                        && top + down <= u64::from(bounds.height) =>
                {
                    rects.push(PatchRect {
                        x: left,
                        y: top,
                        width: across,
                        height: down,
                    });
                }
                _ => self.fail(
                    &item_at,
                    RegressionProblem::RectOutsideCanvas,
                    format!(
                        "the rectangle ({x}, {y}, {width}, {height}) does not lie inside the \
                         {}x{} canvas",
                        bounds.width, bounds.height
                    ),
                ),
            }
        }
        Some(rects)
    }

    /// `expect`: the page and the targets, where each expectation not evaluated is, and at least
    /// one expectation (`expect_empty`).
    fn expectations(&mut self, document: &Map<String, Value>) -> Option<Expectations> {
        let at = "/expect";
        let expect = self.object(document, "", "expect")?;
        self.closed(expect, at, &EXPECT_MEMBERS);
        let mut expectations = Expectations::default();
        if let Some(observe) = expect.get("observe") {
            let observe_at = pointer(at, "observe");
            match observe.as_object() {
                None => self.fail(
                    &observe_at,
                    RegressionProblem::MemberInvalid,
                    "observe is no object".to_owned(),
                ),
                Some(observe) => {
                    self.closed(observe, &observe_at, &OBSERVE_MEMBERS);
                    match self.required(observe, &observe_at, "page") {
                        Some(Value::Null) => expectations.page = Some(ExpectedPage { page: None }),
                        Some(Value::String(page)) if !page.is_empty() => {
                            expectations.page = Some(ExpectedPage {
                                page: Some(page.clone()),
                            });
                        }
                        Some(_) => self.fail(
                            &pointer(&observe_at, "page"),
                            RegressionProblem::MemberInvalid,
                            "page is no page id and not null".to_owned(),
                        ),
                        None => {}
                    }
                    if observe.contains_key("candidates") {
                        expectations
                            .not_evaluated
                            .push(pointer(&observe_at, "candidates"));
                    }
                }
            }
        }
        if let Some(recognize) = expect.get("recognize") {
            let recognize_at = pointer(at, "recognize");
            match recognize.as_array() {
                None => self.fail(
                    &recognize_at,
                    RegressionProblem::MemberInvalid,
                    "recognize is no list".to_owned(),
                ),
                Some(entries) => {
                    for (index, entry) in entries.iter().enumerate() {
                        let entry_at = format!("{recognize_at}/{index}");
                        if let Some(target) = self.recognize_entry(entry, &entry_at) {
                            expectations.targets.push(target);
                        }
                    }
                }
            }
        }
        for name in NOT_EVALUATED {
            if expect.contains_key(name) {
                expectations.not_evaluated.push(pointer(at, name));
            }
        }
        // An empty `recognize` declares nothing.
        let declares = expect.iter().any(|(name, value)| {
            EXPECT_MEMBERS.contains(&name.as_str())
                && (name != "recognize"
                    || value.as_array().is_none_or(|entries| !entries.is_empty()))
        });
        if !declares {
            self.fail(
                at,
                RegressionProblem::ExpectEmpty,
                "expect declares no expectation".to_owned(),
            );
        }
        Some(expectations)
    }

    /// One entry of `expect.recognize`: `target` and `passed`.
    fn recognize_entry(&mut self, entry: &Value, at: &str) -> Option<ExpectedTarget> {
        let Some(entry) = entry.as_object() else {
            self.fail(
                at,
                RegressionProblem::MemberInvalid,
                "a recognize entry is no object".to_owned(),
            );
            return None;
        };
        self.closed(entry, at, &RECOGNIZE_MEMBERS);
        let target = self.string_member(entry, at, "target");
        let passed = match self.required(entry, at, "passed")? {
            Value::Bool(passed) => Some(*passed),
            _ => {
                self.fail(
                    &pointer(at, "passed"),
                    RegressionProblem::MemberInvalid,
                    "passed is no boolean".to_owned(),
                );
                None
            }
        };
        Some(ExpectedTarget {
            at: at.to_owned(),
            target: target?,
            passed: passed?,
        })
    }
}

/// Whether `text` is a value the key table takes for a key of type name: at most
/// [`NAME_MAX_BYTES`] bytes, no control character.
fn is_key_name(text: &str) -> bool {
    text.len() <= NAME_MAX_BYTES && !text.chars().any(char::is_control)
}

/// #463 RC-2: whether `place`, the JSON pointer of a member of a case (it begins with `/`), is a
/// value the key table takes for a key of type pointer: at most [`POINTER_MAX_BYTES`] bytes.
fn is_key_pointer(place: &str) -> bool {
    place.len() <= POINTER_MAX_BYTES
}

/// The JSON pointer of the member `name` of the object at `parent` (RFC 6901).
fn pointer(parent: &str, name: &str) -> String {
    format!("{parent}/{}", name.replace('~', "~0").replace('/', "~1"))
}

/// A word of a closed vocabulary, as its token.
fn token<S: Serializer, T: VocabularyToken>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(value.token())
}

/// A word of a closed vocabulary that may be absent, as its token or `null`.
fn optional_token<S: Serializer, T: VocabularyToken>(
    value: &Option<T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_str(value.token()),
        None => serializer.serialize_none(),
    }
}

/// An outcome as its `actingcommand.outcome.v1` envelope, as the Lab's answers carry a warning.
fn envelope<S: Serializer>(outcome: &Outcome, serializer: S) -> Result<S::Ok, S::Error> {
    outcome.envelope(Detail::Full).serialize(serializer)
}
