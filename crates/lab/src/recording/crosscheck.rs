// SPDX-License-Identifier: AGPL-3.0-only

//! The `record stop` self-checks (Workflow #336 L4, frozen model section 4.6 with the a15 and
//! R24 items): the container round trip, admission by the kernel on the content table, the
//! first decision of an offline simulation, and the per-target cross-check of every page on
//! the live frames of the recording (the step itself, the arrival gate after each effect, and
//! the first step under an overall darkening). Every refusal is
//! `record_artifact_admission_failed` or `record_step_self_mismatch`; warnings never refuse.

use super::container::table_digest;
use super::frames::LoadedFrame;
use super::generate::{Plan, PlanStep};
use super::marks::OCR_NOT_EVALUATED;
use super::model::{MarkFamily, RecordedFrame};
use super::store::{blocked, invalid, with_details};
use actingcommand_contract::{
    ContentDirectory, ContentDirectoryVersion, InputAction, LabError, LabResult,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ExternallyVerifiedBundle, OfflineDecision, PreparedContainedTask, simulate_contained_task,
};
use actingcommand_pack_containment::{
    ContainmentError, ContainmentLimits, ContentContainer, expand_content_container,
};
use actingcommand_recognition::Scene;
use actingcommand_recognition_pack::{RecognitionEvaluator, TargetEvaluation};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const ADMISSION_INSTANCE: &str = "lab.record.stop";
const ADMISSION_BUDGET: Duration = Duration::from_secs(120);
const APPLICATION_OFFLINE: &str = "lab_application_effect_offline";
const APPLICATION_REFUSAL: &str = "application_effect_requires_assigned_application";
/// a15 section 4.6 item 5: every channel times 0.45, rounded down.
const OVERLAY_PERCENT: u16 = 45;

/// The live frames of the recording by sha256, read back after their sha256 check.
pub(crate) type FrameCache = BTreeMap<String, LoadedFrame>;

fn admission_failed(stage: &str, message: String, mut details: Value) -> LabError {
    if let Some(object) = details.as_object_mut() {
        object.insert("stage".to_string(), json!(stage));
    }
    with_details(
        blocked("record_artifact_admission_failed", message),
        details,
    )
}

pub(crate) fn content_reference(digest: &str) -> ContentDirectory {
    ContentDirectory {
        schema_version: ContentDirectoryVersion::V1,
        sha256: digest.to_string(),
    }
}

fn containment_code(error: &ContainmentError) -> Option<&'static str> {
    match error {
        ContainmentError::SourceTree { code } => Some(*code),
        _ => None,
    }
}

/// Section 4.6 item 1: the encoded container expands to exactly the generated table, whose
/// digest is `digest`.
pub(crate) fn round_trip(
    bytes: &[u8],
    kind: ContentContainer,
    entries: &BTreeMap<String, Vec<u8>>,
    digest: &str,
) -> LabResult<()> {
    let expanded =
        expand_content_container(bytes, kind, ContainmentLimits::default()).map_err(|error| {
            admission_failed(
                "container_round_trip",
                format!("the encoded container does not expand: {error}"),
                json!({"loader_code": containment_code(&error), "error": error.to_string()}),
            )
        })?;
    if expanded != *entries {
        let differing = expanded
            .keys()
            .chain(entries.keys())
            .filter(|path| expanded.get(*path) != entries.get(*path))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        return Err(admission_failed(
            "container_round_trip",
            "the encoded container does not expand to the generated files".to_string(),
            json!({"paths": differing}),
        ));
    }
    let actual = table_digest(&expanded);
    if actual != digest {
        return Err(admission_failed(
            "container_round_trip",
            format!("the expanded container has the digest {actual}, not {digest}"),
            json!({"expected": digest, "actual": actual}),
        ));
    }
    Ok(())
}

/// Section 4.6 item 2: the kernel admits the content table as actingd loads a content
/// directory: the digest comparison, source compilation, declaration and `linear_steps`
/// validation. No vision provider: OCR is admitted and only its evaluation would need one.
pub(crate) fn admit(
    entries: &BTreeMap<String, Vec<u8>>,
    digest: &str,
) -> LabResult<PreparedContainedTask> {
    PreparedContainedTask::load_content_entries(
        ADMISSION_INSTANCE,
        entries.clone(),
        &content_reference(digest),
        None,
        Instant::now() + ADMISSION_BUDGET,
    )
    .map_err(|error| {
        admission_failed(
            "admission",
            format!("the generated package is not admitted: {error}"),
            json!({
                "loader_code": error.code(),
                "detail": error.detail(),
                "declaration": error.declaration_issue().map(|issue| json!({
                    "file": issue.declaration_file,
                    "pointer": issue.field_path,
                    "reason": issue.reason
                }))
            }),
        )
    })
}

