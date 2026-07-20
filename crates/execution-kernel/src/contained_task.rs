// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned admission and execution for contained semantic task packages.

use crate::{
    ExecutionBundleError, ExternalExpectedSha256, ExternallyVerifiedBundle, RunDirective,
    RunOperationCandidate, RunStateConfig, RunStateMachine, RunTerminal,
};
use actingcommand_contract::{InputAction, TaskOutcome};
use actingcommand_device::{Frame, PixelFormat};
use actingcommand_pack_containment::{
    AdmittedAction, AdmittedOperation, AdmittedPackage, ContainmentError, ExecutionMode,
    PackageResolution, PageSelector,
};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::{TargetEvaluation, TargetKind};
use serde::Serialize;
use std::error::Error;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

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
    package: AdmittedPackage,
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
            .map_err(contained_task_admission_error)?;
        Self::from_verified_bundle(&bundle)
    }

    /// Prepares an already admitted package without parsing or trusting the ZIP a second time.
    pub fn from_verified_bundle(
        admitted: &ExternallyVerifiedBundle,
    ) -> Result<Self, ContainedTaskError> {
        Ok(Self {
            package: admitted.admitted_package().clone(),
            package_sha256: admitted.package_sha256().to_string(),
            entry_count: admitted.entry_count(),
            task_count: admitted.task_count(),
        })
    }

    pub fn task_label(&self) -> &str {
        self.package.control().entry_task().as_str()
    }

    pub fn package_label(&self) -> &str {
        self.package.control().package_id()
    }

    pub fn package_sha256(&self) -> &str {
        &self.package_sha256
    }

    pub fn semantic_fingerprint(&self) -> &str {
        self.package.semantic_fingerprint()
    }

    pub fn execution_mode(&self) -> &str {
        self.package.control().execution_mode().as_str()
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
        let control = self.package.control();
        let program = self.package.entry_task();
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

        let capture_interval = Duration::from_millis(control.capture_interval_ms());
        let step_timeout = Duration::from_millis(control.step_timeout_ms());
        let task_timeout = Duration::from_millis(control.timeout_ms());
        let started = Instant::now();
        let mut observation = self.capture_until_page(runtime, step_timeout, capture_interval)?;
        if control.execution_mode() == ExecutionMode::RecognizeOnly {
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
            .package
            .entry_task()
            .operations()
            .iter()
            .map(|operation| {
                RunOperationCandidate::new(
                    operation.key().operation(),
                    page_selector_label(operation.from()),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let config = RunStateConfig::new(
            control.game(),
            program.target_page().map(ToString::to_string),
            control.stop_on_confirmation(),
            1,
            control.max_steps(),
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
                        .package
                        .entry_task()
                        .operations()
                        .iter()
                        .find(|candidate| candidate.key().operation() == operation_id)
                        .ok_or_else(|| {
                            ContainedTaskError::new("contained_task_operation_missing")
                        })?;
                    runtime
                        .record(ContainedTaskTrace::StepStarted {
                            step_index,
                            operation_label: operation_id.clone(),
                            from_page,
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    let (guard, target) = guard_outcome(
                        operation,
                        control.game(),
                        &observation,
                        self.package.evaluator(),
                    )?;
                    let action = input_action_from_admitted(
                        operation.action(),
                        control.resolution(),
                        target.as_ref(),
                    )?;
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
                    observation =
                        self.capture_until_page(runtime, step_timeout, capture_interval)?;
                    let observed_page = observation.page_label.clone();
                    runtime
                        .record(ContainedTaskTrace::StepFinished {
                            step_index,
                            operation_label: operation_id.clone(),
                            page_label: observed_page,
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    machine
                        .operation_succeeded(&operation_id, Some(observation.page_label.clone()))
                        .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
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

    fn capture_until_page<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
        timeout: Duration,
        interval: Duration,
    ) -> Result<PageObservation, ContainedTaskRunError<R::Error>> {
        let started = Instant::now();
        loop {
            let frame = runtime.capture().map_err(ContainedTaskRunError::Boundary)?;
            validate_frame_resolution(self.package.control().resolution(), &frame)?;
            runtime
                .record(ContainedTaskTrace::CaptureCompleted {
                    width: frame.width,
                    height: frame.height,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            let scene = scene_from_frame(&frame)?;
            let candidate_pages = self
                .package
                .pages()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            runtime
                .record(ContainedTaskTrace::RecognitionStarted {
                    candidate_pages: candidate_pages.clone(),
                    width: frame.width,
                    height: frame.height,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            let mut matched_pages = self
                .package
                .detector()
                .evaluate_all(self.package.evaluator(), &scene)
                .map_err(|error| {
                    ContainedTaskError::with_detail(
                        "contained_task_recognition_failed",
                        error.to_string(),
                    )
                })?
                .into_iter()
                .filter(|evaluation| evaluation.matched)
                .map(|evaluation| {
                    self.package
                        .page_key_for_detector_id(&evaluation.page_id)
                        .map(ToString::to_string)
                        .ok_or_else(|| {
                            ContainedTaskError::with_detail(
                                "contained_task_recognition_failed",
                                format!(
                                    "page detector returned non-admitted page '{}'",
                                    evaluation.page_id
                                ),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            matched_pages.sort();
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
            if let Some(page_label) = page {
                return Ok(PageObservation { page_label, scene });
            }
            if started.elapsed() >= timeout {
                return Err(ContainedTaskError::new("contained_task_page_unknown").into());
            }
            thread::sleep(interval);
        }
    }
}

struct PageObservation {
    page_label: String,
    scene: Scene,
}

fn input_action_from_admitted(
    admitted: &AdmittedAction,
    resolution: PackageResolution,
    target: Option<&TargetEvaluation>,
) -> Result<InputAction, ContainedTaskError> {
    let action = match admitted {
        AdmittedAction::Tap { point, .. } => InputAction::Tap {
            x: point.x(),
            y: point.y(),
        },
        AdmittedAction::LongTap { point, duration } => InputAction::LongTap {
            x: point.x(),
            y: point.y(),
            duration_ms: duration.milliseconds(),
        },
        AdmittedAction::Drag {
            from, to, duration, ..
        } => InputAction::Swipe {
            x1: from.x(),
            y1: from.y(),
            x2: to.x(),
            y2: to.y(),
            duration_ms: duration.milliseconds(),
        },
        AdmittedAction::TargetTap { offset, .. } => {
            let target = target
                .ok_or_else(|| ContainedTaskError::new("contained_task_guard_target_missing"))?;
            let template = target
                .template
                .ok_or_else(|| ContainedTaskError::new("contained_task_guard_target_invalid"))?;
            let (x, y, width, height) = match offset {
                Some(offset) => (
                    template.x.checked_add(offset.x()).ok_or_else(|| {
                        ContainedTaskError::new("contained_task_input_out_of_bounds")
                    })?,
                    template.y.checked_add(offset.y()).ok_or_else(|| {
                        ContainedTaskError::new("contained_task_input_out_of_bounds")
                    })?,
                    offset.width(),
                    offset.height(),
                ),
                None => (template.x, template.y, template.width, template.height),
            };
            validate_dynamic_rect(resolution, x, y, width, height)?;
            InputAction::Tap {
                x: (i64::from(x) + i64::from(width / 2)) as i32,
                y: (i64::from(y) + i64::from(height / 2)) as i32,
            }
        }
    };
    action
        .validate()
        .map_err(|_| ContainedTaskError::new("contained_task_operation_invalid"))?;
    validate_input_action(resolution, &action)?;
    Ok(action)
}

fn target_kind_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Template => "template",
        TargetKind::Color => "color",
        TargetKind::ClickOnly => "click_only",
    }
}

fn page_selector_label(selector: &PageSelector) -> &str {
    match selector {
        PageSelector::Any => "any",
        PageSelector::Exact(page) => page.page(),
    }
}

fn guard_outcome(
    operation: &AdmittedOperation,
    game: &str,
    observation: &PageObservation,
    evaluator: &actingcommand_recognition_pack::RecognitionEvaluator,
) -> Result<(ContainedTaskGuardOutcome, Option<TargetEvaluation>), ContainedTaskError> {
    if operation.unguarded_trusted_coordinate() {
        return Ok((ContainedTaskGuardOutcome::TrustedCoordinate, None));
    }
    let guard = operation
        .guard()
        .ok_or_else(|| ContainedTaskError::new("contained_task_guard_missing"))?;
    if !crate::page_anchor_matches(game, &observation.page_label, guard.page().page()) {
        return Err(ContainedTaskError::with_detail(
            "contained_task_guard_refused",
            format!(
                "operation={} expected_page={} observed_page={}",
                operation.key().operation(),
                guard.page(),
                observation.page_label
            ),
        ));
    }
    let target = evaluator
        .evaluate_target(&observation.scene, guard.target().as_str())
        .map_err(|error| {
            ContainedTaskError::with_detail(
                "contained_task_guard_evaluation_failed",
                error.to_string(),
            )
        })?;
    if !target.passed {
        return Err(ContainedTaskError::with_detail(
            "contained_task_guard_refused",
            format!(
                "operation={} target={}",
                operation.key().operation(),
                guard.target()
            ),
        ));
    }
    let outcome = ContainedTaskGuardOutcome::Passed {
        page_label: observation.page_label.clone(),
        target_id: target.id.clone(),
        target_kind: target_kind_name(target.kind).to_string(),
    };
    Ok((outcome, Some(target)))
}

fn validate_frame_resolution(
    resolution: PackageResolution,
    frame: &Frame,
) -> Result<(), ContainedTaskError> {
    if frame.width == resolution.width() && frame.height == resolution.height() {
        Ok(())
    } else {
        Err(ContainedTaskError::new(
            "contained_task_frame_resolution_mismatch",
        ))
    }
}

fn validate_dynamic_point(
    resolution: PackageResolution,
    x: i32,
    y: i32,
) -> Result<(), ContainedTaskError> {
    if x < 0 || y < 0 || x as u32 >= resolution.width() || y as u32 >= resolution.height() {
        Err(ContainedTaskError::new(
            "contained_task_input_out_of_bounds",
        ))
    } else {
        Ok(())
    }
}

fn validate_dynamic_rect(
    resolution: PackageResolution,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), ContainedTaskError> {
    if width <= 0 || height <= 0 {
        return Err(ContainedTaskError::new(
            "contained_task_input_out_of_bounds",
        ));
    }
    let right = x
        .checked_add(width - 1)
        .ok_or_else(|| ContainedTaskError::new("contained_task_input_out_of_bounds"))?;
    let bottom = y
        .checked_add(height - 1)
        .ok_or_else(|| ContainedTaskError::new("contained_task_input_out_of_bounds"))?;
    validate_dynamic_point(resolution, x, y)?;
    validate_dynamic_point(resolution, right, bottom)
}

fn validate_input_action(
    resolution: PackageResolution,
    action: &InputAction,
) -> Result<(), ContainedTaskError> {
    match action {
        InputAction::Tap { x, y } | InputAction::LongTap { x, y, .. } => {
            validate_dynamic_point(resolution, *x, *y)
        }
        InputAction::Swipe { x1, y1, x2, y2, .. } => {
            validate_dynamic_point(resolution, *x1, *y1)?;
            validate_dynamic_point(resolution, *x2, *y2)
        }
        _ => Err(ContainedTaskError::new(
            "contained_task_primitive_unsupported",
        )),
    }
}

fn contained_task_admission_error(error: ExecutionBundleError) -> ContainedTaskError {
    match error {
        ExecutionBundleError::Containment(ContainmentError::Admission(error)) => {
            let detail = error.detail().map(str::to_string);
            let code = match (error.code(), error.detail()) {
                ("admission_guard_invalid", _) => "contained_task_guard_missing",
                ("admission_missing_reference", Some(detail))
                    if detail.contains("references missing task") =>
                {
                    "contained_task_recovery_missing"
                }
                _ => "contained_task_admission_failed",
            };
            ContainedTaskError { code, detail }
        }
        other => {
            ContainedTaskError::with_detail("contained_task_admission_failed", other.to_string())
        }
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
