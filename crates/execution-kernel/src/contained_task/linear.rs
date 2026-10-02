// SPDX-License-Identifier: AGPL-3.0-only

//! The `linear_steps` execution mode (Workflow #336; `contracts/linear-steps.md`).
//!
//! A linear task is its recorded path: operation `k` leaves the page of step `k` and must
//! reach the page of step `k + 1`. Every wait evaluates only the page or pages it waits for,
//! as the detector page ids admission resolved; no page is recognized globally and no run
//! state machine is built. An operation may declare an intermediate state after its input: a
//! recognizable page that must be seen before the next step is trusted, or a time window in
//! which nothing is evaluated before its lower bound. A declared retry only repeats an input
//! the screen shows was swallowed.

use super::{
    ContainedTaskBoundaryTiming, ContainedTaskError, ContainedTaskEvaluationTiming,
    ContainedTaskOutcome, ContainedTaskRunError, ContainedTaskRuntime, ContainedTaskTimingContext,
    ContainedTaskTrace, DEFAULT_TASK_TIMEOUT_MS, MAX_CAPTURE_INTERVAL_MS, MAX_STEPS,
    MAX_TASK_TIMEOUT_MS, PageObservation, PostAdmissionOcrCollector, PreparedContainedTask,
    TaskControl, TaskOperation, TaskProgram, observe_instant_span, recognized_page_targets,
    resolve_page_reference, scene_from_frame,
};
use crate::RunOperationPolicy;
use actingcommand_contract::{
    PHASED_CONTROL_SCHEMA, TaskTimingBoundary, TaskTimingCheckPosition, TaskTimingFailure,
    TaskTimingResult, TaskTimingScope, TaskTimingStage,
};
use actingcommand_page_detector::{PageDetector, PageDetectorError, require_all_page_evaluations};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::thread;
use std::time::{Duration, Instant};

/// The control `execution_mode` of a linear task.
pub(super) const LINEAR_STEPS: &str = "linear_steps";
const LINEAR_INVALID: &str = "contained_task_linear_invalid";
const ENTRY_UNMATCHED: &str = "contained_task_linear_entry_unmatched";
const INTERMEDIATE_UNOBSERVED: &str = "contained_task_linear_intermediate_unobserved";
const PAGE_CONFIRMATION_FAILED: &str = "page_confirmation_failed";
const UNRECOGNIZED_PAGE: &str = "<unrecognized>";

/// An operation's declared intermediate state, as the package writes it (Workflow #336 R13).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TaskTransition {
    /// A recognizable page that must be seen after the input, before the next step's page.
    Page {
        page_id: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        interval_ms: Option<u64>,
    },
    /// Nothing is evaluated before `min_ms`; the next step's page may take `max_ms` plus its
    /// own arrival budget.
    Window { min_ms: u64, max_ms: u64 },
}

/// The admitted path; every page is the detector page id admission resolved.
pub(super) struct LinearPlan {
    target: String,
    steps: Vec<LinearStep>,
}

/// One operation of the path: its own page, the next step's page and its intermediate state.
struct LinearStep {
    from: String,
    to: String,
    transition: Option<LinearTransition>,
}

enum LinearTransition {
    Page {
        page: String,
        timeout: Duration,
        interval: Duration,
    },
    Window {
        min: Duration,
        max: Duration,
    },
}

impl LinearStep {
    /// The first page the runtime waits for after this step's input.
    fn gate(&self) -> &str {
        match &self.transition {
            Some(LinearTransition::Page { page, .. }) => page,
            _ => &self.to,
        }
    }

    fn transition_detail(&self) -> String {
        match &self.transition {
            None => "transition=none".to_owned(),
            Some(LinearTransition::Page { .. }) => "transition=page".to_owned(),
            Some(LinearTransition::Window { min, max }) => format!(
                "transition=window min_ms={} max_ms={}",
                min.as_millis(),
                max.as_millis()
            ),
        }
    }
}

/// The run budgets a linear run reads.
#[derive(Clone, Copy)]
pub(super) struct LinearRun {
    pub(super) step_timeout: Duration,
    pub(super) capture_interval: Duration,
    pub(super) timing: ContainedTaskTimingContext,
}

