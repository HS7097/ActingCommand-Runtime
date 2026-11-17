// SPDX-License-Identifier: AGPL-3.0-only

//! The `record stop` self-checks (Workflow #336 L4, frozen model section 4.6 with the a15 and
//! R24 items): the container round trip, admission by the kernel on the content table, the
//! first decision of an offline simulation, and the per-target cross-check of every page on
//! the live frames of the recording (the step itself, the arrival gate after each effect, and
//! the first step under an overall darkening). Every refusal is
//! `record_artifact_admission_failed` or `record_step_self_mismatch`; warnings never refuse.

use super::ending::ending_invalid;
use super::follow::followed_evaluation_box;
use super::frames::LoadedFrame;
use super::generate::{Plan, PlanEnding, PlanStep, follows_located_text, select_policy_document};
use super::model::{
    ClickFollow, ClickGesture, MarkFamily, RecordRect, RecordedFrame, SelectDecision,
    SelectFrameDecision, StepSelect,
};
use super::select::{
    NAME_UNREAD, PRESS_MISMATCH, PRESSED_ABSENT, SELF_TEST_ERROR, decide, on_empty_key,
};
use super::store::{blocked, invalid, with_details};
use crate::LabOcr;
use crate::codes::{EndingAgainst, EndingProblem, LabCode, ReadyCheck, ReadyCheckStop};
use crate::regression_case::RegressionCheck;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    InputAction, LabError, LabResult, SEGMENTED_SWIPE_BRAKE_DISTANCE_PX,
    SEGMENTED_SWIPE_BRAKE_DURATION_MS, SEGMENTED_SWIPE_CORNER_HOLD_MS,
    SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS,
};
use actingcommand_device::{CaptureBackendName, Frame, PixelFormat};
use actingcommand_execution_kernel::{
    ExternallyVerifiedBundle, OfflineDecision, PreparedContainedTask, simulate_contained_task,
    simulate_contained_task_at,
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

/// #463 S2-1c (`contracts/lab-recording.md`, "Self-checks", "What a recording still lacks"): where
/// a self-check's failure is: the step it is about (its serial number and its number in the
/// package), the frames it failed on, and the page or the target it is about; for an ending, its
/// outcome key and its page. Empty for a refusal of the package as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Locus {
    pub(crate) step: Option<(u32, u32)>,
    pub(crate) frames: Vec<String>,
    pub(crate) page: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) outcome_key: Option<String>,
    /// #463 S2-10: the regression case a failure of the self-check `cases` is about (its id), and
    /// the checks of it that failed, as the runner answered them.
    pub(crate) case: Option<String>,
    pub(crate) checks: Vec<RegressionCheck>,
}

impl Locus {
    /// On the package step `step`.
    pub(crate) fn on_step(step: &PlanStep) -> Self {
        Self {
            step: Some((step.step.index, step.number)),
            ..Self::default()
        }
    }

    /// On the ending `ending`, which is no step: its outcome key and its page.
    pub(crate) fn on_ending(ending: &PlanEnding) -> Self {
        Self {
            page: Some(ending.page.clone()),
            outcome_key: Some(ending.ending.outcome_key.clone()),
            ..Self::default()
        }
    }

    /// On the frame `frame`.
    pub(crate) fn with_frame(mut self, frame: &RecordedFrame) -> Self {
        self.frames = vec![frame.frame_id.clone()];
        self
    }

    /// On the frames `frames`.
    pub(crate) fn with_frames(mut self, frames: &[String]) -> Self {
        self.frames = frames.to_vec();
        self
    }

    /// About the page `page`.
    pub(crate) fn with_page(mut self, page: &str) -> Self {
        self.page = Some(page.to_string());
        self
    }

    /// About the target `target`: a template's crop, the mark a click follows, (fix round 1) the
    /// target that reads a select slot's name.
    pub(crate) fn with_target(mut self, target: &str) -> Self {
        self.target = Some(target.to_string());
        self
    }

    /// #463 S2-10: about the regression case `case`, whose checks `checks` failed.
    pub(crate) fn with_case(mut self, case: &str, checks: Vec<RegressionCheck>) -> Self {
        self.case = Some(case.to_string());
        self.checks = checks;
        self
    }
}

/// #463 S2-1c: one failure a self-check met: where it is, and the refusal `record stop` answers for
/// it.
#[derive(Debug, Clone)]
pub(crate) struct CheckFailure {
    pub(crate) locus: Locus,
    pub(crate) error: LabError,
}

/// #463 S2-1c: one self-check that ran: its failures, in the order it met them, and, when it did not
/// go to its end, why (`stopped`); its last failure is then the one it stopped at.
#[derive(Debug, Clone)]
pub(crate) struct CheckRun {
    pub(crate) check: ReadyCheck,
    pub(crate) failures: Vec<CheckFailure>,
    pub(crate) stopped: Option<ReadyCheckStop>,
}

/// #463 S2-1c: how a self-check meets a failure, one implementation for its two callers. `record
/// stop` (`first`) ends the check at its first failure, which is its refusal, as before; `record
/// ready` (`every`) collects every failure and the check goes on to the next thing it judges. An
/// error that is none of the check's verdicts (a target the evaluator cannot evaluate, a frame it
/// cannot decode) is no failure met here: it ends the check for both.
#[derive(Debug)]
pub(crate) struct Failures {
    every: bool,
    found: Vec<CheckFailure>,
}

impl Failures {
    /// `record stop`'s: the first failure is the refusal.
    pub(crate) const fn first() -> Self {
        Self {
            every: false,
            found: Vec::new(),
        }
    }

    /// `record ready`'s: every failure is collected.
    pub(crate) const fn every() -> Self {
        Self {
            every: true,
            found: Vec::new(),
        }
    }

    /// Whether every failure is collected (`record ready`).
    pub(crate) const fn collects(&self) -> bool {
        self.every
    }

    /// The failure `error` at `locus`: for `record stop` the refusal (`Err`), for `record ready`
    /// collected (`Ok`), and the check goes on.
    pub(crate) fn fail(&mut self, locus: Locus, error: LabError) -> LabResult<()> {
        if self.every {
            self.found.push(CheckFailure { locus, error });
            Ok(())
        } else {
            Err(error)
        }
    }

    /// How many failures were collected so far.
    pub(crate) fn count(&self) -> usize {
        self.found.len()
    }

    /// The failures collected so far, which are no longer held here.
    pub(crate) fn take(&mut self) -> Vec<CheckFailure> {
        std::mem::take(&mut self.found)
    }
}

fn admission_failed(stage: &str, message: String, mut details: Value) -> LabError {
    if let Some(object) = details.as_object_mut() {
        object.insert("stage".to_string(), json!(stage));
    }
    with_details(
        blocked("record_artifact_admission_failed", message),
        details,
    )
}

fn containment_code(error: &ContainmentError) -> Option<&'static str> {
    match error {
        ContainmentError::SourceTree { code } => Some(code.as_str()),
        _ => None,
    }
}

/// Section 4.6 item 1: the encoded container expands to exactly the generated table, entry by
/// entry (Workflow #336 PKG-S5: the table itself is compared, no digest of it).
pub(crate) fn round_trip(
    bytes: &[u8],
    kind: ContentContainer,
    entries: &BTreeMap<String, Vec<u8>>,
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
    Ok(())
}

