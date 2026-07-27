// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned admission and execution for contained semantic task packages.

use crate::{
    ExternalExpectedSha256, ExternallyVerifiedBundle, RunDirective, RunFailureObservation,
    RunFailureStage, RunOperationCandidate, RunOperationFailureDecision, RunOperationPolicy,
    RunStateConfig, RunStateMachine, RunTerminal, decide_run_operation_failure,
    select_run_operation,
};
use actingcommand_contract::{
    InputAction, SchedulingEffectCondition, SchedulingOutcomeDeclaration, TaskOutcome,
};
use actingcommand_device::{Frame, PixelFormat};
use actingcommand_pack_containment::LoadedBundle;
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{RecognitionEvaluator, TargetEvaluation, TargetKind};
use serde::Deserialize;
use serde::Serialize;
use std::collections::{BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

const CONTROL_SCHEMA: &str = "Lab-1y.control.v1";
const DEFAULT_CAPTURE_INTERVAL_MS: u64 = 50;
const DEFAULT_TASK_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_STEP_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_STEPS: u32 = 100;
const MAX_TASK_TIMEOUT_MS: u64 = 600_000;
const MAX_STEP_TIMEOUT_MS: u64 = 60_000;
const MAX_CAPTURE_INTERVAL_MS: u64 = 5_000;
const MAX_STEPS: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedTaskError {
    code: &'static str,
    detail: Option<String>,
}

impl ContainedTaskError {
    fn new(code: &'static str) -> Self {
        Self { code, detail: None }
    }

    fn with_detail(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl fmt::Display for ContainedTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "contained task error {}", self.code)?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl Error for ContainedTaskError {}

#[derive(Debug)]
pub enum ContainedTaskRunError<E> {
    Boundary(E),
    Task(ContainedTaskError),
}

impl<E> From<ContainedTaskError> for ContainedTaskRunError<E> {
    fn from(error: ContainedTaskError) -> Self {
        Self::Task(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainedTaskTrace {
    PackageAdmitted {
        task_label: String,
        package_label: String,
        package_sha256: String,
    },
    RunStarted,
    CaptureCompleted {
        width: u32,
        height: u32,
    },
    RecognitionCompleted {
        candidate_pages: Vec<String>,
        page_label: Option<String>,
        width: u32,
        height: u32,
    },
    RecognitionStarted {
        candidate_pages: Vec<String>,
        width: u32,
        height: u32,
    },
    StepStarted {
        step_index: u32,
        operation_label: String,
        from_page: String,
    },
    EffectIntent {
        step_index: u32,
        operation_label: String,
        action: InputAction,
        guard: ContainedTaskGuardOutcome,
    },
    EffectCompleted {
        step_index: u32,
        operation_label: String,
    },
    StepFinished {
        step_index: u32,
        operation_label: String,
        page_label: String,
    },
    Finalizing {
        outcome: TaskOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ContainedTaskGuardOutcome {
    TrustedCoordinate,
    Passed {
        page_label: String,
        target_id: String,
        target_kind: String,
    },
}

/// Runtime boundary used by the semantic engine for device effects and durable facts.
pub trait ContainedTaskRuntime {
    type Error;

    fn capture(&mut self) -> Result<Frame, Self::Error>;

    fn input(&mut self, action: InputAction) -> Result<(), Self::Error>;

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedTaskOutcome {
    pub outcome: TaskOutcome,
    pub final_page: Option<String>,
    pub executed_steps: u32,
}

pub struct PreparedContainedTask {
    control: TaskControl,
    program: TaskProgram,
    evaluator: RecognitionEvaluator,
    detector: PageDetector,
    scheduling_outcome: Option<SchedulingOutcomeDeclaration>,
    package_sha256: String,
    entry_count: usize,
    task_count: usize,
}

impl PreparedContainedTask {
    pub fn load(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
    ) -> Result<Self, ContainedTaskError> {
        let bundle = ExternallyVerifiedBundle::load(instance_label, zip_bytes, expected)
            .map_err(|_| ContainedTaskError::new("contained_task_admission_failed"))?;
        let package_sha256 = bundle.loaded_bundle().verified_hash().to_string();
        let entry_count = bundle.loaded_bundle().entry_count();
        let task_count = bundle.loaded_bundle().task_count();
        let bundle = bundle.into_loaded_bundle();
        let control = bundle
            .control()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_control_missing"))?;
        let control: TaskControl = serde_json::from_value(control)
            .map_err(|_| ContainedTaskError::new("contained_task_control_invalid"))?;
        control.validate()?;
        let program: TaskProgram = serde_json::from_value(bundle.operation().clone())
            .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let evaluator = bundle
            .evaluator()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_recognition_pack_missing"))?;
        let detector = bundle
            .detector()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_page_set_missing"))?;
        detector
            .validate(&evaluator)
            .map_err(|_| ContainedTaskError::new("contained_task_recognition_invalid"))?;
        program.validate(&control, &bundle, &detector)?;
        let scheduling_outcome = program.scheduling_outcome.clone();
        Ok(Self {
            control,
            program,
            evaluator,
            detector,
            scheduling_outcome,
            package_sha256,
            entry_count,
            task_count,
        })
    }

    pub fn task_label(&self) -> &str {
        &self.control.entry_task_id
    }

    pub fn package_label(&self) -> &str {
        &self.control.package_id
    }

    pub fn package_sha256(&self) -> &str {
        &self.package_sha256
    }

    pub fn execution_mode(&self) -> &str {
        &self.control.execution_mode
    }

    pub fn game(&self) -> &str {
        &self.control.game
    }

    pub fn scheduling_outcome(&self) -> Option<&SchedulingOutcomeDeclaration> {
        self.scheduling_outcome.as_ref()
    }

    pub const fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub const fn task_count(&self) -> usize {
        self.task_count
    }

    pub fn run<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::PackageAdmitted {
                task_label: self.task_label().to_string(),
                package_label: self.package_label().to_string(),
                package_sha256: self.package_sha256.clone(),
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        runtime
            .record(ContainedTaskTrace::RunStarted)
            .map_err(ContainedTaskRunError::Boundary)?;

        let capture_interval = Duration::from_millis(
            self.control
                .capture_interval_ms
                .unwrap_or(DEFAULT_CAPTURE_INTERVAL_MS),
        );
        let step_timeout = Duration::from_millis(
            self.control
                .step_timeout_ms
                .unwrap_or(DEFAULT_STEP_TIMEOUT_MS),
        );
        let task_timeout =
            Duration::from_millis(self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS));
        let started = Instant::now();
        let mut observation = self.capture_until_page(runtime, step_timeout, capture_interval)?;
        if self.control.execution_mode == "recognize_only" {
            runtime
                .record(ContainedTaskTrace::Finalizing {
                    outcome: TaskOutcome::Success,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            return Ok(ContainedTaskOutcome {
                outcome: TaskOutcome::Success,
                final_page: Some(observation.page_label),
                executed_steps: 0,
            });
        }

        let candidates = self
            .program
            .operations
            .iter()
            .map(|operation| RunOperationCandidate::new(&operation.id, &operation.from))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let config = RunStateConfig::new_with_target_pages(
            &self.control.game,
            self.program.target_pages()?,
            self.control.stop_on_confirmation.unwrap_or(true),
            1,
            self.control.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
        )
        .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let mut machine = RunStateMachine::new(config, 0)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        machine
            .observe_page(Some(observation.page_label.clone()))
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;

        loop {
            if started.elapsed() > task_timeout {
                return Err(ContainedTaskError::new("contained_task_timeout").into());
            }
            match machine
                .next_directive(&candidates)
                .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?
            {
                RunDirective::AwaitPage => {
                    observation =
                        self.capture_until_page(runtime, step_timeout, capture_interval)?;
                    machine
                        .observe_page(Some(observation.page_label.clone()))
                        .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
                }
                RunDirective::ExecuteOperation {
                    operation_id,
                    current_page: from_page,
                    step_index,
                } => {
                    let operation = self
                        .program
                        .operations
                        .iter()
                        .find(|candidate| candidate.id == operation_id)
                        .ok_or_else(|| {
                            ContainedTaskError::new("contained_task_operation_missing")
                        })?;
                    let retry_policy = operation.retry_policy(
                        self.program.defaults,
                        self.control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
                    )?;
                    let mut attempt = 1;
                    loop {
                        if started.elapsed() > task_timeout {
                            return Err(ContainedTaskError::new("contained_task_timeout").into());
                        }
                        runtime
                            .record(ContainedTaskTrace::StepStarted {
                                step_index,
                                operation_label: operation_id.clone(),
                                from_page: from_page.clone(),
                            })
                            .map_err(ContainedTaskRunError::Boundary)?;
                        let (guard, target) = match operation.guard_outcome(
                            &self.control,
                            &observation,
                            &self.evaluator,
                        ) {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                let Some(policy) = retry_policy.as_ref() else {
                                    return Err(error.into());
                                };
                                match operation.failure_decision(
                                    policy,
                                    attempt,
                                    error.code(),
                                    Some(observation.page_label.clone()),
                                    RunFailureStage::PreExecutionGuard,
                                )? {
                                    RunOperationFailureDecision::RequestRecovery(trigger) => {
                                        machine.operation_needs_recovery(trigger).map_err(
                                            |_| {
                                                ContainedTaskError::new(
                                                    "contained_task_state_invalid",
                                                )
                                            },
                                        )?;
                                        break;
                                    }
                                    RunOperationFailureDecision::Fail(_) => {
                                        return Err(error.into());
                                    }
                                    RunOperationFailureDecision::Retry { .. } => {
                                        return Err(ContainedTaskError::new(
                                            "contained_task_state_invalid",
                                        )
                                        .into());
                                    }
                                }
                            }
                        };
                        let action = operation
                            .click
                            .input_action(&self.control.resolution, target.as_ref())?;
                        runtime
                            .record(ContainedTaskTrace::EffectIntent {
                                step_index,
                                operation_label: operation_id.clone(),
                                action: action.clone(),
                                guard,
                            })
                            .map_err(ContainedTaskRunError::Boundary)?;
                        runtime
                            .input(action)
                            .map_err(ContainedTaskRunError::Boundary)?;
                        runtime
                            .record(ContainedTaskTrace::EffectCompleted {
                                step_index,
                                operation_label: operation_id.clone(),
                            })
                            .map_err(ContainedTaskRunError::Boundary)?;
                        let destination_pages = operation.destination_pages()?;
                        if destination_pages.is_empty() {
                            observation =
                                self.capture_until_page(runtime, step_timeout, capture_interval)?;
                            runtime
                                .record(ContainedTaskTrace::StepFinished {
                                    step_index,
                                    operation_label: operation_id.clone(),
                                    page_label: observation.page_label.clone(),
                                })
                                .map_err(ContainedTaskRunError::Boundary)?;
                            machine
                                .operation_succeeded(
                                    &operation_id,
                                    Some(observation.page_label.clone()),
                                )
                                .map_err(|_| {
                                    ContainedTaskError::new("contained_task_state_invalid")
                                })?;
                            break;
                        }
                        let confirmation_timeout = Duration::from_millis(
                            operation
                                .expect_after
                                .as_ref()
                                .and_then(|expectation| expectation.timeout_ms)
                                .unwrap_or(step_timeout.as_millis() as u64),
                        );
                        let confirmation_interval = Duration::from_millis(
                            operation
                                .expect_after
                                .as_ref()
                                .and_then(|expectation| expectation.interval_ms)
                                .unwrap_or(capture_interval.as_millis() as u64),
                        );
                        let (failed_observation, hit_error_page) = match self.await_postcondition(
                            runtime,
                            operation,
                            confirmation_timeout,
                            confirmation_interval,
                        )? {
                            PostconditionResolution::Reached(reached) => {
                                observation = reached;
                                runtime
                                    .record(ContainedTaskTrace::StepFinished {
                                        step_index,
                                        operation_label: operation_id.clone(),
                                        page_label: observation.page_label.clone(),
                                    })
                                    .map_err(ContainedTaskRunError::Boundary)?;
                                machine
                                    .operation_succeeded(
                                        &operation_id,
                                        Some(observation.page_label.clone()),
                                    )
                                    .map_err(|_| {
                                        ContainedTaskError::new("contained_task_state_invalid")
                                    })?;
                                break;
                            }
                            PostconditionResolution::Failed {
                                observation,
                                hit_error_page,
                            } => (observation, hit_error_page),
                        };
                        let after_page = failed_observation
                            .as_ref()
                            .map(|observation| observation.page_label.clone());
                        let Some(policy) = retry_policy.as_ref() else {
                            Self::finish_effect_attempt(
                                runtime,
                                step_index,
                                &operation_id,
                                failed_observation.as_ref(),
                            )?;
                            return Err(ContainedTaskError::with_detail(
                                "page_confirmation_failed",
                                format!(
                                    "operation={operation_id} attempts={attempt} after_page={} hit_error_page={hit_error_page}",
                                    after_page.as_deref().unwrap_or("<unrecognized>")
                                ),
                            )
                            .into());
                        };
                        match operation.failure_decision(
                            policy,
                            attempt,
                            "page_confirmation_failed",
                            after_page,
                            RunFailureStage::PostExecution { hit_error_page },
                        )? {
                            RunOperationFailureDecision::Retry {
                                next_attempt,
                                delay_ms,
                            } => {
                                let delay = Duration::from_millis(delay_ms);
                                if task_timeout
                                    .checked_sub(started.elapsed())
                                    .is_none_or(|remaining| delay > remaining)
                                {
                                    return Err(
                                        ContainedTaskError::new("contained_task_timeout").into()
                                    );
                                }
                                thread::sleep(delay);
                                match self.await_postcondition(
                                    runtime,
                                    operation,
                                    confirmation_timeout,
                                    confirmation_interval,
                                )? {
                                    PostconditionResolution::Reached(reached) => {
                                        observation = reached;
                                        runtime
                                            .record(ContainedTaskTrace::StepFinished {
                                                step_index,
                                                operation_label: operation_id.clone(),
                                                page_label: observation.page_label.clone(),
                                            })
                                            .map_err(ContainedTaskRunError::Boundary)?;
                                        machine
                                            .operation_succeeded(
                                                &operation_id,
                                                Some(observation.page_label.clone()),
                                            )
                                            .map_err(|_| {
                                                ContainedTaskError::new(
                                                    "contained_task_state_invalid",
                                                )
                                            })?;
                                        break;
                                    }
                                    PostconditionResolution::Failed {
                                        observation: fresh,
                                        hit_error_page: true,
                                    } => {
                                        let after_page = fresh
                                            .as_ref()
                                            .map(|observation| observation.page_label.clone());
                                        Self::finish_effect_attempt(
                                            runtime,
                                            step_index,
                                            &operation_id,
                                            fresh.as_ref(),
                                        )?;
                                        match operation.failure_decision(
                                            policy,
                                            attempt,
                                            "page_confirmation_failed",
                                            after_page,
                                            RunFailureStage::PostExecution {
                                                hit_error_page: true,
                                            },
                                        )? {
                                            RunOperationFailureDecision::RequestRecovery(
                                                trigger,
                                            ) => {
                                                machine.operation_needs_recovery(trigger).map_err(
                                                    |_| {
                                                        ContainedTaskError::new(
                                                            "contained_task_state_invalid",
                                                        )
                                                    },
                                                )?;
                                                break;
                                            }
                                            RunOperationFailureDecision::Fail(_) => {
                                                return Err(ContainedTaskError::with_detail(
                                                    "contained_task_requires_scheduler",
                                                    format!(
                                                        "operation={operation_id} attempts={attempt} reason=page_confirmation_failed"
                                                    ),
                                                )
                                                .into());
                                            }
                                            RunOperationFailureDecision::Retry { .. } => {
                                                return Err(ContainedTaskError::new(
                                                    "contained_task_state_invalid",
                                                )
                                                .into());
                                            }
                                        }
                                    }
                                    PostconditionResolution::Failed {
                                        observation: Some(fresh),
                                        hit_error_page: false,
                                    } => {
                                        Self::finish_effect_attempt(
                                            runtime,
                                            step_index,
                                            &operation_id,
                                            Some(&fresh),
                                        )?;
                                        observation = fresh;
                                    }
                                    PostconditionResolution::Failed {
                                        observation: None,
                                        hit_error_page: false,
                                    } => {
                                        Self::finish_effect_attempt(
                                            runtime,
                                            step_index,
                                            &operation_id,
                                            None,
                                        )?;
                                        return Err(ContainedTaskError::with_detail(
                                            "page_confirmation_failed",
                                            format!(
                                                "operation={operation_id} attempts={attempt} after_page=<unrecognized> hit_error_page=false"
                                            ),
                                        )
                                        .into());
                                    }
                                }
                                attempt = next_attempt;
                            }
                            RunOperationFailureDecision::RequestRecovery(trigger) => {
                                Self::finish_effect_attempt(
                                    runtime,
                                    step_index,
                                    &operation_id,
                                    failed_observation.as_ref(),
                                )?;
                                machine.operation_needs_recovery(trigger).map_err(|_| {
                                    ContainedTaskError::new("contained_task_state_invalid")
                                })?;
                                break;
                            }
                            RunOperationFailureDecision::Fail(_) => {
                                Self::finish_effect_attempt(
                                    runtime,
                                    step_index,
                                    &operation_id,
                                    failed_observation.as_ref(),
                                )?;
                                return Err(ContainedTaskError::with_detail(
                                    "contained_task_requires_scheduler",
                                    format!(
                                        "operation={operation_id} attempts={attempt} reason=page_confirmation_failed"
                                    ),
                                )
                                .into());
                            }
                        }
                    }
                }
                RunDirective::Continue { .. } => {
                    return Err(ContainedTaskError::new("contained_task_state_invalid").into());
                }
                RunDirective::Terminal(RunTerminal::Completed { current_page }) => {
                    runtime
                        .record(ContainedTaskTrace::Finalizing {
                            outcome: TaskOutcome::Success,
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    return Ok(ContainedTaskOutcome {
                        outcome: TaskOutcome::Success,
                        final_page: current_page,
                        executed_steps: machine.completed_steps(),
                    });
                }
                RunDirective::Terminal(
                    RunTerminal::SuccessorSuggested { .. } | RunTerminal::PausedNeedsHuman { .. },
                ) => {
                    return Err(ContainedTaskError::new("contained_task_requires_scheduler").into());
                }
            }
        }
    }

    fn finish_effect_attempt<R: ContainedTaskRuntime>(
        runtime: &mut R,
        step_index: u32,
        operation_label: &str,
        observation: Option<&PageObservation>,
    ) -> Result<(), ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::StepFinished {
                step_index,
                operation_label: operation_label.to_string(),
                page_label: match observation {
                    Some(observation) => observation.page_label.clone(),
                    None => "<unrecognized>".to_string(),
                },
            })
            .map_err(ContainedTaskRunError::Boundary)
    }

    fn capture_until_page<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        timeout: Duration,
        interval: Duration,
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let started = Instant::now();
        loop {
            if let Some(observation) = self.capture_page(runtime)? {
                return Ok(observation);
            }
            if started.elapsed() >= timeout {
                return Err(ContainedTaskError::new("contained_task_page_unknown").into());
            }
            thread::sleep(interval);
        }
    }

    fn capture_page<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
    ) -> Result<Option<PageObservation>, ContainedTaskRunError<R::Error>> {
        let frame = runtime.capture().map_err(ContainedTaskRunError::Boundary)?;
        self.control.resolution.validate_frame(&frame)?;
        runtime
            .record(ContainedTaskTrace::CaptureCompleted {
                width: frame.width,
                height: frame.height,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let scene = scene_from_frame(&frame)?;
        let candidate_pages = self
            .detector
            .page_ids()
            .map(str::to_string)
            .collect::<Vec<_>>();
        runtime
            .record(ContainedTaskTrace::RecognitionStarted {
                candidate_pages: candidate_pages.clone(),
                width: frame.width,
                height: frame.height,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        let matched_pages = self
            .detector
            .evaluate_all(&self.evaluator, &scene)
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    "contained_task_recognition_failed",
                    error.to_string(),
                )
            })?
            .into_iter()
            .filter(|evaluation| evaluation.matched)
            .map(|evaluation| evaluation.page_id)
            .collect::<Vec<_>>();
        if matched_pages.len() > 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_recognition_conflict",
                matched_pages.join(","),
            )
            .into());
        }
        let page = matched_pages.into_iter().next();
        runtime
            .record(ContainedTaskTrace::RecognitionCompleted {
                candidate_pages,
                page_label: page.clone(),
                width: frame.width,
                height: frame.height,
            })
            .map_err(ContainedTaskRunError::Boundary)?;
        Ok(page.map(|page_label| PageObservation { page_label, scene }))
    }

    fn await_postcondition<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        operation: &TaskOperation,
        timeout: Duration,
        interval: Duration,
    ) -> Result<PostconditionResolution, ContainedTaskRunError<R::Error>> {
        let started = Instant::now();
        let mut last_observation = None;
        loop {
            if let Some(observation) = self.capture_page(runtime)? {
                let destination_matches =
                    operation.matching_destination_count(&self.control, &observation)?;
                let hit_error_page = self
                    .program
                    .is_error_page(&self.control, &observation.page_label);
                if destination_matches > 1 || (destination_matches == 1 && hit_error_page) {
                    return Err(ContainedTaskError::with_detail(
                        "contained_task_recognition_conflict",
                        observation.page_label,
                    )
                    .into());
                }
                if destination_matches == 1 {
                    return Ok(PostconditionResolution::Reached(observation));
                }
                if hit_error_page {
                    return Ok(PostconditionResolution::Failed {
                        observation: Some(observation),
                        hit_error_page: true,
                    });
                }
                last_observation = Some(observation);
            }
            if started.elapsed() >= timeout {
                return Ok(PostconditionResolution::Failed {
                    observation: last_observation,
                    hit_error_page: false,
                });
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            thread::sleep(interval.min(remaining));
        }
    }
}

struct PageObservation {
    page_label: String,
    scene: Scene,
}

enum PostconditionResolution {
    Reached(PageObservation),
    Failed {
        observation: Option<PageObservation>,
        hit_error_page: bool,
    },
}

#[derive(Debug, Deserialize)]
struct TaskControl {
    schema_version: String,
    package_id: String,
    execution_mode: String,
    game: String,
    server: String,
    resolution: Resolution,
    entry_task_id: String,
    #[serde(default)]
    capture_interval_ms: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    step_timeout_ms: Option<u64>,
    #[serde(default)]
    max_steps: Option<u32>,
    #[serde(default)]
    stop_on_confirmation: Option<bool>,
}

impl TaskControl {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        if self.schema_version != CONTROL_SCHEMA
            || self.package_id.trim().is_empty()
            || self.game.trim().is_empty()
            || self.server.trim().is_empty()
            || self.entry_task_id.trim().is_empty()
            || !matches!(
                self.execution_mode.as_str(),
                "recognize_only" | "navigable_route" | "in_page_guard"
            )
        {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        }
        self.resolution.validate()?;
        validate_bounded(self.capture_interval_ms, MAX_CAPTURE_INTERVAL_MS)?;
        validate_bounded(self.timeout_ms, MAX_TASK_TIMEOUT_MS)?;
        validate_bounded(self.step_timeout_ms, MAX_STEP_TIMEOUT_MS)?;
        if self
            .max_steps
            .is_some_and(|value| value == 0 || value > MAX_STEPS)
        {
            return Err(ContainedTaskError::new("contained_task_control_invalid"));
        }
        Ok(())
    }
}

fn validate_bounded(value: Option<u64>, maximum: u64) -> Result<(), ContainedTaskError> {
    if value.is_some_and(|value| value == 0 || value > maximum) {
        Err(ContainedTaskError::new("contained_task_control_invalid"))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Resolution {
    width: u32,
    height: u32,
}

impl Resolution {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        if self.width == 0 || self.height == 0 {
            Err(ContainedTaskError::new("contained_task_resolution_invalid"))
        } else {
            Ok(())
        }
    }

    fn validate_frame(&self, frame: &Frame) -> Result<(), ContainedTaskError> {
        if frame.width == self.width && frame.height == self.height {
            Ok(())
        } else {
            Err(ContainedTaskError::new(
                "contained_task_frame_resolution_mismatch",
            ))
        }
    }

    fn validate_point(&self, x: i32, y: i32) -> Result<(), ContainedTaskError> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            Err(ContainedTaskError::new(
                "contained_task_input_out_of_bounds",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Deserialize)]
struct TaskProgram {
    schema_version: String,
    task_id: String,
    game: String,
    #[serde(default)]
    server_scope: Vec<String>,
    coordinate_space: Resolution,
    #[serde(default)]
    target_page: Option<PageDeclaration>,
    #[serde(default)]
    error_pages: Vec<String>,
    #[serde(default)]
    scheduling_outcome: Option<SchedulingOutcomeDeclaration>,
    #[serde(default)]
    recovery: Option<TaskRecovery>,
    #[serde(default)]
    defaults: TaskOperationDefaults,
    operations: Vec<TaskOperation>,
}

impl TaskProgram {
    fn validate(
        &self,
        control: &TaskControl,
        bundle: &LoadedBundle,
        detector: &PageDetector,
    ) -> Result<(), ContainedTaskError> {
        if !matches!(self.schema_version.as_str(), "0.3" | "0.4" | "0.5" | "0.6")
            || self.task_id != control.entry_task_id
            || self.game != control.game
            || (!self.server_scope.is_empty()
                && !self
                    .server_scope
                    .iter()
                    .any(|value| value == &control.server))
            || self.coordinate_space.width != control.resolution.width
            || self.coordinate_space.height != control.resolution.height
            || self.operations.is_empty()
            || self.error_pages.iter().any(|value| value.trim().is_empty())
        {
            return Err(ContainedTaskError::new("contained_task_program_invalid"));
        }
        let target_pages = self.target_pages()?;
        validate_page_references(&control.game, &target_pages, detector)?;
        validate_page_references(&control.game, &self.error_pages, detector)?;
        validate_page_set_overlap(&control.game, &target_pages, &self.error_pages, detector)?;
        if let Some(declaration) = &self.scheduling_outcome {
            validate_scheduling_outcome_execution_mode(control)?;
            declaration.validate().map_err(|_| {
                ContainedTaskError::new("contained_task_outcome_declaration_invalid")
            })?;
            if declaration
                .designated_operation()
                .is_some_and(|designated| {
                    self.operations
                        .iter()
                        .filter(|operation| operation.id == designated)
                        .count()
                        != 1
                })
            {
                return Err(ContainedTaskError::new(
                    "contained_task_outcome_declaration_invalid",
                ));
            }
            let terminal_pages = declaration
                .mappings()
                .iter()
                .flat_map(|mapping| mapping.terminal_pages().iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            validate_page_references(&control.game, &terminal_pages, detector)?;
            validate_page_set_overlap(&control.game, &terminal_pages, &self.error_pages, detector)?;
        }
        let mut operation_ids = BTreeSet::new();
        for operation in &self.operations {
            operation.validate(control, self.defaults)?;
            let destination_pages = operation.destination_pages()?;
            validate_page_references(&control.game, &destination_pages, detector)?;
            validate_page_set_overlap(
                &control.game,
                &destination_pages,
                &self.error_pages,
                detector,
            )?;
            if !operation_ids.insert(&operation.id) {
                return Err(ContainedTaskError::new("contained_task_program_invalid"));
            }
        }
        if let Some(declaration) = &self.scheduling_outcome {
            let observable_pages = detector.page_ids().map(str::to_owned).collect::<Vec<_>>();
            validate_scheduling_outcome_coverage(
                &control.game,
                &target_pages,
                &observable_pages,
                &self.operations,
                declaration,
            )?;
        }
        self.validate_recovery(bundle)?;
        Ok(())
    }

    fn target_pages(&self) -> Result<Vec<String>, ContainedTaskError> {
        self.target_page
            .as_ref()
            .map(PageDeclaration::normalized)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn is_error_page(&self, control: &TaskControl, page_label: &str) -> bool {
        self.error_pages
            .iter()
            .any(|expected| crate::page_anchor_matches(&control.game, page_label, expected))
    }

    fn validate_recovery(&self, bundle: &LoadedBundle) -> Result<(), ContainedTaskError> {
        let mut recovery_tasks = BTreeSet::new();
        if let Some(recovery) = &self.recovery {
            recovery.validate()?;
            recovery_tasks.insert(recovery.task_id());
        }
        if self
            .operations
            .iter()
            .any(|operation| operation.on_error.is_some())
        {
            recovery_tasks.insert("return_home");
        }
        for task_id in recovery_tasks {
            let relative_path = format!("operations/{task_id}/task.json");
            let bytes = bundle.resource_entry(&relative_path).map_err(|_| {
                ContainedTaskError::with_detail(
                    "contained_task_recovery_missing",
                    relative_path.clone(),
                )
            })?;
            let recovery: TaskProgram = serde_json::from_slice(bytes).map_err(|_| {
                ContainedTaskError::with_detail(
                    "contained_task_recovery_invalid",
                    relative_path.clone(),
                )
            })?;
            if recovery.task_id != task_id || recovery.game != self.game {
                return Err(ContainedTaskError::with_detail(
                    "contained_task_recovery_invalid",
                    relative_path,
                ));
            }
        }
        Ok(())
    }
}

fn validate_scheduling_outcome_execution_mode(
    control: &TaskControl,
) -> Result<(), ContainedTaskError> {
    if control.execution_mode == "recognize_only" {
        Err(ContainedTaskError::new(
            "contained_task_outcome_declaration_invalid",
        ))
    } else {
        Ok(())
    }
}

fn validate_scheduling_outcome_coverage(
    game: &str,
    target_pages: &[String],
    observable_pages: &[String],
    operations: &[TaskOperation],
    declaration: &SchedulingOutcomeDeclaration,
) -> Result<(), ContainedTaskError> {
    let designated_operation = declaration.designated_operation();
    let candidates = operations
        .iter()
        .map(|operation| RunOperationCandidate::new(&operation.id, &operation.from))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))?;
    let mut pending = observable_pages
        .iter()
        .map(|page| (page.clone(), SchedulingEffectCondition::NoDesignatedEffect))
        .collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();
    let mut reachable_terminals = BTreeSet::new();

    while let Some((page, condition)) = pending.pop_front() {
        if !visited.insert((page.clone(), condition)) {
            continue;
        }
        if target_pages
            .iter()
            .any(|target| crate::page_anchor_matches(game, &page, target))
        {
            reachable_terminals.insert((condition, page));
            continue;
        }
        if let Some(selected) = select_run_operation(game, &page, &candidates) {
            let operation = operations
                .iter()
                .find(|operation| operation.id == selected.id())
                .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
            if condition == SchedulingEffectCondition::DesignatedEffectCompleted
                && designated_operation == Some(operation.id.as_str())
            {
                return Err(ContainedTaskError::with_detail(
                    "contained_task_outcome_declaration_incomplete",
                    format!(
                        "designated_operation={} is reachable after its effect completed",
                        operation.id
                    ),
                ));
            }
            let next_condition = if condition
                == SchedulingEffectCondition::DesignatedEffectCompleted
                || designated_operation == Some(operation.id.as_str())
            {
                SchedulingEffectCondition::DesignatedEffectCompleted
            } else {
                SchedulingEffectCondition::NoDesignatedEffect
            };
            let destinations = operation.destination_pages()?;
            if destinations.is_empty() {
                return Err(ContainedTaskError::with_detail(
                    "contained_task_outcome_declaration_incomplete",
                    format!("operation={} has no finite postcondition", operation.id),
                ));
            }
            for destination in destinations {
                let matching_pages = observable_pages
                    .iter()
                    .filter(|page| crate::page_anchor_matches(game, page, &destination))
                    .collect::<Vec<_>>();
                let [concrete_page] = matching_pages.as_slice() else {
                    return Err(ContainedTaskError::with_detail(
                        "contained_task_outcome_declaration_incomplete",
                        format!(
                            "operation={} destination={} detector_matches={}",
                            operation.id,
                            destination,
                            matching_pages.len()
                        ),
                    ));
                };
                pending.push_back(((*concrete_page).clone(), next_condition));
            }
        }
    }

    for (condition, terminal_page) in reachable_terminals {
        let mapping_count = declaration
            .mappings()
            .iter()
            .filter(|mapping| {
                mapping.effect() == condition
                    && mapping.terminal_pages().iter().any(|mapped_page| {
                        crate::page_anchor_matches(game, &terminal_page, mapped_page)
                    })
            })
            .count();
        if mapping_count != 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_outcome_declaration_incomplete",
                format!(
                    "effect={condition:?} terminal_page={terminal_page} mappings={mapping_count}"
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PageDeclaration {
    Singleton(String),
    Set(Vec<String>),
}

impl PageDeclaration {
    fn normalized(&self) -> Result<Vec<String>, ContainedTaskError> {
        let pages = match self {
            Self::Singleton(page) => vec![page.clone()],
            Self::Set(pages) => pages.clone(),
        };
        if pages.is_empty() || pages.iter().any(|page| page.trim().is_empty()) {
            return Err(ContainedTaskError::new("contained_task_page_set_invalid"));
        }
        let unique = pages.iter().collect::<BTreeSet<_>>();
        if unique.len() != pages.len() {
            return Err(ContainedTaskError::new("contained_task_page_set_invalid"));
        }
        let mut pages = pages;
        pages.sort();
        Ok(pages)
    }
}

fn validate_page_references(
    game: &str,
    pages: &[String],
    detector: &PageDetector,
) -> Result<(), ContainedTaskError> {
    for page in pages {
        let matches = detector
            .page_ids()
            .filter(|candidate| crate::page_anchor_matches(game, candidate, page))
            .count();
        if matches != 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_page_set_invalid",
                format!("page={page} detector_matches={matches}"),
            ));
        }
    }
    for candidate in detector.page_ids() {
        let matches = pages
            .iter()
            .filter(|page| crate::page_anchor_matches(game, candidate, page))
            .count();
        if matches > 1 {
            return Err(ContainedTaskError::with_detail(
                "contained_task_page_set_invalid",
                format!("detector_page={candidate} declaration_matches={matches}"),
            ));
        }
    }
    Ok(())
}

fn validate_page_set_overlap(
    game: &str,
    destinations: &[String],
    error_pages: &[String],
    detector: &PageDetector,
) -> Result<(), ContainedTaskError> {
    for candidate in detector.page_ids() {
        let destination = destinations
            .iter()
            .any(|page| crate::page_anchor_matches(game, candidate, page));
        let error = error_pages
            .iter()
            .any(|page| crate::page_anchor_matches(game, candidate, page));
        if destination && error {
            return Err(ContainedTaskError::with_detail(
                "contained_task_page_set_invalid",
                format!("detector_page={candidate} is both destination and error"),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct TaskOperationDefaults {
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TaskOperationExpectation {
    page_id: PageDeclaration,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    interval_ms: Option<u64>,
}

impl TaskOperationExpectation {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        self.page_id.normalized()?;
        validate_bounded(self.timeout_ms, MAX_STEP_TIMEOUT_MS)?;
        validate_bounded(self.interval_ms, MAX_CAPTURE_INTERVAL_MS)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TaskRecovery {
    Kind(String),
    Config {
        kind: String,
        #[serde(default)]
        task_id: Option<String>,
    },
}

impl TaskRecovery {
    fn validate(&self) -> Result<(), ContainedTaskError> {
        if self.kind() != "return_home" || self.task_id().trim().is_empty() {
            Err(ContainedTaskError::new("contained_task_recovery_invalid"))
        } else {
            Ok(())
        }
    }

    fn kind(&self) -> &str {
        match self {
            Self::Kind(kind) | Self::Config { kind, .. } => kind,
        }
    }

    fn task_id(&self) -> &str {
        match self {
            Self::Kind(_) => "return_home",
            Self::Config { task_id, .. } => task_id.as_deref().unwrap_or("return_home"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TaskOperation {
    id: String,
    from: String,
    #[serde(default)]
    to: Option<PageDeclaration>,
    #[serde(default)]
    expect_after: Option<TaskOperationExpectation>,
    click: TaskClick,
    #[serde(default)]
    on_error: Option<String>,
    #[serde(default)]
    retryable: Option<bool>,
    #[serde(default)]
    max_attempts: Option<u32>,
    #[serde(default)]
    retry_interval_ms: Option<u64>,
    #[serde(default)]
    guard: Option<OperationGuard>,
    #[serde(default)]
    unguarded_trusted_coordinate: bool,
}

impl TaskOperation {
    fn validate(
        &self,
        control: &TaskControl,
        defaults: TaskOperationDefaults,
    ) -> Result<(), ContainedTaskError> {
        if self.id.trim().is_empty() || self.from.trim().is_empty() {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        self.destination_pages()?;
        if let Some(expectation) = &self.expect_after {
            expectation.validate()?;
        }
        if self
            .on_error
            .as_deref()
            .is_some_and(|value| value != "return_home")
        {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        self.retry_policy(
            defaults,
            control.timeout_ms.unwrap_or(DEFAULT_TASK_TIMEOUT_MS),
        )?;
        match (&self.guard, self.unguarded_trusted_coordinate) {
            (Some(_), true) | (None, false) => {
                return Err(ContainedTaskError::new("contained_task_guard_missing"));
            }
            (Some(guard), false) => guard.validate(self, control)?,
            (None, true) => {}
        }
        self.click
            .validate(&control.resolution, self.guard.as_ref())
    }

    fn retry_policy(
        &self,
        defaults: TaskOperationDefaults,
        task_timeout_ms: u64,
    ) -> Result<Option<RunOperationPolicy>, ContainedTaskError> {
        let (retryable, max_attempts, retry_interval_ms) =
            match (self.retryable, self.max_attempts, self.retry_interval_ms) {
                (None, None, None) => return Ok(None),
                (Some(false), max_attempts, retry_interval_ms) => (
                    false,
                    max_attempts.or(defaults.max_attempts).unwrap_or(1),
                    retry_interval_ms
                        .or(defaults.retry_interval_ms)
                        .unwrap_or(1),
                ),
                (Some(true), max_attempts, retry_interval_ms) => (
                    true,
                    max_attempts.or(defaults.max_attempts).ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?,
                    retry_interval_ms
                        .or(defaults.retry_interval_ms)
                        .ok_or_else(|| {
                            ContainedTaskError::new("contained_task_operation_invalid")
                        })?,
                ),
                _ => {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
            };
        if retryable
            && (self.destination_pages()?.is_empty() || retry_interval_ms > task_timeout_ms)
        {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        RunOperationPolicy::new(
            retryable,
            max_attempts,
            retry_interval_ms,
            self.on_error.clone(),
        )
        .map(Some)
        .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))
    }

    fn failure_decision(
        &self,
        policy: &RunOperationPolicy,
        attempt: u32,
        reason: &str,
        after_page: Option<String>,
        stage: RunFailureStage,
    ) -> Result<RunOperationFailureDecision, ContainedTaskError> {
        let observation = RunFailureObservation::new(&self.id, attempt, reason, after_page, stage)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        decide_run_operation_failure(policy, observation)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))
    }

    fn destination_pages(&self) -> Result<Vec<String>, ContainedTaskError> {
        let to = self
            .to
            .as_ref()
            .map(PageDeclaration::normalized)
            .transpose()?;
        let expected = self
            .expect_after
            .as_ref()
            .map(|expectation| expectation.page_id.normalized())
            .transpose()?;
        match (to, expected) {
            (Some(to), Some(expected)) if to != expected => Err(ContainedTaskError::new(
                "contained_task_destination_conflict",
            )),
            (Some(to), _) => Ok(to),
            (None, Some(expected)) => Ok(expected),
            (None, None) => Ok(Vec::new()),
        }
    }

    fn matching_destination_count(
        &self,
        control: &TaskControl,
        observation: &PageObservation,
    ) -> Result<usize, ContainedTaskError> {
        Ok(self
            .destination_pages()?
            .iter()
            .filter(|expected| {
                crate::page_anchor_matches(&control.game, &observation.page_label, expected)
            })
            .count())
    }

    fn guard_outcome(
        &self,
        control: &TaskControl,
        observation: &PageObservation,
        evaluator: &RecognitionEvaluator,
    ) -> Result<(ContainedTaskGuardOutcome, Option<TargetEvaluation>), ContainedTaskError> {
        if self.unguarded_trusted_coordinate {
            return Ok((ContainedTaskGuardOutcome::TrustedCoordinate, None));
        }
        let guard = self
            .guard
            .as_ref()
            .ok_or_else(|| ContainedTaskError::new("contained_task_guard_missing"))?;
        if !crate::page_anchor_matches(&control.game, &observation.page_label, &guard.page_id) {
            return Err(ContainedTaskError::with_detail(
                "contained_task_guard_refused",
                format!(
                    "operation={} expected_page={} observed_page={}",
                    self.id, guard.page_id, observation.page_label
                ),
            ));
        }
        let target = evaluator
            .evaluate_target(&observation.scene, &guard.target_id)
            .map_err(|error| {
                ContainedTaskError::with_detail(
                    "contained_task_guard_evaluation_failed",
                    error.to_string(),
                )
            })?;
        if !target.passed {
            return Err(ContainedTaskError::with_detail(
                "contained_task_guard_refused",
                format!("operation={} target={}", self.id, guard.target_id),
            ));
        }
        let outcome = ContainedTaskGuardOutcome::Passed {
            page_label: observation.page_label.clone(),
            target_id: target.id.clone(),
            target_kind: target_kind_name(target.kind).to_string(),
        };
        Ok((outcome, Some(target)))
    }
}

#[derive(Debug, Deserialize)]
struct OperationGuard {
    page_id: String,
    target_id: String,
    expected_rect: ClickRect,
    #[serde(default)]
    verify_template: Option<String>,
    #[serde(default)]
    color_probe: Option<String>,
}

impl OperationGuard {
    fn validate(
        &self,
        operation: &TaskOperation,
        control: &TaskControl,
    ) -> Result<(), ContainedTaskError> {
        if self.page_id.trim().is_empty()
            || self.target_id.trim().is_empty()
            || !crate::page_anchor_matches(&control.game, &self.page_id, &operation.from)
            || (self.verify_template.is_none() && self.color_probe.is_none())
        {
            return Err(ContainedTaskError::new("contained_task_guard_invalid"));
        }
        self.expected_rect.validate(&control.resolution)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct TaskClick {
    kind: String,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    offset: Option<ClickRect>,
    #[serde(default)]
    from_rect: Option<ClickRect>,
    #[serde(default)]
    to_rect: Option<ClickRect>,
}

impl TaskClick {
    fn validate(
        &self,
        resolution: &Resolution,
        guard: Option<&OperationGuard>,
    ) -> Result<(), ContainedTaskError> {
        match self.kind.as_str() {
            "point" => {
                resolution.validate_point(required(self.x)?, required(self.y)?)?;
            }
            "rect" | "specific_rect" => ClickRect {
                x: required(self.x)?,
                y: required(self.y)?,
                width: required(self.width)?,
                height: required(self.height)?,
            }
            .validate(resolution)?,
            "long_press" | "long_tap" => {
                resolution.validate_point(required(self.x)?, required(self.y)?)?;
                if self.duration_ms == Some(0) || self.duration_ms.is_none() {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
            }
            "drag" => {
                self.from_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?
                    .validate(resolution)?;
                self.to_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?
                    .validate(resolution)?;
                if self.duration_ms == Some(0) || self.duration_ms.is_none() {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
            }
            "target" | "target_center" | "offset" => {
                let guard =
                    guard.ok_or_else(|| ContainedTaskError::new("contained_task_guard_missing"))?;
                if guard.verify_template.is_none()
                    || self
                        .target_id
                        .as_deref()
                        .is_some_and(|target_id| target_id != guard.target_id)
                {
                    return Err(ContainedTaskError::new("contained_task_operation_invalid"));
                }
                if self.kind == "offset" {
                    self.offset
                        .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?
                        .validate_shape()?;
                } else if let Some(offset) = self.offset {
                    offset.validate_shape()?;
                }
            }
            _ => {
                return Err(ContainedTaskError::new(
                    "contained_task_primitive_unsupported",
                ));
            }
        }
        Ok(())
    }

    fn input_action(
        &self,
        resolution: &Resolution,
        target: Option<&TargetEvaluation>,
    ) -> Result<InputAction, ContainedTaskError> {
        let action = match self.kind.as_str() {
            "point" => InputAction::Tap {
                x: required(self.x)?,
                y: required(self.y)?,
            },
            "rect" | "specific_rect" => {
                let rect = ClickRect {
                    x: required(self.x)?,
                    y: required(self.y)?,
                    width: required(self.width)?,
                    height: required(self.height)?,
                };
                rect.validate(resolution)?;
                InputAction::Tap {
                    x: rect.x + rect.width / 2,
                    y: rect.y + rect.height / 2,
                }
            }
            "long_press" | "long_tap" => InputAction::LongTap {
                x: required(self.x)?,
                y: required(self.y)?,
                duration_ms: self
                    .duration_ms
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?,
            },
            "drag" => {
                let from = self
                    .from_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
                let to = self
                    .to_rect
                    .ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))?;
                from.validate(resolution)?;
                to.validate(resolution)?;
                InputAction::Swipe {
                    x1: from.x + from.width / 2,
                    y1: from.y + from.height / 2,
                    x2: to.x + to.width / 2,
                    y2: to.y + to.height / 2,
                    duration_ms: self.duration_ms.ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?,
                }
            }
            "target" | "target_center" | "offset" => {
                let target = target.ok_or_else(|| {
                    ContainedTaskError::new("contained_task_guard_target_missing")
                })?;
                let template = target.template.ok_or_else(|| {
                    ContainedTaskError::new("contained_task_guard_target_invalid")
                })?;
                let mut rect = ClickRect {
                    x: template.x,
                    y: template.y,
                    width: template.width,
                    height: template.height,
                };
                if self.kind == "offset" {
                    let offset = self.offset.ok_or_else(|| {
                        ContainedTaskError::new("contained_task_operation_invalid")
                    })?;
                    rect = ClickRect {
                        x: rect.x + offset.x,
                        y: rect.y + offset.y,
                        width: offset.width,
                        height: offset.height,
                    };
                } else if let Some(offset) = self.offset {
                    rect = ClickRect {
                        x: rect.x + offset.x,
                        y: rect.y + offset.y,
                        width: offset.width,
                        height: offset.height,
                    };
                }
                rect.validate(resolution)?;
                InputAction::Tap {
                    x: rect.x + rect.width / 2,
                    y: rect.y + rect.height / 2,
                }
            }
            _ => {
                return Err(ContainedTaskError::new(
                    "contained_task_primitive_unsupported",
                ));
            }
        };
        action
            .validate()
            .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))?;
        match &action {
            InputAction::Tap { x, y } | InputAction::LongTap { x, y, .. } => {
                resolution.validate_point(*x, *y)?;
            }
            InputAction::Swipe { x1, y1, x2, y2, .. } => {
                resolution.validate_point(*x1, *y1)?;
                resolution.validate_point(*x2, *y2)?;
            }
            _ => {
                return Err(ContainedTaskError::new(
                    "contained_task_primitive_unsupported",
                ));
            }
        }
        Ok(action)
    }
}

fn target_kind_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Template => "template",
        TargetKind::Color => "color",
        TargetKind::ClickOnly => "click_only",
    }
}

fn required<T: Copy>(value: Option<T>) -> Result<T, ContainedTaskError> {
    value.ok_or_else(|| ContainedTaskError::new("contained_task_operation_invalid"))
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct ClickRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl ClickRect {
    fn validate_shape(&self) -> Result<(), ContainedTaskError> {
        if self.width <= 0 || self.height <= 0 {
            Err(ContainedTaskError::new("contained_task_operation_invalid"))
        } else {
            Ok(())
        }
    }

    fn validate(&self, resolution: &Resolution) -> Result<(), ContainedTaskError> {
        self.validate_shape()?;
        resolution.validate_point(self.x, self.y)?;
        resolution.validate_point(self.x + self.width - 1, self.y + self.height - 1)
    }
}

fn scene_from_frame(frame: &Frame) -> Result<Scene, ContainedTaskError> {
    let format = match frame.pixel_format {
        PixelFormat::Rgb8 => ScenePixelFormat::Rgb8,
        PixelFormat::Rgba8 => ScenePixelFormat::Rgba8,
    };
    Scene::from_pixels(frame.width, frame.height, &frame.pixels, format)
        .map_err(|_| ContainedTaskError::new("contained_task_frame_invalid"))
}

#[cfg(test)]
mod retry_wiring_tests {
    use super::*;
    use actingcommand_device::CaptureBackendName;
    use actingcommand_page_detector::PageSet;
    use actingcommand_recognition_pack::RecognitionPack;
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::path::PathBuf;

    fn control() -> TaskControl {
        serde_json::from_value(json!({
            "schema_version": "Lab-1y.control.v1",
            "package_id": "neutral.semantic.task",
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 2, "height": 1},
            "entry_task_id": "task",
            "capture_interval_ms": 1,
            "step_timeout_ms": 2
        }))
        .expect("task control")
    }

    fn operation(retry: Value, on_error: Option<&str>) -> TaskOperation {
        let mut value = json!({
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        });
        if let Value::Object(fields) = retry {
            value
                .as_object_mut()
                .expect("operation object")
                .extend(fields);
        }
        if let Some(on_error) = on_error {
            value["on_error"] = Value::String(on_error.to_string());
        }
        serde_json::from_value(value).expect("task operation")
    }

    fn scheduling_declaration(value: Value) -> SchedulingOutcomeDeclaration {
        serde_json::from_value(value).expect("scheduling outcome declaration")
    }

    #[test]
    fn scheduling_outcome_coverage_requires_only_reachable_terminal_conditions() {
        let operations = vec![operation(json!({}), None)];
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned(), "alternate".to_owned()],
            &[
                "neutral/home".to_owned(),
                "neutral/terminal".to_owned(),
                "neutral/alternate".to_owned(),
            ],
            &operations,
            &declaration,
        )
        .expect_err("every initial target is a reachable no-effect terminal");

        let incomplete = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "wrong-page",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["home"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &operations,
            &incomplete,
        )
        .expect_err("reachable effect terminal must be covered");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );

        let complete = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminals",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal", "alternate"]
                }
            ]
        }));
        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned(), "alternate".to_owned()],
            &[
                "neutral/home".to_owned(),
                "neutral/terminal".to_owned(),
                "neutral/alternate".to_owned(),
            ],
            &operations,
            &complete,
        )
        .expect("unreachable designated-effect alternate terminal is not required");
    }

    #[test]
    fn scheduling_outcome_coverage_accepts_unique_effect_and_no_effect_paths() {
        let designated = operation(json!({}), None);
        let ordinary: TaskOperation = serde_json::from_value(json!({
            "id": "ordinary_terminal",
            "from": "alternate",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("ordinary operation");
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &[
                "neutral/home".to_owned(),
                "neutral/alternate".to_owned(),
                "neutral/terminal".to_owned(),
            ],
            &[designated, ordinary],
            &declaration,
        )
        .expect("each mechanically reachable terminal condition has one mapping");
    }

    #[test]
    fn scheduling_outcome_coverage_uses_formal_operation_precedence() {
        let ordinary: TaskOperation = serde_json::from_value(json!({
            "id": "ordinary_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("ordinary operation");
        let mut shadowed_designated = operation(json!({}), None);
        shadowed_designated.to = None;
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[ordinary, shadowed_designated],
            &declaration,
        )
        .expect("a later same-page operation is unreachable under first-specific selection");
    }

    #[test]
    fn scheduling_outcome_coverage_does_not_treat_any_as_an_observable_page() {
        let designated = operation(json!({}), None);
        let fallback: TaskOperation = serde_json::from_value(json!({
            "id": "unreachable_fallback",
            "from": "any",
            "to": null,
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("fallback operation");
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));

        validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[designated, fallback],
            &declaration,
        )
        .expect("the fallback is shadowed across the complete observable page domain");
    }

    #[test]
    fn scheduling_outcome_coverage_canonicalizes_destination_anchors() {
        let first: TaskOperation = serde_json::from_value(json!({
            "id": "open_home",
            "from": "neutral/start",
            "to": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("first operation");
        let second: TaskOperation = serde_json::from_value(json!({
            "id": "open_terminal",
            "from": "neutral/home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }))
        .expect("second operation");
        let complete = scheduling_declaration(json!({
            "designated_operation": "open_home",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["neutral/terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["neutral/terminal"]
                }
            ]
        }));
        let observable_pages = [
            "neutral/start".to_owned(),
            "neutral/home".to_owned(),
            "neutral/terminal".to_owned(),
        ];

        validate_scheduling_outcome_coverage(
            "neutral",
            &["neutral/terminal".to_owned()],
            &observable_pages,
            &[first, second],
            &complete,
        )
        .expect("short destinations resolve to the unique concrete detector page");

        let missing_reachable_terminal = scheduling_declaration(json!({
            "designated_operation": "open_home",
            "mappings": [
                {
                    "outcome_key": "wrong-effect-page",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["neutral/home"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["neutral/terminal"]
                }
            ]
        }));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["neutral/terminal".to_owned()],
            &observable_pages,
            &[
                serde_json::from_value(json!({
                    "id": "open_home",
                    "from": "neutral/start",
                    "to": "home",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true
                }))
                .expect("first operation"),
                serde_json::from_value(json!({
                    "id": "open_terminal",
                    "from": "neutral/home",
                    "to": "terminal",
                    "click": {"kind": "point", "x": 1, "y": 0},
                    "unguarded_trusted_coordinate": true
                }))
                .expect("second operation"),
            ],
            &missing_reachable_terminal,
        )
        .expect_err("the concrete intermediate page must expose the reachable terminal");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );
    }

    #[test]
    fn scheduling_outcome_coverage_rejects_unknown_and_repeated_effect_paths() {
        let mut unknown = operation(json!({}), None);
        unknown.to = None;
        let declaration = scheduling_declaration(json!({
            "designated_operation": "open_terminal",
            "mappings": [
                {
                    "outcome_key": "effect-terminal",
                    "effect": "designated_effect_completed",
                    "terminal_pages": ["terminal"]
                },
                {
                    "outcome_key": "no-effect-terminal",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }
            ]
        }));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[unknown],
            &declaration,
        )
        .expect_err("mapped operation without a finite postcondition must fail admission");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );

        let mut cycle = operation(json!({}), None);
        cycle.to = Some(PageDeclaration::Singleton("home".to_owned()));
        let error = validate_scheduling_outcome_coverage(
            "neutral",
            &["terminal".to_owned()],
            &["neutral/home".to_owned(), "neutral/terminal".to_owned()],
            &[cycle],
            &declaration,
        )
        .expect_err("a reachable second designated effect must fail admission");
        assert_eq!(
            error.code(),
            "contained_task_outcome_declaration_incomplete"
        );
    }

    #[test]
    fn recognize_only_rejects_scheduling_outcome_at_admission() {
        let mut recognize_only = control();
        recognize_only.execution_mode = "recognize_only".to_owned();

        let error = validate_scheduling_outcome_execution_mode(&recognize_only)
            .expect_err("recognize-only has no finite declared terminal-page closure");

        assert_eq!(error.code(), "contained_task_outcome_declaration_invalid");
    }

    fn omitted_policy_task(with_destination: bool, with_error_page: bool) -> PreparedContainedTask {
        let control = control();
        let mut task_operation = operation(json!({}), None);
        if !with_destination {
            task_operation.to = None;
        }
        task_operation.unguarded_trusted_coordinate = false;
        task_operation.guard = Some(
            serde_json::from_value(json!({
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            }))
            .expect("operation guard"),
        );
        task_operation
            .validate(&control, TaskOperationDefaults::default())
            .expect("valid omitted-policy operation");
        let program = TaskProgram {
            schema_version: "0.6".to_string(),
            task_id: "task".to_string(),
            game: "neutral".to_string(),
            server_scope: vec!["test".to_string()],
            coordinate_space: control.resolution,
            target_page: Some(PageDeclaration::Singleton("terminal".to_string())),
            error_pages: if with_error_page {
                vec!["error".to_string()]
            } else {
                Vec::new()
            },
            scheduling_outcome: None,
            recovery: None,
            defaults: TaskOperationDefaults::default(),
            operations: vec![task_operation],
        };
        let pack: RecognitionPack = serde_json::from_value(json!({
            "schema_version": "0.3",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 2, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                {
                    "type": "color",
                    "id": "page/home",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 0, 0]
                },
                {
                    "type": "color",
                    "id": "page/terminal",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [0, 0, 255]
                },
                {
                    "type": "color",
                    "id": "page/alternate",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 0, 255]
                },
                {
                    "type": "color",
                    "id": "page/error",
                    "region": {"x": 0, "y": 0, "width": 1, "height": 1},
                    "expected": [255, 255, 0]
                },
                {
                    "type": "color",
                    "id": "guard/ready",
                    "region": {"x": 1, "y": 0, "width": 1, "height": 1},
                    "expected": [0, 255, 0]
                }
            ]
        }))
        .expect("recognition pack");
        let evaluator =
            RecognitionEvaluator::new(PathBuf::new(), pack).expect("recognition evaluator");
        let page_set: PageSet = serde_json::from_value(json!({
            "schema_version": "0.3",
            "pages": [
                {"id": "neutral/home", "required": ["page/home"]},
                {"id": "neutral/terminal", "required": ["page/terminal"]},
                {"id": "neutral/alternate", "required": ["page/alternate"]},
                {"id": "neutral/error", "required": ["page/error"]}
            ]
        }))
        .expect("page set");
        let detector = PageDetector::new(page_set).expect("page detector");
        detector
            .validate(&evaluator)
            .expect("page detector targets");
        PreparedContainedTask {
            control,
            program,
            evaluator,
            detector,
            scheduling_outcome: None,
            package_sha256: "fixture-sha256".to_string(),
            entry_count: 5,
            task_count: 1,
        }
    }

    fn page_frame(page: &str) -> Frame {
        let page_color = match page {
            "home" => [255, 0, 0],
            "terminal" => [0, 0, 255],
            "alternate" => [255, 0, 255],
            "error" => [255, 255, 0],
            _ => panic!("unknown fixture page"),
        };
        Frame::from_pixels(
            2,
            1,
            [page_color, [0, 255, 0]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("fixture frame")
    }

    fn unrecognized_frame() -> Frame {
        Frame::from_pixels(
            2,
            1,
            [0, 0, 0, 0, 255, 0].to_vec(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("unrecognized fixture frame")
    }

    struct ScriptedRuntime {
        frames: VecDeque<Frame>,
        last_frame: Frame,
        captures: usize,
        inputs: usize,
        traces: Vec<ContainedTaskTrace>,
    }

    impl ScriptedRuntime {
        fn new(after_effect_page: &str) -> Self {
            Self::from_pages("home", after_effect_page)
        }

        fn from_pages(initial_page: &str, after_effect_page: &str) -> Self {
            let last_frame = page_frame(after_effect_page);
            Self {
                frames: [page_frame(initial_page), last_frame.clone()].into(),
                last_frame,
                captures: 0,
                inputs: 0,
                traces: Vec::new(),
            }
        }
    }

    impl ContainedTaskRuntime for ScriptedRuntime {
        type Error = &'static str;

        fn capture(&mut self) -> Result<Frame, Self::Error> {
            self.captures += 1;
            Ok(match self.frames.pop_front() {
                Some(frame) => frame,
                None => self.last_frame.clone(),
            })
        }

        fn input(&mut self, _action: InputAction) -> Result<(), Self::Error> {
            self.inputs += 1;
            Ok(())
        }

        fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
            self.traces.push(trace);
            Ok(())
        }
    }

    fn run_omitted_policy(
        with_destination: bool,
        with_error_page: bool,
        after_effect_page: &str,
    ) -> (
        Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
        ScriptedRuntime,
    ) {
        let task = omitted_policy_task(with_destination, with_error_page);
        let mut runtime = ScriptedRuntime::new(after_effect_page);
        let result = task.run(&mut runtime);
        (result, runtime)
    }

    fn assert_single_effect(runtime: &ScriptedRuntime) {
        assert_eq!(runtime.inputs, 1);
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::EffectIntent { .. }))
                .count(),
            1
        );
        assert_eq!(
            runtime
                .traces
                .iter()
                .filter(|trace| matches!(trace, ContainedTaskTrace::EffectCompleted { .. }))
                .count(),
            1
        );
    }

    fn assert_closed_effect_attempts(runtime: &ScriptedRuntime, attempts: usize) {
        let sequence = runtime
            .traces
            .iter()
            .filter_map(|trace| match trace {
                ContainedTaskTrace::StepStarted { .. } => Some("step_started"),
                ContainedTaskTrace::EffectIntent { .. } => Some("effect_intent"),
                ContainedTaskTrace::EffectCompleted { .. } => Some("effect_completed"),
                ContainedTaskTrace::StepFinished { .. } => Some("step_finished"),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            sequence,
            [
                "step_started",
                "effect_intent",
                "effect_completed",
                "step_finished",
            ]
            .repeat(attempts)
        );
    }

    fn assert_page_confirmation_failed(
        result: Result<ContainedTaskOutcome, ContainedTaskRunError<&'static str>>,
        after_page: &str,
    ) {
        let error = match result.expect_err("destination confirmation must fail") {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };
        assert_eq!(error.code(), "page_confirmation_failed");
        assert!(
            error
                .detail()
                .is_some_and(|detail| detail.contains(&format!("after_page=neutral/{after_page}")))
        );
    }

    #[test]
    fn omitted_policy_destination_reached_succeeds_after_fresh_observation() {
        let (result, runtime) = run_omitted_policy(true, false, "terminal");
        let outcome = result.expect("reached destination");

        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn destination_without_expect_after_uses_the_configured_bounded_wait() {
        let mut task = omitted_policy_task(true, false);
        task.control.step_timeout_ms = Some(50);
        assert!(task.program.operations[0].expect_after.is_none());
        let terminal = page_frame("terminal");
        let mut runtime = ScriptedRuntime {
            frames: [page_frame("home"), page_frame("home"), terminal.clone()].into(),
            last_frame: terminal,
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        };

        let outcome = task
            .run(&mut runtime)
            .expect("bounded wait must observe the later destination");

        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_eq!(runtime.captures, 3);
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn omitted_policy_unchanged_page_fails_once_without_retry() {
        let (result, runtime) = run_omitted_policy(true, false, "home");

        assert_page_confirmation_failed(result, "home");
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn omitted_policy_declared_error_page_fails_once_without_recovery() {
        let (result, runtime) = run_omitted_policy(true, true, "error");

        assert_page_confirmation_failed(result, "error");
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn omitted_policy_without_destination_preserves_direct_success() {
        let (result, runtime) = run_omitted_policy(false, false, "terminal");
        let outcome = result.expect("operation without destination");

        assert_eq!(outcome.outcome, TaskOutcome::Success);
        assert_eq!(outcome.final_page.as_deref(), Some("neutral/terminal"));
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn destination_set_accepts_each_declared_fresh_page() {
        for destination in ["terminal", "alternate"] {
            let mut task = omitted_policy_task(true, false);
            task.program.operations[0].to = Some(PageDeclaration::Set(vec![
                "terminal".to_string(),
                "alternate".to_string(),
            ]));
            task.program.target_page = Some(PageDeclaration::Set(vec![
                "terminal".to_string(),
                "alternate".to_string(),
            ]));
            let mut runtime = ScriptedRuntime::new(destination);
            let outcome = task.run(&mut runtime).expect("declared destination");

            assert_eq!(
                outcome.final_page.as_deref(),
                Some(format!("neutral/{destination}").as_str())
            );
            assert_single_effect(&runtime);
        }
    }

    #[test]
    fn expect_after_is_the_canonical_postcondition_when_to_is_null() {
        let mut task = omitted_policy_task(true, false);
        let operation = &mut task.program.operations[0];
        operation.to = None;
        operation.expect_after = Some(
            serde_json::from_value(json!({
                "page_id": ["terminal", "alternate"],
                "timeout_ms": 2,
                "interval_ms": 1
            }))
            .expect("expect_after"),
        );
        task.program.target_page = Some(PageDeclaration::Set(vec![
            "terminal".to_string(),
            "alternate".to_string(),
        ]));
        let mut runtime = ScriptedRuntime::new("alternate");
        let outcome = task.run(&mut runtime).expect("expect_after destination");

        assert_eq!(outcome.final_page.as_deref(), Some("neutral/alternate"));
        assert_single_effect(&runtime);
    }

    #[test]
    fn every_declared_terminal_page_completes_through_run_state() {
        for terminal in ["terminal", "alternate"] {
            let mut task = omitted_policy_task(true, false);
            task.program.target_page = Some(PageDeclaration::Set(vec![
                "terminal".to_string(),
                "alternate".to_string(),
            ]));
            let mut runtime = ScriptedRuntime::from_pages(terminal, terminal);
            let outcome = task.run(&mut runtime).expect("terminal page");

            assert_eq!(
                outcome.final_page.as_deref(),
                Some(format!("neutral/{terminal}").as_str())
            );
            assert_eq!(runtime.inputs, 0);
            assert_eq!(runtime.captures, 1);
        }
    }

    #[test]
    fn page_set_declarations_fail_closed_at_admission() {
        for invalid in [
            PageDeclaration::Set(Vec::new()),
            PageDeclaration::Set(vec!["terminal".to_string(), "terminal".to_string()]),
            PageDeclaration::Set(vec!["".to_string()]),
        ] {
            assert_eq!(
                invalid.normalized().expect_err("invalid page set").code(),
                "contained_task_page_set_invalid"
            );
        }

        let task = omitted_policy_task(true, false);
        let missing = vec!["missing".to_string()];
        assert_eq!(
            validate_page_references(&task.control.game, &missing, &task.detector)
                .expect_err("missing page reference")
                .code(),
            "contained_task_page_set_invalid"
        );
    }

    #[test]
    fn conflicting_destination_declarations_fail_admission() {
        let mut operation = operation(json!({}), None);
        operation.expect_after =
            Some(serde_json::from_value(json!({"page_id": "alternate"})).expect("expect_after"));
        assert_eq!(
            operation
                .destination_pages()
                .expect_err("conflicting destinations")
                .code(),
            "contained_task_destination_conflict"
        );
    }

    #[test]
    fn destination_error_overlap_fails_before_execution() {
        let task = omitted_policy_task(true, true);
        assert_eq!(
            validate_page_set_overlap(
                &task.control.game,
                &["error".to_string()],
                &task.program.error_pages,
                &task.detector,
            )
            .expect_err("destination/error overlap")
            .code(),
            "contained_task_page_set_invalid"
        );
    }

    #[test]
    fn alias_overlap_within_destination_set_fails_admission() {
        let task = omitted_policy_task(true, false);
        assert_eq!(
            validate_page_references(
                &task.control.game,
                &["terminal".to_string(), "neutral/terminal".to_string()],
                &task.detector,
            )
            .expect_err("ambiguous aliases")
            .code(),
            "contained_task_page_set_invalid"
        );
    }

    #[test]
    fn operation_without_explicit_retry_policy_preserves_non_retry_behavior() {
        assert!(
            operation(json!({}), None)
                .retry_policy(TaskOperationDefaults::default(), 100)
                .expect("absent retry policy")
                .is_none()
        );
    }

    #[test]
    fn explicit_retry_policy_uses_existing_bounded_decision_owner() {
        let operation = operation(
            json!({
                "retryable": true,
                "max_attempts": 6,
                "retry_interval_ms": 1
            }),
            None,
        );
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("retry policy")
            .expect("explicit retry policy");
        for attempt in 1..=5 {
            assert_eq!(
                operation
                    .failure_decision(
                        &policy,
                        attempt,
                        "page_confirmation_failed",
                        Some("home".to_string()),
                        RunFailureStage::PostExecution {
                            hit_error_page: false,
                        },
                    )
                    .expect("retry decision"),
                RunOperationFailureDecision::Retry {
                    next_attempt: attempt + 1,
                    delay_ms: 1,
                }
            );
        }
        assert!(matches!(
            operation
                .failure_decision(
                    &policy,
                    6,
                    "page_confirmation_failed",
                    Some("home".to_string()),
                    RunFailureStage::PostExecution {
                        hit_error_page: false,
                    },
                )
                .expect("final decision"),
            RunOperationFailureDecision::Fail(_)
        ));
    }

    #[test]
    fn every_failed_retry_attempt_finishes_before_the_next_attempt_starts() {
        let mut task = omitted_policy_task(true, false);
        let operation = &mut task.program.operations[0];
        operation.retryable = Some(true);
        operation.max_attempts = Some(6);
        operation.retry_interval_ms = Some(1);
        let mut runtime = ScriptedRuntime::new("home");

        let error = match task
            .run(&mut runtime)
            .expect_err("sixth failed attempt must stop")
        {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };

        assert_eq!(error.code(), "contained_task_requires_scheduler");
        assert_eq!(runtime.inputs, 6);
        assert_closed_effect_attempts(&runtime, 6);
    }

    #[test]
    fn unrecognized_fresh_retry_observation_closes_without_second_effect() {
        let mut task = omitted_policy_task(true, false);
        let operation = &mut task.program.operations[0];
        operation.retryable = Some(true);
        operation.max_attempts = Some(6);
        operation.retry_interval_ms = Some(1);
        let unknown = unrecognized_frame();
        let mut runtime = ScriptedRuntime {
            frames: [page_frame("home"), page_frame("home"), unknown.clone()].into(),
            last_frame: unknown,
            captures: 0,
            inputs: 0,
            traces: Vec::new(),
        };

        let error = match task
            .run(&mut runtime)
            .expect_err("unrecognized fresh observation must stop")
        {
            ContainedTaskRunError::Task(error) => error,
            ContainedTaskRunError::Boundary(error) => {
                panic!("unexpected fixture boundary error: {error}")
            }
        };

        assert_eq!(error.code(), "page_confirmation_failed");
        assert_single_effect(&runtime);
        assert_closed_effect_attempts(&runtime, 1);
    }

    #[test]
    fn explicit_retry_policy_consumes_existing_task_defaults() {
        let operation = operation(json!({"retryable": true}), None);
        let policy = operation
            .retry_policy(
                TaskOperationDefaults {
                    max_attempts: Some(3),
                    retry_interval_ms: Some(1),
                },
                100,
            )
            .expect("retry policy from task defaults")
            .expect("explicit retry policy");

        assert!(policy.retryable());
        assert_eq!(policy.max_attempts(), 3);
        assert_eq!(policy.retry_interval_ms(), 1);
    }

    #[test]
    fn non_retryable_and_invalid_policies_fail_closed() {
        let non_retryable = operation(json!({"retryable": false}), None);
        let policy = non_retryable
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("non-retryable policy")
            .expect("explicit non-retryable policy");
        assert!(matches!(
            non_retryable
                .failure_decision(
                    &policy,
                    1,
                    "page_confirmation_failed",
                    Some("home".to_string()),
                    RunFailureStage::PostExecution {
                        hit_error_page: false,
                    },
                )
                .expect("non-retryable decision"),
            RunOperationFailureDecision::Fail(_)
        ));

        for invalid in [
            json!({"retryable": true}),
            json!({"retryable": true, "max_attempts": 0, "retry_interval_ms": 1}),
            json!({"retryable": true, "max_attempts": 2, "retry_interval_ms": 101}),
            json!({"max_attempts": 2, "retry_interval_ms": 1}),
        ] {
            assert_eq!(
                operation(invalid, None)
                    .retry_policy(TaskOperationDefaults::default(), 100)
                    .expect_err("invalid retry policy")
                    .code(),
                "contained_task_operation_invalid"
            );
        }
    }

    #[test]
    fn explicit_non_retryable_operation_without_destination_remains_valid() {
        let operation: TaskOperation = serde_json::from_value(json!({
            "id": "record_observation",
            "from": "home",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "retryable": false
        }))
        .expect("non-retryable operation");

        operation
            .validate(&control(), TaskOperationDefaults::default())
            .expect("explicitly non-retryable operation without to");
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("non-retryable policy")
            .expect("explicit policy");
        assert!(!policy.retryable());
        assert_eq!(policy.max_attempts(), 1);
    }

    #[test]
    fn declared_error_page_requests_recovery_without_ordinary_retry() {
        let program: TaskProgram = serde_json::from_value(json!({
            "schema_version": "0.6",
            "task_id": "task",
            "game": "neutral",
            "coordinate_space": {"width": 2, "height": 1},
            "error_pages": ["error"],
            "operations": [{
                "id": "open_terminal",
                "from": "home",
                "to": "terminal",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true
            }]
        }))
        .expect("task program");
        let operation = operation(
            json!({
                "retryable": true,
                "max_attempts": 6,
                "retry_interval_ms": 1
            }),
            Some("return_home"),
        );
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("retry policy")
            .expect("explicit retry policy");
        let hit_error_page = program.is_error_page(&control(), "neutral/error");

        assert!(hit_error_page);
        assert!(matches!(
            operation
                .failure_decision(
                    &policy,
                    1,
                    "page_confirmation_failed",
                    Some("neutral/error".to_string()),
                    RunFailureStage::PostExecution { hit_error_page },
                )
                .expect("error-page decision"),
            RunOperationFailureDecision::RequestRecovery(trigger)
                if trigger.operation_id == "open_terminal"
                    && trigger.attempts == 1
                    && trigger.after_page.as_deref() == Some("neutral/error")
                    && trigger.recovery_task_id == "return_home"
        ));
    }

    #[test]
    fn final_retry_decision_preserves_existing_recovery_path() {
        let operation = operation(
            json!({
                "retryable": true,
                "max_attempts": 2,
                "retry_interval_ms": 1
            }),
            Some("return_home"),
        );
        let policy = operation
            .retry_policy(TaskOperationDefaults::default(), 100)
            .expect("recovery policy")
            .expect("explicit recovery policy");
        assert!(matches!(
            operation
                .failure_decision(
                    &policy,
                    2,
                    "page_confirmation_failed",
                    Some("home".to_string()),
                    RunFailureStage::PostExecution {
                        hit_error_page: false,
                    },
                )
                .expect("recovery decision"),
            RunOperationFailureDecision::RequestRecovery(trigger)
                if trigger.operation_id == "open_terminal"
                    && trigger.attempts == 2
                    && trigger.recovery_task_id == "return_home"
        ));
    }
}
