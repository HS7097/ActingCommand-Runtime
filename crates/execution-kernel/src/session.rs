// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ExecutionBackendProvider, ExecutionKernelError, ExecutionKernelResult,
    ResolvedExecutionInstance,
};
use actingcommand_contract::{ApplicationLifecycleAction, InputAction, ResourceQuiescence};
use actingcommand_device::{
    CaptureBackend, DeviceCloseAuthority, DeviceError, DeviceResourceClosePhase,
    DeviceResourceKind, DeviceResourceQuiescence, DeviceResult, Frame, InputBackend,
    PreparedSegmentedSwipePlan, SegmentedSwipeAction, prepare_segmented_swipe,
    segmented_swipe_capability_error,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

const SESSION_CHANNEL_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedInputAction {
    Direct(InputAction),
    SegmentedSwipe(PreparedSegmentedSwipePlan),
}

impl PreparedInputAction {
    pub fn segmented_swipe_plan(&self) -> Option<&PreparedSegmentedSwipePlan> {
        match self {
            Self::Direct(_) => None,
            Self::SegmentedSwipe(plan) => Some(plan),
        }
    }
}

impl TryFrom<InputAction> for PreparedInputAction {
    type Error = ExecutionKernelError;

    fn try_from(action: InputAction) -> Result<Self, Self::Error> {
        let InputAction::SingleTouchDragWithVerticalBrakeV1 {
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
            slope_in,
            slope_out,
        } = action
        else {
            return Ok(Self::Direct(action));
        };
        let plan = prepare_segmented_swipe(SegmentedSwipeAction {
            points: [(x1, y1), (x2, y2), (x3, y3)],
            horizontal_duration_ms,
            corner_hold_ms,
            brake_distance_px,
            brake_duration_ms,
            slope_in,
            slope_out,
        })
        .map_err(|error| ExecutionKernelError::device("input_plan_preparation_failed", &error))?;
        Ok(Self::SegmentedSwipe(plan))
    }
}

enum SessionCommand {
    Input {
        action: PreparedInputAction,
        response: SyncSender<ExecutionKernelResult<()>>,
    },
    Capture {
        response: SyncSender<ExecutionKernelResult<Frame>>,
    },
    ApplicationLifecycle {
        action: ApplicationLifecycleAction,
        response: SyncSender<ExecutionKernelResult<()>>,
    },
    Close {
        authority: DeviceCloseAuthority,
        response: SyncSender<ExecutionKernelResult<ExecutionResourceCloseOutcome>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionResourceCloseOutcome {
    resource_count: u16,
}

impl ExecutionResourceCloseOutcome {
    pub(crate) const fn confirmed(resource_count: u16) -> Self {
        Self { resource_count }
    }

    pub const fn quiescence(self) -> ResourceQuiescence {
        ResourceQuiescence::Confirmed
    }

    pub const fn resource_count(self) -> u16 {
        self.resource_count
    }

    fn combine(self, other: Self) -> Self {
        Self::confirmed(self.resource_count.saturating_add(other.resource_count))
    }
}

struct SessionState {
    sender: Option<SyncSender<SessionCommand>>,
    join: Option<JoinHandle<ExecutionKernelResult<()>>>,
    closed: bool,
}

/// One daemon-owned, lazily opened input/capture session for a resolved device instance.
pub struct ExecutionSession {
    resolved: ResolvedExecutionInstance,
    state: Mutex<SessionState>,
}

impl ExecutionSession {
    pub(crate) fn start(
        provider: Arc<dyn ExecutionBackendProvider>,
        instance_alias: String,
        resolved: ResolvedExecutionInstance,
    ) -> ExecutionKernelResult<Self> {
        let (sender, receiver) = mpsc::sync_channel(SESSION_CHANNEL_CAPACITY);
        let join = thread::Builder::new()
            .name("actingcommand-execution-session".to_string())
            .spawn(move || {
                catch_unwind(AssertUnwindSafe(|| {
                    run_session(provider, instance_alias, receiver)
                }))
                .map_err(|_| ExecutionKernelError::fatal("execution_session_panicked"))?
            })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_spawn_failed"))?;
        Ok(Self {
            resolved,
            state: Mutex::new(SessionState {
                sender: Some(sender),
                join: Some(join),
                closed: false,
            }),
        })
    }

    pub const fn resolved(&self) -> &ResolvedExecutionInstance {
        &self.resolved
    }

    pub fn input(&self, action: InputAction) -> ExecutionKernelResult<()> {
        self.input_prepared(action.try_into()?)
    }

    pub(crate) fn input_prepared(&self, action: PreparedInputAction) -> ExecutionKernelResult<()> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(SessionCommand::Input { action, response })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_unavailable"));
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        let result = receiver.recv().unwrap_or_else(|_| {
            Err(ExecutionKernelError::fatal(
                "execution_session_response_lost",
            ))
        });
        finish_after_result(&mut state, result)
    }

    pub fn capture(&self) -> ExecutionKernelResult<Frame> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(SessionCommand::Capture { response })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_unavailable"));
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        let result = receiver.recv().unwrap_or_else(|_| {
            Err(ExecutionKernelError::fatal(
                "execution_session_response_lost",
            ))
        });
        finish_after_result(&mut state, result)
    }

    pub fn control_application(
        &self,
        action: ApplicationLifecycleAction,
    ) -> ExecutionKernelResult<()> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(SessionCommand::ApplicationLifecycle { action, response })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_unavailable"));
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        let result = receiver.recv().unwrap_or_else(|_| {
            Err(ExecutionKernelError::fatal(
                "execution_session_response_lost",
            ))
        });
        finish_after_result(&mut state, result)
    }

    pub fn close(&self) -> ExecutionKernelResult<()> {
        self.close_with_authority(DeviceCloseAuthority::LocalOnly)
            .map(|_| ())
    }

    pub(crate) fn close_with_authority(
        &self,
        authority: DeviceCloseAuthority,
    ) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        if state.closed {
            return join_session(&mut state).map(|()| ExecutionResourceCloseOutcome::confirmed(0));
        }
        state.closed = true;
        let Some(sender) = state.sender.take() else {
            return join_session(&mut state).map(|()| ExecutionResourceCloseOutcome::confirmed(0));
        };
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = sender
            .send(SessionCommand::Close {
                authority,
                response,
            })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_unavailable"));
        drop(sender);
        let close_result = send_result.and_then(|()| {
            receiver
                .recv()
                .map_err(|_| ExecutionKernelError::fatal("execution_session_response_lost"))?
        });
        match (close_result, join_session(&mut state)) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(primary), Err(secondary)) => Err(ExecutionKernelError::merge(primary, secondary)),
        }
    }

    fn lock_state(
        &self,
        code: &'static str,
    ) -> ExecutionKernelResult<MutexGuard<'_, SessionState>> {
        self.state
            .lock()
            .map_err(|_| ExecutionKernelError::fatal(code))
    }
}