/// Section 4.6 item 2: the kernel admits the content table as actingd loads a content
/// directory: source compilation, declaration and `linear_steps` validation (Workflow #336
/// PKG-S1: no digest comparison). The Lab's OCR provider, when it has one, evaluates OCR in the
/// first decision (Workflow #396 LAB-3); admission itself never calls it.
pub(crate) fn admit(
    entries: &BTreeMap<String, Vec<u8>>,
    ocr: &LabOcr,
) -> LabResult<PreparedContainedTask> {
    PreparedContainedTask::load_content_entries(
        ADMISSION_INSTANCE,
        entries.clone(),
        ocr.provider(),
        Instant::now() + ADMISSION_BUDGET,
    )
    .map_err(|error| {
        admission_failed(
            "admission",
            format!("the generated package is not admitted: {error}"),
            json!({
                "loader_code": error.code().as_str(),
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

/// The admitted bundle whose evaluator the cross-check uses (section 4.6 item 4), with the
/// Lab's OCR provider when it has one (Workflow #396 LAB-3).
pub(crate) fn evaluator_bundle(
    entries: &BTreeMap<String, Vec<u8>>,
    ocr: &LabOcr,
) -> LabResult<ExternallyVerifiedBundle> {
    ExternallyVerifiedBundle::load_content_entries(
        ADMISSION_INSTANCE,
        entries.clone(),
        false,
        ocr.provider(),
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

impl FirstDecision {
    /// #463 S2-1c: what the first decision answers once `record ready` collected a failure of it:
    /// no decision. It is never read: a failed self-check leaves the final package unbuilt.
    const fn undecided() -> Self {
        Self {
            value: Value::Null,
            warning: None,
        }
    }
}

fn not_evaluated_warning(reason: &str, message: &str) -> Value {
    json!({
        "code": "first_decision_not_evaluated",
        "step": 1,
        "reason": reason,
        "message": message
    })
}

/// Workflow #396 REC-4: the box a click following a template clicks on `loaded`, as the admitted
/// evaluator matches the template there; `None` when the template does not pass on it. #463 L2:
/// for a click following a located-text mark, the box the admitted evaluator finds its names in
/// there, with the Lab's OCR provider.
fn followed_rect(
    plan: &Plan,
    evaluator: &RecognitionEvaluator,
    loaded: &LoadedFrame,
    follow: &ClickFollow,
) -> LabResult<Option<RecordRect>> {
    let evaluation = evaluator
        // Workflow #390 LAB-2: a followed template that anchors its page is its `page/` target.
        .evaluate_target(&loaded.scene, &plan.target_id(&follow.mark_id))
        .map_err(|error| {
            admission_failed(
                "first_decision",
                format!(
                    "the followed mark '{}' cannot be evaluated: {error}",
                    follow.mark_id
                ),
                json!({"target": follow.mark_id}),
            )
        })?;
    Ok(followed_evaluation_box(&evaluation, follow))
}

/// Section 4.6 item 3 with R24: a package with an application step is simulated on the first
/// live frame and must be refused before any capture; a first step involving OCR is not
/// simulated when the Lab has no OCR model (Workflow #396 LAB-3: `lab_ocr_unavailable`);
/// otherwise the first step's primary frame must give the first click, inside its rectangle.
/// Workflow #396 REC-4: when that click follows a template, every live frame of step 1 must give
/// it inside the box the template gives on that frame (`evaluator` evaluates the template). #463
/// S2-1c: each frame's decision is one judgement; a frame that fails it is a failure `failures`
/// meets (step 1, that frame), and under `record ready` the other frames are still judged.
pub(crate) fn first_decision(
    prepared: &PreparedContainedTask,
    plan: &Plan,
    cache: &FrameCache,
    ocr: &LabOcr,
    evaluator: &RecognitionEvaluator,
    failures: &mut Failures,
) -> LabResult<FirstDecision> {
    let first = plan
        .steps
        .first()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    // #463 L2: a click on located text confirms its guard on one fresh frame before the input
    // (`input_recheck`), which the simulation is given as the same recorded frame once more.
    let rechecks = follows_located_text(plan, first)?;
    let simulate = |frame: &RecordedFrame| -> LabResult<_> {
        let loaded = cached(cache, frame)?;
        let mut frames = vec![device_frame(loaded)?];
        if rechecks {
            frames.push(device_frame(loaded)?);
        }
        simulate_contained_task(prepared, frames).map_err(|error| {
            admission_failed(
                "first_decision",
                format!("the offline simulation failed: {error}"),
                json!({"code": error.code().as_str(), "detail": error.detail()}),
            )
        })
    };
    let on = |frame: &RecordedFrame| Locus::on_step(first).with_frame(frame);
    if plan.has_application_step() {
        let frame = plan
            .steps
            .iter()
            .find_map(PlanStep::primary)
            .ok_or_else(|| invalid("validation_failed", "the package has no frame"))?;
        // One decision on one frame: the run refuses the package before any capture.
        let decided = simulate(frame).and_then(|result| {
            let refused = matches!(&result.decision, OfflineDecision::Refused { code, .. } if code.as_str() == APPLICATION_REFUSAL);
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
            Ok(())
        });
        if let Err(error) = decided {
            failures.fail(on(frame), error)?;
            return Ok(FirstDecision::undecided());
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
    // #463 L2 fix 1: a guard on located text reads OCR though its page does not; #463 R1: so does
    // a select, which reads its slots' names.
    if ocr.ocr_provider().is_none()
        && (plan.involves_ocr(&first.required)?
            || plan.involves_ocr(&first.forbidden)?
            || rechecks
            || first.step.select.is_some())
    {
        let reason = LabCode::LabOcrUnavailable.as_str();
        return Ok(FirstDecision {
            value: json!({"status": "not_evaluated", "reason": reason}),
            warning: Some(not_evaluated_warning(
                reason,
                "step 1 involves OCR and the Lab has no OCR model: the first decision was not \
                 simulated; install the OCR model under the Runtime's vision root, or make sure \
                 step 1 really needs OCR",
            )),
        });
    }
    let frame = first
        .primary()
        .ok_or_else(|| invalid("validation_failed", "step 1 has no frame"))?;
    // #463 R1: a select at step 1 decides on its primary frame by the kernel's dry run; a slot it
    // selects is then simulated as the run taps it, and an empty decision is the run's normal end.
    if let Some(select) = &first.step.select {
        // #463 S2-9a: a scrolling select at step 1 decides over its demonstration's screens by the
        // kernel's offline entry; its tap follows a return that no saved frame shows.
        if let Some(scan) = &select.scan {
            return super::scan::scan_first_decision(
                prepared,
                plan,
                (first, select, scan),
                cache,
                failures,
            );
        }
        return select_first_decision(prepared, plan, first, select, frame, cache, failures);
    }
    let start = failures.count();
    // The primary frame's decision is the one the answer shows.
    let primary = match simulate(frame) {
        Ok(result) => Some(result),
        Err(error) => {
            failures.fail(on(frame), error)?;
            None
        }
    };
    let operation = plan.operation_id(first);
    let click = first.step.click.as_ref();
    let follow = click.and_then(|click| click.follow.as_ref());
    // Workflow #396 REC-4: a click that follows a template clicks the box the template gives
    // on the frame, so every live frame of step 1 is simulated, each against its own box. #463
    // S2-1c: a frame whose simulation failed is judged no further.
    let mut checked = Vec::new();
    if follow.is_none() {
        checked.extend(primary.iter().map(|result| (frame, result.clone())));
    } else {
        for live in first.live_frames() {
            if live.sha256 == frame.sha256 && live.frame_id == frame.frame_id {
                checked.extend(primary.iter().map(|result| (live, result.clone())));
                continue;
            }
            match simulate(live) {
                Ok(decision) => checked.push((live, decision)),
                Err(error) => failures.fail(on(live), error)?,
            }
        }
    }
    let point_in = |x: i32, y: i32, rect: RecordRect| {
        x >= rect.x
            && y >= rect.y
            && i64::from(x) < i64::from(rect.x) + i64::from(rect.width)
            && i64::from(y) < i64::from(rect.y) + i64::from(rect.height)
    };
    // Workflow #390 LAB-2: a drag is one swipe from its rectangle into its destination over its
    // duration; a long press holds a point of its click box (Workflow #390 lpbox) for its
    // duration.
    let gesture = click.and_then(|click| click.gesture);
    for (checked_frame, checked_result) in &checked {
        let rect = match (click, follow) {
            (Some(_), Some(follow)) => {
                followed_rect(plan, evaluator, cached(cache, checked_frame)?, follow)?
            }
            (click, _) => click.map(|click| click.rect),
        };
        let inside = match (&checked_result.decision, rect, gesture) {
            (
                OfflineDecision::WouldClick {
                    operation_label,
                    action: InputAction::Tap { x, y },
                    ..
                },
                Some(rect),
                None,
            ) => *operation_label == operation && point_in(*x, *y, rect),
            (
                OfflineDecision::WouldClick {
                    operation_label,
                    action:
                        InputAction::Swipe {
                            x1,
                            y1,
                            x2,
                            y2,
                            duration_ms,
                        },
                    ..
                },
                Some(rect),
                Some(ClickGesture::Drag {
                    to,
                    duration_ms: declared,
                }),
            ) => {
                *operation_label == operation
                    && point_in(*x1, *y1, rect)
                    && point_in(*x2, *y2, to)
                    && *duration_ms == declared
            }
            (
                OfflineDecision::WouldClick {
                    operation_label,
                    action: InputAction::LongTap { x, y, duration_ms },
                    ..
                },
                Some(rect),
                Some(ClickGesture::LongPress {
                    duration_ms: declared,
                }),
            ) => {
                *operation_label == operation && point_in(*x, *y, rect) && *duration_ms == declared
            }
            // #411 Lab BD: a brake drag presses inside its rectangle, moves into its corner box
            // and brakes straight up by the brake distance, with the Runtime's fixed timings.
            (
                OfflineDecision::WouldClick {
                    operation_label,
                    action:
                        InputAction::SingleTouchDragWithVerticalBrakeV1 {
                            x1,
                            y1,
                            x2,
                            y2,
                            x3,
                            y3,
                            horizontal_duration_ms,
                            corner_hold_ms,
                            brake_distance_px,
                            brake_duration_ms,
                            ..
                        },
                    ..
                },
                Some(rect),
                Some(ClickGesture::SingleTouchDragWithVerticalBrakeV1 { corner }),
            ) => {
                *operation_label == operation
                    && point_in(*x1, *y1, rect)
                    && point_in(*x2, *y2, corner)
                    && *x3 == *x2
                    && i64::from(*y3)
                        == i64::from(*y2) - i64::from(SEGMENTED_SWIPE_BRAKE_DISTANCE_PX)
                    && *horizontal_duration_ms == SEGMENTED_SWIPE_HORIZONTAL_DURATION_MS
                    && *corner_hold_ms == SEGMENTED_SWIPE_CORNER_HOLD_MS
                    && *brake_distance_px == SEGMENTED_SWIPE_BRAKE_DISTANCE_PX
                    && *brake_duration_ms == SEGMENTED_SWIPE_BRAKE_DURATION_MS
            }
            // Workflow #406 T4: a path drag turns inside its box, the recorded rectangle (which
            // lies inside the frame): the kernel's path holds that box, `turns + 2` points all
            // inside it, segment durations that sum to the declared duration, and the declared
            // corner hold.
            (
                OfflineDecision::WouldClick {
                    operation_label,
                    action:
                        InputAction::SingleTouchPathDragV1 {
                            box_x,
                            box_y,
                            box_width,
                            box_height,
                            points,
                            segment_duration_ms,
                            corner_hold_ms,
                        },
                    ..
                },
                Some(rect),
                Some(ClickGesture::SingleTouchPathDragV1 {
                    turns,
                    duration_ms,
                    corner_hold_ms: declared_hold,
                    ..
                }),
            ) => {
                *operation_label == operation
                    && (*box_x, *box_y, *box_width, *box_height)
                        == (rect.x, rect.y, rect.width, rect.height)
                    && u64::try_from(points.len()).ok() == turns.checked_add(2)
                    && points.iter().all(|point| point_in(point.x, point.y, rect))
                    && segment_duration_ms
                        .iter()
                        .try_fold(0_u64, |total, segment| total.checked_add(*segment))
                        == Some(duration_ms)
                    && *corner_hold_ms == declared_hold
            }
            _ => false,
        };
        if !inside {
            let message = match follow {
                None => format!(
                    "on its primary frame, step 1 must give the click {operation} inside its \
                     rectangle"
                ),
                Some(follow) => format!(
                    "on frame {}, step 1 must give the click {operation} inside the box that \
                     its followed mark '{}' gives there",
                    checked_frame.frame_id, follow.mark_id
                ),
            };
            let mut locus = Locus::on_step(first).with_frame(checked_frame);
            if let Some(follow) = follow {
                locus = locus.with_target(&follow.mark_id);
            }
            failures.fail(
                locus,
                admission_failed(
                    "first_decision",
                    message,
                    json!({
                        "decision": checked_result.decision,
                        "frame": checked_frame.frame_id,
                        "rect": rect
                    }),
                ),
            )?;
        }
    }
    let Some(result) = primary.filter(|_| failures.count() == start) else {
        return Ok(FirstDecision::undecided());
    };
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

fn rect_holds(x: i32, y: i32, rect: RecordRect) -> bool {
    x >= rect.x
        && y >= rect.y
        && i64::from(x) < i64::from(rect.x) + i64::from(rect.width)
        && i64::from(y) < i64::from(rect.y) + i64::from(rect.height)
}

/// #463 R1: the first decision of a package whose step 1 selects in a list: the kernel's dry run
/// of the select on step 1's primary frame with the sealed policy (the wanted value's default).
/// Every slot's name reads a known name there, or the decision is refused; a selected slot is
/// then the tap the offline simulation of the package makes (`simulate_contained_task_at`, the
/// confirmation on the same frame), inside that slot's click box; an empty decision is the run's
/// normal end with the `on_empty` key, which the simulation does not report
/// (`offline_simulation_executed_step_invariant`). #463 S2-1c: one frame's decision, each part
/// read from the one before, so its first failure is the step's only one (`failures`: step 1, its
/// primary frame).
fn select_first_decision(
    prepared: &PreparedContainedTask,
    plan: &Plan,
    first: &PlanStep,
    select: &StepSelect,
    frame: &RecordedFrame,
    cache: &FrameCache,
    failures: &mut Failures,
) -> LabResult<FirstDecision> {
    let operation = plan.operation_id(first);
    let loaded = cached(cache, frame)?;
    let mut refuse = |error: LabError| -> LabResult<FirstDecision> {
        failures.fail(Locus::on_step(first).with_frame(frame), error)?;
        Ok(FirstDecision::undecided())
    };
    let decided = match decide(
        prepared,
        &operation,
        &loaded.frame,
        None,
        plan.generated_at_unix_ms,
    )? {
        Ok(decided) => decided,
        Err(failure) => {
            return refuse(admission_failed(
                "first_decision",
                format!(
                    "the dry run of step 1's select fails on its primary frame {}",
                    frame.frame_id
                ),
                json!({"frame": frame.frame_id, "failure": failure}),
            ));
        }
    };
    if !decided.unread.is_empty() {
        return refuse(admission_failed(
            "first_decision",
            format!(
                "on its primary frame {} a slot of step 1's select reads no known name",
                frame.frame_id
            ),
            json!({
                "frame": frame.frame_id,
                "unread": decided
                    .unread
                    .iter()
                    .map(|(slot, cause)| json!({"slot": slot, "cause": cause}))
                    .collect::<Vec<_>>()
            }),
        ));
    }
    // #463 R1 fix 1: only an empty decision is the run's normal end; any other decision that
    // selects no slot is one the run fails on.
    let slot = match decided.kind {
        SelectDecision::Selected { slot } => slot,
        SelectDecision::Empty => {
            return Ok(FirstDecision {
                value: json!({
                    "status": "would_end_empty",
                    "operation_label": operation,
                    "outcome_key": on_empty_key(&plan.task_id, &select.declaration),
                    "frame": frame.frame_id
                }),
                warning: None,
            });
        }
        other => {
            return refuse(admission_failed(
                "first_decision",
                format!(
                    "on its primary frame {} the select of step 1 selects no slot, and the run \
                     would fail there",
                    frame.frame_id
                ),
                json!({"frame": frame.frame_id, "decision": other}),
            ));
        }
    };
    let rect = decided.clicks.get(&slot).copied().ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("the dry run of step 1 selected slot {slot}, which it lacks"),
        )
    })?;
    let result = match simulate_contained_task_at(
        prepared,
        vec![device_frame(loaded)?],
        plan.generated_at_unix_ms,
    ) {
        Ok(result) => result,
        Err(error) => {
            return refuse(admission_failed(
                "first_decision",
                format!("the offline simulation failed: {error}"),
                json!({"code": error.code().as_str(), "detail": error.detail()}),
            ));
        }
    };
    let inside = matches!(
        &result.decision,
        OfflineDecision::WouldClick {
            operation_label,
            action: InputAction::Tap { x, y },
            ..
        } if *operation_label == operation && rect_holds(*x, *y, rect)
    );
    if !inside {
        return refuse(admission_failed(
            "first_decision",
            format!(
                "on its primary frame, step 1 must tap {operation} inside slot {slot}, the slot its \
                 select chooses"
            ),
            json!({
                "decision": result.decision,
                "frame": frame.frame_id,
                "slot": slot,
                "rect": rect
            }),
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
        object.insert("slot".to_string(), json!(slot));
    }
    Ok(FirstDecision {
        value,
        warning: None,
    })
}

/// #463 R1: what the cross-check of the select steps leaves: one entry per select step (its
/// decisions on every live frame) and the steps not evaluated without an OCR model.
pub(crate) struct SelectChecks {
    pub(crate) selects: Vec<Value>,
    pub(crate) not_evaluated: Vec<u32>,
}

/// #463 R1: a select step's frame on which the package would not choose as recorded:
/// `record_step_self_mismatch` naming the step, the frame and why (`reason`).
fn select_mismatch(
    step: &PlanStep,
    frame: &RecordedFrame,
    reason: &str,
    detail: Value,
) -> LabError {
    let mut details = json!({
        "step": step.number,
        "record_index": step.step.index,
        "frame_id": frame.frame_id,
        "reason": reason
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), detail.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    with_details(
        blocked(
            "record_step_self_mismatch",
            format!(
                "the select of step {} does not choose on its own frame {} as recorded: {reason}",
                step.step.index, frame.frame_id
            ),
        ),
        details,
    )
}

/// #463 R1, self-check item 8: on every live frame of every select step the kernel's dry run of
/// the admitted package decides; the step's page must be the one page of the package that passes
/// on the frame (the page rule and the layout share it), every slot's name reads a known name, and
/// on the primary frame the pressed name selects a slot, the slot the recorded press hit. Each
/// frame's decisions are listed; any other frame is `record_step_self_mismatch`. Without an OCR
/// model nothing is called and the step is undetermined. #463 S2-1c: each frame's decision is one
/// judgement, its parts read from the one before; a frame that fails it is a failure `failures`
/// meets (the step, the frame, its page), and under `record ready` the next frame is judged. Fix
/// round 1: a frame on which slots read no known name is one failure per such slot (the target that
/// reads its name), so that one read lists them all.
pub(crate) fn select_checks(
    prepared: &PreparedContainedTask,
    plan: &Plan,
    cache: &FrameCache,
    ocr: &LabOcr,
    failures: &mut Failures,
) -> LabResult<SelectChecks> {
    let mut selects = Vec::new();
    let mut not_evaluated = Vec::new();
    for (step, select) in plan.select_steps() {
        let spec = &select.declaration;
        let operation = plan.operation_id(step);
        let mut item = json!({
            "step": step.number,
            "record_index": step.step.index,
            "op": operation,
            "layout": plan.select_layout_id(step),
            "wanted": spec.wanted,
            "pressed": spec.pressed_name(),
            "on_empty_key": on_empty_key(&plan.task_id, spec)
        });
        if ocr.ocr_provider().is_none() {
            not_evaluated.push(step.number);
            item["frames"] = json!(
                step.live_frames()
                    .iter()
                    .map(|frame| json!({"frame_id": frame.frame_id, "result": "undetermined"}))
                    .collect::<Vec<_>>()
            );
            selects.push(item);
            continue;
        }
        // #463 S2-9a: a scrolling select is checked on its own frames and on the screens of its
        // demonstration, with the kernel's offline entry over them.
        if let Some(scan) = &select.scan {
            super::scan::scan_select_checks(
                prepared,
                plan,
                (step, select, scan),
                cache,
                failures,
                &mut item,
            )?;
            selects.push(item);
            continue;
        }
        let override_document = (spec.pressed_name() != spec.wanted.default)
            .then(|| select_policy_document(plan, step, spec, spec.pressed_name()));
        let executed = step
            .step
            .click
            .as_ref()
            .filter(|click| click.executed())
            .and_then(|click| click.execution.as_ref())
            .map(|execution| execution.point);
        let mut frames = Vec::new();
        for frame in step.live_frames() {
            let loaded = cached(cache, frame)?;
            let on = || Locus {
                page: step.page.clone(),
                ..Locus::on_step(step).with_frame(frame)
            };
            // The kernel's dry run: a decision, or the frame's `self_test_error`.
            let run = |document: Option<&Value>| {
                decide(
                    prepared,
                    &operation,
                    &loaded.frame,
                    document,
                    plan.generated_at_unix_ms,
                )
                .map(|decided| {
                    decided.map_err(|failure| {
                        select_mismatch(step, frame, SELF_TEST_ERROR, json!({"failure": failure}))
                    })
                })
            };
            let default = match run(None)? {
                Ok(default) => default,
                Err(mismatch) => {
                    failures.fail(on(), mismatch)?;
                    continue;
                }
            };
            // Fix round 1: every slot of the frame whose name reads no known name, each about the
            // target that reads it; `record stop` answers the first, as before.
            if !default.unread.is_empty() {
                for (slot, cause) in &default.unread {
                    failures.fail(
                        on().with_target(&plan.select_slot_target_id(step, *slot as usize)),
                        select_mismatch(
                            step,
                            frame,
                            NAME_UNREAD,
                            json!({"slot": slot, "cause": cause, "default": default.kind}),
                        ),
                    )?;
                }
                continue;
            }
            let pressed = match &override_document {
                None => default.kind,
                Some(document) => match run(Some(document))? {
                    Ok(decided) => decided.kind,
                    Err(mismatch) => {
                        failures.fail(on(), mismatch)?;
                        continue;
                    }
                },
            };
            if frame.role == "primary" {
                let Some(slot) = pressed.slot() else {
                    failures.fail(
                        on(),
                        select_mismatch(
                            step,
                            frame,
                            PRESSED_ABSENT,
                            json!({"pressed": spec.pressed_name(), "decision": pressed}),
                        ),
                    )?;
                    continue;
                };
                if let Some(point) = executed {
                    let rect = default.clicks.get(&slot).copied().ok_or_else(|| {
                        invalid(
                            "validation_failed",
                            format!("the dry run selected slot {slot}, which it lacks"),
                        )
                    })?;
                    if !rect_holds(point.x, point.y, rect) {
                        failures.fail(
                            on(),
                            select_mismatch(
                                step,
                                frame,
                                PRESS_MISMATCH,
                                json!({"slot": slot, "click_rect": rect, "point": point}),
                            ),
                        )?;
                        continue;
                    }
                }
            }
            // #463 R1 fix 1: each decision by its kind, as at mark time.
            frames.push(json!(SelectFrameDecision {
                frame_id: frame.frame_id.clone(),
                pressed,
                default: default.kind,
            }));
        }
        item["frames"] = json!(frames);
        selects.push(item);
    }
    Ok(SelectChecks {
        selects,
        not_evaluated,
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
    /// Workflow #390 LAB-2: the forbidden marks that pass.
    forbidden_passed: Vec<String>,
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

/// Per-target evaluation with the admitted evaluator: OCR with the Lab's OCR provider, and
/// undetermined without an OCR model, when it is never called (Workflow #396 LAB-3); a check
/// is derived from its members as at mark time.
struct Evaluation<'a> {
    plan: &'a Plan,
    evaluator: &'a RecognitionEvaluator,
    ocr_available: bool,
}

impl Evaluation<'_> {
    fn target(&self, scene: &Scene, id: &str) -> LabResult<(Verdict, Option<f64>)> {
        let mark = self.plan.mark(id)?;
        match mark.family {
            // #411 Lab B-9: a value test reads OCR; with a model the admitted evaluator evaluates
            // it below, as any other target.
            MarkFamily::Ocr | MarkFamily::ValueTest if !self.ocr_available => {
                Ok((Verdict::Undetermined, None))
            }
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
                // Workflow #390 LAB-2: a template that anchors a page is its `page/` target.
                let target = self.plan.target_id(id);
                let evaluation =
                    self.evaluator
                        .evaluate_target(scene, &target)
                        .map_err(|error| {
                            admission_failed(
                                "cross_check",
                                format!("target '{target}' cannot be evaluated: {error}"),
                                json!({"target": target}),
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

    /// A page matches when every required target passes and (Workflow #390 LAB-2) no forbidden
    /// target passes, does not when a required one fails or a forbidden one passes, and is
    /// undetermined otherwise.
    fn page(
        &self,
        scene: &Scene,
        required: &[String],
        forbidden: &[String],
    ) -> LabResult<PageResult> {
        let mut failed = Vec::new();
        let mut forbidden_passed = Vec::new();
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
                margins.push((self.plan.target_id(id), margin));
            }
        }
        for id in forbidden {
            match self.target(scene, id)?.0 {
                Verdict::Passed => forbidden_passed.push(id.clone()),
                Verdict::Failed => {}
                Verdict::Undetermined => undetermined = true,
            }
        }
        let verdict = if !failed.is_empty() || !forbidden_passed.is_empty() {
            PageVerdict::NoMatch
        } else if undetermined {
            PageVerdict::Undetermined
        } else {
            PageVerdict::Match
        };
        Ok(PageResult {
            verdict,
            failed,
            forbidden_passed,
            margins,
        })
    }

    /// #463 L2 fix 2: the located-text target `id` on `scene`, as the guard of the click that
    /// follows it evaluates it there; `None` without an OCR model, when OCR is never called.
    fn located_guard(&self, scene: &Scene, id: &str) -> LabResult<Option<TargetEvaluation>> {
        if !self.ocr_available {
            return Ok(None);
        }
        let target = self.plan.target_id(id);
        self.evaluator
            .evaluate_target(scene, &target)
            .map(Some)
            .map_err(|error| {
                admission_failed(
                    "cross_check",
                    format!("target '{target}' cannot be evaluated: {error}"),
                    json!({"target": target}),
                )
            })
    }
}

/// The cross-check results the output and the package provenance carry. #463 S2-1c: empty
/// (`Default`) once `record ready` collected a failure of the cross-check, when nothing reads them.
#[derive(Default)]
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
    result: &PageResult,
) -> LabError {
    let mut details = json!({
        "step": step.number,
        "record_index": step.step.index,
        "page": page,
        "frame_id": frame.frame_id,
        "failed": result.failed
    });
    // Workflow #390 LAB-2: forbidden marks that pass on the page's own frame.
    if !result.forbidden_passed.is_empty()
        && let Some(object) = details.as_object_mut()
    {
        object.insert(
            "forbidden_passed".to_string(),
            json!(result.forbidden_passed),
        );
    }
    let mut reasons = Vec::new();
    if !result.failed.is_empty() {
        reasons.push(format!("{} fail on it", result.failed.join(", ")));
    }
    if !result.forbidden_passed.is_empty() {
        reasons.push(format!(
            "the forbidden {} pass on it",
            result.forbidden_passed.join(", ")
        ));
    }
    with_details(
        blocked(
            "record_step_self_mismatch",
            format!(
                "page {page} of step {} does not match its own frame {}: {}",
                step.step.index,
                frame.frame_id,
                reasons.join("; ")
            ),
        ),
        details,
    )
}

/// #463 L2 fix 2: the guard of a step whose click follows a located-text mark, that mark's names,
/// is not found on one of the step's own frames (`evaluation`, with the kernel's typed reason).
fn guard_mismatch(
    step: &PlanStep,
    mark_id: &str,
    frame: &RecordedFrame,
    evaluation: &TargetEvaluation,
) -> LabError {
    with_details(
        blocked(
            "record_step_self_mismatch",
            format!(
                "the guard of step {}, the located-text mark '{mark_id}' its click follows, does \
                 not pass on its own frame {}: {}",
                step.step.index, frame.frame_id, evaluation.message
            ),
        ),
        json!({
            "step": step.number,
            "record_index": step.step.index,
            "mark_id": mark_id,
            "frame_id": frame.frame_id,
            "failure": evaluation.failure
        }),
    )
}

/// The frames a page matches on and the frames it is undetermined on.
fn page_on_frames(
    evaluation: &Evaluation<'_>,
    cache: &FrameCache,
    required: &[String],
    forbidden: &[String],
    frames: &[&RecordedFrame],
) -> LabResult<(Vec<String>, Vec<String>)> {
    let mut matched = Vec::new();
    let mut undetermined = Vec::new();
    for frame in frames {
        match evaluation
            .page(&cached(cache, frame)?.scene, required, forbidden)?
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
        // #463 R4 fix round 2: `step` is the package's number of the optional step, `record_index`
        // its serial number, which `record mark --step` takes.
        json!({
            "step": step.number,
            "record_index": step.step.index,
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
        // #463 R4 fix round 2: `step` and `optional_step` are the package's numbers of the skip
        // target and of the optional step, `record_index` and `optional_record_index` their serial
        // numbers, which `record mark --step` takes.
        json!({
            "step": skip.number,
            "record_index": skip.step.index,
            "optional_step": optional.number,
            "optional_record_index": optional.step.index,
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
/// #463 S2-1c: each page against each screen is one judgement; one that fails is a failure
/// `failures` meets (a: the optional step, the frames its page passes on, its page; b: the skip
/// target, the optional step's frames it passes on, its page), and under `record ready` the next
/// is judged; a member whose skip target failed has no gate result.
fn optional_checks(
    evaluation: &Evaluation<'_>,
    plan: &Plan,
    cache: &FrameCache,
    failures: &mut Failures,
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
                    page_on_frames(evaluation, cache, &step.required, &step.forbidden, &frames)?;
                let other_step = &plan.steps[other];
                if !matched.is_empty() {
                    let same_click = matches!(kind, Against::OtherOptional)
                        && step.step.click.as_ref().map(|click| click.rect)
                            == other_step.step.click.as_ref().map(|click| click.rect);
                    if !same_click {
                        failures.fail(
                            Locus::on_step(step).with_frames(&matched).with_page(page),
                            optional_ambiguous(step, page, kind, other_step, &matched),
                        )?;
                        continue;
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
            let (matched, undetermined) = page_on_frames(
                evaluation,
                cache,
                &skip.required,
                &skip.forbidden,
                &step.live_frames(),
            )?;
            if !matched.is_empty() {
                failures.fail(
                    Locus::on_step(skip)
                        .with_frames(&matched)
                        .with_page(skip_page),
                    skip_target_insensitive(skip, skip_page, step, &matched),
                )?;
                continue;
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

/// #463 L2 fix 1: every step whose click follows a located-text mark whose names were changed
/// after the click was executed, with the followed mark and the recorded press; fix 2: with the
/// live frames its new names are not found on (`without_names`, by step), when there are any.
fn names_changed_after_press(
    plan: &Plan,
    without_names: &BTreeMap<u32, Vec<String>>,
) -> Vec<Value> {
    plan.steps
        .iter()
        .filter_map(|step| {
            let click = step.step.click.as_ref()?;
            let follow = click
                .follow
                .as_ref()
                .filter(|follow| follow.names_changed_after_press)?;
            let mut item = json!({
                "step": step.number,
                "record_index": step.step.index,
                "mark_id": follow.mark_id,
                "recorded_point": click.execution.as_ref().map(|execution| execution.point)
            });
            if let Some(frames) = without_names.get(&step.number) {
                item["frames_without_names"] = json!(frames);
            }
            Some(item)
        })
        .collect()
}

/// Section 4.6 item 4 (self and arrival gates) and a15 item 5 (entry overlay). #463 S2-1c: each
/// page on each of its frames, each guard on each frame and each optional page against each screen
/// is one judgement; one that fails is a failure `failures` meets (the step, the frame, the page or
/// the guard's mark), and under `record ready` the next is judged. The arrival gates and the entry
/// overlay only warn: once a failure is collected they are not evaluated, and nothing is answered.
pub(crate) fn cross_check(
    plan: &Plan,
    evaluator: &RecognitionEvaluator,
    cache: &FrameCache,
    ocr: &LabOcr,
    failures: &mut Failures,
) -> LabResult<CrossCheck> {
    let evaluation = Evaluation {
        plan,
        evaluator,
        ocr_available: ocr.ocr_provider().is_some(),
    };
    let start = failures.count();
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
                step.forbidden.clone(),
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
                Vec::new(),
                step.transition_live_frames(),
                true,
            ));
        }
        for (page, required, forbidden, frames, transition) in pages {
            let mut results = Vec::new();
            for frame in frames {
                let result =
                    evaluation.page(&cached(cache, frame)?.scene, &required, &forbidden)?;
                if result.verdict == PageVerdict::NoMatch {
                    failures.fail(
                        Locus::on_step(step).with_frame(frame).with_page(&page),
                        self_mismatch(step, &page, frame, &result),
                    )?;
                    continue;
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

    // #463 L2 fix 2: the guard of every click that follows a located-text mark, its names and no
    // mark of the page, on the step's own live frames as the pages above. A frame it does not pass
    // on is the step's mismatch, except on a step whose names were changed after its press: its
    // frames show the demonstration, not the new value, so such a frame is listed with the step.
    let mut located_guards = Vec::new();
    let mut guards_not_evaluated = Vec::new();
    let mut without_names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for step in &plan.steps {
        let Some(follow) = step
            .step
            .click
            .as_ref()
            .and_then(|click| click.follow.as_ref())
        else {
            continue;
        };
        if !follows_located_text(plan, step)? {
            continue;
        }
        let mut results = Vec::new();
        let mut undetermined = false;
        for frame in step.live_frames() {
            let scene = &cached(cache, frame)?.scene;
            let result = match evaluation.located_guard(scene, &follow.mark_id)? {
                None => {
                    undetermined = true;
                    "undetermined"
                }
                Some(found) if found.passed => "found",
                Some(_) if follow.names_changed_after_press => {
                    without_names
                        .entry(step.number)
                        .or_default()
                        .push(frame.frame_id.clone());
                    "not_found"
                }
                Some(refused) => {
                    failures.fail(
                        Locus::on_step(step)
                            .with_frame(frame)
                            .with_target(&follow.mark_id),
                        guard_mismatch(step, &follow.mark_id, frame, &refused),
                    )?;
                    continue;
                }
            };
            results.push(json!({"frame_id": frame.frame_id, "result": result}));
        }
        if undetermined {
            partial = true;
            guards_not_evaluated.push(step.number);
        }
        located_guards.push(json!({
            "step": step.number,
            "record_index": step.step.index,
            "mark_id": follow.mark_id,
            "frames": results
        }));
    }

    // Workflow #339 section 4.7: the optional steps, refusals before any warning.
    let optional = optional_checks(&evaluation, plan, cache, failures)?;
    partial |= optional.partial;
    // #463 S2-1c: what follows only warns, and a failure leaves nothing that reads it.
    if failures.count() > start {
        return Ok(CrossCheck::default());
    }

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
        let (gate, required, gate_forbidden) = match run_after.or(member) {
            Some(run) => {
                let skip = &plan.steps[run.skip];
                (
                    skip.page_id()?.to_string(),
                    skip.required.clone(),
                    skip.forbidden.clone(),
                )
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
                .page(&cached(cache, frame)?.scene, &required, &gate_forbidden)?
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
                match evaluation
                    .page(&scene, &first.required, &first.forbidden)?
                    .verdict
                {
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
    // #463 L2 fix 1: a step whose located-text names were changed after its press is judged by
    // the box the new names give on its frames, never by the recorded press, which the package
    // does not repeat; the result names each such step.
    let renamed = names_changed_after_press(plan, &without_names);
    if !renamed.is_empty() {
        cross_check.insert("names_changed_after_press".to_string(), json!(renamed));
    }
    // #463 L2 fix 2: the guards on located text, and the steps whose guard was not evaluated
    // (no OCR model), present only when a step has such a guard.
    if !located_guards.is_empty() {
        cross_check.insert("located_text_guards".to_string(), json!(located_guards));
    }
    if !guards_not_evaluated.is_empty() {
        cross_check.insert(
            "guards_not_evaluated".to_string(),
            json!(guards_not_evaluated),
        );
    }
    Ok(CrossCheck {
        warnings,
        arrival_by_time_window,
        cross_check: Value::Object(cross_check),
        entry_overlay,
        same_as: optional.same_as,
    })
}

/// #463 R2: the warning that the usual ending's page passes on an ending's frames.
pub(crate) const ENDING_USUAL_INSENSITIVE: &str = "ending_usual_insensitive";
/// #463 R2: the warning that a check of item 9 could not be evaluated on some frames: a mark it
/// needs reads OCR and the Lab has no OCR model (fix 1: an ending page that rests on OCR alone is
/// refused instead, `ocr_alone_unchecked`).
pub(crate) const ENDING_NOT_EVALUATED: &str = "ending_ambiguity_not_evaluated";

const ENDING_USUAL_MESSAGE: &str = "The usual ending's page passes on this ending's frames too. The \
    run evaluates the endings before the usual ending, so it ends on this ending there, as it \
    should; but on a run whose screen shows the usual ending under this ending's message, the two \
    are told apart by this ending's marks alone. Give the usual ending's page a mark that does not \
    hold on this ending's screen if the two can look alike.";
const ENDING_NOT_EVALUATED_MESSAGE: &str = "Not evaluated: a mark this check needs reads OCR and \
    the Lab has no OCR model, so whether the page the check names passes on these frames is \
    unknown. Check again with record stop --dry-run where the Lab has its OCR model, or give that \
    page a mark that is not OCR and does not hold on these frames, such as a template of the \
    ending's message.";

/// #463 R2: what self-check item 9 leaves when it refuses nothing: one entry per ending (its page
/// on its own frames and the usual ending's page on them), its warnings, and whether a check was
/// not evaluated.
pub(crate) struct EndingChecks {
    pub(crate) endings: Vec<Value>,
    pub(crate) warnings: Vec<Value>,
    pub(crate) partial: bool,
}

/// #463 R2: a recorded screen an ending's page is checked against: which (`against`), what names
/// it in the refusal's details, and its live frames.
struct EndingScreen<'p> {
    against: EndingAgainst,
    names: Value,
    frames: Vec<&'p RecordedFrame>,
}

/// #463 R2: an ending's page does not match one of its own frames: `record_step_self_mismatch`,
/// as for a step's page.
fn ending_self_mismatch(
    ending: &PlanEnding,
    frame: &RecordedFrame,
    result: &PageResult,
) -> LabError {
    with_details(
        blocked(
            "record_step_self_mismatch",
            format!(
                "page {} of the ending '{}' does not match its own frame {}: {} fail on it",
                ending.page,
                ending.ending.outcome_key,
                frame.frame_id,
                result.failed.join(", ")
            ),
        ),
        json!({
            "outcome_key": ending.ending.outcome_key,
            "page": ending.page,
            "frame_id": frame.frame_id,
            "failed": result.failed
        }),
    )
}

/// #463 R2: what names the recorded screen `against` in a message, and what would happen if the
/// ending's page passed on it.
const fn ending_screen_words(against: EndingAgainst) -> (&'static str, &'static str) {
    match against {
        EndingAgainst::Previous => (
            "a step after whose press the run awaits the endings (its page, or its page \
             transition's frames when it has one)",
            "the run would end on the ending before the result of that press showed",
        ),
        EndingAgainst::Usual => (
            "the last step, the usual ending",
            "the run, which evaluates the endings first, would end on the ending wherever the usual \
             ending shows",
        ),
        EndingAgainst::OtherEnding => (
            "another ending",
            "the run could not tell the two endings apart",
        ),
    }
}

/// #463 R2 fix 1: `record_ending_invalid` with `ocr_alone_unchecked`: the ending's page rests on
/// OCR marks alone and, without an OCR model, item 9 could not check it on `frames` of `screen`.
fn ending_unchecked(ending: &PlanEnding, screen: &EndingScreen<'_>, frames: &[String]) -> LabError {
    let key = ending.ending.outcome_key.as_str();
    let (what, consequence) = ending_screen_words(screen.against);
    let mut extra = json!({
        "page": ending.page,
        "against": screen.against.as_str(),
        "frames": frames
    });
    if let (Some(object), Some(names)) = (extra.as_object_mut(), screen.names.as_object()) {
        for (name, value) in names {
            object.insert(name.clone(), value.clone());
        }
    }
    ending_invalid(
        EndingProblem::OcrAloneUnchecked,
        Some(key),
        format!(
            "page {} of the ending '{key}' rests on OCR marks alone and the Lab has no OCR \
             model, so whether it passes on frames {} of {what} could not be checked; if it \
             did, {consequence}, a failure taken for a success. Run record stop where the Lab \
             has its OCR model, or give up the ending with record mark --remove-ending {key} \
             and take it again with a mark that is not OCR, such as a template of its message; \
             nothing was written",
            ending.page,
            frames.join(", ")
        ),
        extra,
    )
}

/// #463 R2: `record_ending_ambiguous`: the ending's page passes on `frames` of `screen`.
fn ending_ambiguous(ending: &PlanEnding, screen: &EndingScreen<'_>, frames: &[String]) -> LabError {
    let (what, consequence) = ending_screen_words(screen.against);
    let mut details = json!({
        "outcome_key": ending.ending.outcome_key,
        "page": ending.page,
        "against": screen.against.as_str(),
        "frames": frames,
        "outcome": Outcome::new(LabCode::RecordEndingAmbiguous)
            .against(screen.against)
            .envelope(Detail::Full)
    });
    if let (Some(object), Some(names)) = (details.as_object_mut(), screen.names.as_object()) {
        for (key, value) in names {
            object.insert(key.clone(), value.clone());
        }
    }
    with_details(
        blocked(
            LabCode::RecordEndingAmbiguous.as_str(),
            format!(
                "page {} of the ending '{}' passes on frames {} of {what}: {consequence}. Give up the \
                 ending with record mark --remove-ending {} and take it again with a mark that \
                 holds on its screen only, such as a template of its message, and check with \
                 record stop --dry-run",
                ending.page,
                ending.ending.outcome_key,
                frames.join(", "),
                ending.ending.outcome_key
            ),
        ),
        details,
    )
}

/// #463 R2, self-check item 9 (the coordinator's ruling 4 on the design of board #463: the run
/// evaluates the endings before the usual ending): on live frames with the evaluator of item 4,
/// every ending's page matches its own frames (`record_step_self_mismatch` otherwise) and passes
/// on no frame of a screen the run may show instead of it (`record_ending_ambiguous`, in this
/// order): `previous`, each step after whose press the run awaits the endings (its page
/// transition's frames when it has one, the screen left after its press); `usual`, the last step;
/// `other_ending`, every other ending. Fix 1: an ending page that rests on OCR marks alone, which
/// no OCR model checked against such a screen, is `record_ending_invalid` with
/// `ocr_alone_unchecked`. Every refusal comes before every warning. The usual ending's page passing
/// on an ending's frames is the warning `ending_usual_insensitive`; any other undetermined
/// evaluation (OCR without a model) the warning `ending_ambiguity_not_evaluated`, and the
/// cross-check is then `partially_evaluated`. #463 S2-1c: each ending's page on each of its own
/// frames and against each screen is one judgement; one that fails is a failure `failures` meets
/// (the ending's outcome key and page, the frames), and under `record ready` the next is judged.
pub(crate) fn ending_checks(
    plan: &Plan,
    evaluator: &RecognitionEvaluator,
    cache: &FrameCache,
    ocr: &LabOcr,
    failures: &mut Failures,
) -> LabResult<EndingChecks> {
    let mut checks = EndingChecks {
        endings: Vec::new(),
        warnings: Vec::new(),
        partial: false,
    };
    if plan.endings.is_empty() {
        return Ok(checks);
    }
    let evaluation = Evaluation {
        plan,
        evaluator,
        ocr_available: ocr.ocr_provider().is_some(),
    };
    let last = plan
        .steps
        .last()
        .ok_or_else(|| invalid("validation_failed", "the package has no step"))?;
    let usual_page = last.page_id()?;
    let awaiting = plan.ending_awaiting_steps();
    for ending in &plan.endings {
        let key = ending.ending.outcome_key.as_str();
        let ocr_alone = !evaluation.ocr_available && plan.rests_on_ocr(&ending.required)?;
        let own_frames = ending.live_frames();
        let mut own = Vec::new();
        for frame in &own_frames {
            let result = evaluation.page(&cached(cache, frame)?.scene, &ending.required, &[])?;
            if result.verdict == PageVerdict::NoMatch {
                failures.fail(
                    Locus::on_ending(ending).with_frame(frame),
                    ending_self_mismatch(ending, frame, &result),
                )?;
                continue;
            }
            checks.partial |= result.verdict == PageVerdict::Undetermined;
            own.push(json!({"frame_id": frame.frame_id, "result": result.verdict.as_str()}));
        }
        let mut screens = Vec::new();
        for step in &awaiting {
            screens.push(EndingScreen {
                against: EndingAgainst::Previous,
                names: json!({"step": step.number}),
                frames: if step.transition_page.is_some() {
                    step.transition_live_frames()
                } else {
                    step.live_frames()
                },
            });
        }
        screens.push(EndingScreen {
            against: EndingAgainst::Usual,
            names: json!({"step": last.number}),
            frames: last.live_frames(),
        });
        for other in plan
            .endings
            .iter()
            .filter(|other| other.number != ending.number)
        {
            screens.push(EndingScreen {
                against: EndingAgainst::OtherEnding,
                names: json!({"other_outcome_key": other.ending.outcome_key}),
                frames: other.live_frames(),
            });
        }
        for screen in &screens {
            // An application entry step has no frame of its own.
            if screen.frames.is_empty() {
                continue;
            }
            let (matched, undetermined) =
                page_on_frames(&evaluation, cache, &ending.required, &[], &screen.frames)?;
            if !matched.is_empty() {
                failures.fail(
                    Locus::on_ending(ending).with_frames(&matched),
                    ending_ambiguous(ending, screen, &matched),
                )?;
                continue;
            }
            // Fix 1: an ending only OCR marks tell apart, which no model checked, could turn a
            // failure into a silent success: refused; one with a mark of another family is the
            // warning below.
            if !undetermined.is_empty() && ocr_alone {
                failures.fail(
                    Locus::on_ending(ending).with_frames(&undetermined),
                    ending_unchecked(ending, screen, &undetermined),
                )?;
                continue;
            }
            if !undetermined.is_empty() {
                checks.partial = true;
                let mut warning = json!({
                    "code": ENDING_NOT_EVALUATED,
                    "check": LabCode::RecordEndingAmbiguous.as_str(),
                    "outcome_key": key,
                    "page": ending.page,
                    "against": screen.against.as_str(),
                    "frames": undetermined,
                    "message": ENDING_NOT_EVALUATED_MESSAGE
                });
                if let (Some(object), Some(names)) =
                    (warning.as_object_mut(), screen.names.as_object())
                {
                    for (name, value) in names {
                        object.insert(name.clone(), value.clone());
                    }
                }
                checks.warnings.push(warning);
            }
        }
        let (matched, undetermined) = page_on_frames(
            &evaluation,
            cache,
            &last.required,
            &last.forbidden,
            &own_frames,
        )?;
        let usual = if !matched.is_empty() {
            checks.warnings.push(json!({
                "code": ENDING_USUAL_INSENSITIVE,
                "outcome_key": key,
                "page": ending.page,
                "usual_page": usual_page,
                "frames": matched,
                "message": ENDING_USUAL_MESSAGE
            }));
            "passes"
        } else if !undetermined.is_empty() {
            checks.partial = true;
            checks.warnings.push(json!({
                "code": ENDING_NOT_EVALUATED,
                "check": ENDING_USUAL_INSENSITIVE,
                "outcome_key": key,
                "page": ending.page,
                "usual_page": usual_page,
                "frames": undetermined,
                "message": ENDING_NOT_EVALUATED_MESSAGE
            }));
            "not_evaluated"
        } else {
            "distinct"
        };
        checks.endings.push(json!({
            "ending": ending.number,
            "outcome_key": key,
            "page": ending.page,
            "frames": own,
            "usual": usual
        }));
    }
    Ok(checks)
}
