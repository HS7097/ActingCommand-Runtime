// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{InputBackend, InputSelectionContext, PreparedSegmentedSwipePlan, SegmentedSwipeEvent};
use actingcommand_contract::{FencedWrite, InputAction};
use std::ffi::{CString, c_char};

// HS-4 (#402): the reset, the long tap and the swipe of the Nemu input, on a fake device.
#[cfg(test)]
mod gate_hs4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NemuAppIndex(i32);

impl TryFrom<u32> for NemuAppIndex {
    type Error = DeviceError;

    fn try_from(value: u32) -> DeviceResult<Self> {
        i32::try_from(value)
            .map(Self)
            .map_err(|_| DeviceError::fatal("Nemu application index exceeds i32"))
    }
}

#[derive(Debug, Clone)]
pub struct NemuApplicationTarget {
    application_id: CString,
    app_index: NemuAppIndex,
}

impl NemuApplicationTarget {
    pub fn new(application_id: &str, app_index: NemuAppIndex) -> DeviceResult<Self> {
        if application_id.trim().is_empty() || application_id.len() > 256 {
            return Err(DeviceError::fatal("Nemu application identity is invalid"));
        }
        Ok(Self {
            application_id: CString::new(application_id)
                .map_err(|_| DeviceError::fatal("Nemu application identity contains NUL"))?,
            app_index,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NemuInputConfig {
    pub command_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub tap_hold: Duration,
}

#[derive(Debug, Clone, Copy)]
pub enum InputCheckPhase {
    Continue,
    FinalUp,
}

/// Reads the existing Runtime lease and deadline; this interface grants no authority.
///
/// ISO-2b (#419 R1, ISO-M row E08): a check that stops the input because the work that admitted
/// it ended (its deadline, its lease, its cancellation) answers
/// [`DeviceError::input_check_stopped`] with the [`InputCheckStop`], not a device failure; the
/// touch protocol carries that answer to its caller unchanged.
pub trait InputOperationCheck: Send + Sync {
    fn check(&self, phase: InputCheckPhase) -> DeviceResult<Duration>;
}

/// ISO-2b (#419 R1, ISO-M row E08): why an input's operation check stopped it. The device was
/// not at fault: the work that admitted the input ended, so its owner ends that work with its
/// own code instead of reading a device failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCheckStop {
    /// The deadline of the work that admitted the input passed.
    Deadline,
    /// The lease or step that admitted the input no longer admits it.
    Lease,
    /// The work that admitted the input was asked to stop.
    Cancelled,
}

#[derive(Clone)]
pub struct InputExecutionContext {
    pub geometry: Option<Arc<NemuFrameGeometry>>,
    pub check: Arc<dyn InputOperationCheck>,
}

#[derive(Debug, Clone)]
pub struct NemuFrameGeometry {
    owner: Arc<()>,
    generation: u64,
    display_id: i32,
    width: u32,
    height: u32,
}

impl PartialEq for NemuFrameGeometry {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
            && self.generation == other.generation
            && self.display_id == other.display_id
            && self.width == other.width
            && self.height == other.height
    }
}

impl Eq for NemuFrameGeometry {}

impl NemuFrameGeometry {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

pub struct NemuSessionBackends {
    pub owner: Arc<NemuIpcSession>,
    pub input: Box<dyn InputBackend>,
    pub capture: Box<dyn CaptureBackend>,
}

pub struct NemuIpcSession {
    backend: Mutex<NemuIpcBackend>,
    input_config: NemuInputConfig,
    resources: Arc<crate::DeviceCloseOccurrence>,
}

impl NemuIpcSession {
    /// Opens the paired owner. With a frame budget (a Capture command's open) the capture view
    /// primes its first frame through the owner exactly like an explicit capture backend
    /// (Workflow #317 sc2): the report's `capture_check` and frame dimensions come from that
    /// frame and the first `capture()` returns it. Without one (an Input command's open, which
    /// has no committed frame and fails afterwards) nothing is acquired.
    pub fn open(
        capture: CaptureBackendConfig,
        application: NemuApplicationTarget,
        input: NemuInputConfig,
        memory: Option<&crate::FrameMemoryBudget>,
    ) -> DeviceResult<crate::OpenedBackend<NemuSessionBackends>> {
        if capture.requested != CaptureBackendChoice::NemuIpc
            || input.command_timeout.is_zero()
            || input.shutdown_timeout.is_zero()
            || input.tap_hold.is_zero()
        {
            return Err(DeviceError::fatal(
                "Nemu paired session configuration is invalid",
            ));
        }
        let configured_adb = capture.adb_config.adb_path.clone();
        // Workflow #398 RD-1a (L1): the selection names the targets by their labels.
        let configured_serial = capture.target.configured_label();
        let capture = prepare_capture_backend_config(capture)?;
        let root = capture
            .nemu
            .nemu_folder
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("Nemu input installation identity is missing"))?;
        require_input_version(root, input.command_timeout)?;
        let serial = capture.target.label();
        let selection = Arc::new(CaptureSelectionContext {
            requested: CaptureBackendChoice::NemuIpc,
            configured_adb,
            configured_serial,
            resolved_adb: capture.adb_config.adb_path.clone(),
            selected_serial: serial.clone(),
            mumu: capture.resolved_mumu.clone(),
            nemu_frame: None,
        });
        let backend = NemuIpcBackend::new_with_input(
            capture.target,
            capture.nemu,
            capture.capture_timeout,
            Some(NemuInputState::new(application, input.clone())),
        )?;
        let mut report = actingcommand_contract::BackendOpenReport::unobserved(
            actingcommand_contract::BackendOpenEntry::NemuPair,
        );
        report.source = actingcommand_contract::BackendOpenSource::Native;
        report.requested = "nemu_ipc".into();
        report.selected = Some("nemu_ipc".into());
        report.status = actingcommand_contract::BackendObservationStatus::Passed;
        report.connection = actingcommand_contract::BackendObservationStatus::Passed;
        report.input_check = actingcommand_contract::BackendObservationStatus::Passed;
        report.frame_width = Some(backend.frame_width);
        report.frame_height = Some(backend.frame_height);
        report.serial_configured = Some(selection.configured_serial.is_some());
        report.installation_source = selection.mumu.as_ref().map(|context| context.source.into());
        let owner = Arc::new(Self {
            resources: Arc::clone(&backend.worker.as_ref().expect("new Nemu worker").resources),
            backend: Mutex::new(backend),
            input_config: input,
        });
        let capture: Box<dyn CaptureBackend> = Box::new(NemuCaptureView {
            owner: Arc::clone(&owner),
            selection,
            vendor_stdio: Vec::new(),
            detached: false,
        });
        let capture = if memory.is_some() {
            match prime_capture_backend(CaptureBackendName::NemuIpc, capture, memory) {
                Ok((primed, _probe_message, _vendor_stdio)) => {
                    if let Some((width, height)) = primed.opened_dimensions() {
                        report.capture_check =
                            actingcommand_contract::BackendObservationStatus::Passed;
                        report.frame_width = Some(width);
                        report.frame_height = Some(height);
                    }
                    primed
                }
                Err(primary) => {
                    // The prime closed only the view; the owner's worker is closed here.
                    let primary = match owner.close_once(DeviceCloseAuthority::LocalOnly, None) {
                        Ok(outcome) => primary.with_close_outcome(&outcome),
                        Err(cleanup) => primary.merge_resource_cleanup(cleanup),
                    };
                    if let Some(check) = primary.capture_probe_check() {
                        check.apply_failure(&mut report, &primary);
                    }
                    return Err(crate::observe_open_failure(report, primary));
                }
            }
        } else {
            capture
        };
        Ok(crate::OpenedBackend::new(
            NemuSessionBackends {
                owner: Arc::clone(&owner),
                input: Box::new(NemuInputView {
                    owner,
                    serial,
                    detached: false,
                }),
                capture,
            },
            report,
        ))
    }

    fn lock(&self) -> DeviceResult<std::sync::MutexGuard<'_, NemuIpcBackend>> {
        self.backend.lock().map_err(|_| {
            contact_unconfirmed("Nemu session state is poisoned")
                .with_backend_resource_owner(&self.resources)
        })
    }

    pub fn invalidate_display(&self) -> DeviceResult<()> {
        self.lock()?
            .worker
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("Nemu session is closed"))?
            .request(NemuIpcCommand::InvalidateDisplay)
    }

