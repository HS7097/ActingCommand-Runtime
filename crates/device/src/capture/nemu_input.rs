// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{InputBackend, InputSelectionContext, PreparedSegmentedSwipePlan, SegmentedSwipeEvent};
use std::ffi::{CString, c_char};

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
pub trait InputOperationCheck: Send + Sync {
    fn check(&self, phase: InputCheckPhase) -> DeviceResult<Duration>;
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
}

impl NemuIpcSession {
    pub fn open(
        capture: CaptureBackendConfig,
        application: NemuApplicationTarget,
        input: NemuInputConfig,
    ) -> DeviceResult<NemuSessionBackends> {
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
        let configured_serial = capture.target.serial.clone();
        let capture = prepare_capture_backend_config(capture)?;
        let root = capture
            .nemu
            .nemu_folder
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("Nemu input installation identity is missing"))?;
        require_input_version(root, input.command_timeout)?;
        let serial = capture.target.resolved_serial();
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
        let owner = Arc::new(Self {
            backend: Mutex::new(backend),
            input_config: input,
        });
        Ok(NemuSessionBackends {
            owner: Arc::clone(&owner),
            input: Box::new(NemuInputView {
                owner: Arc::clone(&owner),
                serial,
                detached: false,
            }),
            capture: Box::new(NemuCaptureView {
                owner,
                selection,
                vendor_stdio: Vec::new(),
                detached: false,
            }),
        })
    }

    fn lock(&self) -> DeviceResult<std::sync::MutexGuard<'_, NemuIpcBackend>> {
        self.backend
            .lock()
            .map_err(|_| contact_unconfirmed("Nemu session state is poisoned"))
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
    fn capture(&mut self) -> DeviceResult<Frame> {
        if self.detached {
            return Err(DeviceError::fatal("Nemu capture view is detached"));
        }
        let mut backend = self.owner.lock()?;
        let mut frame = backend.capture()?;
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
        .with_diagnostic(DeviceErrorCategory::Protocol, "input.nemu.capability")
}

impl InputBackend for NemuInputView {
    fn selection_context(&self) -> Option<InputSelectionContext> {
        Some(InputSelectionContext {
            backend: crate::TouchBackendName::NemuIpc,
            serial: self.serial.clone(),
        })
    }
    fn tap(&mut self, _: i32, _: i32) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn long_tap(&mut self, _: i32, _: i32, _: u64) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn swipe(&mut self, _: i32, _: i32, _: i32, _: i32, _: u64) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn key(&mut self, _: &str) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn text(&mut self, _: &str) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn reset(&mut self) -> DeviceResult<()> {
        Err(unsupported())
    }
    fn supports_segmented_swipe(&self) -> bool {
        true
    }
    fn tap_in_frame(
        &mut self,
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
        let display = unsafe {
            query(
                self.connect_id,
                input.application.application_id.as_ptr(),
                input.application.app_index.0,
            )
        };
        let snapshot = self.record_vendor_stdio_snapshot();
        let input = self.input.as_mut().expect("input target exists");
        if display < 0 {
            let native = Err(DeviceError::fatal(format!(
                "Nemu application display query failed with code {display}"
            )));
            return merge_input_snapshot(
                merge_input_snapshot(native, snapshot),
                input.invalidate(),
            );
        }
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
        self.input_events(
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
        self.input_events(plan.events(), context, stopped)
    }

    fn input_events(
        &mut self,
        events: &[SegmentedSwipeEvent],
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
        let geometry = context
            .geometry
            .as_ref()
            .ok_or_else(|| DeviceError::fatal("Nemu input requires the committed source frame"))?;
        let (width, height) = self.probe_resolution_with_input(Some((context, stopped)))?;
        let input = self.input.as_mut().ok_or_else(unsupported)?;
        if !Arc::ptr_eq(&input.owner, &geometry.owner)
            || input.generation != geometry.generation
            || self.display_id != geometry.display_id
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
                    if point.0 < 0
                        || point.1 < 0
                        || point.0 as u32 >= width
                        || point.1 as u32 >= height
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
        let result = (|| {
            for event in events {
                match event {
                    SegmentedSwipeEvent::Down((x, y)) => {
                        self.finger_down(*x, *y, context, stopped)?
                    }
                    SegmentedSwipeEvent::Move {
                        point: (x, y),
                        delay_before_ms,
                    } => {
                        input_wait(
                            Duration::from_millis(*delay_before_ms),
                            context.check.as_ref(),
                            stopped,
                        )?;
                        self.finger_down(*x, *y, context, stopped)?;
                    }
                    SegmentedSwipeEvent::Hold(ms) => {
                        input_wait(Duration::from_millis(*ms), context.check.as_ref(), stopped)?
                    }
                    SegmentedSwipeEvent::Up => self.finger_up(context.check.as_ref(), stopped)?,
                }
            }
            Ok(())
        })();
        match result {
            Err(primary)
                if self
                    .input
                    .as_ref()
                    .is_some_and(|input| input.contact == Contact::MayBeDown) =>
            {
                match self.finger_up(context.check.as_ref(), stopped) {
                    Ok(()) => Err(primary),
                    Err(secondary) => Err(primary
                        .merge_resource_cleanup(secondary)
                        .with_resource_quiescence(DeviceResourceQuiescence::Unconfirmed, 1)),
                }
            }
            result => result,
        }
    }

    fn finger_down(
        &mut self,
        x: i32,
        y: i32,
        context: &InputExecutionContext,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        let down = unsafe { self.symbol::<FingerDown>(b"nemu_input_event_finger_touch_down\0")? };
        input_check(context.check.as_ref(), InputCheckPhase::Continue, stopped)?;
        self.input.as_mut().ok_or_else(unsupported)?.contact = Contact::MayBeDown;
        let result = unsafe { down(self.connect_id, self.display_id, 1, x, y) };
        let native = if result != 0 {
            Err(DeviceError::fatal(format!(
                "Nemu finger down failed with code {result}"
            )))
        } else {
            Ok(())
        };
        merge_input_snapshot(native, self.record_vendor_stdio_snapshot())
    }

    fn finger_up(
        &mut self,
        check: &dyn InputOperationCheck,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        let up = unsafe { self.symbol::<FingerUp>(b"nemu_input_event_finger_touch_up\0")? };
        input_check(check, InputCheckPhase::FinalUp, stopped)?;
        self.input.as_mut().ok_or_else(unsupported)?.contact = Contact::UpAttempted;
        let result = unsafe { up(self.connect_id, self.display_id, 1) };
        if result == 0 {
            self.input.as_mut().expect("input exists").contact = Contact::Released;
        }
        let native = if result != 0 {
            Err(contact_unconfirmed(&format!(
                "Nemu finger up failed with code {result}"
            )))
        } else {
            Ok(())
        };
        merge_input_snapshot(native, self.record_vendor_stdio_snapshot())
    }

    pub(super) fn close_input_contact(
        &mut self,
        authority: DeviceCloseAuthority,
        check: Option<&dyn InputOperationCheck>,
        stopped: &AtomicBool,
    ) -> DeviceResult<()> {
        match self.input.as_ref().map(|input| input.contact) {
            Some(Contact::MayBeDown) => {
                if authority != DeviceCloseAuthority::FencedDeviceWrite {
                    return Err(contact_unconfirmed(
                        "Nemu final up requires current fenced close",
                    ));
                }
                self.finger_up(
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
        return Err(DeviceError::fatal("MuMuManager version output is invalid"));
    }
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
}
