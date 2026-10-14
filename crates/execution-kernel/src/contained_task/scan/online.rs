// SPDX-License-Identifier: AGPL-3.0-only

//! The sources a scan session reads (Workflow #308 slice SEL-L3): a select step's runtime, and
//! a saved frame sequence replayed through the same path (`dry_run_scan`).
//!
//! Every capture goes through the kernel's capture path without the OCR collector, so it is
//! recorded as every capture is and feeds neither post-admission OCR nor stability sampling
//! (S0 §2.3). Every capture must be the step's page with the dispatched frame's geometry. A
//! viewport is the step's guard on the latest frame, the layout's projection and the declared
//! start and end checks. A drag is a `task.effect_intent` of the select step, sampled between
//! random points of its two rectangles with the step's action seed and the drag's index, bound
//! to the latest frame, and then the input; it records no `task.effect_completed`.

use super::{
    ScanDeclaration, ScanDrag, ScanDragMotion, ScanDryRun, ScanReadInstance, ScanReading,
    ScanSession, ScanSource,
};
use crate::codes::{ExecutionKernelCode, ExecutionKernelLocation};
use crate::contained_task::{
    ContainedTaskBoundaryTiming, ContainedTaskError, ContainedTaskGuardOutcome,
    ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext, ContainedTaskTrace,
    PageObservation, PreparedContainedTask, TaskOperation, sample_geometry_matches, sampled_swipe,
};
use crate::{InputFrameContext, ObservedFrame};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    CandidateRect, InputAction, TaskTimingBoundary, TaskTimingBudgetOrigin,
};
use actingcommand_device::Frame;
use actingcommand_recognition::color_digest::{ColorDigest, ColorDigestGrid};
use actingcommand_recognition::{Rect, Scene};
use actingcommand_recognition_pack::{
    CandidateLayout, RecognitionPackCode, RecognitionPackLocation, TargetSearch,
};
use actingcommand_selection_policy::SelectionFactSnapshot;
use std::time::{Duration, Instant};

const SELECT_INVALID: ExecutionKernelCode = ExecutionKernelCode::ContainedTaskSelectInvalid;