    pub fn close_once(
        &self,
        authority: DeviceCloseAuthority,
        input_check: Option<Arc<dyn InputOperationCheck>>,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        let mut backend = self.lock()?;
        if let Some(result) = &backend.close_result {
            return result.clone();
        }
        let result = match backend.worker.as_mut() {
            Some(worker) => worker.shutdown_with_input_check(authority, input_check),
            None => Ok(DeviceResourceCloseOutcome::confirmed(0)),
        };
        if result.is_ok() {
            backend.worker.take();
        }
        backend.close_result = Some(result.clone());
        result
    }

    fn input(
        &self,
        context: &InputExecutionContext,
        command: impl FnOnce(mpsc::Sender<DeviceResult<()>>) -> NemuIpcCommand,
    ) -> DeviceResult<()> {
        let timeout = context
            .check
            .check(InputCheckPhase::Continue)?
            .min(self.input_config.command_timeout);
        self.lock()?
            .worker
            .as_mut()
            .ok_or_else(|| DeviceError::fatal("Nemu session is closed"))?
            .request_with_timeout(command, timeout)
    }
}

struct NemuCaptureView {
    owner: Arc<NemuIpcSession>,
    selection: Arc<CaptureSelectionContext>,
    vendor_stdio: Vec<VendorStdioCapture>,
    detached: bool,
}

