// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    ExecutionBackendProvider, ExecutionKernelError, ExecutionKernelResult,
    ResolvedExecutionInstance,
};
use actingcommand_contract::{
    ApplicationLifecycleAction, CaptureGeometryObservation, CaptureGeometryUnknownReason,
    FencedWrite, FrameId, InputAction, InputFrameReference, ResourceQuiescence,
};
use actingcommand_device::{
    CaptureBackend, DeviceCloseAuthority, DeviceError, DeviceResourceClosePhase,
    DeviceResourceKind, DeviceResourceQuiescence, DeviceResult, Frame, InputBackend,
    InputExecutionContext, InputOperationCheck, NemuFrameGeometry, NemuSessionBackends,
    PreparedSegmentedSwipePlan, SegmentedSwipeAction, prepare_segmented_swipe,
    segmented_swipe_capability_error,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SESSION_CHANNEL_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFrameContext {
    reference: InputFrameReference,
    geometry: Option<Arc<NemuFrameGeometry>>,
}

impl InputFrameContext {
    pub const fn reference(&self) -> InputFrameReference {
        self.reference
    }

    fn captured(frame_id: FrameId, frame: &Frame) -> Self {
        Self {
            reference: InputFrameReference {
                frame_id,
                width: frame.width,
                height: frame.height,
            },
            geometry: frame
                .selection
                .as_ref()
                .and_then(|selection| selection.nemu_frame.clone()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct ObservedFrame {
    pub frame: Frame,
    /// Capture request data; the committed context is obtained after CaptureCompleted.
    pub input_reference: Option<InputFrameReference>,
}

impl From<Frame> for ObservedFrame {
    fn from(frame: Frame) -> Self {
        Self {
            frame,
            input_reference: None,
        }
    }
}

impl std::ops::Deref for ObservedFrame {
    type Target = Frame;
    fn deref(&self) -> &Frame {
        &self.frame
    }
}

enum SessionBackends {
    Pending,
    Independent {
        input: Option<Box<dyn InputBackend>>,
        capture: Option<Box<dyn CaptureBackend>>,
    },
    Nemu(NemuSessionBackends),
}

impl SessionBackends {
    fn take(&mut self) -> Self {
        std::mem::replace(self, Self::Pending)
    }

    fn prepare(
        &mut self,
        provider: &dyn ExecutionBackendProvider,
        alias: &str,
    ) -> ExecutionKernelResult<Vec<actingcommand_device::BackendOpenObservation>> {
        if matches!(self, Self::Pending) {
            let mut observations = Vec::new();
            *self = match provider.open_nemu_session(alias).map_err(|error| {
                observed_open_error(
                    "paired_backend_open_failed",
                    actingcommand_contract::BackendOpenEntry::NemuPair,
                    error,
                )
            })? {
                Some(pair) => {
                    observations.push(pair.observation);
                    Self::Nemu(pair.backend)
                }
                None => Self::Independent {
                    input: None,
                    capture: None,
                },
            };
            return Ok(observations);
        }
        Ok(Vec::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionInputOutcome {
    pub backend_open_observations: Vec<actingcommand_device::BackendOpenObservation>,
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
        frame: Option<InputFrameReference>,
        check: Option<Arc<dyn InputOperationCheck>>,
        step: Option<Arc<FencedWrite>>,
        response: SyncSender<ExecutionKernelResult<ExecutionInputOutcome>>,
    },
    Capture {
        frame_id: Option<FrameId>,
        memory: Option<actingcommand_device::FrameMemoryBudget>,
        response: SyncSender<ExecutionKernelResult<Frame>>,
    },
    CommitFrame {
        reference: InputFrameReference,
        response: SyncSender<ExecutionKernelResult<InputFrameContext>>,
    },
    ResolveFrame {
        reference: InputFrameReference,
        response: SyncSender<ExecutionKernelResult<InputFrameContext>>,
    },
    ObserveGeometry {
        deadline: Instant,
        response: SyncSender<ExecutionKernelResult<CaptureGeometryObservation>>,
    },
    ApplicationLifecycle {
        action: ApplicationLifecycleAction,
        step: Option<Arc<FencedWrite>>,
        response: SyncSender<ExecutionKernelResult<()>>,
    },
    Close {
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
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
        generation: u64,
    ) -> ExecutionKernelResult<Self> {
        let (sender, receiver) = mpsc::sync_channel(SESSION_CHANNEL_CAPACITY);
        let join = thread::Builder::new()
            .name("actingcommand-execution-session".to_string())
            .spawn(move || {
                let mut backends = SessionBackends::Pending;
                match catch_unwind(AssertUnwindSafe(|| {
                    run_session(
                        provider,
                        instance_alias,
                        receiver,
                        &mut backends,
                        generation,
                    )
                })) {
                    Ok(result) => result,
                    Err(_) => Err(close_after_failure(
                        backends.take(),
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
        self.input_prepared_in_frame(action, None, None, None)
    }

    pub(crate) fn input_prepared_in_frame(
        &self,
        action: PreparedInputAction,
        frame: Option<InputFrameReference>,
        check: Option<Arc<dyn InputOperationCheck>>,
        step: Option<Arc<FencedWrite>>,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(SessionCommand::Input {
                action,
                frame,
                check,
                step,
                response,
            })
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
        self.capture_frame_retained(None, None)
    }

    pub(crate) fn capture_frame_retained(
        &self,
        frame_id: Option<FrameId>,
        memory: Option<actingcommand_device::FrameMemoryBudget>,
    ) -> ExecutionKernelResult<Frame> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(SessionCommand::Capture {
                frame_id,
                memory,
                response,
            })
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

    /// Host calls this only after the original capture fact has committed.
    pub(crate) fn commit_input_frame(
        &self,
        reference: InputFrameReference,
    ) -> ExecutionKernelResult<InputFrameContext> {
        self.frame_request(|response| SessionCommand::CommitFrame {
            reference,
            response,
        })
    }

    pub(crate) fn resolve_input_frame(
        &self,
        reference: InputFrameReference,
    ) -> ExecutionKernelResult<InputFrameContext> {
        self.frame_request(|response| SessionCommand::ResolveFrame {
            reference,
            response,
        })
    }

    fn frame_request(
        &self,
        command: impl FnOnce(SyncSender<ExecutionKernelResult<InputFrameContext>>) -> SessionCommand,
    ) -> ExecutionKernelResult<InputFrameContext> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let sent = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(command(response))
            .map_err(|_| ExecutionKernelError::fatal("execution_session_unavailable"));
        if let Err(error) = sent {
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
        match self.control_application_retained(action, None) {
            Ok(()) => Ok(()),
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

    pub(crate) fn control_application_retained(
        &self,
        action: ApplicationLifecycleAction,
        step: Option<Arc<FencedWrite>>,
    ) -> ExecutionKernelResult<()> {
        let mut state = self.lock_state("execution_session_state_poisoned")?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .send(SessionCommand::ApplicationLifecycle {
                action,
                step,
                response,
            })
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

    pub(crate) fn observe_geometry(
        &self,
        deadline: Instant,
    ) -> ExecutionKernelResult<CaptureGeometryObservation> {
        let state = self.geometry_state(deadline)?;
        geometry_remaining(deadline)?;
        let (response, receiver) = mpsc::sync_channel(1);
        state
            .sender
            .as_ref()
            .ok_or_else(|| ExecutionKernelError::fatal("execution_session_closed"))?
            .try_send(SessionCommand::ObserveGeometry { deadline, response })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => geometry_unavailable(
                    "capture_geometry_queue_full",
                    "the existing session command queue is full",
                ),
                mpsc::TrySendError::Disconnected(_) => {
                    ExecutionKernelError::fatal("execution_session_unavailable")
                }
            })?;
        drop(state);
        match receiver.recv_timeout(geometry_remaining(deadline)?) {
            Ok(Ok(observation)) => {
                geometry_remaining(deadline)?;
                Ok(observation)
            }
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(geometry_unavailable(
                "capture_geometry_deadline_elapsed",
                "the geometry reply did not arrive within the task deadline",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(ExecutionKernelError::fatal(
                "execution_session_response_lost",
            )),
        }
    }

    pub(crate) fn validate_geometry_open(&self, deadline: Instant) -> ExecutionKernelResult<()> {
        self.geometry_state(deadline).map(drop)
    }

    fn geometry_state(
        &self,
        deadline: Instant,
    ) -> ExecutionKernelResult<MutexGuard<'_, SessionState>> {
        geometry_remaining(deadline)?;
        let state = self.state.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => geometry_unavailable(
                "capture_geometry_session_busy",
                "the existing session state is busy",
            ),
            TryLockError::Poisoned(_) => {
                ExecutionKernelError::fatal("execution_session_state_poisoned")
            }
        })?;
        if state.closed || state.sender.is_none() {
            return Err(state
                .close_result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .cloned()
                .unwrap_or_else(|| {
                    geometry_unavailable(
                        "capture_geometry_session_closed",
                        "the producing session is closed",
                    )
                }));
        }
        geometry_remaining(deadline)?;
        Ok(state)
    }

    pub fn close(&self) -> ExecutionKernelResult<()> {
        self.close_with_authority(DeviceCloseAuthority::LocalOnly)
            .map(|_| ())
    }

    pub(crate) fn close_with_authority(
        &self,
        authority: DeviceCloseAuthority,
    ) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
        self.close_with_input_check(authority, None)
    }

    pub(crate) fn close_with_input_check(
        &self,
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
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
                input_check,
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

pub(crate) fn geometry_unavailable(
    code: &'static str,
    detail: &'static str,
) -> ExecutionKernelError {
    ExecutionKernelError::device(code, &DeviceError::transient(detail))
}

pub(crate) fn geometry_remaining(deadline: Instant) -> ExecutionKernelResult<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            geometry_unavailable(
                "capture_geometry_deadline_elapsed",
                "the original task deadline has elapsed",
            )
        })
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
    backends: &mut SessionBackends,
    generation: u64,
) -> ExecutionKernelResult<()> {
    let mut pending_frame: Option<InputFrameContext> = None;
    let mut committed_frame: Option<InputFrameContext> = None;
    while let Ok(command) = receiver.recv() {
        match command {
            SessionCommand::Input {
                action,
                frame,
                check,
                step,
                response,
            } => {
                let result = execute_input(
                    provider.as_ref(),
                    &instance_alias,
                    backends,
                    action,
                    frame,
                    committed_frame.as_ref(),
                    check,
                );
                drop(step);
                let result = result
                    .map(|mut outcome| {
                        for observation in &mut outcome.backend_open_observations {
                            observation.report.session_generation = generation;
                        }
                        outcome
                    })
                    .map_err(|error| error.with_backend_session_generation(generation));
                let context = match result {
                    Ok(context) => context,
                    Err(error) => {
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
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
                            backends,
                            error,
                            ResourceCloseOrder::InputFirst,
                        );
                    }
                };
                if response.send(Ok(context)).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::Capture {
                frame_id,
                memory,
                response,
            } => {
                pending_frame = None;
                committed_frame = None;
                let result = execute_capture(
                    provider.as_ref(),
                    &instance_alias,
                    backends,
                    memory.as_ref(),
                )
                .map(|mut frame| {
                    for observation in &mut frame.backend_open_observations {
                        observation.report.session_generation = generation;
                    }
                    frame
                })
                .map_err(|error| error.with_backend_session_generation(generation));
                if let (Some(frame_id), Ok(frame)) = (frame_id, &result) {
                    pending_frame = Some(InputFrameContext::captured(frame_id, frame));
                }
                match result {
                    Ok(frame) => response.send(Ok(frame)).map_err(|_| {
                        close_after_failure(
                            backends.take(),
                            ExecutionKernelError::fatal("execution_session_response_lost"),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::LocalOnly,
                        )
                    })?,
                    Err(error) => {
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
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
                            backends,
                            error,
                            ResourceCloseOrder::CaptureFirst,
                        );
                    }
                }
            }
            SessionCommand::CommitFrame {
                reference,
                response,
            } => {
                let result = if let Some(frame) = pending_frame
                    .take()
                    .filter(|frame| frame.reference == reference)
                {
                    committed_frame = Some(frame.clone());
                    Ok(frame)
                } else {
                    committed_frame = None;
                    Err(ExecutionKernelError::fatal("input_frame_capture_mismatch"))
                };
                if response.send(result).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::ResolveFrame {
                reference,
                response,
            } => {
                let result = committed_frame
                    .as_ref()
                    .filter(|frame| frame.reference == reference)
                    .cloned()
                    .ok_or_else(|| ExecutionKernelError::fatal("input_frame_not_committed"));
                if response.send(result).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::ObserveGeometry { deadline, response } => {
                let result = (|| {
                    geometry_remaining(deadline)?;
                    let backend = match backends {
                        SessionBackends::Independent {
                            capture: Some(backend),
                            ..
                        } => backend.as_mut(),
                        SessionBackends::Nemu(pair) => pair.capture.as_mut(),
                        SessionBackends::Pending
                        | SessionBackends::Independent { capture: None, .. } => {
                            return Ok(CaptureGeometryObservation::Unknown(
                                CaptureGeometryUnknownReason::ProducerObservationAbsent,
                            ));
                        }
                    };
                    let observation = backend.observe_geometry(deadline).map_err(|error| {
                        ExecutionKernelError::device("capture_geometry_read_failed", &error)
                    })?;
                    geometry_remaining(deadline)?;
                    Ok(observation)
                })();
                if let Err(mpsc::SendError(result)) = response.send(result) {
                    let primary = match result {
                        // An expired caller has already returned Unavailable. A late value
                        // does not establish a successful Task check or admit cleanup.
                        Ok(_) if Instant::now() >= deadline => continue,
                        Ok(_) => ExecutionKernelError::fatal("execution_session_response_lost"),
                        Err(primary) => primary,
                    };
                    // Preserve backend errors and unexpected reply loss for the owner's Close.
                    let cleanup = close_retained_after_failure(
                        &receiver,
                        backends,
                        primary.clone(),
                        ResourceCloseOrder::CaptureFirst,
                    );
                    return Err(match cleanup {
                        Ok(()) => primary,
                        Err(cleanup) => ExecutionKernelError::merge(primary, cleanup),
                    });
                }
            }
            SessionCommand::ApplicationLifecycle {
                action,
                step,
                response,
            } => {
                pending_frame = None;
                committed_frame = None;
                let invalidation = match backends {
                    SessionBackends::Nemu(pair) => {
                        pair.owner.invalidate_display().map_err(|error| {
                            ExecutionKernelError::device(
                                "application_frame_invalidation_failed",
                                &error,
                            )
                        })
                    }
                    _ => close_resources(
                        backends.take(),
                        DeviceCloseAuthority::LocalOnly,
                        ResourceCloseOrder::CaptureFirst,
                        None,
                    )
                    .map(|_| ()),
                };
                if let Err(error) = invalidation {
                    drop(step);
                    if response.send(Err(error.clone())).is_err() {
                        return Err(close_after_failure(
                            backends.take(),
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
                        backends,
                        error,
                        ResourceCloseOrder::CaptureFirst,
                    );
                }
                let result = provider
                    .control_application(&instance_alias, action)
                    .map_err(|error| {
                        ExecutionKernelError::device("application_backend_operation_failed", &error)
                    });
                drop(step);
                if let Err(error) = result {
                    if response.send(Err(error.clone())).is_err() {
                        return Err(close_after_failure(
                            backends.take(),
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
                        backends,
                        error,
                        ResourceCloseOrder::CaptureFirst,
                    );
                }
                if response.send(Ok(())).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::Close {
                authority,
                input_check,
                response,
            } => {
                let result = close_resources(
                    backends.take(),
                    authority,
                    ResourceCloseOrder::CaptureFirst,
                    input_check,
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
        backends.take(),
        DeviceCloseAuthority::LocalOnly,
        ResourceCloseOrder::CaptureFirst,
        None,
    )
    .map(|_| ())
}

fn close_retained_after_failure(
    receiver: &Receiver<SessionCommand>,
    backends: &mut SessionBackends,
    primary: ExecutionKernelError,
    order: ResourceCloseOrder,
) -> ExecutionKernelResult<()> {
    // Keep the actual backends here until the Host chooses close admission.
    let mut geometry_response_lost = false;
    loop {
        match receiver.recv() {
            Ok(
                SessionCommand::CommitFrame { response, .. }
                | SessionCommand::ResolveFrame { response, .. },
            ) => {
                if response
                    .send(Err(ExecutionKernelError::device(
                        "execution_session_close_pending",
                        &DeviceError::transient(
                            "input frame is unavailable while its owner closes the session",
                        ),
                    )))
                    .is_ok()
                {
                    continue;
                }
                return Err(close_after_failure(
                    backends.take(),
                    primary,
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
            Ok(SessionCommand::Close {
                authority,
                input_check,
                response,
            }) => {
                let result = close_resources(backends.take(), authority, order, input_check);
                if response.send(result.clone()).is_err() {
                    return Err(match result {
                        Ok(outcome) => {
                            ExecutionKernelError::fatal("execution_session_response_lost")
                                .with_stdio_observations(outcome.vendor_stdio())
                        }
                        Err(cleanup) => cleanup,
                    });
                }
                if geometry_response_lost {
                    let primary = ExecutionKernelError::merge(
                        primary,
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                    );
                    return Err(match result {
                        Ok(_) => primary,
                        Err(cleanup) => ExecutionKernelError::merge(primary, cleanup),
                    });
                }
                return result.map(|_| ());
            }
            Ok(SessionCommand::Capture { response, .. }) => {
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
                    backends.take(),
                    ExecutionKernelError::merge(
                        primary,
                        ExecutionKernelError::fatal("execution_session_response_lost"),
                    ),
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
            Ok(SessionCommand::ObserveGeometry { deadline, response }) => {
                let rejected = response.send(Err(geometry_unavailable(
                    "execution_session_close_pending",
                    "geometry is unavailable while the resource owner closes the session",
                )));
                if rejected.is_err() && Instant::now() < deadline {
                    geometry_response_lost = true;
                }
                // Expired observers cannot consume the close handoff; unexpected reply loss
                // is returned with the original primary after the owner's Close.
                continue;
            }
            _ => {
                return Err(close_after_failure(
                    backends.take(),
                    primary,
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
        }
    }
}

fn observed_open_error(
    code: &'static str,
    entry: actingcommand_contract::BackendOpenEntry,
    mut error: DeviceError,
) -> ExecutionKernelError {
    if error.backend_open_observations().is_empty() {
        let mut report = actingcommand_contract::BackendOpenReport::unobserved(entry);
        report.status = actingcommand_contract::BackendObservationStatus::Failed;
        error = error.with_backend_open_observation(
            actingcommand_device::BackendOpenObservation::new(report),
        );
    }
    ExecutionKernelError::device(code, &error)
}

fn execute_input(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backends: &mut SessionBackends,
    action: PreparedInputAction,
    frame: Option<InputFrameReference>,
    committed_frame: Option<&InputFrameContext>,
    check: Option<Arc<dyn InputOperationCheck>>,
) -> ExecutionKernelResult<ExecutionInputOutcome> {
    let mut observations = backends.prepare(provider, instance_alias)?;
    let execute = || -> ExecutionKernelResult<ExecutionInputOutcome> {
        let context = match backends {
            SessionBackends::Nemu(_) => {
                let reference = frame
                    .ok_or_else(|| ExecutionKernelError::fatal("nemu_input_frame_required"))?;
                let committed = committed_frame
                    .filter(|value| value.reference == reference)
                    .ok_or_else(|| ExecutionKernelError::fatal("input_frame_not_committed"))?;
                if committed.geometry.is_none() {
                    return Err(ExecutionKernelError::fatal(
                        "nemu_input_frame_source_missing",
                    ));
                }
                Some(InputExecutionContext {
                    geometry: committed.geometry.clone(),
                    check: check.ok_or_else(|| {
                        ExecutionKernelError::fatal("nemu_input_fencing_required")
                    })?,
                })
            }
            _ => None,
        };
        let backend = match backends {
            SessionBackends::Independent { input, .. } => {
                if input.is_none() {
                    let opened = provider.open_input(instance_alias).map_err(|error| {
                        observed_open_error(
                            "input_backend_open_failed",
                            actingcommand_contract::BackendOpenEntry::Input,
                            error,
                        )
                    })?;
                    observations.push(opened.observation);
                    *input = Some(opened.backend);
                }
                input
                    .as_mut()
                    .ok_or_else(|| ExecutionKernelError::fatal("input_backend_missing"))?
                    .as_mut()
            }
            SessionBackends::Nemu(pair) => pair.input.as_mut(),
            SessionBackends::Pending => {
                return Err(ExecutionKernelError::fatal("input_backend_missing"));
            }
        };
        let recovery = backend.take_adb_recovery();
        execute_action(backend, &action, context.as_ref()).map_err(|error| {
            let error = match &recovery {
                Some(report) => error.with_adb_recovery(report.clone()),
                None => error,
            };
            ExecutionKernelError::device("input_backend_operation_failed", &error)
        })?;
        Ok(ExecutionInputOutcome {
            backend_open_observations: Vec::new(),
            selection: backend.selection_context(),
            recovery: recovery.as_ref().map(crate::error::adb_recovery_record),
        })
    };
    let result = execute();
    match result {
        Ok(mut outcome) => {
            outcome.backend_open_observations = observations;
            Ok(outcome)
        }
        Err(error) => Err(error.with_backend_open_observations(&observations)),
    }
}

fn execute_capture(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backends: &mut SessionBackends,
    memory: Option<&actingcommand_device::FrameMemoryBudget>,
) -> ExecutionKernelResult<Frame> {
    let mut observations = backends.prepare(provider, instance_alias)?;
    let mut execute = || -> ExecutionKernelResult<Frame> {
        let backend = match backends {
            SessionBackends::Independent { capture, .. } => {
                if capture.is_none() {
                    let opened =
                        provider
                            .open_capture(instance_alias, memory)
                            .map_err(|error| {
                                observed_open_error(
                                    "capture_backend_open_failed",
                                    actingcommand_contract::BackendOpenEntry::Capture,
                                    error,
                                )
                            })?;
                    observations.push(opened.observation);
                    *capture = Some(opened.backend);
                }
                capture
                    .as_mut()
                    .ok_or_else(|| ExecutionKernelError::fatal("capture_backend_missing"))?
                    .as_mut()
            }
            SessionBackends::Nemu(pair) => pair.capture.as_mut(),
            SessionBackends::Pending => {
                return Err(ExecutionKernelError::fatal("capture_backend_missing"));
            }
        };
        backend
            .capture()
            .and_then(|mut frame| {
                if let Some(memory) = memory {
                    frame.admit_memory(memory)?;
                }
                Ok(frame)
            })
            .map_err(|error| {
                ExecutionKernelError::device("capture_backend_operation_failed", &error)
            })
    };
    let result = execute();
    match result {
        Ok(mut frame) => {
            frame.backend_open_observations = observations;
            Ok(frame)
        }
        Err(error) => Err(error.with_backend_open_observations(&observations)),
    }
}

fn execute_action(
    backend: &mut dyn InputBackend,
    action: &PreparedInputAction,
    context: Option<&InputExecutionContext>,
) -> DeviceResult<()> {
    match action {
        PreparedInputAction::Direct(InputAction::Tap { x, y }) => match context {
            Some(context) => backend.tap_in_frame(*x, *y, context),
            None => backend.tap(*x, *y),
        },
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
            match context {
                Some(context) => backend.segmented_swipe_prepared_in_frame(plan, context),
                None => backend.segmented_swipe_prepared(plan),
            }
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
    backends: SessionBackends,
    primary: ExecutionKernelError,
    order: ResourceCloseOrder,
    authority: DeviceCloseAuthority,
) -> ExecutionKernelError {
    match close_resources(backends, authority, order, None) {
        Ok(outcome) => primary.with_stdio_observations(outcome.vendor_stdio()),
        Err(secondary) => ExecutionKernelError::merge_cleanup(primary, secondary),
    }
}

fn close_resources(
    backends: SessionBackends,
    authority: DeviceCloseAuthority,
    order: ResourceCloseOrder,
    input_check: Option<Arc<dyn InputOperationCheck>>,
) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
    let (capture, input) = match backends {
        SessionBackends::Pending => return Ok(ExecutionResourceCloseOutcome::confirmed(0)),
        SessionBackends::Independent { capture, input } => (capture, input),
        SessionBackends::Nemu(pair) => {
            let NemuSessionBackends {
                owner,
                input,
                capture,
            } = pair;
            drop(input);
            drop(capture);
            return owner
                .close_once(authority, input_check)
                .map(ExecutionResourceCloseOutcome::from_device)
                .map_err(|error| {
                    ExecutionKernelError::device("nemu_session_close_failed", &error)
                });
        }
    };
    let (first, second) = match order {
        ResourceCloseOrder::CaptureFirst => (
            close_capture(capture, authority.clone()),
            close_input(input, authority),
        ),
        ResourceCloseOrder::InputFirst => (
            close_input(input, authority.clone()),
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