/// What a wait is for: its sleep boundary and the stage of a task timeout inside it.
#[derive(Clone, Copy)]
enum LinearWaitPurpose {
    /// The first step's page, before any input.
    Entry,
    /// The intermediate page or the next step's page, after an input.
    AfterInput,
    /// The retry decision between the gate and this step's own page.
    RetryDecision,
}

impl LinearWaitPurpose {
    const fn boundary(self) -> TaskTimingBoundary {
        match self {
            Self::Entry | Self::RetryDecision => TaskTimingBoundary::PageRecognitionWait,
            Self::AfterInput => TaskTimingBoundary::PostconditionWait,
        }
    }

    const fn timeout_stage(self) -> TaskTimingStage {
        match self {
            Self::Entry => TaskTimingStage::EntryRecognition,
            Self::AfterInput => TaskTimingStage::Postcondition,
            Self::RetryDecision => TaskTimingStage::PageRecognition,
        }
    }
}

/// Why one attempt did not reach the next step's page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinearMissKind {
    /// The declared intermediate page was not seen.
    Intermediate,
    /// The next step's page did not pass, and no intermediate page was seen.
    Arrival,
    /// The intermediate page was seen, then the next step's page did not pass.
    AfterIntermediate,
}

struct LinearMiss {
    kind: LinearMissKind,
    elapsed: Duration,
    limit: Duration,
}

/// The budgets of the wait for the next step's page.
#[derive(Clone, Copy)]
struct LinearArrival {
    budget: Duration,
    interval: Duration,
    post_input_delay: Duration,
}

type LinearAttemptResult<E> = Result<Result<PageObservation, LinearMiss>, ContainedTaskRunError<E>>;

fn postcondition_timing(elapsed: Duration, limit: Duration) -> Option<TaskTimingFailure> {
    Some(TaskTimingFailure {
        scope: TaskTimingScope::Postcondition,
        stage: TaskTimingStage::Postcondition,
        elapsed_ms: elapsed.as_millis() as u64,
        limit_ms: limit.as_millis() as u64,
        required_delay_ms: None,
    })
}

fn page_confirmation_failed(
    operation: &TaskOperation,
    step: &LinearStep,
    attempt: u32,
    intermediate_seen: bool,
    elapsed: Duration,
    limit: Duration,
) -> ContainedTaskError {
    ContainedTaskError::with_detail(
        PAGE_CONFIRMATION_FAILED,
        format!(
            "operation={} attempts={attempt} after_page={UNRECOGNIZED_PAGE} hit_error_page=false {}{}",
            operation.id,
            step.transition_detail(),
            if intermediate_seen {
                " intermediate_seen=true"
            } else {
                ""
            }
        ),
    )
    .with_timing(postcondition_timing(elapsed, limit))
}