impl CaptureBackend for NemuCaptureView {
    fn capture_timed(&mut self) -> DeviceResult<Frame> {
        self.capture()
    }

    fn capture(&mut self) -> DeviceResult<Frame> {
        if self.detached {
            return Err(DeviceError::fatal("Nemu capture view is detached"));
        }
        let mut backend = self.owner.lock()?;
        let mut frame = backend.capture_timed()?;
        self.vendor_stdio = backend.vendor_stdio.clone();
        let mut selection = self.selection.as_ref().clone();
        selection.nemu_frame = frame
            .selection
            .as_ref()
            .and_then(|value| value.nemu_frame.clone());
        frame.selection = Some(Arc::new(selection));
        Ok(frame)
    }

    fn observe_geometry(&mut self, deadline: Instant) -> DeviceResult<CaptureGeometryObservation> {
        if self.detached {
            return Err(DeviceError::fatal("Nemu capture view is detached"));
        }
        self.owner.lock()?.observe_geometry(deadline)
    }

    fn close_once(&mut self, _: DeviceCloseAuthority) -> DeviceResult<DeviceResourceCloseOutcome> {
        self.detached = true;
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }

    fn vendor_stdio(&self) -> &[VendorStdioCapture] {
        &self.vendor_stdio
    }
}

struct NemuInputView {
    owner: Arc<NemuIpcSession>,
    serial: String,
    detached: bool,
}

fn unsupported() -> DeviceError {
    DeviceError::fatal("selected Nemu input capability is unavailable for this operation")
        .with_diagnostic(
            DeviceErrorCategory::Protocol,
            crate::codes::DeviceStage::InputNemuCapability.location(),
        )
}

/// HS-4 (#402, design `rd-1031/backend-switch/model-backend-switch.md` §4.2, §4.4): what the Nemu
/// input does with an input action. Every action it performs runs under the Runtime's check of its
/// step (its lease, its deadline, its cancellation); every one but the reset also on the
/// committed frame its step decided on, bound by that frame's geometry fence and by the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemuInputSupport {
    /// Performed on the step's committed frame: the tap, the long tap, the swipe and the three
    /// segmented drags.
    InFrame,
    /// Performed without a frame: the reset, which only lifts a contact whose lift was not
    /// confirmed.
    WithoutFrame,
    /// Not performed: `key` and `text`.
    Unsupported,
}

impl NemuInputSupport {
    pub fn of(action: &InputAction) -> Self {
        match action {
            InputAction::Tap { .. }
            | InputAction::LongTap { .. }
            | InputAction::Swipe { .. }
            | InputAction::SingleTouchDragWithVerticalBrakeV1 { .. }
            | InputAction::SingleTouchDragWithPauseAndBrakeV1 { .. }
            | InputAction::SingleTouchPathDragV1 { .. } => Self::InFrame,
            InputAction::Reset => Self::WithoutFrame,
            InputAction::Key { .. } | InputAction::Text { .. } => Self::Unsupported,
        }
    }
}

/// HS-4 (#402, §4.4): a long tap as one contact: the press at `point`, the hold of
/// `duration_ms` and the release.
fn long_tap_events(point: (i32, i32), duration_ms: u64) -> Vec<SegmentedSwipeEvent> {
    vec![
        SegmentedSwipeEvent::Down(point),
        SegmentedSwipeEvent::Hold(duration_ms),
        SegmentedSwipeEvent::Up,
    ]
}

impl NemuInputView {
    fn require_attached(&self) -> DeviceResult<()> {
        if self.detached {
            return Err(DeviceError::fatal("Nemu input view is detached"));
        }
        Ok(())
    }

    /// HS-4: `events` as one contact on the step's frame, through the worker's geometry fence
    /// and deadline, as the tap's.
    fn events_in_frame(
        &self,
        events: Vec<SegmentedSwipeEvent>,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        self.require_attached()?;
        self.owner
            .input(context, |response| NemuIpcCommand::InputEvents {
                events,
                context: context.clone(),
                response,
            })
    }
}