/// The admitted bundle whose evaluator the cross-check uses (section 4.6 item 4).
pub(crate) fn evaluator_bundle(
    entries: &BTreeMap<String, Vec<u8>>,
    digest: &str,
) -> LabResult<ExternallyVerifiedBundle> {
    ExternallyVerifiedBundle::load_content_entries(
        ADMISSION_INSTANCE,
        entries.clone(),
        &content_reference(digest),
        false,
        None,
        Instant::now() + ADMISSION_BUDGET,
    )
    .map_err(|error| {
        admission_failed(
            "evaluator",
            format!("the generated package has no evaluator: {error}"),
            json!({"error": error.to_string()}),
        )
    })
}

fn cached<'c>(cache: &'c FrameCache, frame: &RecordedFrame) -> LabResult<&'c LoadedFrame> {
    cache.get(&frame.sha256).ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("frame {} was not loaded", frame.frame_id),
        )
    })
}

fn device_frame(loaded: &LoadedFrame) -> LabResult<Frame> {
    Frame::from_png(loaded.png.clone(), CaptureBackendName::AdbScreencap).map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to decode a stored frame: {error}"),
        )
    })
}

/// The first decision and the warning it adds.
pub(crate) struct FirstDecision {
    pub(crate) value: Value,
    pub(crate) warning: Option<Value>,
}

fn not_evaluated_warning(reason: &str, message: &str) -> Value {
    json!({
        "code": "first_decision_not_evaluated",
        "step": 1,
        "reason": reason,
        "message": message
    })
}