impl Drop for ExecutionSession {
    fn drop(&mut self) {
        if thread::panicking() {
            return;
        }
        if let Err(error) = self.close() {
            panic!("{error}");
        }
    }
}

fn ensure_open(state: &SessionState) -> ExecutionKernelResult<()> {
    if state.closed || state.sender.is_none() {
        Err(ExecutionKernelError::fatal("execution_session_closed"))
    } else {
        Ok(())
    }
}

fn finish_after_result<T>(
    state: &mut SessionState,
    result: ExecutionKernelResult<T>,
) -> ExecutionKernelResult<T> {
    if result.is_ok() {
        return result;
    }
    state.closed = true;
    state.sender.take();
    let join = join_session(state);
    match (result, join) {
        (Err(primary), Err(secondary)) => Err(ExecutionKernelError::merge(primary, secondary)),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(value), _) => Ok(value),
    }
}

fn join_session(state: &mut SessionState) -> ExecutionKernelResult<()> {
    let Some(join) = state.join.take() else {
        return Ok(());
    };
    join.join()
        .map_err(|_| ExecutionKernelError::fatal("execution_session_panicked"))?
}

fn run_session(
    provider: Arc<dyn ExecutionBackendProvider>,
    instance_alias: String,
    receiver: Receiver<SessionCommand>,
) -> ExecutionKernelResult<()> {
    let mut input = None;
    let mut capture = None;
    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Input { action, response } => {
                let result = execute_input(provider.as_ref(), &instance_alias, &mut input, action);
                if let Err(error) = result {
                    let terminal = close_after_failure(
                        capture.take(),
                        input.take(),
                        error,
                        ResourceCloseOrder::InputFirst,
                        DeviceCloseAuthority::FencedDeviceWrite,
                    );
                    let response_result = terminal.clone();
                    if response.send(Err(response_result)).is_err() {
                        return Err(ExecutionKernelError::merge(
                            terminal,
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                        ));
                    }
                    return Err(terminal);
                }
                if response.send(Ok(())).is_err() {
                    return Err(close_after_failure(
                        capture.take(),
                        input.take(),
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::FencedDeviceWrite,
                    ));
                }
            }
            SessionCommand::Capture { response } => {
                let result = execute_capture(provider.as_ref(), &instance_alias, &mut capture);
                match result {
                    Ok(frame) => response.send(Ok(frame)).map_err(|_| {
                        close_after_failure(
                            capture.take(),
                            input.take(),
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::FencedDeviceWrite,
                        )
                    })?,
                    Err(error) => {
                        let terminal = close_after_failure(
                            capture.take(),
                            input.take(),
                            error,
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::FencedDeviceWrite,
                        );
                        if response.send(Err(terminal.clone())).is_err() {
                            return Err(ExecutionKernelError::merge(
                                terminal,
                                ExecutionKernelError::fatal("execution_session_response_lost"),
                            ));
                        }
                        return Err(terminal);
                    }
                }
            }
            SessionCommand::ApplicationLifecycle { action, response } => {
                if let Err(error) = close_resources(
                    capture.take(),
                    input.take(),
                    DeviceCloseAuthority::FencedDeviceWrite,
                    ResourceCloseOrder::CaptureFirst,
                ) {
                    if response.send(Err(error.clone())).is_err() {
                        return Err(ExecutionKernelError::merge(
                            error,
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                        ));
                    }
                    return Err(error);
                }
                let result = provider
                    .control_application(&instance_alias, action)
                    .map_err(|error| {
                        ExecutionKernelError::device("application_backend_operation_failed", &error)
                    });
                if let Err(error) = result {
                    if response.send(Err(error.clone())).is_err() {
                        return Err(ExecutionKernelError::merge(
                            error,
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                        ));
                    }
                    return Err(error);
                }
                response
                    .send(Ok(()))
                    .map_err(|_| ExecutionKernelError::fatal("execution_session_response_lost"))?;
            }
            SessionCommand::Close {
                authority,
                response,
            } => {
                let result = close_resources(
                    capture.take(),
                    input.take(),
                    authority,
                    ResourceCloseOrder::CaptureFirst,
                );
                if response.send(result.clone()).is_err() {
                    return match result {
                        Ok(_) => Err(ExecutionKernelError::fatal(
                            "execution_session_response_lost",
                        )),
                        Err(error) => Err(ExecutionKernelError::merge(
                            error,
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                        )),
                    };
                }
                return result.map(|_| ());
            }
        }
    }
    close_resources(
        capture.take(),
        input.take(),
        DeviceCloseAuthority::LocalOnly,
        ResourceCloseOrder::CaptureFirst,
    )
    .map(|_| ())
}