impl InputBackend for NemuInputView {
    fn selection_context(&self) -> Option<InputSelectionContext> {
        Some(InputSelectionContext {
            backend: crate::TouchBackendName::NemuIpc,
            serial: self.serial.clone(),
        })
    }
    fn tap(&mut self, _: &FencedWrite, _: i32, _: i32) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn long_tap(&mut self, _: &FencedWrite, _: i32, _: i32, _: u64) -> DeviceResult<()> {
        Err(unsupported())
    }
    /// HS-4 (#402, §4.4): the press, the hold of `duration_ms` and the release, on the step's
    /// frame.
    fn long_tap_in_frame(
        &mut self,
        _witness: &FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        self.events_in_frame(long_tap_events((x, y), duration_ms), context)
    }
    /// HS-4 (#402, §4.4): the press at `from`, evenly timed moves to `to` and the release, on the
    /// step's frame.
    fn swipe_in_frame(
        &mut self,
        _witness: &FencedWrite,
        from: (i32, i32),
        to: (i32, i32),
        duration_ms: u64,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        self.events_in_frame(
            crate::input::linear_swipe_events(from, to, duration_ms),
            context,
        )
    }
    fn swipe(
        &mut self,
        _: &FencedWrite,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
        _: u64,
    ) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn key(&mut self, _: &FencedWrite, _: &str) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn text(&mut self, _: &FencedWrite, _: &str) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn reset(&mut self, _: &FencedWrite) -> DeviceResult<()> {
        Err(unsupported())
    }
    /// HS-4 (#402, §4.4): lifts a contact whose lift was not confirmed, else does nothing; it
    /// reads no frame, as maatouch's and minitouch's resets reset their contacts.
    fn reset_with_check(
        &mut self,
        _witness: &FencedWrite,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        self.require_attached()?;
        self.owner
            .input(context, |response| NemuIpcCommand::InputReset {
                context: context.clone(),
                response,
            })
    }
    fn supports_segmented_swipe(&self) -> bool {
        true
    }
    fn tap_in_frame(
        &mut self,
        _witness: &FencedWrite,
        x: i32,
        y: i32,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        if self.detached {
            return Err(DeviceError::fatal("Nemu input view is detached"));
        }
        self.owner
            .input(context, |response| NemuIpcCommand::InputTap {
                x,
                y,
                context: context.clone(),
                response,
            })
    }
    fn segmented_swipe_prepared_in_frame(
        &mut self,
        _witness: &FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        if self.detached {
            return Err(DeviceError::fatal("Nemu input view is detached"));
        }
        self.owner
            .input(context, |response| NemuIpcCommand::InputSegmented {
                plan: plan.clone(),
                context: context.clone(),
                response,
            })
    }
    fn close_once(&mut self, _: DeviceCloseAuthority) -> DeviceResult<DeviceResourceCloseOutcome> {
        self.detached = true;
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contact {
    NotStarted,
    MayBeDown,
    UpAttempted,
    Released,
}

pub(super) struct NemuInputState {
    application: NemuApplicationTarget,
    config: NemuInputConfig,
    owner: Arc<()>,
    generation: u64,
    display: Option<i32>,
    size: Option<(u32, u32)>,
    contact: Contact,
}

impl NemuInputState {
    fn new(application: NemuApplicationTarget, config: NemuInputConfig) -> Self {
        Self {
            application,
            config,
            owner: Arc::new(()),
            generation: 1,
            display: None,
            size: None,
            contact: Contact::NotStarted,
        }
    }
    fn invalidate(&mut self) -> DeviceResult<()> {
        self.display = None;
        self.size = None;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| DeviceError::fatal("Nemu display generation exhausted"))?;
        Ok(())
    }
}

type GetDisplay = unsafe extern "C" fn(i32, *const c_char, i32) -> i32;
type FingerDown = unsafe extern "C" fn(i32, i32, i32, i32, i32) -> i32;
type FingerUp = unsafe extern "C" fn(i32, i32, i32) -> i32;

impl NemuIpcWorkerState {
    /// #411 A50: a capture open failure's `cause`, with the instance index this worker
    /// connects to and, for a paired session, the application package whose display it looks up.
    pub(super) fn capture_open_failure(
        &self,
        cause: CaptureOpenCause,
    ) -> crate::CaptureOpenFailure {
        crate::CaptureOpenFailure {
            cause,
            application_id: self.input.as_ref().map(|input| {
                input
                    .application
                    .application_id
                    .to_string_lossy()
                    .into_owned()
            }),
            instance_index: Some(i64::from(self.instance_id)),
        }
    }

