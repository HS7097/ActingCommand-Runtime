// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-owned admission and execution for contained semantic task packages.

use crate::{
    ExternalExpectedSha256, ExternallyVerifiedBundle, RunDirective, RunOperationCandidate,
    RunStateConfig, RunStateMachine, RunTerminal,
};
use actingcommand_contract::{InputAction, TaskOutcome};
use actingcommand_device::{Frame, PixelFormat};
use actingcommand_page_detector::PageDetector;
use actingcommand_recognition::{Scene, ScenePixelFormat};
use actingcommand_recognition_pack::RecognitionEvaluator;
use serde::Deserialize;
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
}

impl ContainedTaskError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ContainedTaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "contained task error {}", self.code)
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
    },
    RunStarted,
    CaptureCompleted {
        width: u32,
        height: u32,
    },
    RecognitionCompleted {
        page_label: Option<String>,
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
}

impl PreparedContainedTask {
    pub fn load(
        instance_label: &str,
        zip_bytes: &[u8],
        expected: ExternalExpectedSha256,
    ) -> Result<Self, ContainedTaskError> {
        let bundle = ExternallyVerifiedBundle::load(instance_label, zip_bytes, expected)
            .map_err(|_| ContainedTaskError::new("contained_task_admission_failed"))?
            .into_loaded_bundle();
        let control = bundle
            .control()
            .cloned()
            .ok_or_else(|| ContainedTaskError::new("contained_task_control_missing"))?;
        let control: TaskControl = serde_json::from_value(control)
            .map_err(|_| ContainedTaskError::new("contained_task_control_invalid"))?;
        control.validate()?;
        let program: TaskProgram = serde_json::from_value(bundle.operation().clone())
            .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        program.validate(&control)?;
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
        Ok(Self {
            control,
            program,
            evaluator,
            detector,
        })
    }

    pub fn task_label(&self) -> &str {
        &self.control.entry_task_id
    }

    pub fn package_label(&self) -> &str {
        &self.control.package_id
    }

    pub fn run<R: ContainedTaskRuntime>(
        &self,
        runtime: &mut R,
    ) -> Result<ContainedTaskOutcome, ContainedTaskRunError<R::Error>> {
        runtime
            .record(ContainedTaskTrace::PackageAdmitted {
                task_label: self.task_label().to_string(),
                package_label: self.package_label().to_string(),
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
        let mut current_page = self.capture_until_page(runtime, step_timeout, capture_interval)?;
        if self.control.execution_mode == "recognize_only" {
            runtime
                .record(ContainedTaskTrace::Finalizing {
                    outcome: TaskOutcome::Success,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            return Ok(ContainedTaskOutcome {
                outcome: TaskOutcome::Success,
                final_page: current_page,
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
        let config = RunStateConfig::new(
            &self.control.game,
            self.program.target_page.clone(),
            self.control.stop_on_confirmation.unwrap_or(true),
            1,
            self.control.max_steps.unwrap_or(DEFAULT_MAX_STEPS),
        )
        .map_err(|_| ContainedTaskError::new("contained_task_program_invalid"))?;
        let mut machine = RunStateMachine::new(config, 0)
            .map_err(|_| ContainedTaskError::new("contained_task_state_invalid"))?;
        machine
            .observe_page(current_page.clone())
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
                    current_page =
                        self.capture_until_page(runtime, step_timeout, capture_interval)?;
                    machine
                        .observe_page(current_page.clone())
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
                    runtime
                        .record(ContainedTaskTrace::StepStarted {
                            step_index,
                            operation_label: operation_id.clone(),
                            from_page,
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    let action = operation.click.input_action(&self.control.resolution)?;
                    runtime
                        .record(ContainedTaskTrace::EffectIntent {
                            step_index,
                            operation_label: operation_id.clone(),
                            action: action.clone(),
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
                    current_page =
                        self.capture_until_page(runtime, step_timeout, capture_interval)?;
                    let observed_page = current_page
                        .clone()
                        .ok_or_else(|| ContainedTaskError::new("contained_task_page_unmatched"))?;
                    runtime
                        .record(ContainedTaskTrace::StepFinished {
                            step_index,
                            operation_label: operation_id.clone(),
                            page_label: observed_page,
                        })
                        .map_err(ContainedTaskRunError::Boundary)?;
                    machine
                        .operation_succeeded(&operation_id, current_page.clone())
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
    ) -> Result<Option<String>, ContainedTaskRunError<R::Error>> {
        let started = Instant::now();
        loop {
            let frame = runtime.capture().map_err(ContainedTaskRunError::Boundary)?;
            self.control.resolution.validate_frame(&frame)?;
            runtime
                .record(ContainedTaskTrace::CaptureCompleted {
                    width: frame.width,
                    height: frame.height,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            let scene = scene_from_frame(&frame)?;
            let page = self
                .detector
                .evaluate_all(&self.evaluator, &scene)
                .map_err(|_| ContainedTaskError::new("contained_task_recognition_failed"))?
                .into_iter()
                .find(|evaluation| evaluation.matched)
                .map(|evaluation| evaluation.page_id);
            runtime
                .record(ContainedTaskTrace::RecognitionCompleted {
                    page_label: page.clone(),
                    width: frame.width,
                    height: frame.height,
                })
                .map_err(ContainedTaskRunError::Boundary)?;
            if page.is_some() || started.elapsed() >= timeout {
                return Ok(page);
            }
            thread::sleep(interval);
        }
    }
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
    target_page: Option<String>,
    operations: Vec<TaskOperation>,
}

impl TaskProgram {
    fn validate(&self, control: &TaskControl) -> Result<(), ContainedTaskError> {
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
            || self
                .target_page
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ContainedTaskError::new("contained_task_program_invalid"));
        }
        for operation in &self.operations {
            operation.validate(&control.resolution)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct TaskOperation {
    id: String,
    from: String,
    #[serde(default)]
    to: Option<String>,
    click: TaskClick,
}

impl TaskOperation {
    fn validate(&self, resolution: &Resolution) -> Result<(), ContainedTaskError> {
        if self.id.trim().is_empty()
            || self.from.trim().is_empty()
            || self
                .to
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
        self.click.input_action(resolution).map(|_| ())
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
    from_rect: Option<ClickRect>,
    #[serde(default)]
    to_rect: Option<ClickRect>,
}

impl TaskClick {
    fn input_action(&self, resolution: &Resolution) -> Result<InputAction, ContainedTaskError> {
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
    fn validate(&self, resolution: &Resolution) -> Result<(), ContainedTaskError> {
        if self.width <= 0 || self.height <= 0 {
            return Err(ContainedTaskError::new("contained_task_operation_invalid"));
        }
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
