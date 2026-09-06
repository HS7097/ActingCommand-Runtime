// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ExecutionBackendProvider, ExecutionKernelError, ExecutionKernelResult,
    ResolvedExecutionInstance,
};
use actingcommand_contract::{ApplicationLifecycleAction, InputAction};
use actingcommand_device::{
    CaptureBackend, DeviceError, DeviceResult, Frame, InputBackend, PreparedSegmentedSwipePlan,
    SegmentedSwipeAction, prepare_segmented_swipe, segmented_swipe_capability_error,
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
        response:
            SyncSender<ExecutionKernelResult<Option<actingcommand_device::InputSelectionContext>>>,
    },
    Capture {
        response: SyncSender<ExecutionKernelResult<Frame>>,
    },
    ApplicationLifecycle {
        action: ApplicationLifecycleAction,
        response: SyncSender<ExecutionKernelResult<()>>,
    },
    Close {
        response: SyncSender<ExecutionKernelResult<()>>,
    },
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
        self.input_prepared(action.try_into()?).map(|_| ())
    }

    pub(crate) fn input_prepared(
        &self,
        action: PreparedInputAction,
    ) -> ExecutionKernelResult<Option<actingcommand_device::InputSelectionContext>> {
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
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        if state.closed {
            return join_session(&mut state);
        }
        state.closed = true;
        let Some(sender) = state.sender.take() else {
            return join_session(&mut state);
        };
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = sender
            .send(SessionCommand::Close { response })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_unavailable"));
        drop(sender);
        let close_result = send_result.and_then(|()| {
            receiver
                .recv()
                .map_err(|_| ExecutionKernelError::fatal("execution_session_response_lost"))?
        });
        merge_results(close_result, join_session(&mut state))
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

fn merge_results(
    primary: ExecutionKernelResult<()>,
    secondary: ExecutionKernelResult<()>,
) -> ExecutionKernelResult<()> {
    match (primary, secondary) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => Err(ExecutionKernelError::merge(primary, secondary)),
    }
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
                let context = match result {
                    Ok(context) => context,
                    Err(error) => {
                        let terminal = close_after_failure(input.take(), error);
                        let response_result = terminal.clone();
                        response.send(Err(response_result)).map_err(|_| {
                            ExecutionKernelError::fatal("execution_session_response_lost")
                        })?;
                        return Err(terminal);
                    }
                };
                response
                    .send(Ok(context))
                    .map_err(|_| ExecutionKernelError::fatal("execution_session_response_lost"))?;
            }
            SessionCommand::Capture { response } => {
                let result = execute_capture(provider.as_ref(), &instance_alias, &mut capture);
                match result {
                    Ok(frame) => response.send(Ok(frame)).map_err(|_| {
                        ExecutionKernelError::fatal("execution_session_response_lost")
                    })?,
                    Err(error) => {
                        capture.take();
                        let terminal = close_after_failure(input.take(), error);
                        response.send(Err(terminal.clone())).map_err(|_| {
                            ExecutionKernelError::fatal("execution_session_response_lost")
                        })?;
                        return Err(terminal);
                    }
                }
            }
            SessionCommand::ApplicationLifecycle { action, response } => {
                capture.take();
                if let Err(error) = close_input(input.take()) {
                    response.send(Err(error.clone())).map_err(|_| {
                        ExecutionKernelError::fatal("execution_session_response_lost")
                    })?;
                    return Err(error);
                }
                let result = provider
                    .control_application(&instance_alias, action)
                    .map_err(|error| {
                        ExecutionKernelError::device("application_backend_operation_failed", &error)
                    });
                if let Err(error) = result {
                    response.send(Err(error.clone())).map_err(|_| {
                        ExecutionKernelError::fatal("execution_session_response_lost")
                    })?;
                    return Err(error);
                }
                response
                    .send(Ok(()))
                    .map_err(|_| ExecutionKernelError::fatal("execution_session_response_lost"))?;
            }
            SessionCommand::Close { response } => {
                capture.take();
                let result = close_input(input.take());
                response
                    .send(result.clone())
                    .map_err(|_| ExecutionKernelError::fatal("execution_session_response_lost"))?;
                return result;
            }
        }
    }
    capture.take();
    close_input(input.take())
}

fn execute_input(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backend: &mut Option<Box<dyn InputBackend>>,
    action: PreparedInputAction,
) -> ExecutionKernelResult<Option<actingcommand_device::InputSelectionContext>> {
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
        .map_err(|error| ExecutionKernelError::device("input_backend_operation_failed", &error))?;
    Ok(backend.selection_context())
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

fn close_after_failure(
    input: Option<Box<dyn InputBackend>>,
    primary: ExecutionKernelError,
) -> ExecutionKernelError {
    match close_input(input) {
        Ok(()) => primary,
        Err(secondary) => ExecutionKernelError::merge_cleanup(primary, secondary),
    }
}

fn close_input(mut input: Option<Box<dyn InputBackend>>) -> ExecutionKernelResult<()> {
    let Some(backend) = input.as_mut() else {
        return Ok(());
    };
    backend
        .close()
        .map_err(|error| ExecutionKernelError::device("input_backend_close_failed", &error))
}