/// The action seed of one drag of an attempt: the step's seed mixed with the drag's index
/// (SplitMix64), so every drag samples its own points and a replay samples the same ones.
fn drag_seed(seed: u64, index: u32) -> u64 {
    let mut value = seed ^ (u64::from(index) + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// A select step's runtime as a scan source. `replay` marks a saved frame sequence: it holds
/// only the frames of forward advances, so a reverse move cannot be replayed, and it does not
/// wait between captures.
pub(in crate::contained_task) struct RuntimeScanSource<'s, R: ContainedTaskRuntime> {
    task: &'s PreparedContainedTask,
    runtime: &'s mut R,
    operation: &'s TaskOperation,
    scan: &'s ScanDeclaration,
    layout: &'s CandidateLayout,
    step_index: u32,
    timing: ContainedTaskTimingContext,
    started: Instant,
    /// The scan's deadline on the observation clock: candidate sampling stops at it.
    deadline: Instant,
    /// The step's dispatched frame F1.
    first: &'s PageObservation,
    latest: Option<PageObservation>,
    /// The guard outcome of the latest viewport, which every drag from it carries.
    guard: Option<ContainedTaskGuardOutcome>,
    action_seed: Option<u64>,
    replay: bool,
}

impl<'s, R: ContainedTaskRuntime> RuntimeScanSource<'s, R> {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::contained_task) fn new(
        task: &'s PreparedContainedTask,
        runtime: &'s mut R,
        operation: &'s TaskOperation,
        scan: &'s ScanDeclaration,
        layout: &'s CandidateLayout,
        first: &'s PageObservation,
        (step_index, timing, deadline, action_seed): (
            u32,
            ContainedTaskTimingContext,
            Instant,
            Option<u64>,
        ),
        replay: bool,
    ) -> Self {
        let started = runtime.observation_now();
        Self {
            task,
            runtime,
            operation,
            scan,
            layout,
            step_index,
            timing,
            started,
            deadline,
            first,
            latest: None,
            guard: None,
            action_seed,
            replay,
        }
    }

    /// The instance rectangles of a scene: every accepted anchor match moved by the layout's
    /// `instance_rect` (anchor matching only, no features).
    fn anchors(&self, scene: &Scene) -> Result<Vec<CandidateRect>, ContainedTaskError> {
        let invalid = || {
            ContainedTaskError::with_detail(
                SELECT_INVALID.code(),
                format!("layout={} scan=not_repeated_anchor", self.layout.id),
            )
        };
        let (Some(anchor), Some(instance_rect)) =
            (self.layout.anchor.as_deref(), self.layout.instance_rect)
        else {
            return Err(invalid());
        };
        let search = self
            .task
            .evaluator
            .scene_context(scene)
            .evaluate_target_all(anchor, self.layout.suppress_iou_milli.unwrap_or(0))
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    SharedCode::ContainedTaskRecognitionFailed.code(),
                    error.to_string(),
                )
            })?;
        match search {
            TargetSearch::Complete(matches) => matches
                .iter()
                .map(|matched| {
                    match (
                        matched.x.checked_add(instance_rect.x),
                        matched.y.checked_add(instance_rect.y),
                    ) {
                        (Some(x), Some(y)) => Ok(CandidateRect {
                            x,
                            y,
                            width: instance_rect.width,
                            height: instance_rect.height,
                        }),
                        _ => Err(invalid()),
                    }
                })
                .collect(),
            TargetSearch::Incomplete { found, timeout_ms } => {
                let count = u64::try_from(found).unwrap_or(u64::MAX);
                Err(ContainedTaskError::valued(
                    RecognitionPackCode::CandidateSearchIncomplete.code(),
                    |outcome| {
                        outcome
                            .stage(RecognitionPackLocation::CandidateSearchRepeatedAnchor)
                            .timeout_ms(timeout_ms)
                            .count(count)
                    },
                ))
            }
        }
    }

    /// The signature of the instance at `rect` (S0 §2.1): the color digest of the declared
    /// rectangle relative to the anchor match; `None` when it does not lie in the frame.
    fn signature(&self, scene: &Scene, rect: CandidateRect) -> Option<ColorDigest> {
        let instance_rect = self.layout.instance_rect?;
        let signature = self.scan.track.signature;
        let x = rect
            .x
            .checked_sub(instance_rect.x)?
            .checked_add(signature.rect.x)?;
        let y = rect
            .y
            .checked_sub(instance_rect.y)?
            .checked_add(signature.rect.y)?;
        let grid = ColorDigestGrid::new(signature.columns, signature.rows).ok()?;
        ColorDigest::compute(
            scene,
            Rect {
                x,
                y,
                width: signature.rect.width,
                height: signature.rect.height,
            },
            grid,
        )
        .ok()
    }

    /// Whether the declared check target passes on `scene`.
    fn check(&self, scene: &Scene, target_id: &str) -> Result<bool, ContainedTaskError> {
        self.task
            .evaluator
            .evaluate_target(scene, target_id)
            .map(|evaluation| evaluation.passed)
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    SharedCode::ContainedTaskRecognitionFailed.code(),
                    error.to_string(),
                )
            })
    }
}

