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
pub struct ExecutionInputOutcome {
    pub selection: Option<actingcommand_device::InputSelectionContext>,
    pub recovery: Option<actingcommand_contract::AdbTargetRecovery>,
}

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
        response: SyncSender<ExecutionKernelResult<ExecutionInputOutcome>>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResourceCloseOutcome {
    resource_count: u16,
    vendor_stdio: Vec<crate::ExecutionStdioObservation>,
}

impl ExecutionResourceCloseOutcome {
    pub(crate) const fn confirmed(resource_count: u16) -> Self {
        Self {
            resource_count,
            vendor_stdio: Vec::new(),
        }
    }

    fn from_device(outcome: actingcommand_device::DeviceResourceCloseOutcome) -> Self {
        Self {
            resource_count: outcome.resource_count(),
            vendor_stdio: outcome
                .vendor_stdio()
                .iter()
                .map(crate::ExecutionStdioObservation::from_device)
                .collect(),
        }
    }

    pub fn vendor_stdio(&self) -> &[crate::ExecutionStdioObservation] {
        &self.vendor_stdio
    }

    pub const fn quiescence(&self) -> ResourceQuiescence {
        ResourceQuiescence::Confirmed
    }

    pub const fn resource_count(&self) -> u16 {
        self.resource_count
    }

    fn combine(mut self, other: Self) -> Self {
        self.resource_count = self.resource_count.saturating_add(other.resource_count);
        crate::error::merge_stdio_observations(&mut self.vendor_stdio, &other.vendor_stdio);
        self
    }
}