impl TaskProgram {
    /// Admission of a `linear_steps` task (§5.2): its operations form one chain from the entry
    /// page to the target page, and every page it names resolves to one detector page.
    pub(super) fn validate_linear(
        &self,
        control: &TaskControl,
        detector: &PageDetector,
    ) -> Result<LinearPlan, ContainedTaskError> {
        let invalid = |reason: &str, operation: Option<&TaskOperation>| {
            ContainedTaskError::with_detail(
                LINEAR_INVALID,
                match operation {
                    Some(operation) => format!("reason={reason} operation={}", operation.id),
                    None => format!("reason={reason}"),
                },
            )
        };
        if self.schema_version != "0.9" || control.schema_version != PHASED_CONTROL_SCHEMA {
            return Err(invalid("schema_version", None));
        }
        if self.phases.is_some() || control.phases.is_some() {
            return Err(invalid("phases", None));
        }
        let count = u32::try_from(self.operations.len())
            .ok()
            .filter(|count| (1..=MAX_STEPS).contains(count))
            .ok_or_else(|| invalid("operation_count", None))?;
        if self.max_steps != Some(count) || control.max_steps != Some(count) {
            return Err(invalid("max_steps", None));
        }
        for (reason, declared) in [
            ("error_pages", !self.error_pages.is_empty()),
            ("recovery", self.recovery.is_some()),
            (
                "stability_termination",
                self.stability_termination.is_some() || control.stability_termination.is_some(),
            ),
            ("post_admission_ocr", self.post_admission_ocr.is_some()),
            ("resource_readings", self.resource_readings.is_some()),
            (
                "stop_on_confirmation",
                control.stop_on_confirmation == Some(false),
            ),
        ] {
            if declared {
                return Err(invalid(reason, None));
            }
        }
        let resolve = |page: &str| resolve_page_reference(&control.game, page, detector);
        let entry = resolve(
            self.entry_page
                .as_deref()
                .ok_or_else(|| invalid("entry_page", None))?,
        )?;
        let target = match self.target_pages()?.as_slice() {
            [page] => resolve(page.as_str())?,
            _ => return Err(invalid("target_page", None)),
        };
        let mut operation_ids = BTreeSet::new();
        let mut steps = Vec::with_capacity(self.operations.len());
        for operation in &self.operations {
            operation.validate(control, self.defaults, &self.schema_version)?;
            if !operation_ids.insert(operation.id.as_str()) {
                return Err(ContainedTaskError::new("contained_task_program_invalid"));
            }
            if operation.click.is_none()
                || operation.application.is_some()
                || operation.select.is_some()
                || operation.on_error.is_some()
            {
                return Err(invalid("operation_effect", Some(operation)));
            }
            let from = resolve(operation.from.as_str())?;
            let to = match operation.destination_pages()?.as_slice() {
                [page] => resolve(page.as_str())?,
                _ => return Err(invalid("destination", Some(operation))),
            };
            if to == from {
                return Err(invalid("to_equals_from", Some(operation)));
            }
            let transition = match &operation.transition {
                None => None,
                Some(TaskTransition::Page {
                    page_id,
                    timeout_ms,
                    interval_ms,
                }) => {
                    let page = resolve(page_id.as_str())?;
                    if page == from || page == to {
                        return Err(invalid("transition_page", Some(operation)));
                    }
                    if timeout_ms.is_some_and(|value| value == 0 || value > MAX_TASK_TIMEOUT_MS) {
                        return Err(invalid("transition_timeout", Some(operation)));
                    }
                    if interval_ms
                        .is_some_and(|value| value == 0 || value > MAX_CAPTURE_INTERVAL_MS)
                    {
                        return Err(invalid("transition_interval", Some(operation)));
                    }
                    let arrival = operation.effective_timing(control);
                    Some(LinearTransition::Page {
                        page,
                        timeout: Duration::from_millis(
                            timeout_ms.unwrap_or(arrival.timeout.milliseconds),
                        ),
                        interval: Duration::from_millis(
                            interval_ms.unwrap_or(control.capture_interval().milliseconds),
                        ),
                    })
                }
                Some(TaskTransition::Window { min_ms, max_ms }) => {
                    if min_ms > max_ms
                        || *max_ms == 0
                        || *max_ms > MAX_TASK_TIMEOUT_MS
                        || *min_ms >= control.task_timeout().milliseconds
                    {
                        return Err(invalid("transition_window", Some(operation)));
                    }
                    Some(LinearTransition::Window {
                        min: Duration::from_millis(*min_ms),
                        max: Duration::from_millis(*max_ms),
                    })
                }
            };
            steps.push(LinearStep {
                from,
                to,
                transition,
            });
        }
        if steps[0].from != entry {
            return Err(invalid("entry_page", Some(&self.operations[0])));
        }
        for (index, pair) in steps.windows(2).enumerate() {
            if pair[0].to != pair[1].from {
                return Err(invalid("chain", Some(&self.operations[index + 1])));
            }
        }
        let last = steps.len() - 1;
        if steps[last].to != target {
            return Err(invalid("target_page", Some(&self.operations[last])));
        }
        if let Some(declaration) = &self.scheduling_outcome {
            declaration.validate().map_err(|_| {
                ContainedTaskError::new("contained_task_outcome_declaration_invalid")
            })?;
            if declaration.designated_operation().is_some() {
                return Err(invalid("designated_operation", None));
            }
            for page in declaration
                .mappings()
                .iter()
                .flat_map(|mapping| mapping.terminal_pages())
            {
                if resolve(page.as_str())? != target {
                    return Err(invalid("terminal_page", None));
                }
            }
        }
        Ok(LinearPlan { target, steps })
    }
}