    pub(super) fn invalidate_display(&mut self) -> DeviceResult<()> {
        if let Some(input) = &mut self.input {
            if matches!(input.contact, Contact::MayBeDown | Contact::UpAttempted) {
                return Err(contact_unconfirmed(
                    "Nemu contact prevents display invalidation",
                ));
            }
            input.invalidate()?;
        }
        Ok(())
    }

    pub(super) fn resolve_input_display(
        &mut self,
        context: Option<(&InputExecutionContext, &AtomicBool)>,
    ) -> DeviceResult<()> {
        let Some(input) = &self.input else {
            return Ok(());
        };
        let query = unsafe { self.symbol::<GetDisplay>(b"nemu_get_display_id\0")? };
        let _: FingerDown = unsafe { self.symbol(b"nemu_input_event_finger_touch_down\0")? };
        let _: FingerUp = unsafe { self.symbol(b"nemu_input_event_finger_touch_up\0")? };
        if let Some((context, stopped)) = context {
            input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
        }
        // The application name stays owned by `self.input`; the redirection does not touch it.
        let application_id = input.application.application_id.as_ptr();
        let app_index = input.application.app_index.0;
        self.resume_vendor_stdio()?;
        let display = unsafe { query(self.connect_id, application_id, app_index) };
        let snapshot = self.record_vendor_stdio_snapshot();
        if display < 0 {
            // #411 A50: no display for the package: the application is not running.
            let native = Err(DeviceError::fatal(format!(
                "Nemu application display query failed with code {display}"
            ))
            .input_parameter_failure()
            .with_capture_open_failure(
                self.capture_open_failure(CaptureOpenCause::DisplayMissing),
            ));
            let input = self.input.as_mut().expect("input target exists");
            return merge_input_snapshot(
                merge_input_snapshot(native, snapshot),
                input.invalidate(),
            );
        }
        let input = self.input.as_mut().expect("input target exists");
        snapshot?;
        if input.display.is_some_and(|previous| previous != display) {
            input.invalidate()?;
        }
        input.display = Some(display);
        self.display_id = display;
        Ok(())
    }

    pub(super) fn input_geometry(
        &mut self,
        width: u32,
        height: u32,
    ) -> DeviceResult<Option<Arc<NemuFrameGeometry>>> {
        let Some(input) = &mut self.input else {
            return Ok(None);
        };
        if input
            .size
            .is_some_and(|previous| previous != (width, height))
        {
            input.invalidate()?;
        }
        input.display = Some(self.display_id);
        input.size = Some((width, height));
        Ok(Some(Arc::new(NemuFrameGeometry {
            owner: Arc::clone(&input.owner),
            generation: input.generation,
            display_id: self.display_id,
            width,
            height,
        })))
    }

    pub(super) fn input_tap(
        &mut self,
        x: i32,
        y: i32,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        let hold = self.input.as_ref().ok_or_else(unsupported)?.config.tap_hold;
        let hold = u64::try_from(hold.as_millis()).map_err(|_| unsupported())?;
        perform_events(
            self,
            &[
                SegmentedSwipeEvent::Down((x, y)),
                SegmentedSwipeEvent::Hold(hold),
                SegmentedSwipeEvent::Up,
            ],
            context,
            stopped,
        )
    }

    pub(super) fn input_segmented(
        &mut self,
        plan: &PreparedSegmentedSwipePlan,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        plan.action().validate()?;
        perform_events(self, plan.events(), context, stopped)
    }

    /// HS-4 (#402): a long tap or a swipe, the events its view planned.
    pub(super) fn input_gesture(
        &mut self,
        events: &[SegmentedSwipeEvent],
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        perform_events(self, events, context, stopped)
    }

    /// HS-4 (#402): the reset.
    pub(super) fn input_reset(
        &mut self,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        reset_contact(self, context, stopped)
    }

    pub(super) fn close_input_contact(
        &mut self,
        authority: DeviceCloseAuthority,
        check: Option<&dyn InputOperationCheck>,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        match self.input.as_ref().map(|input| input.contact) {
            Some(Contact::MayBeDown) => {
                if authority.resource_close_witness().is_none() {
                    return Err(contact_unconfirmed(
                        "Nemu final up requires current fenced close",
                    ));
                }
                finger_up(
                    self,
                    check.ok_or_else(|| {
                        contact_unconfirmed("Nemu final up has no current close check")
                    })?,
                    stopped,
                )
            }
            Some(Contact::UpAttempted) => {
                Err(contact_unconfirmed("Nemu final up was not confirmed"))
            }
            _ => Ok(()),
        }
    }
}

/// HS-4 (#402): one native contact call of the Nemu input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NemuTouchCall {
    /// The finger down at a point: a press, or a move of the pressed finger.
    Down(i32, i32),
    /// The finger up.
    Up,
}