impl<R: ContainedTaskRuntime> ScanSource for RuntimeScanSource<'_, R> {
    type Error = R::Error;

    fn elapsed(&mut self) -> Duration {
        self.runtime
            .observation_now()
            .saturating_duration_since(self.started)
    }

    fn dispatched(&mut self) -> Result<Vec<CandidateRect>, ContainedTaskRunError<R::Error>> {
        Ok(self.anchors(&self.first.scene)?)
    }

    fn capture(
        &mut self,
        interval: Duration,
    ) -> Result<Vec<CandidateRect>, ContainedTaskRunError<R::Error>> {
        if !self.replay {
            let boundary = TaskTimingBoundary::PageRecognitionWait;
            let identity = self.runtime.task_boundary_identity(boundary);
            let started = Instant::now();
            // The wait never runs past the scan's deadline; the session fails at it.
            let left = self
                .deadline
                .saturating_duration_since(self.runtime.observation_now());
            std::thread::sleep(interval.min(left));
            let ended = Instant::now();
            self.runtime
                .observe_task_boundary(ContainedTaskBoundaryTiming {
                    boundary,
                    identity,
                    context: self.timing,
                    started,
                    ended,
                    succeeded: true,
                });
        }
        let observation = match self.task.capture_frame(
            &mut *self.runtime,
            None,
            None,
            self.timing,
            None,
            None,
        )? {
            Some(observation)
                if crate::page_anchor_matches(
                    &self.task.control.game,
                    &observation.page_label,
                    &self.operation.from,
                ) =>
            {
                observation
            }
            other => {
                let observed =
                    other.map_or_else(|| "<unrecognized>".to_owned(), |frame| frame.page_label);
                return Err(ContainedTaskError::with_detail(
                    ExecutionKernelCode::SelectionPageChanged.code(),
                    format!(
                        "operation={} from={} observed_page={observed}",
                        self.operation.id, self.operation.from
                    ),
                )
                .into());
            }
        };
        if observation.scene.width() != self.first.scene.width()
            || observation.scene.height() != self.first.scene.height()
            || !sample_geometry_matches(&self.first.input_context, &observation.input_context)
        {
            return Err(ContainedTaskError::new(
                ExecutionKernelCode::CandidateSamplePageOrGeometryChanged.code(),
            )
            .into());
        }
        let anchors = self.anchors(&observation.scene)?;
        self.latest = Some(observation);
        Ok(anchors)
    }

    fn viewport(&mut self) -> Result<ScanReading, ContainedTaskRunError<R::Error>> {
        let observation = self.latest.as_ref().unwrap_or(self.first);
        let (guard, _) = self.operation.guard_outcome(
            &self.task.control,
            observation,
            &self.task.evaluator,
            &mut *self.runtime,
            Some(self.timing),
        )?;
        let (projection, _) = self.task.project_transaction(
            &mut *self.runtime,
            self.operation,
            observation,
            self.layout,
            self.timing,
            self.deadline,
        )?;
        let start_check = self.check(&observation.scene, &self.scan.start.check)?;
        let end_check = self.check(&observation.scene, &self.scan.end.check)?;
        let instances = projection
            .candidates()
            .iter()
            .map(|candidate| ScanReadInstance {
                rect: candidate.rect,
                signature: self.signature(&observation.scene, candidate.rect),
                actionable: candidate.actionable,
                features: candidate.features.clone(),
            })
            .collect();
        let frame_id = observation
            .input_context
            .as_ref()
            .map(|context| context.reference().frame_id);
        self.guard = Some(guard);
        Ok(ScanReading {
            instances,
            start_check,
            end_check,
            frame_id,
        })
    }

    fn drag(&mut self, drag: ScanDrag) -> Result<(), ContainedTaskRunError<R::Error>> {
        if self.replay && drag.motion != ScanDragMotion::Advance {
            let stage = match drag.motion {
                ScanDragMotion::Rewind => ExecutionKernelLocation::SelectScanStart,
                _ => ExecutionKernelLocation::SelectScanAlign,
            };
            return Err(ContainedTaskError::valued(
                ExecutionKernelCode::SelectionScanReplayFrameMissing.code(),
                |outcome| outcome.stage(stage),
            )
            .into());
        }
        let resolution = &self.task.control.resolution;
        drag.from.validate(resolution)?;
        drag.to.validate(resolution)?;
        let seed = self.action_seed.map(|seed| drag_seed(seed, drag.index));
        let (action, sampling) = sampled_swipe(drag.from, drag.to, drag.duration_ms, seed)?;
        let guard = self.guard.clone().ok_or_else(|| {
            ContainedTaskError::with_detail(
                SELECT_INVALID.code(),
                format!(
                    "operation={} scan drag before a viewport",
                    self.operation.id
                ),
            )
        })?;
        let input_context = self
            .latest
            .as_ref()
            .unwrap_or(self.first)
            .input_context
            .clone();
        self.runtime
            .record(ContainedTaskTrace::EffectIntent {
                step_index: self.step_index,
                operation_label: self.operation.id.clone(),
                action: action.clone(),
                sampling,
                guard,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        self.runtime
            .input(action, input_context)
            .map_err(ContainedTaskRunError::operation::<R>)
    }
}

/// A saved frame sequence as a runtime: every capture takes the next frame, every input and
/// record is accepted, and a capture past the last frame fails
/// `selection_scan_replay_frame_missing`.
struct ReplayRuntime {
    frames: std::vec::IntoIter<Frame>,
}

impl ContainedTaskRuntime for ReplayRuntime {
    type Error = ContainedTaskError;

    fn capture(&mut self) -> Result<ObservedFrame, ContainedTaskError> {
        self.frames.next().map(ObservedFrame::from).ok_or_else(|| {
            ContainedTaskError::new(ExecutionKernelCode::SelectionScanReplayFrameMissing.code())
        })
    }

    fn input(
        &mut self,
        _action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), ContainedTaskError> {
        Ok(())
    }

    fn record(&mut self, _trace: ContainedTaskTrace) -> Result<(), ContainedTaskError> {
        Ok(())
    }
}

/// The task error of a replayed scan: the runtime's own errors are the replay's.
fn replay_error(error: ContainedTaskRunError<ContainedTaskError>) -> ContainedTaskError {
    match error {
        ContainedTaskRunError::Task(error)
        | ContainedTaskRunError::Boundary(error)
        | ContainedTaskRunError::NonfatalOperation(error) => error,
    }
}

/// One scan select evaluated offline over a saved frame sequence (Workflow #308 SEL-L3, S0 §1.1
/// and §6 row L3): the frames are the step's dispatched frame and then every capture the scan
/// takes, in order, as a forward run records them. The session runs as online up to `Decide`'s
/// coverage checks, with `facts` at `now_unix_ms` and the step's sealed policy. A scan that
/// needs a frame the sequence does not hold (a corrective or a rewind, which move backward, or
/// a capture past the last frame) fails `selection_scan_replay_frame_missing`; it never passes.
pub fn dry_run_scan(
    task: &PreparedContainedTask,
    operation_id: &str,
    scan: &ScanDeclaration,
    frames: Vec<Frame>,
    facts: &SelectionFactSnapshot,
    now_unix_ms: u64,
) -> Result<ScanDryRun, ContainedTaskError> {
    let mut runtime = ReplayRuntime {
        frames: frames.into_iter(),
    };
    run_scan(
        task,
        &mut runtime,
        (operation_id, scan),
        (facts, now_unix_ms),
        true,
    )
    .map_err(replay_error)
}

/// One scan session of the select step `operation_id` over `runtime`, whose next capture is the
/// step's dispatched frame: the path `dry_run_scan` replays (`replay`) and a runtime drives
/// online. The scan's time is its `max_ms`, from the runtime's clock.
pub(in crate::contained_task) fn run_scan<R: ContainedTaskRuntime>(
    task: &PreparedContainedTask,
    runtime: &mut R,
    (operation_id, scan): (&str, &ScanDeclaration),
    (facts, now_unix_ms): (&SelectionFactSnapshot, u64),
    replay: bool,
) -> Result<ScanDryRun, ContainedTaskRunError<R::Error>> {
    let operation = task
        .program
        .operations
        .iter()
        .find(|operation| operation.id == operation_id)
        .ok_or_else(|| {
            ContainedTaskError::with_detail(
                ExecutionKernelCode::ContainedTaskOperationMissing.code(),
                operation_id,
            )
        })?;
    let prepared = operation.prepared_select.as_deref().ok_or_else(|| {
        ContainedTaskError::with_detail(
            SELECT_INVALID.code(),
            format!("operation={operation_id} is not a select step"),
        )
    })?;
    let layout = task
        .evaluator
        .candidate_layout(prepared.layout_id())
        .ok_or_else(|| ContainedTaskError::new(SELECT_INVALID.code()))?;
    let started = runtime.observation_now();
    let budget = Duration::from_millis(scan.bounds.max_ms);
    let deadline = started + budget;
    let timing = ContainedTaskTimingContext::new(started, deadline, TaskTimingBudgetOrigin::Task);
    let first = match task.capture_frame(&mut *runtime, None, None, timing, None, None)? {
        Some(first)
            if crate::page_anchor_matches(
                &task.control.game,
                &first.page_label,
                &operation.from,
            ) =>
        {
            first
        }
        other => {
            let observed =
                other.map_or_else(|| "<unrecognized>".to_owned(), |frame| frame.page_label);
            return Err(ContainedTaskError::with_detail(
                ExecutionKernelCode::SelectPageMismatch.code(),
                format!(
                    "operation={operation_id} from={} matched_pages={observed}",
                    operation.from
                ),
            )
            .into());
        }
    };
    let frame = (first.scene.width(), first.scene.height());
    let mut session = ScanSession::new(
        scan,
        layout,
        prepared.policy(),
        facts,
        now_unix_ms,
        frame,
        budget,
    )?;
    let mut source = RuntimeScanSource::new(
        task,
        runtime,
        operation,
        scan,
        layout,
        &first,
        (0, timing, deadline, None),
        replay,
    );
    let end = session.run(&mut source)?;
    Ok(session.summary(Some(&end)))
}