impl PreparedContainedTask {
    /// Runs an admitted linear task after `PackageAdmitted` and `RunStarted` (§5.3).
    pub(super) fn run_linear_steps<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        ocr_collector: &mut PostAdmissionOcrCollector<'_>,
        plan: &LinearPlan,
        run: LinearRun,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        let first = plan
            .steps
            .first()
            .ok_or_else(|| ContainedTaskError::new("contained_task_state_invalid"))?;
        let entry_started = Instant::now();
        let mut observation = self
            .linear_wait(
                runtime,
                &[first.from.as_str()],
                run.step_timeout,
                run.capture_interval,
                LinearWaitPurpose::Entry,
                run.timing,
            )?
            .ok_or_else(|| {
                ContainedTaskError::with_detail(ENTRY_UNMATCHED, format!("page={}", first.from))
                    .with_timing(Some(TaskTimingFailure {
                        scope: TaskTimingScope::PageRecognition,
                        stage: TaskTimingStage::EntryRecognition,
                        elapsed_ms: entry_started.elapsed().as_millis() as u64,
                        limit_ms: run.step_timeout.as_millis() as u64,
                        required_delay_ms: None,
                    }))
            })?;
        for (index, (operation, step)) in
            self.program.operations.iter().zip(&plan.steps).enumerate()
        {
            let step_index = u32::try_from(index)
                .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
            observation =
                self.run_linear_step(runtime, run, step_index, operation, step, observation)?;
        }
        let executed_steps = u32::try_from(plan.steps.len())
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        self.finish_success(
            runtime,
            ocr_collector,
            Some(&observation),
            Some(plan.target.clone()),
            executed_steps,
        )
    }

    /// One operation (§5.3 a–h, §5.5), from the frame on which its own page passed to the frame
    /// on which the next step's page passed.
    fn run_linear_step<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        step: &LinearStep,
        mut observation: PageObservation,
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        // The dispatched logical step, as the page-graph path counts it; a retry adds none.
        runtime.update_run_progress(step_index.saturating_add(1));
        let policy = operation.retry_policy(
            self.program.defaults,
            self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
        )?;
        let max_attempts = policy.as_ref().map_or(1, RunOperationPolicy::max_attempts);
        let retry_interval = Duration::from_millis(
            policy
                .as_ref()
                .map_or(1, RunOperationPolicy::retry_interval_ms),
        );
        let timing = operation.effective_timing(&self.control);
        let window_min = match &step.transition {
            Some(LinearTransition::Window { min, .. }) => *min,
            _ => Duration::ZERO,
        };
        let arrival = LinearArrival {
            budget: Duration::from_millis(timing.timeout.milliseconds),
            interval: Duration::from_millis(timing.interval.milliseconds),
            post_input_delay: Duration::from_millis(operation.post_delay_ms.unwrap_or(0))
                .max(window_min),
        };
        let mut attempt = 1;
        loop {
            if Instant::now() >= deadline {
                return Err(self
                    .task_timeout_error(TaskTimingStage::Dispatch, deadline, None)
                    .into());
            }
            runtime
                .record(ContainedTaskTrace::StepStarted {
                    step_index,
                    operation_label: operation.id.clone(),
                    from_page: step.from.clone(),
                    phase: None,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            self.linear_input(runtime, run, step_index, operation, &observation)?;
            self.linear_post_input_delay(runtime, run, arrival.post_input_delay)?;
            let miss = match self.linear_after_input(runtime, run, step, arrival)? {
                Ok(reached) => {
                    Self::linear_step_finished(runtime, step_index, operation, &step.to)?;
                    return Ok(reached);
                }
                Err(miss) => miss,
            };
            if miss.kind == LinearMissKind::AfterIntermediate || attempt >= max_attempts {
                Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                return Err(match (miss.kind, &step.transition) {
                    (LinearMissKind::Intermediate, Some(LinearTransition::Page { page, .. })) => {
                        ContainedTaskError::with_detail(
                            INTERMEDIATE_UNOBSERVED,
                            format!(
                                "operation={} attempts={attempt} intermediate_page={page}",
                                operation.id
                            ),
                        )
                        .with_timing(postcondition_timing(miss.elapsed, miss.limit))
                    }
                    (kind, _) => page_confirmation_failed(
                        operation,
                        step,
                        attempt,
                        kind == LinearMissKind::AfterIntermediate,
                        miss.elapsed,
                        miss.limit,
                    ),
                }
                .into());
            }
            // The retry decision (§5.5): the gate first, then this step's own page.
            self.linear_retry_delay(runtime, run, retry_interval)?;
            let decision_started = Instant::now();
            let decided = self.linear_wait(
                runtime,
                &[step.gate(), step.from.as_str()],
                run.step_timeout,
                run.capture_interval,
                LinearWaitPurpose::RetryDecision,
                run.timing,
            )?;
            match decided {
                Some(frame) if frame.page_label == step.gate() => {
                    // The input took effect late: this attempt goes on, without another input.
                    if !matches!(step.transition, Some(LinearTransition::Page { .. })) {
                        Self::linear_step_finished(runtime, step_index, operation, &step.to)?;
                        return Ok(frame);
                    }
                    match self.linear_arrival(
                        runtime,
                        run,
                        step,
                        arrival.budget,
                        arrival.interval,
                        LinearMissKind::AfterIntermediate,
                    )? {
                        Ok(reached) => {
                            Self::linear_step_finished(runtime, step_index, operation, &step.to)?;
                            return Ok(reached);
                        }
                        Err(miss) => {
                            Self::linear_step_finished(
                                runtime,
                                step_index,
                                operation,
                                UNRECOGNIZED_PAGE,
                            )?;
                            return Err(page_confirmation_failed(
                                operation,
                                step,
                                attempt,
                                true,
                                miss.elapsed,
                                miss.limit,
                            )
                            .into());
                        }
                    }
                }
                Some(frame) => {
                    // This step's own page: the input was swallowed; input again on this frame.
                    Self::linear_step_finished(runtime, step_index, operation, &frame.page_label)?;
                    attempt += 1;
                    observation = frame;
                }
                None => {
                    Self::linear_step_finished(runtime, step_index, operation, UNRECOGNIZED_PAGE)?;
                    return Err(page_confirmation_failed(
                        operation,
                        step,
                        attempt,
                        false,
                        decision_started.elapsed(),
                        run.step_timeout,
                    )
                    .into());
                }
            }
        }
    }

    /// The guard on the frame that passed this step, then the effect intent and the input
    /// bound to that frame's input context, in the order of the page-graph path.
    fn linear_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step_index: u32,
        operation: &TaskOperation,
        observation: &PageObservation,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        let (guard, target) =
            operation.guard_outcome(&self.control, observation, &self.evaluator, runtime)?;
        let action_seed = runtime
            .action_seed(step_index, &operation.id)
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let click = operation
            .click
            .as_ref()
            .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
        let (action, sampling) =
            click.input_action(&self.control.resolution, target.as_ref(), action_seed)?;
        runtime
            .record(ContainedTaskTrace::EffectIntent {
                step_index,
                operation_label: operation.id.clone(),
                action: action.clone(),
                sampling,
                guard,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        if Instant::now() >= deadline {
            return Err(self
                .task_timeout_error(TaskTimingStage::BeforeInput, deadline, None)
                .into());
        }
        runtime
            .input(action, observation.input_context.clone())
            .map_err(ContainedTaskRunError::operation::<R>)?;
        let boundary = TaskTimingBoundary::EffectCompletedRecord;
        let identity = runtime.task_boundary_identity(boundary);
        let effect_started = Instant::now();
        let effected = runtime.record(ContainedTaskTrace::EffectCompleted {
            step_index,
            operation_label: operation.id.clone(),
        });
        let effect_ended = Instant::now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: run.timing,
            started: effect_started,
            ended: effect_ended,
            succeeded: effected.is_ok(),
        });
        effected.map_err(ContainedTaskRunError::Boundary)
    }

    /// The wait after an input: `post_delay_ms`, or the window's lower bound when larger.
    fn linear_post_input_delay<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        delay: Duration,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        let boundary = TaskTimingBoundary::PostInputWait;
        let identity = runtime.task_boundary_identity(boundary);
        let wait_started = Instant::now();
        let waited = if delay.is_zero() {
            Ok(())
        } else if deadline
            .checked_duration_since(Instant::now())
            .is_none_or(|remaining| delay >= remaining)
        {
            Err(self.task_timeout_error(TaskTimingStage::PostInputDelay, deadline, Some(delay)))
        } else {
            thread::sleep(delay);
            if Instant::now() >= deadline {
                Err(self.task_timeout_error(TaskTimingStage::PostInputDelay, deadline, None))
            } else {
                Ok(())
            }
        };
        let wait_ended = Instant::now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: run.timing,
            started: wait_started,
            ended: wait_ended,
            succeeded: waited.is_ok(),
        });
        waited.map_err(Into::into)
    }

    /// One attempt after its post-input wait (§5.4): the intermediate state, then the next
    /// step's page.
    fn linear_after_input<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step: &LinearStep,
        arrival: LinearArrival,
    ) -> LinearAttemptResult<R::Error> {
        match &step.transition {
            None => self.linear_arrival(
                runtime,
                run,
                step,
                arrival.budget,
                arrival.interval,
                LinearMissKind::Arrival,
            ),
            Some(LinearTransition::Page {
                page,
                timeout,
                interval,
            }) => {
                let started = Instant::now();
                match self.linear_wait(
                    runtime,
                    &[page.as_str()],
                    *timeout,
                    *interval,
                    LinearWaitPurpose::AfterInput,
                    run.timing,
                )? {
                    // The next step's arrival budget starts at the frame that showed the page.
                    Some(_) => self.linear_arrival(
                        runtime,
                        run,
                        step,
                        arrival.budget,
                        arrival.interval,
                        LinearMissKind::AfterIntermediate,
                    ),
                    None => Ok(Err(LinearMiss {
                        kind: LinearMissKind::Intermediate,
                        elapsed: started.elapsed(),
                        limit: *timeout,
                    })),
                }
            }
            Some(LinearTransition::Window { max, .. }) => self.linear_arrival(
                runtime,
                run,
                step,
                max.saturating_sub(arrival.post_input_delay)
                    .saturating_add(arrival.budget),
                arrival.interval,
                LinearMissKind::Arrival,
            ),
        }
    }

    /// The wait for the next step's page.
    fn linear_arrival<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        step: &LinearStep,
        budget: Duration,
        interval: Duration,
        miss: LinearMissKind,
    ) -> LinearAttemptResult<R::Error> {
        let started = Instant::now();
        Ok(self
            .linear_wait(
                runtime,
                &[step.to.as_str()],
                budget,
                interval,
                LinearWaitPurpose::AfterInput,
                run.timing,
            )?
            .ok_or_else(|| LinearMiss {
                kind: miss,
                elapsed: started.elapsed(),
                limit: budget,
            }))
    }

    fn linear_retry_delay<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        run: LinearRun,
        delay: Duration,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        let deadline = run.timing.deadline();
        if deadline
            .checked_duration_since(Instant::now())
            .is_none_or(|remaining| delay > remaining)
        {
            return Err(self
                .task_timeout_error(TaskTimingStage::RetryDelay, deadline, Some(delay))
                .into());
        }
        let boundary = TaskTimingBoundary::RetryWait;
        let identity = runtime.task_boundary_identity(boundary);
        let wait_started = Instant::now();
        thread::sleep(delay);
        let wait_ended = Instant::now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: run.timing,
            started: wait_started,
            ended: wait_ended,
            succeeded: true,
        });
        Ok(())
    }

    fn linear_step_finished<R: ContainedTaskRuntime>(
        runtime: &mut R,
        step_index: u32,
        operation: &TaskOperation,
        page_label: &str,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::StepFinished {
                step_index,
                operation_label: operation.id.clone(),
                page_label: page_label.to_owned(),
                phase: None,
            })
            .map_err(ContainedTaskRunError::Boundary)
    }

    /// Captures until one of `candidates` passes, at most `budget` after the first capture;
    /// `None` once the budget is spent. A task deadline inside the wait fails the task.
    fn linear_wait<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        candidates: &[&str],
        budget: Duration,
        interval: Duration,
        purpose: LinearWaitPurpose,
        timing: ContainedTaskTimingContext,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        let deadline = timing.deadline();
        let started = Instant::now();
        loop {
            if Instant::now() >= deadline {
                return Err(self
                    .linear_task_timeout(
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionBeforeCapture,
                    )
                    .into());
            }
            let observation = self.linear_observe(runtime, candidates, timing)?;
            if Instant::now() >= deadline {
                return Err(self
                    .linear_task_timeout(
                        purpose,
                        deadline,
                        TaskTimingCheckPosition::PostconditionAfterCapture,
                    )
                    .into());
            }
            if observation.is_some() {
                return Ok(observation);
            }
            if started.elapsed() >= budget {
                return Ok(None);
            }
            let boundary = purpose.boundary();
            let identity = runtime.task_boundary_identity(boundary);
            let wait_started = Instant::now();
            thread::sleep(
                interval
                    .min(budget.saturating_sub(started.elapsed()))
                    .min(deadline.saturating_duration_since(Instant::now())),
            );
            let wait_ended = Instant::now();
            runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
                boundary,
                identity,
                context: timing,
                started: wait_started,
                ended: wait_ended,
                succeeded: true,
            });
        }
    }

    fn linear_task_timeout(
        &self,
        purpose: LinearWaitPurpose,
        deadline: Instant,
        position: TaskTimingCheckPosition,
    ) -> ContainedTaskError {
        let error = self.task_timeout_error(purpose.timeout_stage(), deadline, None);
        match purpose {
            LinearWaitPurpose::AfterInput => error.with_timing_check_position(position),
            LinearWaitPurpose::Entry | LinearWaitPurpose::RetryDecision => error,
        }
    }

    /// One capture and the evaluation of `candidates` only, recorded as every capture is, at
    /// the `CapturePage` boundary (§5.3). The first passing candidate is the frame's page; with
    /// the frame's input reference the committed input context binds a following input.
    fn linear_observe<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        candidates: &[&str],
        timing: ContainedTaskTimingContext,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        let boundary = TaskTimingBoundary::CapturePage;
        let identity = runtime.task_boundary_identity(boundary);
        let capture_started = Instant::now();
        let result = (|| {
            let frame = runtime
                .capture()
                .map_err(ContainedTaskRunError::operation::<R>)?;
            self.control.resolution.validate_frame(&frame)?;
            runtime
                .record(ContainedTaskTrace::CaptureCompleted {
                    width: frame.width,
                    height: frame.height,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            let scene = scene_from_frame(&frame)?;
            let input_context = match frame.input_reference {
                Some(reference) => runtime
                    .committed_input_frame(reference)
                    .map_err(ContainedTaskRunError::Boundary)?,
                None => None,
            };
            let context = self.evaluator.scene_context(&scene);
            let candidate_pages = candidates
                .iter()
                .map(|page| (*page).to_owned())
                .collect::<Vec<_>>();
            runtime
                .record(ContainedTaskTrace::RecognitionStarted {
                    candidate_pages: candidate_pages.clone(),
                    width: frame.width,
                    height: frame.height,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            let evaluation_started = Instant::now();
            let budget_before = timing.budget_at(evaluation_started);
            let results = self
                .detector
                .evaluate_pages_outcomes_in_context(&context, candidates);
            let evaluation_timing = ContainedTaskEvaluationTiming {
                elapsed_us: observe_instant_span(evaluation_started, Instant::now()),
                budget_before,
                result: if results.is_ok() {
                    TaskTimingResult::Ok
                } else {
                    TaskTimingResult::Err
                },
            };
            runtime
                .record_page_evaluations("page", &results, Some(evaluation_timing))
                .map_err(ContainedTaskRunError::Boundary)?;
            let evaluations = results
                .map_err(|error| {
                    PageDetectorError::fatal(error.to_string())
                        .with_ppocr_diagnostics(error.ppocr_diagnostics())
                })
                .and_then(require_all_page_evaluations)
                .map_err(|error| {
                    ContainedTaskError::with_detail(
                        "contained_task_recognition_failed",
                        error.to_string(),
                    )
                    .with_ppocr_diagnostics(error.ppocr_diagnostics())
                })?;
            let page = evaluations
                .iter()
                .find(|evaluation| evaluation.matched)
                .map(|evaluation| evaluation.page_id.clone());
            let targets = recognized_page_targets(
                &self.evaluator,
                &evaluations,
                page.as_deref(),
                &candidate_pages,
            )?;
            runtime
                .record(ContainedTaskTrace::RecognitionCompleted {
                    candidate_pages,
                    page_label: page.clone(),
                    width: frame.width,
                    height: frame.height,
                    targets,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            Ok(page.map(|page_label| PageObservation {
                page_label,
                scene,
                stability_sample: None,
                input_context,
                captured_at: frame.captured_at,
            }))
        })();
        let capture_ended = Instant::now();
        runtime.observe_task_boundary(ContainedTaskBoundaryTiming {
            boundary,
            identity,
            context: timing,
            started: capture_started,
            ended: capture_ended,
            succeeded: result.is_ok(),
        });
        result
    }
}