/// HS-4 (#402): what one Nemu touch needs of its connection: the paired input's state, the
/// display its native calls address and that display's size, the two native contact calls with
/// the vendor output around each, and the waits between events. The worker's state makes them on
/// the vendor library; the gate tests' fake device records them, with no library and no clock.
trait NemuTouchPort {
    /// The paired input's state; `None` on a capture-only connection.
    fn touch_input(&mut self) -> Option<&mut NemuInputState>;
    /// The display the native calls address.
    fn touch_display(&self) -> i32;
    /// Resolves the application's display and reads its size, checking the input first.
    fn touch_resolution(
        &mut self,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<(u32, u32)>;
    /// Looks the native call up; a call the library lacks fails before any check.
    fn touch_require(&self, call: NemuTouchCall) -> DeviceResult<()>;
    /// Redirects the vendor output for the native call that follows.
    fn touch_resume(&mut self) -> DeviceResult<()>;
    /// Makes the native call; its return code, 0 for success.
    fn touch_native(&mut self, call: NemuTouchCall) -> DeviceResult<i32>;
    /// Reads the vendor output of the native call that returned.
    fn touch_snapshot(&mut self) -> DeviceResult<()>;
    /// Waits `duration` between two events, checking the input throughout.
    fn touch_wait(
        &mut self,
        duration: Duration,
        check: &dyn InputOperationCheck,
        stopped: &AtomicBool,
    ) -> DeviceResult<()>;
}

impl NemuTouchPort for NemuIpcWorkerState {
    fn touch_input(&mut self) -> Option<&mut NemuInputState> {
        self.input.as_mut()
    }

    fn touch_display(&self) -> i32 {
        self.display_id
    }