/// Section 4.6 item 3 with R24: a package with an application step is simulated on the first
/// live frame and must be refused before any capture; a first step involving OCR is not
/// simulated; otherwise the first step's primary frame must give the first click, inside its
/// rectangle.
pub(crate) fn first_decision(
    prepared: &PreparedContainedTask,
    plan: &Plan,
    cache: &FrameCache,
) -> LabResult<FirstDecision> {
    let first = plan
        .steps
        .first()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    let simulate = |frame: &RecordedFrame| -> LabResult<_> {
        let frame = device_frame(cached(cache, frame)?)?;
        simulate_contained_task(prepared, vec![frame]).map_err(|error| {
            admission_failed(
                "first_decision",
                format!("the offline simulation failed: {error}"),
                json!({"code": error.code(), "detail": error.detail()}),
            )
        })
    };
    if plan.has_application_step() {
        let frame = plan
            .steps
            .iter()
            .find_map(PlanStep::primary)
            .ok_or_else(|| invalid("validation_failed", "the package has no frame"))?;
        let result = simulate(frame)?;
        let refused = matches!(&result.decision, OfflineDecision::Refused { code, .. } if code == APPLICATION_REFUSAL);
        if !refused || result.capture_count != 0 {
            return Err(admission_failed(
                "first_decision",
                format!(
                    "a package with an application step must be refused offline with \
                     {APPLICATION_REFUSAL} before any capture"
                ),
                json!({"decision": result.decision, "capture_count": result.capture_count}),
            ));
        }
        // Only a click of step 1 is left unsimulated; an application step 1 has nothing to
        // simulate offline.
        let warning = first.step.click.is_some().then(|| {
            not_evaluated_warning(
                APPLICATION_OFFLINE,
                "the package has an application step, so the offline simulation stops before \
                 the first step: the click of step 1 was not simulated",
            )
        });
        return Ok(FirstDecision {
            value: json!({
                "status": "not_evaluated",
                "reason": APPLICATION_OFFLINE,
                "refusal": APPLICATION_REFUSAL,
                "frame": frame.frame_id
            }),
            warning,
        });
    }
    if plan.involves_ocr(&first.required)? {
        return Ok(FirstDecision {
            value: json!({"status": "not_evaluated", "reason": OCR_NOT_EVALUATED}),
            warning: Some(not_evaluated_warning(
                OCR_NOT_EVALUATED,
                "step 1 involves OCR and Lab has no OCR provider: the first decision was not \
                 simulated; make sure step 1 really needs OCR",
            )),
        });
    }
    let frame = first
        .primary()
        .ok_or_else(|| invalid("validation_failed", "step 1 has no frame"))?;
    let result = simulate(frame)?;
    let operation = plan.operation_id(first);
    let rect = first.step.click.as_ref().map(|click| click.rect);
    let inside = match (&result.decision, rect) {
        (
            OfflineDecision::WouldClick {
                operation_label,
                action: InputAction::Tap { x, y },
                ..
            },
            Some(rect),
        ) => {
            *operation_label == operation
                && *x >= rect.x
                && *y >= rect.y
                && i64::from(*x) < i64::from(rect.x) + i64::from(rect.width)
                && i64::from(*y) < i64::from(rect.y) + i64::from(rect.height)
        }
        _ => false,
    };
    if !inside {
        return Err(admission_failed(
            "first_decision",
            format!(
                "on its primary frame, step 1 must give the click {operation} inside its \
                 rectangle"
            ),
            json!({"decision": result.decision, "frame": frame.frame_id, "rect": rect}),
        ));
    }
    let mut value = serde_json::to_value(&result.decision).map_err(|error| {
        invalid(
            "validation_failed",
            format!("failed to encode the first decision: {error}"),
        )
    })?;
    if let Some(object) = value.as_object_mut() {
        object.insert("frame".to_string(), json!(frame.frame_id));
    }
    Ok(FirstDecision {
        value,
        warning: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Passed,
    Failed,
    Undetermined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageVerdict {
    Match,
    NoMatch,
    Undetermined,
}

impl PageVerdict {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::NoMatch => "no_match",
            Self::Undetermined => "undetermined",
        }
    }
}

struct PageResult {
    verdict: PageVerdict,
    failed: Vec<String>,
    margins: Vec<(String, f64)>,
}

/// The margin of a target evaluation: score minus threshold, color distance limit minus
/// distance, digest mean limit minus mean.
fn evaluation_margin(evaluation: &TargetEvaluation) -> Option<f64> {
    if let Some(template) = &evaluation.template {
        return Some(f64::from(template.score) - f64::from(template.threshold));
    }
    if let Some(color) = &evaluation.color {
        return Some(f64::from(color.max_distance) - f64::from(color.distance));
    }
    evaluation
        .color_digest
        .as_ref()
        .map(|digest| f64::from(digest.max_mean_milli) - f64::from(digest.mean_milli))
}

/// Per-target evaluation with the admitted evaluator: OCR is undetermined and never called
/// (Lab has no provider), a check is derived from its members as at mark time.
struct Evaluation<'a> {
    plan: &'a Plan,
    evaluator: &'a RecognitionEvaluator,
}

impl Evaluation<'_> {
    fn target(&self, scene: &Scene, id: &str) -> LabResult<(Verdict, Option<f64>)> {
        let mark = self.plan.mark(id)?;
        match mark.family {
            MarkFamily::Ocr => Ok((Verdict::Undetermined, None)),
            MarkFamily::Check => {
                let mut members = Vec::new();
                for member in mark.check_members() {
                    members.push(self.target(scene, member)?.0);
                }
                let verdict = if mark.all_of.is_some() {
                    if members.contains(&Verdict::Failed) {
                        Verdict::Failed
                    } else if members.iter().all(|member| *member == Verdict::Passed) {
                        Verdict::Passed
                    } else {
                        Verdict::Undetermined
                    }
                } else if members.contains(&Verdict::Passed) {
                    Verdict::Passed
                } else if members.iter().all(|member| *member == Verdict::Failed) {
                    Verdict::Failed
                } else {
                    Verdict::Undetermined
                };
                Ok((verdict, None))
            }
            _ => {
                let evaluation = self.evaluator.evaluate_target(scene, id).map_err(|error| {
                    admission_failed(
                        "cross_check",
                        format!("target '{id}' cannot be evaluated: {error}"),
                        json!({"target": id}),
                    )
                })?;
                let verdict = if evaluation.passed {
                    Verdict::Passed
                } else {
                    Verdict::Failed
                };
                Ok((verdict, evaluation_margin(&evaluation)))
            }
        }
    }

    /// A page matches when every required target passes, does not when one fails, and is
    /// undetermined otherwise.
    fn page(&self, scene: &Scene, required: &[String]) -> LabResult<PageResult> {
        let mut failed = Vec::new();
        let mut undetermined = false;
        let mut margins = Vec::new();
        for id in required {
            let (verdict, margin) = self.target(scene, id)?;
            match verdict {
                Verdict::Passed => {}
                Verdict::Failed => failed.push(id.clone()),
                Verdict::Undetermined => undetermined = true,
            }
            if let Some(margin) = margin {
                margins.push((id.clone(), margin));
            }
        }
        let verdict = if !failed.is_empty() {
            PageVerdict::NoMatch
        } else if undetermined {
            PageVerdict::Undetermined
        } else {
            PageVerdict::Match
        };
        Ok(PageResult {
            verdict,
            failed,
            margins,
        })
    }
}