fn execute_input(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backend: &mut Option<Box<dyn InputBackend>>,
    action: PreparedInputAction,
) -> ExecutionKernelResult<()> {
    if backend.is_none() {
        *backend =
            Some(provider.open_input(instance_alias).map_err(|error| {
                ExecutionKernelError::device("input_backend_open_failed", &error)
            })?);
    }
    let backend = backend
        .as_mut()
        .ok_or_else(|| ExecutionKernelError::fatal("input_backend_missing"))?;
    execute_action(backend.as_mut(), &action)
        .map_err(|error| ExecutionKernelError::device("input_backend_operation_failed", &error))
}

fn execute_capture(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backend: &mut Option<Box<dyn CaptureBackend>>,
) -> ExecutionKernelResult<Frame> {
    if backend.is_none() {
        *backend = Some(provider.open_capture(instance_alias).map_err(|error| {
            ExecutionKernelError::device("capture_backend_open_failed", &error)
        })?);
    }
    backend
        .as_mut()
        .ok_or_else(|| ExecutionKernelError::fatal("capture_backend_missing"))?
        .capture()
        .map_err(|error| ExecutionKernelError::device("capture_backend_operation_failed", &error))
}

fn execute_action(
    backend: &mut dyn InputBackend,
    action: &PreparedInputAction,
) -> DeviceResult<()> {
    match action {
        PreparedInputAction::Direct(InputAction::Tap { x, y }) => backend.tap(*x, *y),
        PreparedInputAction::Direct(InputAction::LongTap { x, y, duration_ms }) => {
            backend.long_tap(*x, *y, *duration_ms)
        }
        PreparedInputAction::Direct(InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        }) => backend.swipe(*x1, *y1, *x2, *y2, *duration_ms),
        PreparedInputAction::SegmentedSwipe(plan) => {
            if !backend.supports_segmented_swipe() {
                return Err(segmented_swipe_capability_error());
            }
            backend.segmented_swipe_prepared(plan)
        }
        PreparedInputAction::Direct(InputAction::Key { key }) => backend.key(key),
        PreparedInputAction::Direct(InputAction::Text { text }) => backend.text(text),
        PreparedInputAction::Direct(InputAction::Reset) => backend.reset(),
        PreparedInputAction::Direct(InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }) => Err(
            DeviceError::fatal("segmented input action was not prepared"),
        ),
    }
}

