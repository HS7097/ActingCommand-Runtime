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
use std::collections::BTreeMap;
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
        let warning = (!first.is_entry()).then(|| {
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

    // b. the arrival gate after every effect, on the frames of its own step
    let mut warnings = Vec::new();
    let mut arrival_by_time_window = Vec::new();
    let mut gates = Vec::new();
    for (index, step) in plan.steps.iter().enumerate() {
        if !step.has_effect() {
            continue;
        }
        let (gate, required) = plan.gate(index)?;
        if step.is_entry() {
            gates.push(json!({"step": step.number, "gate": gate, "result": "not_applicable"}));
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
    })
}