/// The cross-check results the output and the package provenance carry.
pub(crate) struct CrossCheck {
    pub(crate) warnings: Vec<Value>,
    pub(crate) arrival_by_time_window: Vec<u32>,
    pub(crate) cross_check: Value,
    pub(crate) entry_overlay: Value,
    /// Workflow #339 section 4.7c: an optional step (index into `Plan::steps`) and the earlier
    /// step of its run recorded for the same pop-up.
    pub(crate) same_as: BTreeMap<usize, usize>,
}

const ARRIVAL_MESSAGE: &str = "Arrival unconfirmed: the screen the run waits for after this \
    step's effect already passes on this step's own frames, so the run cannot confirm that the \
    effect took place. Add a recognizable transition, declare a time window, or give the next \
    step a mark that does not hold on this step's frames. Templates match with ccoeff_normed by \
    default and do not see an overall darkening: when a pop-up darkens the background another \
    template does not help; add a --color or color digest mark on a darkened bright area, or \
    use a time window. The final record stop closes the recording and it cannot be marked \
    again: check with record stop --dry-run first.";
const ARRIVAL_APPLICATION_MESSAGE: &str = " An application operation (especially a launch of \
    an application already in the foreground) may leave the screen unchanged: use restart, add \
    a recognizable transition, or declare a time window.";
const ARRIVAL_NOT_EVALUATED_MESSAGE: &str = "Arrival not evaluated: the screen the run waits \
    for after this step's effect could not be evaluated on this step's frames (OCR is not \
    evaluated in Lab), so whether the run can confirm the effect is unknown. Give the next step \
    a mark that is not OCR and does not hold on this step's frames, or declare a time window. \
    Check with record stop --dry-run before the final record stop.";
const ARRIVAL_DETOUR_MESSAGE: &str = " A run of optional steps follows this step: after its \
    click the run waits for the page after the run, which already passes on this step's frames. \
    If the screen leaves this page after the click (into a battle, for example), a run on a day \
    without the pop-up fails after the settle; a detour that returns to this page after a \
    confirmation (restoring stamina, a purchase confirmation) needs a page-graph package or two \
    packages.";
const SAME_AS_MESSAGE: &str = "Arrival unconfirmed: these optional steps record the same pop-up \
    with the same click (same_as). After the click of this step its old screen still passes as \
    the other copy's page, so a run may click the same position again before the screen changes. \
    Declare a time window transition on this step to wait before the next recognition.";
const AMBIGUITY_NOT_EVALUATED_MESSAGE: &str = "Not evaluated: OCR is not evaluated in Lab, so \
    whether this page passes on the other step's frames is unknown. Give the page a mark that is \
    not OCR and does not hold on the screen a run may show instead (a pop-up's title template or \
    button, or a --color mark on a bright area the pop-up darkens).";
const OVERLAY_MESSAGE: &str = "Step 1 still passes on its frames darkened as a whole: while a \
    darkening pop-up (an event reminder, for example) covers the screen, the Runtime takes it \
    for step 1 and a prerequisite package does not run. Add a --color or color digest mark on a \
    fixed bright area of step 1.";
const OVERLAY_NOT_EVALUATED_MESSAGE: &str = "Step 1 could not be evaluated on its darkened \
    frames (OCR is not evaluated in Lab): whether a darkening pop-up is taken for step 1 is \
    unknown. Add a --color or color digest mark on a fixed bright area of step 1.";