    fn touch_resolution(
        &mut self,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<(u32, u32)> {
        self.probe_resolution_with_input(Some((context, stopped)))
    }

    fn touch_require(&self, call: NemuTouchCall) -> DeviceResult<()> {
        match call {
            NemuTouchCall::Down(..) => {
                let _: FingerDown =
                    unsafe { self.symbol(b"nemu_input_event_finger_touch_down\0")? };
            }
            NemuTouchCall::Up => {
                let _: FingerUp = unsafe { self.symbol(b"nemu_input_event_finger_touch_up\0")? };
            }
        }
        Ok(())
    }

    fn touch_resume(&mut self) -> DeviceResult<()> {
        self.resume_vendor_stdio()
    }

    fn touch_native(&mut self, call: NemuTouchCall) -> DeviceResult<i32> {
        let (connect_id, display_id) = (self.connect_id, self.display_id);
        Ok(match call {
            NemuTouchCall::Down(x, y) => {
                let down: FingerDown =
                    unsafe { self.symbol(b"nemu_input_event_finger_touch_down\0")? };
                unsafe { down(connect_id, display_id, 1, x, y) }
            }
            NemuTouchCall::Up => {
                let up: FingerUp = unsafe { self.symbol(b"nemu_input_event_finger_touch_up\0")? };
                unsafe { up(connect_id, display_id, 1) }
            }
        })
    }

    fn touch_snapshot(&mut self) -> DeviceResult<()> {
        self.record_vendor_stdio_snapshot()
    }

    fn touch_wait(
        &mut self,
        duration: Duration,
        check: &dyn InputOperationCheck,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        input_wait(duration, check, stopped)
    }
}

/// One contact through `events` on the frame `context` names (§4.2 items 4 and 5): the frame's
/// geometry fence first, then a deadline that covers the whole gesture and the final lift, then
/// the events, a press or a move each a finger down at its point. A failure while the finger may
/// be down lifts it once more.
fn perform_events<P: NemuTouchPort>(
    port: &mut P,
    events: &[SegmentedSwipeEvent],
    context: &InputExecutionContext,
    stopped: &AtomicBool,
) -> DeviceResult<()> {
    input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
    let geometry = context
        .geometry
        .as_ref()
        .ok_or_else(|| DeviceError::fatal("Nemu input requires the committed source frame"))?;
    let (width, height) = port.touch_resolution(context, stopped)?;
    let display_id = port.touch_display();
    let input = port.touch_input().ok_or_else(unsupported)?;
    if !Arc::ptr_eq(&input.owner, &geometry.owner)
        || input.generation != geometry.generation
        || display_id != geometry.display_id
        || (width, height) != geometry.dimensions()
    {
        input.invalidate()?;
        return Err(DeviceError::fatal(
            "Nemu input frame target or geometry changed",
        ));
    }
    if matches!(input.contact, Contact::MayBeDown | Contact::UpAttempted) {
        return Err(contact_unconfirmed("Nemu input contact is unresolved"));
    }
    let mut delay = Duration::ZERO;
    for event in events {
        let milliseconds = match event {
            SegmentedSwipeEvent::Down(point) | SegmentedSwipeEvent::Move { point, .. } => {
                if point.0 < 0 || point.1 < 0 || point.0 as u32 >= width || point.1 as u32 >= height
                {
                    return Err(DeviceError::fatal(
                        "Nemu input point is outside the source frame",
                    ));
                }
                match event {
                    SegmentedSwipeEvent::Move {
                        delay_before_ms, ..
                    } => *delay_before_ms,
                    _ => 0,
                }
            }
            SegmentedSwipeEvent::Hold(ms) => *ms,
            SegmentedSwipeEvent::Up => 0,
        };
        delay = delay
            .checked_add(Duration::from_millis(milliseconds))
            .ok_or_else(unsupported)?;
    }
    let reserve = input
        .config
        .shutdown_timeout
        .min(input.config.command_timeout);
    if delay.checked_add(reserve).ok_or_else(unsupported)?
        >= input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?
    {
        return Err(DeviceError::fatal(
            "Nemu input deadline cannot cover the gesture and final up",
        ));
    }
    let result = run_events(port, events, context, stopped);
    let unreleased = result.is_err()
        && port
            .touch_input()
            .is_some_and(|input| input.contact == Contact::MayBeDown);
    match result {
        Err(primary) if unreleased => match finger_up(port, context.check.as_ref(), stopped) {
            Ok(()) => Err(primary),
            Err(secondary) => Err(primary
                .merge_resource_cleanup(secondary)
                .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1)),
        },
        result => result,
    }
}

/// The events of one contact in order, each checked before it (§4.2 item 5).
fn run_events<P: NemuTouchPort>(
    port: &mut P,
    events: &[SegmentedSwipeEvent],
    context: &InputExecutionContext,
    stopped: &AtomicBool,
) -> DeviceResult<()> {
    let check = context.check.as_ref();
    for event in events {
        match event {
            SegmentedSwipeEvent::Down((x, y)) => finger_down(port, *x, *y, context, stopped)?,
            SegmentedSwipeEvent::Move {
                point: (x, y),
                delay_before_ms,
            } => {
                port.touch_wait(Duration::from_millis(*delay_before_ms), check, stopped)?;
                finger_down(port, *x, *y, context, stopped)?;
            }
            SegmentedSwipeEvent::Hold(ms) => {
                port.touch_wait(Duration::from_millis(*ms), check, stopped)?;
            }
            SegmentedSwipeEvent::Up => finger_up(port, check, stopped)?,
        }
    }
    Ok(())
}

fn finger_down<P: NemuTouchPort>(
    port: &mut P,
    x: i32,
    y: i32,
    context: &InputExecutionContext,
    stopped: &AtomicBool,
) -> DeviceResult<()> {
    let call = NemuTouchCall::Down(x, y);
    port.touch_require(call)?;
    input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
    port.touch_input().ok_or_else(unsupported)?;
    port.touch_resume()?;
    port.touch_input().ok_or_else(unsupported)?.contact = Contact::MayBeDown;
    let native = port.touch_native(call).and_then(|result| {
        if result == 0 {
            Ok(())
        } else {
            Err(DeviceError::fatal(format!(
                "Nemu finger down failed with code {result}"
            )))
        }
    });
    merge_input_snapshot(native, port.touch_snapshot())
}

fn finger_up<P: NemuTouchPort>(
    port: &mut P,
    check: &dyn InputOperationCheck,
    stopped: &AtomicBool,
) -> DeviceResult<()> {
    port.touch_require(NemuTouchCall::Up)?;
    input_check(check, InputCheckPhase::FinalUp, stopped)?;
    port.touch_input().ok_or_else(unsupported)?;
    // Workflow #191 H: the contact is released even when its output cannot be captured;
    // that failure is reported with the snapshot's.
    let resumed = port.touch_resume();
    port.touch_input().ok_or_else(unsupported)?.contact = Contact::UpAttempted;
    let native = match port.touch_native(NemuTouchCall::Up) {
        Ok(0) => {
            if let Some(input) = port.touch_input() {
                input.contact = Contact::Released;
            }
            Ok(())
        }
        Ok(result) => Err(contact_unconfirmed(&format!(
            "Nemu finger up failed with code {result}"
        ))),
        Err(error) => Err(error.with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1)),
    };
    merge_input_snapshot(merge_input_snapshot(native, resumed), port.touch_snapshot())
}