#[derive(Clone, Copy)]
enum ResourceCloseOrder {
    CaptureFirst,
    InputFirst,
}

fn close_after_failure(
    capture: Option<Box<dyn CaptureBackend>>,
    input: Option<Box<dyn InputBackend>>,
    primary: ExecutionKernelError,
    order: ResourceCloseOrder,
    authority: DeviceCloseAuthority,
) -> ExecutionKernelError {
    match close_resources(capture, input, authority, order) {
        Ok(_) => primary,
        Err(secondary) => ExecutionKernelError::merge_cleanup(primary, secondary),
    }
}

fn close_resources(
    capture: Option<Box<dyn CaptureBackend>>,
    input: Option<Box<dyn InputBackend>>,
    authority: DeviceCloseAuthority,
    order: ResourceCloseOrder,
) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
    let (first, second) = match order {
        ResourceCloseOrder::CaptureFirst => (
            close_capture(capture, authority),
            close_input(input, authority),
        ),
        ResourceCloseOrder::InputFirst => (
            close_input(input, authority),
            close_capture(capture, authority),
        ),
    };
    match (first, second) {
        (Ok(first), Ok(second)) => Ok(first.combine(second)),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => {
            Err(ExecutionKernelError::merge_cleanup(primary, secondary))
        }
    }
}

fn close_capture(
    mut capture: Option<Box<dyn CaptureBackend>>,
    authority: DeviceCloseAuthority,
) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
    let Some(mut backend) = capture.take() else {
        return Ok(ExecutionResourceCloseOutcome::confirmed(0));
    };
    match backend.close_once(authority) {
        Ok(outcome) => Ok(ExecutionResourceCloseOutcome::confirmed(
            outcome.resource_count(),
        )),
        Err(error) => {
            let quiescence = error
                .resource_quiescence()
                .unwrap_or(DeviceResourceQuiescence::Unconfirmed);
            let error = if error.resource_close_causes().is_empty() {
                error.with_resource_close_cause(
                    DeviceResourceKind::CaptureBackend,
                    DeviceResourceClosePhase::Close,
                    "capture_backend",
                    None,
                    None,
                    quiescence,
                    1,
                )
            } else {
                error
            };
            if quiescence == DeviceResourceQuiescence::Unconfirmed {
                std::mem::forget(backend);
            }
            Err(ExecutionKernelError::device(
                "capture_backend_close_failed",
                &error,
            ))
        }
    }
}

fn close_input(
    mut input: Option<Box<dyn InputBackend>>,
    authority: DeviceCloseAuthority,
) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
    let Some(backend) = input.as_mut() else {
        return Ok(ExecutionResourceCloseOutcome::confirmed(0));
    };
    match backend.close_once(authority) {
        Ok(outcome) => Ok(ExecutionResourceCloseOutcome::confirmed(
            outcome.resource_count(),
        )),
        Err(error) => {
            let quiescence = error
                .resource_quiescence()
                .unwrap_or(DeviceResourceQuiescence::Unconfirmed);
            let error = if error.resource_close_causes().is_empty() {
                error.with_resource_close_cause(
                    DeviceResourceKind::InputBackend,
                    DeviceResourceClosePhase::Close,
                    "input_backend",
                    None,
                    None,
                    quiescence,
                    1,
                )
            } else {
                error
            };
            if quiescence == DeviceResourceQuiescence::Unconfirmed {
                std::mem::forget(input.take().expect("input backend is present"));
            }
            Err(ExecutionKernelError::device(
                "input_backend_close_failed",
                &error,
            ))
        }
    }
}