/// Every channel times 0.45, rounded down (alpha kept): an overall darkening such as a
/// pop-up's backdrop.
fn darkened(loaded: &LoadedFrame) -> LabResult<Scene> {
    let frame = &loaded.frame;
    let (stride, colors) = match frame.pixel_format {
        PixelFormat::Rgb8 => (3, 3),
        PixelFormat::Rgba8 => (4, 3),
    };
    let pixels = frame
        .pixels
        .chunks(stride)
        .flat_map(|pixel| {
            pixel.iter().enumerate().map(|(channel, value)| {
                if channel < colors {
                    u8::try_from(u16::from(*value) * OVERLAY_PERCENT / 100).unwrap_or(u8::MAX)
                } else {
                    *value
                }
            })
        })
        .collect::<Vec<u8>>();
    let scene = match frame.pixel_format {
        PixelFormat::Rgb8 => Scene::from_rgb8(frame.width, frame.height, &pixels),
        PixelFormat::Rgba8 => Scene::from_rgba8(frame.width, frame.height, &pixels),
    };
    scene.map_err(|error| {
        invalid(
            "record_frame_unreadable",
            format!("failed to build a darkened scene: {error}"),
        )
    })
}

fn self_mismatch(
    step: &PlanStep,
    page: &str,
    frame: &RecordedFrame,
    failed: &[String],
) -> LabError {
    with_details(
        blocked(
            "record_step_self_mismatch",
            format!(
                "page {page} of step {} does not match its own frame {}: {} fail on it",
                step.step.index,
                frame.frame_id,
                failed.join(", ")
            ),
        ),
        json!({
            "step": step.number,
            "record_index": step.step.index,
            "page": page,
            "frame_id": frame.frame_id,
            "failed": failed
        }),
    )
}

/// The frames a page matches on and the frames it is undetermined on.
fn page_on_frames(
    evaluation: &Evaluation<'_>,
    cache: &FrameCache,
    required: &[String],
    frames: &[&RecordedFrame],
) -> LabResult<(Vec<String>, Vec<String>)> {
    let mut matched = Vec::new();
    let mut undetermined = Vec::new();
    for frame in frames {
        match evaluation
            .page(&cached(cache, frame)?.scene, required)?
            .verdict
        {
            PageVerdict::Match => matched.push(frame.frame_id.clone()),
            PageVerdict::Undetermined => undetermined.push(frame.frame_id.clone()),
            PageVerdict::NoMatch => {}
        }
    }
    Ok((matched, undetermined))
}

/// What the optional-step checks leave when none refuses.
#[derive(Default)]
struct OptionalChecks {
    warnings: Vec<Value>,
    arrival_by_time_window: Vec<u32>,
    same_as: BTreeMap<usize, usize>,
    /// The skip target on each member's frames (`distinct` or `not_evaluated`), by step index.
    member_gates: BTreeMap<usize, &'static str>,
    partial: bool,
}

/// The screen a run may show when an optional page is absent (section 4.7a).
#[derive(Clone, Copy)]
enum Against {
    Previous,
    SkipTarget,
    OtherOptional,
}

impl Against {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Previous => "previous",
            Self::SkipTarget => "skip_target",
            Self::OtherOptional => "other_optional",
        }
    }

    const fn consequence(self) -> &'static str {
        match self {
            Self::Previous => {
                "the screen left over after that step's click, or after a click that did not \
                 register, would be taken for the pop-up and this step's click executed on it"
            }
            Self::SkipTarget => {
                "on every run without the pop-up this step's click would be executed on that \
                 page"
            }
            Self::OtherOptional => {
                "when only that pop-up appears it would be taken for this one and this step's \
                 different click executed on it"
            }
        }
    }
}

fn optional_ambiguous(
    step: &PlanStep,
    page: &str,
    against: Against,
    other: &PlanStep,
    frames: &[String],
) -> LabError {
    with_details(
        blocked(
            "record_optional_step_ambiguous",
            format!(
                "page {page} of optional step {} (recording step {}) passes on frames {} of step \
                 {} ({}): {}. Give step {} a mark that does not hold on that screen, such as the \
                 pop-up's title template or a button, and check with record stop --dry-run",
                step.number,
                step.step.index,
                frames.join(", "),
                other.number,
                match against {
                    Against::Previous => "the step before the run",
                    Against::SkipTarget => "the page after the run",
                    Against::OtherOptional => "another optional step of the run",
                },
                against.consequence(),
                step.step.index
            ),
        ),
        json!({
            "step": step.number,
            "page": page,
            "against": against.as_str(),
            "frames": frames
        }),
    )
}