struct SessionState {
    sender: Option<SyncSender<SessionCommand>>,
    join: Option<JoinHandle<ExecutionKernelResult<()>>>,
    closed: bool,
    close_result: Option<ExecutionKernelResult<ExecutionResourceCloseOutcome>>,
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
                let mut input = None;
                let mut capture = None;
                match catch_unwind(AssertUnwindSafe(|| {
                    run_session(provider, instance_alias, receiver, &mut input, &mut capture)
                })) {
                    Ok(result) => result,
                    Err(_) => Err(close_after_failure(
                        capture.take(),
                        input.take(),
                        ExecutionKernelError::fatal("execution_session_panicked"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    )),
                }
            })
            .map_err(|_| ExecutionKernelError::fatal("execution_session_spawn_failed"))?;
        Ok(Self {
            resolved,
            state: Mutex::new(SessionState {
                sender: Some(sender),
                join: Some(join),
                closed: false,
                close_result: None,
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
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        match self.input_prepared_retained(action) {
            Ok(selection) => Ok(selection),
            Err(primary) => {
                let error = match self.close_with_authority(DeviceCloseAuthority::LocalOnly) {
                    Ok(_) => primary,
                    Err(cleanup) => ExecutionKernelError::merge_cleanup(primary, cleanup),
                };
                let mut state = self.lock_state("execution_session_state_poisoned")?;
                finish_after_result(&mut state, Err(error))
            }
        }
    }

    pub(crate) fn input_prepared_retained(
        &self,
        action: PreparedInputAction,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
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
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    "execution_session_response_lost",
                )),
            ),
        }
    }

    pub fn capture(&self) -> ExecutionKernelResult<Frame> {
        match self.capture_retained() {
            Ok(frame) => Ok(frame),
            Err(primary) => Err(
                match self.close_with_authority(DeviceCloseAuthority::LocalOnly) {
                    Ok(_) => primary,
                    Err(cleanup) => ExecutionKernelError::merge_cleanup(primary, cleanup),
                },
            ),
        }
    }

    pub(crate) fn capture_retained(&self) -> ExecutionKernelResult<Frame> {
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
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    "execution_session_response_lost",
                )),
            ),
        }
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
        if let Some(result) = &state.close_result {
            return result.clone();
        }
        state.closed = true;
        let Some(sender) = state.sender.take() else {
            let result =
                join_session(&mut state).map(|()| ExecutionResourceCloseOutcome::confirmed(0));
            state.close_result = Some(result.clone());
            return result;
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
        let result = match (close_result, join_session(&mut state)) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), Ok(())) => Err(error),
            (Ok(outcome), Err(error)) => Err(error.with_stdio_observations(outcome.vendor_stdio())),
            (Err(primary), Err(secondary)) => Err(ExecutionKernelError::merge(primary, secondary)),
        };
        state.close_result = Some(result.clone());
        result
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
        if self
            .state
            .get_mut()
            .is_ok_and(|state| state.close_result.is_some())
        {
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
    state.close_result = Some(match &join {
        Err(error) => Err(error.clone()),
        Ok(()) => match &result {
            Err(error) => Err(error.clone()),
            Ok(_) => Ok(ExecutionResourceCloseOutcome::confirmed(0)),
        },
    });
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
    input: &mut Option<Box<dyn InputBackend>>,
    capture: &mut Option<Box<dyn CaptureBackend>>,
) -> ExecutionKernelResult<()> {
    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Input { action, response } => {
                let result = execute_input(provider.as_ref(), &instance_alias, input, action);
                let context = match result {
                    Ok(context) => context,
                    Err(error) => {
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                capture.take(),
                                input.take(),
                                ExecutionKernelError::merge(
                                    error,
                                    ExecutionKernelError::fatal("execution_session_response_lost"),
                                ),
                                ResourceCloseOrder::InputFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                        return close_retained_after_failure(
                            &receiver,
                            capture,
                            input,
                            error,
                            ResourceCloseOrder::InputFirst,
                        );
                    }
                };
                if response.send(Ok(context)).is_err() {
                    return Err(close_after_failure(
                        capture.take(),
                        input.take(),
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::Capture { response } => {
                let result = execute_capture(provider.as_ref(), &instance_alias, capture);
                match result {
                    Ok(frame) => response.send(Ok(frame)).map_err(|_| {
                        close_after_failure(
                            capture.take(),
                            input.take(),
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::LocalOnly,
                        )
                    })?,
                    Err(error) => {
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                capture.take(),
                                input.take(),
                                ExecutionKernelError::merge(
                                    error,
                                    ExecutionKernelError::fatal("execution_session_response_lost"),
                                ),
                                ResourceCloseOrder::CaptureFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                        return close_retained_after_failure(
                            &receiver,
                            capture,
                            input,
                            error,
                            ResourceCloseOrder::CaptureFirst,
                        );
                    }
                }
            }
            SessionCommand::ApplicationLifecycle { action, response } => {
                if let Err(error) = close_resources(
                    capture.take(),
                    input.take(),
                    DeviceCloseAuthority::LocalOnly,
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
                        Ok(outcome) => Err(ExecutionKernelError::fatal(
                            "execution_session_response_lost",
                        )
                        .with_stdio_observations(outcome.vendor_stdio())),
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

fn close_retained_after_failure(
    receiver: &Receiver<SessionCommand>,
    capture: &mut Option<Box<dyn CaptureBackend>>,
    input: &mut Option<Box<dyn InputBackend>>,
    primary: ExecutionKernelError,
    order: ResourceCloseOrder,
) -> ExecutionKernelResult<()> {
    // Keep the actual backends here until the Host chooses close admission.
    loop {
        match receiver.recv() {
            Ok(SessionCommand::Close {
                authority,
                response,
            }) => {
                let result = close_resources(capture.take(), input.take(), authority, order);
                if response.send(result.clone()).is_err() {
                    return Err(match result {
                        Ok(outcome) => {
                            ExecutionKernelError::fatal("execution_session_response_lost")
                                .with_stdio_observations(outcome.vendor_stdio())
                        }
                        Err(cleanup) => cleanup,
                    });
                }
                return result.map(|_| ());
            }
            Ok(SessionCommand::Capture { response }) => {
                // A concurrent observer can already be waiting behind the failed input.
                // Refuse that observation without consuming the input owner's close handoff.
                if response
                    .send(Err(ExecutionKernelError::device(
                        "execution_session_close_pending",
                        &DeviceError::transient(
                            "capture is unavailable while the resource owner closes the session",
                        ),
                    )))
                    .is_ok()
                {
                    continue;
                }
                return Err(close_after_failure(
                    capture.take(),
                    input.take(),
                    ExecutionKernelError::merge(
                        primary,
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                    ),
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
            _ => {
                return Err(close_after_failure(
                    capture.take(),
                    input.take(),
                    primary,
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
        }
    }
}

fn execute_input(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backend: &mut Option<Box<dyn InputBackend>>,
    action: PreparedInputAction,
) -> ExecutionKernelResult<ExecutionInputOutcome> {
    if backend.is_none() {
        *backend =
            Some(provider.open_input(instance_alias).map_err(|error| {
                ExecutionKernelError::device("input_backend_open_failed", &error)
            })?);
    }
    let backend = backend
        .as_mut()
        .ok_or_else(|| ExecutionKernelError::fatal("input_backend_missing"))?;
    let recovery = backend.take_adb_recovery();
    execute_action(backend.as_mut(), &action).map_err(|error| {
        let error = match &recovery {
            Some(report) => error.with_adb_recovery(report.clone()),
            None => error,
        };
        ExecutionKernelError::device("input_backend_operation_failed", &error)
    })?;
    Ok(ExecutionInputOutcome {
        selection: backend.selection_context(),
        recovery: recovery.as_ref().map(crate::error::adb_recovery_record),
    })
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
        Ok(outcome) => primary.with_stdio_observations(outcome.vendor_stdio()),
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
        (Err(error), Ok(outcome)) | (Ok(outcome), Err(error)) => {
            Err(error.with_stdio_observations(outcome.vendor_stdio()))
        }
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
    let result =
        catch_unwind(AssertUnwindSafe(|| backend.close_once(authority))).unwrap_or_else(|_| {
            Err(DeviceError::fatal("capture backend panicked during close")
                .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1))
        });
    match result {
        Ok(outcome) => Ok(ExecutionResourceCloseOutcome::from_device(outcome)),
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
    let result =
        catch_unwind(AssertUnwindSafe(|| backend.close_once(authority))).unwrap_or_else(|_| {
            Err(DeviceError::fatal("input backend panicked during close")
                .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1))
        });
    match result {
        Ok(outcome) => Ok(ExecutionResourceCloseOutcome::from_device(outcome)),
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
