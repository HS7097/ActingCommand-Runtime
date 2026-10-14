// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::ExecutionKernelCode;
use crate::{
    ExecutionBackendProvider, ExecutionKernelError, ExecutionKernelResult,
    ResolvedExecutionInstance,
};
use actingcommand_contract::codes::{DeviceSelfCheckFailure, SharedCode};
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{
    ApplicationLifecycleAction, CaptureGeometryObservation, CaptureGeometryUnknownReason,
    FencedWrite, FrameId, InputAction, InputFrameReference, ResourceDisposition,
    ResourceDispositionScope, ResourceQuiescence,
};
use actingcommand_device::{
    CaptureBackend, DeviceCloseAuthority, DeviceError, DeviceResourceClosePhase,
    DeviceResourceKind, DeviceResourceQuiescence, DeviceResult, Frame, InputBackend,
    InputExecutionContext, InputOperationCheck, NemuFrameGeometry, NemuSessionBackends,
    PreparedSegmentedSwipePlan, SegmentedSwipeAction, prepare_segmented_swipe,
    segmented_swipe_capability_error,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
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
    pub(crate) fn same_geometry(&self, other: &Self) -> bool {
        self.reference.width == other.reference.width
            && self.reference.height == other.reference.height
            && self.geometry == other.geometry
    }

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
    /// A completed close attempt still owns its failure, including unconfirmed native
    /// resources. The original owner's Close consumes this result without retrying it.
    CloseFailed(Box<ExecutionKernelError>),
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
        memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> ExecutionKernelResult<Vec<actingcommand_device::BackendOpenObservation>> {
        if let Self::CloseFailed(error) = self {
            return Err(error.as_ref().clone());
        }
        if matches!(self, Self::Pending) {
            let mut observations = Vec::new();
            *self = match provider.open_nemu_session(alias, memory).map_err(|error| {
                observed_open_error(
                    DeviceSelfCheckFailure::PairedBackendOpenFailed.code(),
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
    /// The successful synchronous action call only, before outcome metadata is collected.
    pub touch_response_us: Option<u64>,
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
        // Workflow #387 D1c-F: every producer refuses an invalid segmented swipe under its own
        // code before it gets here (a pack at admission, a request by its contract check), so a
        // plan that cannot be prepared is a Runtime-internal defect of this one action. No device
        // work has happened: the error carries no device severity and no resource disposition.
        let plan = prepare_segmented_swipe(SegmentedSwipeAction {
            points: [(x1, y1), (x2, y2), (x3, y3)],
            horizontal_duration_ms,
            corner_hold_ms,
            brake_distance_px,
            brake_duration_ms,
            slope_in,
            slope_out,
        })
        .map_err(|_| {
            ExecutionKernelError::fatal(ExecutionKernelCode::InputPlanPreparationFailed.code())
        })?;
        Ok(Self::SegmentedSwipe(plan))
    }
}

enum SessionCommand {
    Input {
        action: PreparedInputAction,
        frame: Option<InputFrameReference>,
        check: Option<Arc<dyn InputOperationCheck>>,
        step: Arc<FencedWrite>,
        response: SyncSender<ExecutionKernelResult<ExecutionInputOutcome>>,
    },
    Capture {
        frame_id: Option<FrameId>,
        memory: Option<actingcommand_device::FrameMemoryBudget>,
        /// #398 R1 §5.2 (RD-3i): the floor the captured frame must meet.
        floor: Option<actingcommand_device::CaptureFloor>,
        response: SyncSender<ExecutionKernelResult<Frame>>,
    },
    OpenBackends {
        deadline: Option<Instant>,
        memory: actingcommand_device::FrameMemoryBudget,
        response:
            SyncSender<ExecutionKernelResult<Vec<actingcommand_device::BackendOpenObservation>>>,
    },
    CommitFrame {
        reference: InputFrameReference,
        response: SyncSender<ExecutionKernelResult<InputFrameContext>>,
    },
    ResolveFrame {
        reference: InputFrameReference,
        response: SyncSender<ExecutionKernelResult<InputFrameContext>>,
    },
    /// Workflow #191 H: a lease end keeps the session and drops its input frames.
    ForgetInputFrames {
        response: SyncSender<ExecutionKernelResult<()>>,
    },
    ObserveGeometry {
        deadline: Instant,
        response: SyncSender<ExecutionKernelResult<CaptureGeometryObservation>>,
    },
    ApplicationLifecycle {
        action: ApplicationLifecycleAction,
        step: Arc<FencedWrite>,
        response: SyncSender<ExecutionKernelResult<()>>,
    },
    PrepareApplication {
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
        response: SyncSender<ExecutionKernelResult<ExecutionResourceCloseOutcome>>,
    },
    Close {
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
        response: SyncSender<ExecutionKernelResult<ExecutionResourceCloseOutcome>>,
    },
}

#[derive(Debug, Clone)]
pub struct ExecutionResourceCloseOutcome {
    resource_count: u16,
    vendor_stdio: Vec<crate::ExecutionStdioObservation>,
    pub(crate) resources: Vec<crate::error::ExecutionResourceContribution>,
}

impl PartialEq for ExecutionResourceCloseOutcome {
    fn eq(&self, other: &Self) -> bool {
        self.resource_count == other.resource_count
            && self.vendor_stdio == other.vendor_stdio
            && self.resource_dispositions() == other.resource_dispositions()
    }
}
impl Eq for ExecutionResourceCloseOutcome {}

impl ExecutionResourceCloseOutcome {
    pub(crate) fn confirmed(resource_count: u16) -> Self {
        Self {
            resource_count,
            vendor_stdio: Vec::new(),
            resources: vec![crate::error::ExecutionResourceContribution {
                occurrence: Arc::default(),
                disposition: ResourceDisposition {
                    scope: ResourceDispositionScope::SessionBackends,
                    resource_count,
                    quiescence: ResourceQuiescence::Confirmed,
                },
            }],
        }
    }

    fn from_device(outcome: actingcommand_device::DeviceResourceCloseOutcome) -> Self {
        Self {
            resource_count: outcome.resource_count(),
            resources: outcome
                .resource_contributions()
                .map(|(occurrence, quiescence, resource_count, scope)| {
                    crate::error::ExecutionResourceContribution {
                        occurrence: Arc::clone(occurrence),
                        disposition: ResourceDisposition {
                            scope: scope.unwrap_or(ResourceDispositionScope::SessionBackends),
                            resource_count,
                            quiescence: crate::error::runtime_quiescence(quiescence),
                        },
                    }
                })
                .collect(),
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

    pub fn resource_dispositions(&self) -> Option<Vec<ResourceDisposition>> {
        crate::error::resource_dispositions(&self.resources)
    }

    fn with_confirmed_failure(mut self, primary: &ExecutionKernelError) -> Self {
        crate::error::merge_resource_contributions(
            &mut self.resources,
            primary.resource_contributions(),
        );
        self.resource_count = self.resources.iter().fold(0_u16, |count, item| {
            count
                .checked_add(item.disposition.resource_count)
                .expect("execution resource count overflow")
        });
        crate::error::merge_stdio_observations(&mut self.vendor_stdio, primary.vendor_stdio());
        self
    }

    fn combine(mut self, other: Self) -> Self {
        crate::error::merge_resource_contributions(&mut self.resources, &other.resources);
        self.resource_count = self.resources.iter().fold(0_u16, |count, item| {
            count
                .checked_add(item.disposition.resource_count)
                .expect("execution resource count overflow")
        });
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
    /// Workflow #191 H: set once the worker has stopped or holds its backends for the owner's
    /// close after a failure; such a session is closed, never kept, at a lease end.
    needs_close: Arc<AtomicBool>,
}

impl ExecutionSession {
    pub(crate) fn start(
        provider: Arc<dyn ExecutionBackendProvider>,
        instance_alias: String,
        resolved: ResolvedExecutionInstance,
        generation: u64,
    ) -> ExecutionKernelResult<Self> {
        let (sender, receiver) = mpsc::sync_channel(SESSION_CHANNEL_CAPACITY);
        let needs_close = Arc::new(AtomicBool::new(false));
        let worker_needs_close = Arc::clone(&needs_close);
        let join = thread::Builder::new()
            .name("actingcommand-execution-session".to_string())
            .spawn(move || {
                let mut backends = SessionBackends::Pending;
                let result = match catch_unwind(AssertUnwindSafe(|| {
                    run_session(
                        provider,
                        instance_alias,
                        receiver,
                        &mut backends,
                        generation,
                        &worker_needs_close,
                    )
                })) {
                    Ok(result) => result,
                    Err(_) => Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionPanicked.code(),
                        ),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    )),
                };
                worker_needs_close.store(true, Ordering::Release);
                result
            })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionSpawnFailed.code())
            })?;
        Ok(Self {
            resolved,
            state: Mutex::new(SessionState {
                sender: Some(sender),
                join: Some(join),
                closed: false,
                close_result: None,
            }),
            needs_close,
        })
    }

    pub const fn resolved(&self) -> &ResolvedExecutionInstance {
        &self.resolved
    }

    /// Workflow #191 H: the worker has stopped or retained its backends after a failure.
    pub(crate) fn needs_close(&self) -> bool {
        self.needs_close.load(Ordering::Acquire)
    }

    pub fn input(&self, action: InputAction, step: Arc<FencedWrite>) -> ExecutionKernelResult<()> {
        self.input_prepared(action.try_into()?, step).map(|_| ())
    }

    pub(crate) fn input_prepared(
        &self,
        action: PreparedInputAction,
        step: Arc<FencedWrite>,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        match self.input_prepared_retained(action, step) {
            Ok(selection) => Ok(selection),
            Err(primary) => {
                let error = match self.close_with_authority(DeviceCloseAuthority::LocalOnly) {
                    Ok(_) => primary,
                    Err(cleanup) => ExecutionKernelError::merge_cleanup(primary, cleanup),
                };
                let mut state =
                    self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
                finish_after_result(&mut state, Err(error))
            }
        }
    }

    pub(crate) fn input_prepared_retained(
        &self,
        action: PreparedInputAction,
        step: Arc<FencedWrite>,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        self.input_prepared_in_frame(action, None, None, step)
    }

    pub(crate) fn input_prepared_in_frame(
        &self,
        action: PreparedInputAction,
        frame: Option<InputFrameReference>,
        check: Option<Arc<dyn InputOperationCheck>>,
        step: Arc<FencedWrite>,
    ) -> ExecutionKernelResult<ExecutionInputOutcome> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(SessionCommand::Input {
                action,
                frame,
                check,
                step,
                response,
            })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
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
        self.capture_frame_retained(None, None, None)
    }

    pub(crate) fn capture_frame_retained(
        &self,
        frame_id: Option<FrameId>,
        memory: Option<actingcommand_device::FrameMemoryBudget>,
        floor: Option<actingcommand_device::CaptureFloor>,
    ) -> ExecutionKernelResult<Frame> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(SessionCommand::Capture {
                frame_id,
                memory,
                floor,
                response,
            })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                )),
            ),
        }
    }

    /// Workflow #191 ps2: opens every backend this session does not hold yet, now instead of at
    /// the first input or capture, through the same provider opens as the lazy paths. A held
    /// backend is reused; a Nemu pair opens once. Returns the reports of the opens it made. A
    /// failure keeps the session for the Host's close, as a retained capture failure does.
    pub(crate) fn open_backends(
        &self,
        memory: actingcommand_device::FrameMemoryBudget,
        deadline: Option<Instant>,
    ) -> ExecutionKernelResult<Vec<actingcommand_device::BackendOpenObservation>> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(SessionCommand::OpenBackends {
                memory,
                deadline,
                response,
            })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
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

    /// Workflow #191 H: drops the pending and committed input frames of a kept session.
    pub(crate) fn forget_input_frames(&self) -> ExecutionKernelResult<()> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let sent = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(SessionCommand::ForgetInputFrames { response })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = sent {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                )),
            ),
        }
    }

    fn frame_request(
        &self,
        command: impl FnOnce(SyncSender<ExecutionKernelResult<InputFrameContext>>) -> SessionCommand,
    ) -> ExecutionKernelResult<InputFrameContext> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let sent = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(command(response))
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = sent {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                )),
            ),
        }
    }

    /// Retires independent backends under the original owner's close authority while
    /// preserving this session and its generation. Paired Nemu keeps its owner.
    pub(crate) fn prepare_application_resources(
        &self,
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
    ) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let sent = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(SessionCommand::PrepareApplication {
                authority,
                input_check,
                response,
            })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = sent {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                )),
            ),
        }
    }

    pub fn control_application(
        &self,
        action: ApplicationLifecycleAction,
        step: Arc<FencedWrite>,
    ) -> ExecutionKernelResult<()> {
        match self.control_application_retained(action, step) {
            Ok(()) => Ok(()),
            Err(primary) => {
                let error = match self.close_with_authority(DeviceCloseAuthority::LocalOnly) {
                    Ok(_) => primary,
                    Err(cleanup) => ExecutionKernelError::merge_cleanup(primary, cleanup),
                };
                let mut state =
                    self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
                finish_after_result(&mut state, Err(error))
            }
        }
    }

    pub(crate) fn control_application_retained(
        &self,
        action: ApplicationLifecycleAction,
        step: Arc<FencedWrite>,
    ) -> ExecutionKernelResult<()> {
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
        ensure_open(&state)?;
        let (response, receiver) = mpsc::sync_channel(1);
        let send_result = state
            .sender
            .as_ref()
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .send(SessionCommand::ApplicationLifecycle {
                action,
                step,
                response,
            })
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        if let Err(error) = send_result {
            return finish_after_result(&mut state, Err(error));
        }
        match receiver.recv() {
            Ok(result) => result,
            Err(_) => finish_after_result(
                &mut state,
                Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
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
            .ok_or_else(|| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionClosed.code())
            })?
            .try_send(SessionCommand::ObserveGeometry { deadline, response })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => geometry_unavailable(
                    ExecutionKernelCode::CaptureGeometryQueueFull.code(),
                    "the existing session command queue is full",
                ),
                mpsc::TrySendError::Disconnected(_) => ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionUnavailable.code(),
                ),
            })?;
        drop(state);
        match receiver.recv_timeout(geometry_remaining(deadline)?) {
            Ok(Ok(observation)) => {
                geometry_remaining(deadline)?;
                Ok(observation)
            }
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(geometry_unavailable(
                ExecutionKernelCode::CaptureGeometryDeadlineElapsed.code(),
                "the geometry reply did not arrive within the task deadline",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(ExecutionKernelError::fatal(
                ExecutionKernelCode::ExecutionSessionResponseLost.code(),
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
                ExecutionKernelCode::CaptureGeometrySessionBusy.code(),
                "the existing session state is busy",
            ),
            TryLockError::Poisoned(_) => ExecutionKernelError::fatal(
                ExecutionKernelCode::ExecutionSessionStatePoisoned.code(),
            ),
        })?;
        if state.closed || state.sender.is_none() {
            return Err(state
                .close_result
                .as_ref()
                .and_then(|result| result.as_ref().err())
                .cloned()
                .unwrap_or_else(|| {
                    geometry_unavailable(
                        ExecutionKernelCode::CaptureGeometrySessionClosed.code(),
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
        let mut state =
            self.lock_state(ExecutionKernelCode::ExecutionSessionStatePoisoned.code())?;
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
            .map_err(|_| {
                ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionUnavailable.code())
            });
        drop(sender);
        let close_result = send_result.and_then(|()| {
            receiver.recv().map_err(|_| {
                ExecutionKernelError::fatal(
                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                )
            })?
        });
        let result = match (close_result, join_session(&mut state)) {
            (Ok(outcome), Ok(())) => Ok(outcome),
            (Err(error), Ok(())) => Err(error),
            (Ok(outcome), Err(error)) => {
                Err(error.with_close_outcome(&outcome).with_close_recording())
            }
            (Err(primary), Err(secondary)) => Err(ExecutionKernelError::merge(primary, secondary)),
        };
        state.close_result = Some(result.clone());
        result
    }

    fn lock_state(&self, code: Code) -> ExecutionKernelResult<MutexGuard<'_, SessionState>> {
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
        Err(ExecutionKernelError::fatal(
            ExecutionKernelCode::ExecutionSessionClosed.code(),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn geometry_unavailable(code: Code, detail: &'static str) -> ExecutionKernelError {
    ExecutionKernelError::device(code, &DeviceError::transient(detail))
}

pub(crate) fn geometry_remaining(deadline: Instant) -> ExecutionKernelResult<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            geometry_unavailable(
                ExecutionKernelCode::CaptureGeometryDeadlineElapsed.code(),
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
    join.join().map_err(|_| {
        ExecutionKernelError::fatal(ExecutionKernelCode::ExecutionSessionPanicked.code())
    })?
}

fn run_session(
    provider: Arc<dyn ExecutionBackendProvider>,
    instance_alias: String,
    receiver: Receiver<SessionCommand>,
    backends: &mut SessionBackends,
    generation: u64,
    needs_close: &AtomicBool,
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
                    &step,
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
                        // Workflow #191 H: marked first; the Host may end the lease on this reply.
                        needs_close.store(true, Ordering::Release);
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
                                ExecutionKernelError::merge(
                                    error,
                                    ExecutionKernelError::fatal(
                                        ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                    ),
                                ),
                                ResourceCloseOrder::InputFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                        return close_retained_after_failure(
                            &receiver,
                            needs_close,
                            backends,
                            error,
                            ResourceCloseOrder::InputFirst,
                        );
                    }
                };
                if response.send(Ok(context)).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::Capture {
                frame_id,
                memory,
                floor,
                response,
            } => {
                pending_frame = None;
                committed_frame = None;
                let result = execute_capture(
                    provider.as_ref(),
                    &instance_alias,
                    backends,
                    memory.as_ref(),
                    floor,
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
                            ExecutionKernelError::fatal(
                                ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                            ),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::LocalOnly,
                        )
                    })?,
                    Err(error) => {
                        // Workflow #191 H: marked first; the Host may end the lease on this reply.
                        needs_close.store(true, Ordering::Release);
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
                                ExecutionKernelError::merge(
                                    error,
                                    ExecutionKernelError::fatal(
                                        ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                    ),
                                ),
                                ResourceCloseOrder::CaptureFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                        return close_retained_after_failure(
                            &receiver,
                            needs_close,
                            backends,
                            error,
                            ResourceCloseOrder::CaptureFirst,
                        );
                    }
                }
            }
            SessionCommand::OpenBackends {
                memory,
                deadline,
                response,
            } => {
                let capture_opens = matches!(
                    backends,
                    SessionBackends::Pending | SessionBackends::Independent { capture: None, .. }
                );
                if capture_opens {
                    pending_frame = None;
                    committed_frame = None;
                }
                let result = open_backends(
                    provider.as_ref(),
                    &instance_alias,
                    backends,
                    &memory,
                    deadline,
                )
                .map(|mut observations| {
                    for observation in &mut observations {
                        observation.report.session_generation = generation;
                    }
                    observations
                })
                .map_err(|error| error.with_backend_session_generation(generation));
                match result {
                    Ok(observations) => response.send(Ok(observations)).map_err(|_| {
                        close_after_failure(
                            backends.take(),
                            ExecutionKernelError::fatal(
                                ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                            ),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::LocalOnly,
                        )
                    })?,
                    Err(error) => {
                        // Workflow #191 H: marked first; the Host may end the lease on this reply.
                        needs_close.store(true, Ordering::Release);
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
                                ExecutionKernelError::merge(
                                    error,
                                    ExecutionKernelError::fatal(
                                        ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                    ),
                                ),
                                ResourceCloseOrder::CaptureFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                        return close_retained_after_failure(
                            &receiver,
                            needs_close,
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
                    Err(ExecutionKernelError::fatal(
                        ExecutionKernelCode::InputFrameCaptureMismatch.code(),
                    ))
                };
                if response.send(result).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
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
                    .ok_or_else(|| {
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::InputFrameNotCommitted.code(),
                        )
                    });
                if response.send(result).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::ForgetInputFrames { response } => {
                pending_frame = None;
                committed_frame = None;
                if response.send(Ok(())).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                        ResourceCloseOrder::CaptureFirst,
                        DeviceCloseAuthority::LocalOnly,
                    ));
                }
            }
            SessionCommand::ObserveGeometry { deadline, response } => {
                let result = (|| {
                    geometry_remaining(deadline)?;
                    let backend = match backends {
                        SessionBackends::CloseFailed(error) => return Err(error.as_ref().clone()),
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
                        ExecutionKernelError::device(
                            ExecutionKernelCode::CaptureGeometryReadFailed.code(),
                            &error,
                        )
                    })?;
                    geometry_remaining(deadline)?;
                    Ok(observation)
                })();
                if let Err(mpsc::SendError(result)) = response.send(result) {
                    let primary = match result {
                        // An expired caller has already returned Unavailable. A late value
                        // does not establish a successful Task check or admit cleanup.
                        Ok(_) if Instant::now() >= deadline => continue,
                        Ok(_) => ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                        Err(primary) => primary,
                    };
                    // Preserve backend errors and unexpected reply loss for the owner's Close.
                    let cleanup = close_retained_after_failure(
                        &receiver,
                        needs_close,
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
            SessionCommand::PrepareApplication {
                authority,
                input_check,
                response,
            } => {
                pending_frame = None;
                committed_frame = None;
                // This call drops every witness/check reference before the response; Host
                // must finish ResourceClose before it can issue the Business step.
                let result = prepare_application_backends(backends, authority, input_check);
                match result {
                    Ok(outcome) => {
                        if response.send(Ok(outcome)).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
                                ExecutionKernelError::fatal(
                                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                ),
                                ResourceCloseOrder::CaptureFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                    }
                    Err(error) => {
                        needs_close.store(true, Ordering::Release);
                        if response.send(Err(error.clone())).is_err() {
                            return Err(close_after_failure(
                                backends.take(),
                                ExecutionKernelError::merge(
                                    error,
                                    ExecutionKernelError::fatal(
                                        ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                    ),
                                ),
                                ResourceCloseOrder::CaptureFirst,
                                DeviceCloseAuthority::LocalOnly,
                            ));
                        }
                        return close_retained_after_failure(
                            &receiver,
                            needs_close,
                            backends,
                            error,
                            ResourceCloseOrder::CaptureFirst,
                        );
                    }
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
                                ExecutionKernelCode::ApplicationFrameInvalidationFailed.code(),
                                &error,
                            )
                        })
                    }
                    SessionBackends::Pending
                    | SessionBackends::Independent {
                        input: None,
                        capture: None,
                        ..
                    } => Ok(()),
                    SessionBackends::CloseFailed(error) => Err(error.as_ref().clone()),
                    SessionBackends::Independent { .. } => Err(ExecutionKernelError::fatal(
                        ExecutionKernelCode::ApplicationResourceCloseRequired.code(),
                    )),
                };
                if let Err(error) = invalidation {
                    drop(step);
                    // Workflow #191 H: marked first; the Host may end the lease on this reply.
                    needs_close.store(true, Ordering::Release);
                    if response.send(Err(error.clone())).is_err() {
                        return Err(close_after_failure(
                            backends.take(),
                            ExecutionKernelError::merge(
                                error,
                                ExecutionKernelError::fatal(
                                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                ),
                            ),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::LocalOnly,
                        ));
                    }
                    return close_retained_after_failure(
                        &receiver,
                        needs_close,
                        backends,
                        error,
                        ResourceCloseOrder::CaptureFirst,
                    );
                }
                let result = provider
                    .control_application(&step, &instance_alias, action)
                    .map_err(|error| {
                        ExecutionKernelError::device(
                            SharedCode::ApplicationBackendOperationFailed.code(),
                            &error,
                        )
                    });
                drop(step);
                if let Err(error) = result {
                    // Workflow #191 H: marked first; the Host may end the lease on this reply.
                    needs_close.store(true, Ordering::Release);
                    if response.send(Err(error.clone())).is_err() {
                        return Err(close_after_failure(
                            backends.take(),
                            ExecutionKernelError::merge(
                                error,
                                ExecutionKernelError::fatal(
                                    ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                                ),
                            ),
                            ResourceCloseOrder::CaptureFirst,
                            DeviceCloseAuthority::LocalOnly,
                        ));
                    }
                    return close_retained_after_failure(
                        &receiver,
                        needs_close,
                        backends,
                        error,
                        ResourceCloseOrder::CaptureFirst,
                    );
                }
                if response.send(Ok(())).is_err() {
                    return Err(close_after_failure(
                        backends.take(),
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
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
                )
                .map_err(ExecutionKernelError::with_close_recording);
                if response.send(result.clone()).is_err() {
                    return match result {
                        Ok(outcome) => Err(ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        )
                        .with_close_outcome(&outcome)
                        .with_close_recording()),
                        Err(error) => Err(ExecutionKernelError::merge(
                            error,
                            ExecutionKernelError::fatal(
                                ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                            ),
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
    needs_close: &AtomicBool,
    backends: &mut SessionBackends,
    primary: ExecutionKernelError,
    order: ResourceCloseOrder,
) -> ExecutionKernelResult<()> {
    needs_close.store(true, Ordering::Release);
    // Keep the actual backends here until the Host chooses close admission.
    let mut geometry_response_lost = false;
    loop {
        match receiver.recv() {
            Ok(SessionCommand::PrepareApplication {
                authority,
                input_check,
                response,
            }) => {
                drop(input_check);
                drop(authority);
                // An earlier failure cannot be replaced by application preparation. Keep
                // its actual backends/result until the original owner's terminal Close.
                if response.send(Err(primary.clone())).is_ok() {
                    continue;
                }
                return Err(close_after_failure(
                    backends.take(),
                    ExecutionKernelError::merge(
                        primary,
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                    ),
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
            // Workflow #191 H: handled here explicitly; the catch-all below would close the
            // backends locally and bypass the Host's fenced close.
            Ok(SessionCommand::ForgetInputFrames { response }) => {
                if response
                    .send(Err(ExecutionKernelError::device(
                        ExecutionKernelCode::ExecutionSessionClosePending.code(),
                        &DeviceError::transient(
                            "input frames are unavailable while their owner closes the session",
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
            Ok(
                SessionCommand::CommitFrame { response, .. }
                | SessionCommand::ResolveFrame { response, .. },
            ) => {
                if response
                    .send(Err(ExecutionKernelError::device(
                        ExecutionKernelCode::ExecutionSessionClosePending.code(),
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
                let primary = if geometry_response_lost {
                    ExecutionKernelError::merge(
                        primary,
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                    )
                } else {
                    primary
                };
                let result = match result {
                    Ok(outcome)
                        if primary.resource_quiescence()
                            == Some(ResourceQuiescence::Unconfirmed)
                            || geometry_response_lost =>
                    {
                        Err(primary
                            .clone()
                            .with_close_outcome(&outcome)
                            .with_close_recording())
                    }
                    Ok(outcome) => Ok(outcome.with_confirmed_failure(&primary)),
                    Err(cleanup) => Err(ExecutionKernelError::merge_cleanup(
                        primary.clone(),
                        cleanup,
                    )
                    .with_close_recording()),
                };
                if response.send(result.clone()).is_err() {
                    return Err(match result {
                        Ok(outcome) => ExecutionKernelError::merge(
                            primary,
                            ExecutionKernelError::fatal(
                                ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                            ),
                        )
                        .with_close_outcome(&outcome)
                        .with_close_recording(),
                        Err(cleanup) => ExecutionKernelError::merge(
                            cleanup,
                            ExecutionKernelError::fatal(
                                ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                            ),
                        ),
                    });
                }
                return result.map(|_| ());
            }
            Ok(SessionCommand::Capture { response, .. }) => {
                // A concurrent observer can already be waiting behind the failed input.
                // Refuse that observation without consuming the input owner's close handoff.
                if response
                    .send(Err(ExecutionKernelError::device(
                        ExecutionKernelCode::ExecutionSessionClosePending.code(),
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
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::ExecutionSessionResponseLost.code(),
                        ),
                    ),
                    order,
                    DeviceCloseAuthority::LocalOnly,
                ));
            }
            Ok(SessionCommand::ObserveGeometry { deadline, response }) => {
                let rejected = response.send(Err(geometry_unavailable(
                    ExecutionKernelCode::ExecutionSessionClosePending.code(),
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
    code: Code,
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
    ExecutionKernelError::device_acquisition(code, &error)
}

#[allow(clippy::too_many_arguments)]
fn execute_input(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backends: &mut SessionBackends,
    action: PreparedInputAction,
    frame: Option<InputFrameReference>,
    committed_frame: Option<&InputFrameContext>,
    check: Option<Arc<dyn InputOperationCheck>>,
    witness: &FencedWrite,
) -> ExecutionKernelResult<ExecutionInputOutcome> {
    let mut observations = backends.prepare(provider, instance_alias, None)?;
    let execute = || -> ExecutionKernelResult<ExecutionInputOutcome> {
        let context = match backends {
            SessionBackends::Nemu(..) => {
                let reference = frame.ok_or_else(|| {
                    ExecutionKernelError::fatal(SharedCode::NemuInputFrameRequired.code())
                })?;
                let committed = committed_frame
                    .filter(|value| value.reference == reference)
                    .ok_or_else(|| {
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::InputFrameNotCommitted.code(),
                        )
                    })?;
                if committed.geometry.is_none() {
                    return Err(ExecutionKernelError::fatal(
                        ExecutionKernelCode::NemuInputFrameSourceMissing.code(),
                    ));
                }
                Some(InputExecutionContext {
                    geometry: committed.geometry.clone(),
                    check: check.ok_or_else(|| {
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::NemuInputFencingRequired.code(),
                        )
                    })?,
                })
            }
            _ => None,
        };
        let backend = match backends {
            SessionBackends::CloseFailed(error) => return Err(error.as_ref().clone()),
            SessionBackends::Independent { input, .. } => {
                if input.is_none() {
                    let opened = provider.open_input(instance_alias).map_err(|error| {
                        observed_open_error(
                            DeviceSelfCheckFailure::InputBackendOpenFailed.code(),
                            actingcommand_contract::BackendOpenEntry::Input,
                            error,
                        )
                    })?;
                    observations.push(opened.observation);
                    *input = Some(opened.backend);
                }
                input
                    .as_mut()
                    .ok_or_else(|| {
                        ExecutionKernelError::fatal(ExecutionKernelCode::InputBackendMissing.code())
                    })?
                    .as_mut()
            }
            SessionBackends::Nemu(pair) => pair.input.as_mut(),
            SessionBackends::Pending => {
                return Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::InputBackendMissing.code(),
                ));
            }
        };
        let recovery = backend.take_adb_recovery();
        let started = Instant::now();
        let executed = execute_action(backend, witness, &action, context.as_ref());
        let touch_response_us = Instant::now()
            .checked_duration_since(started)
            .and_then(|span| u64::try_from(span.as_micros()).ok());
        observations.extend(backend.take_backend_open_observations());
        executed.map_err(|mut error| {
            observations.extend(error.take_backend_open_observations());
            let error = match &recovery {
                Some(report) => error.with_adb_recovery(report.clone()),
                None => error,
            };
            ExecutionKernelError::device(SharedCode::InputBackendOperationFailed.code(), &error)
        })?;
        Ok(ExecutionInputOutcome {
            touch_response_us,
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
    floor: Option<actingcommand_device::CaptureFloor>,
) -> ExecutionKernelResult<Frame> {
    let mut observations = backends.prepare(provider, instance_alias, memory)?;
    let mut execute = || -> ExecutionKernelResult<Frame> {
        let backend = match backends {
            SessionBackends::CloseFailed(error) => return Err(error.as_ref().clone()),
            SessionBackends::Independent { capture, .. } => {
                if capture.is_none() {
                    let opened =
                        provider
                            .open_capture(instance_alias, memory)
                            .map_err(|error| {
                                observed_open_error(
                                    DeviceSelfCheckFailure::CaptureBackendOpenFailed.code(),
                                    actingcommand_contract::BackendOpenEntry::Capture,
                                    error,
                                )
                            })?;
                    observations.push(opened.observation);
                    *capture = Some(opened.backend);
                }
                capture
                    .as_mut()
                    .ok_or_else(|| {
                        ExecutionKernelError::fatal(
                            ExecutionKernelCode::CaptureBackendMissing.code(),
                        )
                    })?
                    .as_mut()
            }
            SessionBackends::Nemu(pair) => pair.capture.as_mut(),
            SessionBackends::Pending => {
                return Err(ExecutionKernelError::fatal(
                    ExecutionKernelCode::CaptureBackendMissing.code(),
                ));
            }
        };
        // #398 R1 §5.2 (RD-3i): a snapshot backend meets any floor (`capture_fresh` is its
        // `capture_timed`); a stream backend delivers only a frame newer than the floor.
        let captured = backend.capture_fresh(floor);
        // These reports have not left this Capture request. A reused session has
        // no new open occurrence, so its historical report is never rewritten.
        for observation in &mut observations {
            use actingcommand_contract::{
                BackendObservationStatus, BackendOpenEntry, BackendOpenSource,
            };
            let report = &mut observation.report;
            if matches!(
                report.entry,
                BackendOpenEntry::Capture | BackendOpenEntry::NemuPair
            ) && report.source == BackendOpenSource::Native
            {
                report.capture_check = match &captured {
                    Ok(frame) if frame.validate_layout().is_ok() => {
                        report.frame_width = Some(frame.width);
                        report.frame_height = Some(frame.height);
                        BackendObservationStatus::Passed
                    }
                    _ => BackendObservationStatus::Failed,
                };
            }
        }
        captured
            .and_then(|mut frame| {
                if let Some(memory) = memory {
                    frame.admit_memory(memory)?;
                }
                Ok(frame)
            })
            .map_err(|error| {
                ExecutionKernelError::device(
                    SharedCode::CaptureBackendOperationFailed.code(),
                    &error,
                )
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

/// Workflow #191 ps2: the explicit open behind [`ExecutionSession::open_backends`]. It opens what
/// the lazy paths open, each only when absent: the Nemu pair, else the input and then the capture
/// backend. A capture opened here gives its first frame at once, exactly as the first lazy
/// capture does, so its report carries that frame's check and size and no primed frame outlives
/// this command; the frame itself is dropped.
fn open_backends(
    provider: &dyn ExecutionBackendProvider,
    instance_alias: &str,
    backends: &mut SessionBackends,
    memory: &actingcommand_device::FrameMemoryBudget,
    deadline: Option<Instant>,
) -> ExecutionKernelResult<Vec<actingcommand_device::BackendOpenObservation>> {
    let before_open = |observations: &[actingcommand_device::BackendOpenObservation]| {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Err(ExecutionKernelError::fatal(
                ExecutionKernelCode::PreparationDeadlineExceeded.code(),
            )
            .with_backend_open_observations(observations))
        } else {
            Ok(())
        }
    };
    before_open(&[])?;
    let capture_opens = matches!(
        backends,
        SessionBackends::Pending | SessionBackends::Independent { capture: None, .. }
    );
    let mut observations = backends.prepare(provider, instance_alias, Some(memory))?;
    if let SessionBackends::Independent { input, .. } = backends
        && input.is_none()
    {
        before_open(&observations)?;
        match provider.open_input(instance_alias) {
            Ok(opened) => {
                observations.push(opened.observation);
                *input = Some(opened.backend);
            }
            Err(error) => {
                return Err(observed_open_error(
                    DeviceSelfCheckFailure::InputBackendOpenFailed.code(),
                    actingcommand_contract::BackendOpenEntry::Input,
                    error,
                )
                .with_backend_open_observations(&observations));
            }
        }
    }
    if capture_opens {
        before_open(&observations)?;
        match execute_capture(provider, instance_alias, backends, Some(memory), None) {
            Ok(mut frame) => observations.append(&mut frame.backend_open_observations),
            Err(error) => return Err(error.with_backend_open_observations(&observations)),
        }
    }
    Ok(observations)
}

fn execute_action(
    backend: &mut dyn InputBackend,
    witness: &FencedWrite,
    action: &PreparedInputAction,
    context: Option<&InputExecutionContext>,
) -> DeviceResult<()> {
    match action {
        PreparedInputAction::Direct(InputAction::Tap { x, y }) => match context {
            Some(context) => backend.tap_in_frame(witness, *x, *y, context),
            None => backend.tap(witness, *x, *y),
        },
        PreparedInputAction::Direct(InputAction::LongTap { x, y, duration_ms }) => {
            backend.long_tap(witness, *x, *y, *duration_ms)
        }
        PreparedInputAction::Direct(InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        }) => backend.swipe(witness, *x1, *y1, *x2, *y2, *duration_ms),
        PreparedInputAction::SegmentedSwipe(plan) => {
            if !backend.supports_segmented_swipe() {
                return Err(segmented_swipe_capability_error());
            }
            match context {
                Some(context) => backend.segmented_swipe_prepared_in_frame(witness, plan, context),
                None => backend.segmented_swipe_prepared(witness, plan),
            }
        }
        PreparedInputAction::Direct(InputAction::Key { key }) => backend.key(witness, key),
        PreparedInputAction::Direct(InputAction::Text { text }) => backend.text(witness, text),
        PreparedInputAction::Direct(InputAction::Reset) => backend.reset(witness),
        PreparedInputAction::Direct(InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }) => Err(
            DeviceError::fatal("segmented input action was not prepared"),
        ),
        // Workflow #406 (register §1.18): reserved until T-3; the host refuses it first
        // (`input_action_unsupported`), and no backend performs it.
        PreparedInputAction::Direct(InputAction::SingleTouchPathDragV1 { .. }) => Err(
            DeviceError::fatal("the single-touch path drag is reserved until #406 T-3"),
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
        Ok(outcome) => primary.with_close_outcome(&outcome),
        Err(secondary) => ExecutionKernelError::merge_cleanup(primary, secondary),
    }
    .with_close_recording()
}

fn prepare_application_backends(
    backends: &mut SessionBackends,
    authority: DeviceCloseAuthority,
    input_check: Option<Arc<dyn InputOperationCheck>>,
) -> ExecutionKernelResult<ExecutionResourceCloseOutcome> {
    match backends {
        SessionBackends::Pending | SessionBackends::Nemu(..) => {
            Ok(ExecutionResourceCloseOutcome::confirmed(0))
        }
        SessionBackends::CloseFailed(error) => Err(error.as_ref().clone()),
        SessionBackends::Independent { .. } => {
            let result = close_resources(
                backends.take(),
                authority,
                ResourceCloseOrder::CaptureFirst,
                input_check,
            );
            if let Err(error) = &result {
                *backends = SessionBackends::CloseFailed(Box::new(error.clone()));
            }
            result
        }
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
        SessionBackends::CloseFailed(error) => return Err(*error),
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
                    ExecutionKernelError::device_close(
                        ExecutionKernelCode::NemuSessionCloseFailed.code(),
                        &error,
                    )
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
            Err(error.with_close_outcome(&outcome))
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
            let count = u16::from(error.resource_quiescence().is_none());
            let error = if error.resource_close_causes().is_empty() {
                error.with_resource_close_cause(
                    DeviceResourceKind::CaptureBackend,
                    DeviceResourceClosePhase::Close,
                    "capture_backend",
                    None,
                    None,
                    quiescence,
                    count,
                )
            } else {
                error
            };
            if quiescence == DeviceResourceQuiescence::Unconfirmed {
                std::mem::forget(backend);
            }
            Err(ExecutionKernelError::device_close(
                ExecutionKernelCode::CaptureBackendCloseFailed.code(),
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
            let count = u16::from(error.resource_quiescence().is_none());
            let error = if error.resource_close_causes().is_empty() {
                error.with_resource_close_cause(
                    DeviceResourceKind::InputBackend,
                    DeviceResourceClosePhase::Close,
                    "input_backend",
                    None,
                    None,
                    quiescence,
                    count,
                )
            } else {
                error
            };
            if quiescence == DeviceResourceQuiescence::Unconfirmed {
                std::mem::forget(input.take().expect("input backend is present"));
            }
            Err(ExecutionKernelError::device_close(
                ExecutionKernelCode::InputBackendCloseFailed.code(),
                &error,
            ))
        }
    }
}