fn skip_target_insensitive(
    skip: &PlanStep,
    page: &str,
    optional: &PlanStep,
    frames: &[String],
) -> LabError {
    with_details(
        blocked(
            "record_optional_skip_target_insensitive",
            format!(
                "page {page} of step {} (recording step {}), the page after a run of optional \
                 steps, passes on frames {} of optional step {}: under the pop-up a run would \
                 take the screen for {page}, so a close click that did not register, a changed \
                 pop-up or a pop-up never recorded would end the wait as if no pop-up had \
                 appeared instead of failing. Templates match with ccoeff_normed by default and \
                 do not see an overall darkening: add a --color or color digest mark on a bright \
                 area of step {} that the pop-up darkens, or, for a small pop-up that does not \
                 darken the screen, a mark in the area it covers; the stored frames suffice, \
                 check with record stop --dry-run",
                skip.number,
                skip.step.index,
                frames.join(", "),
                optional.number,
                skip.step.index
            ),
        ),
        json!({
            "step": skip.number,
            "optional_step": optional.number,
            "frames": frames
        }),
    )
}

/// Workflow #339 section 4.7 a–f, per run of optional steps with the step before it and its
/// skip target, on live frames only. An optional page must not pass on a screen a run may show
/// when the page is absent (a), the skip target must not pass on an optional page (b), a page
/// recorded twice with the same click is `same_as` with one `arrival_unconfirmed` (c), and an
/// undetermined evaluation is a warning (e). Every pair of frames and page is evaluated once and
/// any refusal comes before every warning (f); the step before a run (d) is the arrival gate.
fn optional_checks(
    evaluation: &Evaluation<'_>,
    plan: &Plan,
    cache: &FrameCache,
) -> LabResult<OptionalChecks> {
    let mut checks = OptionalChecks::default();
    for run in &plan.runs {
        let previous = &plan.steps[run.previous];
        let skip = &plan.steps[run.skip];
        let skip_page = skip.page_id()?;
        // A page transition of the step before the run is the screen left after its click.
        let previous_frames = if previous.transition_page.is_some() {
            previous.transition_live_frames()
        } else {
            previous.live_frames()
        };
        // (frames step, page step) -> the frames of the first on which the second passes, for
        // the members recorded with the same click.
        let mut same_pages: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
        let mut same_pairs = BTreeSet::new();
        for &member in &run.members {
            let step = &plan.steps[member];
            let page = step.page_id()?;
            let mut against = vec![
                (Against::Previous, run.previous, previous_frames.clone()),
                (Against::SkipTarget, run.skip, skip.live_frames()),
            ];
            for &other in run.members.iter().filter(|&&other| other != member) {
                against.push((
                    Against::OtherOptional,
                    other,
                    plan.steps[other].live_frames(),
                ));
            }
            for (kind, other, frames) in against {
                // An application entry step before the run has no frame of its own.
                if frames.is_empty() {
                    continue;
                }
                let (matched, undetermined) =
                    page_on_frames(evaluation, cache, &step.required, &frames)?;
                let other_step = &plan.steps[other];
                if !matched.is_empty() {
                    let same_click = matches!(kind, Against::OtherOptional)
                        && step.step.click.as_ref().map(|click| click.rect)
                            == other_step.step.click.as_ref().map(|click| click.rect);
                    if !same_click {
                        return Err(optional_ambiguous(step, page, kind, other_step, &matched));
                    }
                    same_pairs.insert((member.min(other), member.max(other)));
                    same_pages.insert((other, member), matched);
                } else if !undetermined.is_empty() {
                    checks.partial = true;
                    checks.warnings.push(json!({
                        "code": "optional_ambiguity_not_evaluated",
                        "check": "record_optional_step_ambiguous",
                        "step": step.number,
                        "page": page,
                        "against": kind.as_str(),
                        "frames": undetermined,
                        "message": AMBIGUITY_NOT_EVALUATED_MESSAGE
                    }));
                }
            }
        }
        for &member in &run.members {
            let step = &plan.steps[member];
            let (matched, undetermined) =
                page_on_frames(evaluation, cache, &skip.required, &step.live_frames())?;
            if !matched.is_empty() {
                return Err(skip_target_insensitive(skip, skip_page, step, &matched));
            }
            let result = if undetermined.is_empty() {
                "distinct"
            } else {
                checks.partial = true;
                checks.warnings.push(json!({
                    "code": "optional_ambiguity_not_evaluated",
                    "check": "record_optional_skip_target_insensitive",
                    "step": skip.number,
                    "optional_step": step.number,
                    "frames": undetermined,
                    "message": AMBIGUITY_NOT_EVALUATED_MESSAGE
                }));
                "not_evaluated"
            };
            checks.member_gates.insert(member, result);
        }
        // c. One pop-up recorded more than once: no refusal, `same_as` names the earliest
        // copy, and one arrival warning per pair on the step whose old screen passes as the
        // other's page (a window transition lists it in `arrival_by_time_window` instead).
        for &(first, second) in &same_pairs {
            checks.same_as.entry(second).or_insert(first);
            let (from, gate, frames) = match same_pages.get(&(first, second)) {
                Some(frames) => (first, second, frames),
                None => (
                    second,
                    first,
                    same_pages.get(&(second, first)).ok_or_else(|| {
                        invalid("validation_failed", "a same_as pair has no matching frames")
                    })?,
                ),
            };
            let step = &plan.steps[from];
            if step.window().is_some() {
                if !checks.arrival_by_time_window.contains(&step.number) {
                    checks.arrival_by_time_window.push(step.number);
                }
            } else {
                checks.warnings.push(json!({
                    "code": "arrival_unconfirmed",
                    "step": step.number,
                    "gate": plan.steps[gate].page_id()?,
                    "frames": frames,
                    "same_as": [plan.steps[first].number, plan.steps[second].number],
                    "message": SAME_AS_MESSAGE
                }));
            }
        }
    }
    Ok(checks)
}