/// HS-4 (#402, §4.4): the reset. A contact whose lift was not confirmed (pressed, or lifted
/// without a confirmed answer) is lifted once, under the input's check; any other contact is left
/// as it is and nothing is called. It reads no frame and no display, so `context` needs no
/// geometry.
fn reset_contact<P: NemuTouchPort>(
    port: &mut P,
    context: &InputExecutionContext,
    stopped: &AtomicBool,
) -> DeviceResult<()> {
    let contact = port.touch_input().ok_or_else(unsupported)?.contact;
    if matches!(contact, Contact::MayBeDown | Contact::UpAttempted) {
        finger_up(port, context.check.as_ref(), stopped)
    } else {
        Ok(())
    }
}

fn contact_unconfirmed(message: &str) -> DeviceError {
    DeviceError::fatal(message).with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1)
}

fn merge_input_snapshot(native: DeviceResult<()>, snapshot: DeviceResult<()>) -> DeviceResult<()> {
    match (native, snapshot) {
        (Err(primary), Err(secondary)) => Err(primary.merge_resource_cleanup(secondary)),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub(super) fn input_check(
    check: &dyn InputOperationCheck,
    phase: InputCheckPhase,
    stopped: &AtomicBool,
) -> DeviceResult<Duration> {
    if stopped.load(Ordering::Acquire) {
        return Err(contact_unconfirmed("Nemu input receipt is unconfirmed"));
    }
    let remaining = check.check(phase).inspect_err(|_| {
        if matches!(phase, InputCheckPhase::FinalUp) {
            stopped.store(true, Ordering::Release);
        }
    })?;
    if remaining.is_zero() {
        return Err(contact_unconfirmed("Nemu input deadline expired"));
    }
    Ok(remaining)
}

fn input_wait(
    duration: Duration,
    check: &dyn InputOperationCheck,
    stopped: &AtomicBool,
) -> DeviceResult<()> {
    let end = Instant::now()
        .checked_add(duration)
        .ok_or_else(unsupported)?;
    loop {
        let remaining = input_check(check, InputCheckPhase::Continue, stopped)?;
        let delay = end.saturating_duration_since(Instant::now());
        if delay.is_zero() {
            return Ok(());
        }
        if delay >= remaining {
            return Err(DeviceError::fatal(
                "Nemu input wait exceeds the original deadline",
            ));
        }
        thread::sleep(delay.min(Duration::from_millis(16)));
    }
}

fn require_input_version(root: &Path, timeout: Duration) -> DeviceResult<()> {
    let candidates = [
        root.join("nx_main/MuMuManager.exe"),
        root.join("shell/MuMuManager.exe"),
    ];
    let managers: Vec<_> = candidates.iter().filter(|path| path.is_file()).collect();
    if managers.len() != 1 {
        return Err(DeviceError::fatal(
            "Nemu input requires one matching MuMuManager installation",
        ));
    }
    let manager = managers[0];
    let output = crate::adb::run_text_in_directory_with_timeout(
        manager
            .to_str()
            .ok_or_else(|| DeviceError::fatal("MuMuManager path is not valid Unicode"))?,
        &["version"],
        timeout,
        manager.parent().ok_or_else(unsupported)?,
    )?;
    if output.stdout_lossy_decode || output.stdout.len() > 4096 {
        return Err(
            DeviceError::fatal("MuMuManager version output is invalid").input_parameter_failure()
        );
    }
    let checked = (|| -> DeviceResult<()> {
        let value: serde_json::Value = serde_json::from_str(&output.stdout).map_err(|error| {
            DeviceError::fatal(format!("MuMuManager version JSON is invalid: {error}"))
        })?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(unsupported)?;
        let mut parts = version.split('.');
        let mut parsed = [0_u32; 4];
        for part in &mut parsed {
            *part = parts
                .next()
                .ok_or_else(unsupported)?
                .parse()
                .map_err(|_| unsupported())?;
        }
        if parts.next().is_some() || parsed < [6, 3, 2, 0] {
            return Err(DeviceError::fatal(
                "Nemu input requires MuMuManager version 6.3.2.0 or later",
            ));
        }
        Ok(())
    })();
    checked.map_err(DeviceError::input_parameter_failure)
}