/// Section 4.6 item 4 (self and arrival gates) and a15 item 5 (entry overlay).
pub(crate) fn cross_check(
    plan: &Plan,
    evaluator: &RecognitionEvaluator,
    cache: &FrameCache,
) -> LabResult<CrossCheck> {
    let evaluation = Evaluation { plan, evaluator };
    let mut partial = false;
    let mut own = Vec::new();
    let mut margins: BTreeMap<(u32, String, String), f64> = BTreeMap::new();
    let mut single_sample = Vec::new();

    // a. every page on its own live frames
    for step in &plan.steps {
        let mut pages = Vec::new();
        if let Some(page) = &step.page {
            pages.push((
                page.clone(),
                step.required.clone(),
                step.live_frames(),
                false,
            ));
            if step.live_frames().len() == 1 {
                single_sample.push(step.number);
            }
        }
        if let Some(page) = &step.transition_page {
            pages.push((
                page.clone(),
                step.transition_required.clone(),
                step.transition_live_frames(),
                true,
            ));
        }
        for (page, required, frames, transition) in pages {
            let mut results = Vec::new();
            for frame in frames {
                let result = evaluation.page(&cached(cache, frame)?.scene, &required)?;
                if result.verdict == PageVerdict::NoMatch {
                    return Err(self_mismatch(step, &page, frame, &result.failed));
                }
                partial |= result.verdict == PageVerdict::Undetermined;
                for (target, margin) in result.margins {
                    let entry = margins
                        .entry((step.number, page.clone(), target))
                        .or_insert(margin);
                    *entry = entry.min(margin);
                }
                results
                    .push(json!({"frame_id": frame.frame_id, "result": result.verdict.as_str()}));
            }
            own.push(json!({
                "step": step.number,
                "page": page,
                "transition": transition,
                "frames": results
            }));
        }
    }

    // Workflow #339 section 4.7: the optional steps, refusals before any warning.
    let optional = optional_checks(&evaluation, plan, cache)?;
    partial |= optional.partial;

    // b. the arrival gate after every effect, on the frames of its own step
    let mut warnings = Vec::new();
    let mut arrival_by_time_window = Vec::new();
    let mut gates = Vec::new();
    for (index, step) in plan.steps.iter().enumerate() {
        if !step.has_effect() {
            continue;
        }
        // Workflow #339 section 4.7d: without a page transition, the gate of the step before a
        // run of optional steps is the run's skip target; a member's gate was checked above.
        let run_after = plan
            .run_after(index)
            .filter(|_| step.transition_page.is_none());
        let member = plan
            .run_of(index)
            .filter(|_| step.transition_page.is_none());
        let (gate, required) = match run_after.or(member) {
            Some(run) => {
                let skip = &plan.steps[run.skip];
                (skip.page_id()?.to_string(), skip.required.clone())
            }
            None => plan.gate(index)?,
        };
        if step.is_entry() {
            gates.push(json!({"step": step.number, "gate": gate, "result": "not_applicable"}));
            continue;
        }
        if member.is_some() {
            let result = optional.member_gates.get(&index).copied().ok_or_else(|| {
                invalid(
                    "validation_failed",
                    format!("optional step {} has no skip target result", step.number),
                )
            })?;
            gates.push(json!({"step": step.number, "gate": gate, "result": result}));
            continue;
        }
        let mut matched = Vec::new();
        let mut undetermined = Vec::new();
        for frame in step.live_frames() {
            match evaluation
                .page(&cached(cache, frame)?.scene, &required)?
                .verdict
            {
                PageVerdict::Match => matched.push(frame.frame_id.clone()),
                PageVerdict::Undetermined => undetermined.push(frame.frame_id.clone()),
                PageVerdict::NoMatch => {}
            }
        }
        let result = if !matched.is_empty() {
            "passes_on_previous"
        } else if !undetermined.is_empty() {
            partial = true;
            "not_evaluated"
        } else {
            "distinct"
        };
        if result != "distinct" {
            if step.window().is_some() {
                arrival_by_time_window.push(step.number);
            } else if result == "passes_on_previous" {
                let mut message = ARRIVAL_MESSAGE.to_string();
                if step.step.application.is_some() {
                    message.push_str(ARRIVAL_APPLICATION_MESSAGE);
                }
                if run_after.is_some() {
                    message.push_str(ARRIVAL_DETOUR_MESSAGE);
                }
                warnings.push(json!({
                    "code": "arrival_unconfirmed",
                    "step": step.number,
                    "gate": gate,
                    "frames": matched,
                    "message": message
                }));
            } else {
                warnings.push(json!({
                    "code": "arrival_unconfirmed",
                    "step": step.number,
                    "gate": gate,
                    "frames": undetermined,
                    "reason": "not_evaluated",
                    "message": ARRIVAL_NOT_EVALUATED_MESSAGE
                }));
            }
        }
        gates.push(json!({"step": step.number, "gate": gate, "result": result}));
    }
    warnings.extend(optional.warnings);
    for number in optional.arrival_by_time_window {
        if !arrival_by_time_window.contains(&number) {
            arrival_by_time_window.push(number);
        }
    }
    arrival_by_time_window.sort_unstable();

    // a15 item 5: the first step under an overall darkening
    let first = plan
        .steps
        .first()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    let entry_overlay = match &first.page {
        None => json!({"status": "not_applicable", "reason": "application_entry"}),
        Some(page) => {
            let mut insensitive = Vec::new();
            let mut undetermined = Vec::new();
            for frame in first.live_frames() {
                let scene = darkened(cached(cache, frame)?)?;
                match evaluation.page(&scene, &first.required)?.verdict {
                    PageVerdict::Match => insensitive.push(frame.frame_id.clone()),
                    PageVerdict::Undetermined => undetermined.push(frame.frame_id.clone()),
                    PageVerdict::NoMatch => {}
                }
            }
            if !insensitive.is_empty() {
                warnings.push(json!({
                    "code": "entry_overlay_insensitive",
                    "step": 1,
                    "frames": insensitive,
                    "message": OVERLAY_MESSAGE
                }));
                json!({"status": "insensitive", "page": page, "frames": insensitive})
            } else if !undetermined.is_empty() {
                warnings.push(json!({
                    "code": "entry_overlay_insensitive",
                    "step": 1,
                    "frames": undetermined,
                    "reason": "not_evaluated",
                    "message": OVERLAY_NOT_EVALUATED_MESSAGE
                }));
                json!({"status": "not_evaluated", "page": page, "frames": undetermined})
            } else {
                json!({"status": "sensitive", "page": page})
            }
        }
    };

    let margins = margins
        .into_iter()
        .map(|((step, page, target), margin)| {
            json!({"step": step, "page": page, "target": target, "min_margin": margin})
        })
        .collect::<Vec<_>>();
    let mut cross_check = Map::new();
    cross_check.insert(
        "status".to_string(),
        json!(if partial {
            "partially_evaluated"
        } else {
            "passed"
        }),
    );
    cross_check.insert("self".to_string(), json!(own));
    cross_check.insert("gates".to_string(), json!(gates));
    cross_check.insert("margins".to_string(), json!(margins));
    cross_check.insert("single_sample_steps".to_string(), json!(single_sample));
    Ok(CrossCheck {
        warnings,
        arrival_by_time_window,
        cross_check: Value::Object(cross_check),
        entry_overlay,
        same_as: optional.same_as,
    })
}
