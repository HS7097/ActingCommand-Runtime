// SPDX-License-Identifier: AGPL-3.0-only

pub(super) fn host_with_state(root: &TempDir, alias: &str, state: Arc<FakeState>) -> RuntimeHost {
    RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(alias, instance_id(), state)),
    )
    .expect("runtime host")
}

#[derive(Default)]
pub(super) struct FakeState {
    physical_task_geometry: AtomicBool,
    // Workflow #392 DA-A: the frame and display extent of `physical_task_geometry`, 16x9 when
    // not set.
    physical_task_extent: std::sync::Mutex<Option<(u32, u32)>>,
    adb_recovery: std::sync::Mutex<Option<actingcommand_device::AdbTargetRecovery>>,
    input_selection: std::sync::Mutex<Option<actingcommand_device::InputSelectionContext>>,
    capture_selection: std::sync::Mutex<Option<actingcommand_device::CaptureSelectionContext>>,
    open_count: AtomicUsize,
    input_open_error: std::sync::Mutex<Option<DeviceError>>,
    input_count: AtomicUsize,
    close_count: AtomicUsize,
    close_error: std::sync::Mutex<Option<DeviceError>>,
    fail_input: AtomicBool,
    input_error: std::sync::Mutex<Option<DeviceError>>,
    // Workflow #403: the error the next input returns, taken once, so one input can end with an
    // unconfirmed close while later ones succeed.
    input_error_once: std::sync::Mutex<Option<DeviceError>>,
    block_input: AtomicBool,
    input_started: AtomicBool,
    capture_open_count: AtomicUsize,
    capture_open_error: std::sync::Mutex<Option<DeviceError>>,
    capture_count: AtomicUsize,
    capture_delay_ms: AtomicU64,
    // Workflow #369 E6 (#670 final review M-3): when not 0, a capture waits while at least this
    // many captures are done (raising it lets more through).
    capture_hold_after: AtomicUsize,
    capture_close_count: AtomicUsize,
    require_fenced_capture_close: AtomicBool,
    unfenced_capture_close_count: AtomicUsize,
    capture_close_error: std::sync::Mutex<Option<DeviceError>>,
    fail_capture: AtomicBool,
    transient_capture_failure: AtomicBool,
    fail_capture_on: AtomicUsize,
    unknown_capture: AtomicBool,
    refuse_guard_capture: AtomicBool,
    transition_capture_after_input: AtomicBool,
    transition_capture_after_inputs: AtomicUsize,
    stability_region_transition_after_inputs: AtomicUsize,
    transition_capture_after_capture: AtomicUsize,
    error_capture_after_input: AtomicBool,
    error_capture_after_capture: AtomicUsize,
    monitor_observation_count: AtomicUsize,
    monitor_mode: AtomicUsize,
    application_count: AtomicUsize,
    fail_application: AtomicBool,
    // Workflow #403: the error the next application command returns, taken once, so one
    // command can end with an unconfirmed close while later ones succeed.
    application_error: std::sync::Mutex<Option<DeviceError>>,
    // Workflow #387 D1c-G: the error every ADB baseline probe of the instance returns while it
    // is set (`probe_adb_baseline`); unset, adbd answers.
    adb_probe_error: std::sync::Mutex<Option<DeviceError>>,
    // Workflow #387 D1c-G: the error the next foreground query returns, taken once.
    foreground_error_once: std::sync::Mutex<Option<DeviceError>>,
    input_actions: std::sync::Mutex<Vec<InputAction>>,
    segmented_swipe_plans: std::sync::Mutex<Vec<PreparedSegmentedSwipePlan>>,
    // Workflow #369 review C-2: a discovery-bound instance with emulator control
    // (`bind_fake_emulator`): its Stop and Start are recorded, Stop returns the binding to
    // pending, and Start binds it again. With `hold_android_boot`, Android reports no foreground
    // activity after a Start (not booted) until the flag is cleared. A Start notes
    // `emulator_start_entered`, waits while `block_emulator_start` is set, and with
    // `fail_emulator_start` ends as the vendor's readiness wait timing out. A Stop, once in
    // `emulator_actions`, waits while `block_emulator_stop` is set (#382 H-4 G8). Workflow #381 B:
    // every other result, the state reads and the binding come from the scripted controller
    // (HOST-I1).
    emulator_control: AtomicBool,
    emulator_stopped: AtomicBool,
    hold_android_boot: AtomicBool,
    android_down: AtomicBool,
    emulator_start_entered: AtomicBool,
    block_emulator_start: AtomicBool,
    block_emulator_stop: AtomicBool,
    fail_emulator_start: AtomicBool,
    emulator_actions: std::sync::Mutex<Vec<actingcommand_contract::EmulatorInstanceAction>>,
    emulator: emulator_controller::FakeEmulatorController,
    // Workflow #369 (verify-666 L6): opens report a passed native open, so a preparation's
    // self-check passes (`fake_opened_backend`).
    observed_open: AtomicBool,
    // #398 R1 RD-3i: with `stream_capture` every frame carries a stream stamp whose sequence is
    // its capture number; each capture records the floor it was given, and each completed input
    // records when it completed and how many captures came before it. A capture after an input
    // fails with `capture_error_after_input` when one is set.
    stream_capture: AtomicBool,
    capture_floors: std::sync::Mutex<Vec<Option<actingcommand_device::CaptureFloor>>>,
    inputs_completed: std::sync::Mutex<Vec<(Instant, usize)>>,
    capture_error_after_input: std::sync::Mutex<Option<DeviceError>>,
    // Workflow #373 NAV-6: given a capture's number and the inputs so far, the pixels (index,
    // RGB) the capture shows over the frame the flags above give, such as an overlay.
    pixel_script: std::sync::Mutex<Option<PixelScript>>,
}

/// Workflow #373 NAV-6: a frame script of [`FakeState`].
type PixelScript = Arc<dyn Fn(usize, usize) -> Vec<(usize, [u8; 3])> + Send + Sync>;

impl FakeState {
    /// The frame and display extent of `physical_task_geometry` (Workflow #392 DA-A).
    fn physical_task_extent(&self) -> (u32, u32) {
        self.physical_task_extent
            .lock()
            .expect("fake physical task extent")
            .unwrap_or((16, 9))
    }
}

struct FakeBackend {
    state: Arc<FakeState>,
    close_outcome: Option<DeviceResult<actingcommand_device::DeviceResourceCloseOutcome>>,
}

struct FakeCapture {
    state: Arc<FakeState>,
    provenance: ExecutionBackendProvenance,
    audit_endpoint: String,
    close_outcome: Option<DeviceResult<actingcommand_device::DeviceResourceCloseOutcome>>,
}

impl FakeBackend {
    fn input(&self, action: InputAction) -> DeviceResult<()> {
        self.state
            .input_actions
            .lock()
            .expect("fake input actions lock")
            .push(action);
        self.complete_input()
    }

    fn complete_input(&self) -> DeviceResult<()> {
        self.state.input_started.store(true, Ordering::Release);
        while self.state.block_input.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(5));
        }
        if let Some(error) = self.state.input_error.lock().expect("input error").as_ref() {
            return Err(error.clone());
        }
        if let Some(error) = self
            .state
            .input_error_once
            .lock()
            .expect("one-shot input error")
            .take()
        {
            return Err(error);
        }
        if self.state.fail_input.load(Ordering::Acquire) {
            return Err(DeviceError::fatal("injected backend failure")
                .with_diagnostic(
                    DeviceErrorCategory::Native,
                    crate::codes::HostLocation::DeviceRegistryInputOperation.location(),
                )
                .with_diagnostic_context(
                    "adb_shell_input",
                    actingcommand_device::codes::DeviceLocation::InputReset,
                    DeviceErrorSensitivity::Sensitive,
                ));
        }
        self.state.input_count.fetch_add(1, Ordering::AcqRel);
        self.state
            .inputs_completed
            .lock()
            .expect("completed inputs")
            .push((
                Instant::now(),
                self.state.capture_count.load(Ordering::Acquire),
            ));
        Ok(())
    }
}

impl InputBackend for FakeBackend {
    fn take_adb_recovery(&mut self) -> Option<actingcommand_device::AdbTargetRecovery> {
        self.state
            .adb_recovery
            .lock()
            .expect("input recovery")
            .take()
    }
    fn selection_context(&self) -> Option<actingcommand_device::InputSelectionContext> {
        self.state
            .input_selection
            .lock()
            .expect("input selection")
            .clone()
    }

    fn tap(&mut self, _witness: &actingcommand_contract::FencedWrite, x: i32, y: i32) -> DeviceResult<()> {
        self.input(InputAction::Tap { x, y })
    }

    fn long_tap(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.input(InputAction::LongTap { x, y, duration_ms })
    }

    fn swipe(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.input(InputAction::Swipe {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        })
    }

    fn supports_segmented_swipe(&self) -> bool {
        true
    }

    fn segmented_swipe_prepared(
        &mut self,
        _witness: &actingcommand_contract::FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
    ) -> DeviceResult<()> {
        self.state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans lock")
            .push(plan.clone());
        self.complete_input()
    }

    fn key(&mut self, _witness: &actingcommand_contract::FencedWrite, key: &str) -> DeviceResult<()> {
        self.input(InputAction::Key {
            key: key.to_string(),
        })
    }

    fn text(&mut self, _witness: &actingcommand_contract::FencedWrite, text: &str) -> DeviceResult<()> {
        self.input(InputAction::Text {
            text: text.to_string(),
        })
    }

    fn reset(&mut self, _witness: &actingcommand_contract::FencedWrite) -> DeviceResult<()> {
        self.input(InputAction::Reset)
    }

    fn close_once(
        &mut self,
        _authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        if let Some(outcome) = &self.close_outcome {
            return outcome.clone();
        }
        self.state.close_count.fetch_add(1, Ordering::AcqRel);
        let outcome = match self.state.close_error.lock().expect("close error").clone() {
            Some(error) => Err(error),
            None => Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                1,
            )),
        };
        self.close_outcome = Some(outcome.clone());
        outcome
    }
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        loop {
            let hold_after = self.state.capture_hold_after.load(Ordering::Acquire);
            if hold_after == 0 || self.state.capture_count.load(Ordering::Acquire) < hold_after {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let capture_delay_ms = self.state.capture_delay_ms.load(Ordering::Acquire);
        if capture_delay_ms != 0 {
            thread::sleep(Duration::from_millis(capture_delay_ms));
        }
        let capture_number = self.state.capture_count.fetch_add(1, Ordering::AcqRel) + 1;
        if self.state.fail_capture.load(Ordering::Acquire)
            || self.state.fail_capture_on.load(Ordering::Acquire) == capture_number
        {
            let error = if self.state.transient_capture_failure.load(Ordering::Acquire) {
                DeviceError::transient("injected capture failure")
            } else {
                DeviceError::fatal("injected capture failure")
            };
            return Err(error
                .with_diagnostic(
                    DeviceErrorCategory::Native,
                    actingcommand_device::codes::DeviceStage::CaptureFrameLayout.location(),
                )
                .with_diagnostic_context(
                    "nemu_ipc",
                    actingcommand_contract::codes::ContractLocation::CaptureSequence,
                    DeviceErrorSensitivity::Sensitive,
                ));
        }
        let input_count = self.state.input_count.load(Ordering::Acquire);
        let transition_after = self
            .state
            .transition_capture_after_inputs
            .load(Ordering::Acquire);
        let transition_after_capture = self
            .state
            .transition_capture_after_capture
            .load(Ordering::Acquire);
        let error_after_capture = self
            .state
            .error_capture_after_capture
            .load(Ordering::Acquire);
        let first = if self.state.unknown_capture.load(Ordering::Acquire) {
            [1, 2, 3]
        } else if (self.state.error_capture_after_input.load(Ordering::Acquire) && input_count > 0)
            || (error_after_capture > 0 && capture_number >= error_after_capture)
        {
            [255, 255, 0]
        } else if (self
            .state
            .transition_capture_after_input
            .load(Ordering::Acquire)
            && input_count > 0)
            || (transition_after > 0 && input_count >= transition_after)
            || (transition_after_capture > 0 && capture_number >= transition_after_capture)
        {
            [0, 0, 255]
        } else {
            [255, 0, 0]
        };
        let stability_region_transition_after_inputs = self
            .state
            .stability_region_transition_after_inputs
            .load(Ordering::Acquire);
        let guard = if self.state.refuse_guard_capture.load(Ordering::Acquire) {
            [1, 2, 3]
        } else if stability_region_transition_after_inputs > 0
            && input_count >= stability_region_transition_after_inputs
        {
            [0, 0, 255]
        } else {
            [0, 255, 0]
        };
        let physical_task_geometry = self.provenance == ExecutionBackendProvenance::PhysicalDevice
            && self.state.physical_task_geometry.load(Ordering::Acquire);
        let (width, height) = if physical_task_geometry {
            self.state.physical_task_extent()
        } else {
            (2, 1)
        };
        let mut pixels = [first.as_slice(), guard.as_slice()].concat();
        if physical_task_geometry {
            pixels.resize(width as usize * height as usize * 3, 0);
        }
        let script = self
            .state
            .pixel_script
            .lock()
            .expect("pixel script")
            .clone();
        if let Some(script) = script {
            for (index, rgb) in script(capture_number, input_count) {
                if let Some(pixel) = pixels.get_mut(index * 3..index * 3 + 3) {
                    pixel.copy_from_slice(&rgb);
                }
            }
        }
        let mut frame = Frame::from_pixels(
            width,
            height,
            pixels,
            PixelFormat::Rgb8,
            match self.provenance {
                ExecutionBackendProvenance::PhysicalDevice => CaptureBackendName::AdbScreencap,
                ExecutionBackendProvenance::FixtureSimulation => {
                    CaptureBackendName::FixtureSimulation
                }
            },
        )?;
        if physical_task_geometry {
            frame.geometry = self.observe_geometry(std::time::Instant::now())?;
            if let actingcommand_contract::CaptureGeometryObservation::Observed(geometry) =
                &mut frame.geometry
            {
                geometry.frame_transform =
                    Some(actingcommand_contract::CaptureFrameTransform::Identity);
            }
        }
        frame.selection = self
            .state
            .capture_selection
            .lock()
            .expect("capture selection")
            .clone()
            .map(Arc::new);
        Ok(frame)
    }

    fn capture_fresh(
        &mut self,
        floor: Option<actingcommand_device::CaptureFloor>,
    ) -> DeviceResult<Frame> {
        self.state
            .capture_floors
            .lock()
            .expect("capture floors")
            .push(floor);
        if self.state.input_count.load(Ordering::Acquire) > 0
            && let Some(error) = self
                .state
                .capture_error_after_input
                .lock()
                .expect("capture error after input")
                .clone()
        {
            return Err(error);
        }
        let mut frame = self.capture_timed()?;
        if self.state.stream_capture.load(Ordering::Acquire) {
            frame.stream = Some(actingcommand_device::FrameStreamStamp {
                sequence: self.state.capture_count.load(Ordering::Acquire) as u64,
                device_pts_us: 0,
                received_at: Instant::now(),
            });
        }
        Ok(frame)
    }

    fn observe_geometry(
        &mut self,
        _deadline: std::time::Instant,
    ) -> DeviceResult<actingcommand_contract::CaptureGeometryObservation> {
        use actingcommand_contract::{
            CaptureExtent, CaptureGeometry, CaptureGeometryNotApplicable,
            CaptureGeometryObservation, CaptureGeometrySource, CaptureGeometryUnknownReason,
            CaptureRotation, CaptureRotationObservation, CaptureRotationSource, CaptureWmSizeKind,
        };
        if self.provenance == ExecutionBackendProvenance::FixtureSimulation {
            return Ok(CaptureGeometryObservation::NotApplicable(
                CaptureGeometryNotApplicable::FixtureSimulation,
            ));
        }
        if !self.state.physical_task_geometry.load(Ordering::Acquire) {
            return Ok(CaptureGeometryObservation::Unknown(
                CaptureGeometryUnknownReason::BackendUnsupported,
            ));
        }
        // The existing fake supplies these dimensions and binding; no device is queried.
        let (width, height) = self.state.physical_task_extent();
        let extent = CaptureExtent::new(width, height).expect("positive fake Task extent");
        Ok(CaptureGeometryObservation::Observed(CaptureGeometry {
            backend: CaptureBackendName::AdbScreencap,
            source: CaptureGeometrySource::AdbDefaultDisplay {
                serial: self.audit_endpoint.clone(),
                wm_extent: extent,
                wm_size_kind: CaptureWmSizeKind::Unlabelled,
            },
            logical_display_extent: extent,
            rotation: CaptureRotationObservation::Observed {
                rotation: CaptureRotation::R0,
                source: CaptureRotationSource::DumpsysDisplayOrientation,
            },
            sampled_at: std::time::SystemTime::now(),
            frame_transform: None,
            safe_insets: None,
        }))
    }

    fn close_once(
        &mut self,
        authority: actingcommand_device::DeviceCloseAuthority,
    ) -> DeviceResult<actingcommand_device::DeviceResourceCloseOutcome> {
        if let Some(outcome) = &self.close_outcome {
            return outcome.clone();
        }
        self.state
            .capture_close_count
            .fetch_add(1, Ordering::AcqRel);
        if self
            .state
            .require_fenced_capture_close
            .load(Ordering::Acquire)
            && authority.resource_close_witness().is_none()
        {
            self.state
                .unfenced_capture_close_count
                .fetch_add(1, Ordering::AcqRel);
            let outcome = Err(DeviceError::fatal(
                "capture close requires current fenced authority",
            )
            .with_resource_close_cause(
                actingcommand_device::DeviceResourceKind::ProviderConnection,
                actingcommand_device::DeviceResourceClosePhase::DisconnectCall,
                "fake_capture",
                None,
                None,
                actingcommand_device::DeviceResourceQuiescence::Unconfirmed,
                1,
            ));
            self.close_outcome = Some(outcome.clone());
            return outcome;
        }
        let outcome = match self
            .state
            .capture_close_error
            .lock()
            .expect("capture close error")
            .clone()
        {
            Some(error) => Err(error),
            None => Ok(actingcommand_device::DeviceResourceCloseOutcome::confirmed(
                1,
            )),
        };
        self.close_outcome = Some(outcome.clone());
        outcome
    }
}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        if self.close_outcome.is_none() {
            self.close_outcome = Some(Ok(
                actingcommand_device::DeviceResourceCloseOutcome::confirmed(1),
            ));
            self.state
                .capture_close_count
                .fetch_add(1, Ordering::AcqRel);
        }
    }
}

struct FakeEntry {
    instance_id: InstanceId,
    state: Arc<FakeState>,
}

const FAKE_EMULATOR_INDEX: u16 = 0;
const FAKE_EMULATOR_ADB_PORT: u16 = 16384;
/// INST-1 (Workflow #390): the first neutral port of `FakeProvider::with_device_endpoints`.
const FAKE_DEVICE_ADB_PORT_BASE: u16 = 15000;

/// Workflow #369 review C-2: the discovery-bound fake instance, bound to `adb_port` while
/// running and pending while stopped (`None`).
fn fake_discovered_instance(
    instance_id: InstanceId,
    adb_port: Option<u16>,
) -> ResolvedExecutionInstance {
    let binding = actingcommand_execution_kernel::DiscoveredInstanceBinding::new(
        FAKE_EMULATOR_INDEX,
        "fixture",
        "1.0.0",
        "C:/fixture/MuMuManager.exe",
    );
    let resolved = ResolvedExecutionInstance::new(instance_id, "127.0.0.1:16384");
    match adb_port {
        None => resolved.with_pending_endpoint(
            actingcommand_execution_kernel::PendingAdbEndpoint::new("127.0.0.1", binding),
        ),
        Some(adb_port) => resolved.with_adb_endpoint(
            actingcommand_execution_kernel::ResolvedAdbEndpoint::new("127.0.0.1", adb_port, false)
                .with_discovered_binding(binding),
        ),
    }
}

/// Workflow #369 review C-2: gives a started host's fake instance emulator control and its
/// discovery binding, so a ladder's rung 3 runs on it.
fn bind_fake_emulator(
    host: &RuntimeHost,
    instance_alias: &str,
    instance_id: InstanceId,
    state: &FakeState,
) {
    state.emulator_control.store(true, Ordering::Release);
    host.replace_instance_endpoint_for_test(
        instance_alias,
        fake_discovered_instance(instance_id, Some(FAKE_EMULATOR_ADB_PORT))
            .adb_endpoint()
            .cloned(),
    )
    .expect("bind the fake emulator's discovered endpoint");
}

pub(super) struct FakeProvider {
    entries: BTreeMap<String, FakeEntry>,
    advertised_aliases: Option<Vec<String>>,
    provenance: ExecutionBackendProvenance,
    vision_provider: Option<Arc<dyn VisionProvider>>,
    resolved_override: Option<Arc<std::sync::Mutex<ResolvedExecutionInstance>>>,
    // Workflow #398 RD-1b: one entry's own resolution (`with_resolved_for`), ahead of the
    // others.
    resolved_for: BTreeMap<String, ResolvedExecutionInstance>,
    // INST-1 (Workflow #390): every physical entry resolves with a bound ADB endpoint of its
    // own (`with_device_endpoints`).
    device_endpoints: bool,
    // Workflow #383 AO-4q: the scripted answer of an on-demand instance discovery
    // (`with_discovery`); without one the provider refuses discovery as a provider without a
    // discovery surface does.
    discovery: Option<ProviderInstanceDiscovery>,
    // Workflow #398 RD-2: the device-link watches a provider with handsets opens and the
    // reconnects it was asked for (`with_link_watch`); without it the provider has none.
    link_watches: Option<Arc<FakeLinkWatches>>,
}

// Workflow #398 RD-2: what a fake provider's device-link watches saw. A watch reports nothing by
// itself (a test hands the host its events, `device_link_event_for_test`); it blocks its reader
// until it is stopped. The fake records each reconnect by alias; the provider surface has no
// other command, so no `kill-server` can reach a device through it.
#[derive(Default)]
pub(super) struct FakeLinkWatches {
    opened: AtomicUsize,
    stopped: AtomicUsize,
    reconnects: std::sync::Mutex<Vec<String>>,
}

impl FakeLinkWatches {
    fn opened(&self) -> usize {
        self.opened.load(Ordering::Acquire)
    }

    fn stopped(&self) -> usize {
        self.stopped.load(Ordering::Acquire)
    }

    fn reconnects(&self) -> Vec<String> {
        self.reconnects.lock().expect("fake reconnects").clone()
    }
}

type FakeLinkWatchStopped = Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

struct FakeLinkWatch {
    watches: Arc<FakeLinkWatches>,
    stopped: FakeLinkWatchStopped,
}

impl actingcommand_execution_kernel::DeviceLinkWatch for FakeLinkWatch {
    fn next_event(&mut self) -> actingcommand_execution_kernel::DeviceLinkWatchEvent {
        let (stopped, changed) = &*self.stopped;
        let mut guard = stopped.lock().expect("fake link watch");
        while !*guard {
            guard = changed.wait(guard).expect("fake link watch");
        }
        actingcommand_execution_kernel::DeviceLinkWatchEvent::Ended
    }

    fn stopper(&self) -> Arc<dyn actingcommand_execution_kernel::DeviceLinkWatchStop> {
        Arc::new(FakeLinkWatchStop {
            watches: Arc::clone(&self.watches),
            stopped: Arc::clone(&self.stopped),
        })
    }
}

struct FakeLinkWatchStop {
    watches: Arc<FakeLinkWatches>,
    stopped: FakeLinkWatchStopped,
}

impl actingcommand_execution_kernel::DeviceLinkWatchStop for FakeLinkWatchStop {
    fn stop(&self) -> DeviceResult<()> {
        let (stopped, changed) = &*self.stopped;
        let mut guard = stopped.lock().expect("fake link watch");
        if !*guard {
            *guard = true;
            self.watches.stopped.fetch_add(1, Ordering::AcqRel);
        }
        changed.notify_all();
        Ok(())
    }
}

impl FakeProvider {
    pub(super) fn one(alias: &str, instance_id: InstanceId, state: Arc<FakeState>) -> Self {
        Self::from_entries([(alias.to_string(), instance_id, state)])
    }

    fn from_entries(
        entries: impl IntoIterator<Item = (String, InstanceId, Arc<FakeState>)>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(alias, instance_id, state)| (alias, FakeEntry { instance_id, state }))
                .collect(),
            advertised_aliases: None,
            provenance: ExecutionBackendProvenance::PhysicalDevice,
            vision_provider: None,
            resolved_override: None,
            resolved_for: BTreeMap::new(),
            device_endpoints: false,
            discovery: None,
            link_watches: None,
        }
    }

    /// Workflow #398 RD-2: the provider opens device-link watches into `watches` and records the
    /// reconnects it is asked for there.
    fn with_link_watch(mut self, watches: Arc<FakeLinkWatches>) -> Self {
        self.link_watches = Some(watches);
        self
    }

    /// Workflow #383 AO-4q: answers an on-demand instance discovery with `discovery`.
    fn with_discovery(mut self, discovery: ProviderInstanceDiscovery) -> Self {
        self.discovery = Some(discovery);
        self
    }

    /// Workflow #398 RD-1b: `instance_alias` resolves as `resolved`; the other entries as before.
    fn with_resolved_for(mut self, instance_alias: &str, resolved: ResolvedExecutionInstance) -> Self {
        self.resolved_for.insert(instance_alias.to_owned(), resolved);
        self
    }

    /// INST-1 (Workflow #390): every physical entry resolves with a bound ADB endpoint of its
    /// own (one neutral port per entry, in alias order), as configured instances do, so a
    /// start prepares and self-checks each of them (`device_self_checked`).
    fn with_device_endpoints(mut self) -> Self {
        self.device_endpoints = true;
        self
    }

    fn fixture_simulation(mut self) -> Self {
        self.provenance = ExecutionBackendProvenance::FixtureSimulation;
        self
    }

    fn with_inventory(mut self, aliases: impl IntoIterator<Item = String>) -> Self {
        self.advertised_aliases = Some(aliases.into_iter().collect());
        self
    }

    fn with_vision_provider(mut self, vision_provider: Arc<dyn VisionProvider>) -> Self {
        self.vision_provider = Some(vision_provider);
        self
    }

    fn with_resolved_override(
        mut self,
        resolved: Arc<std::sync::Mutex<ResolvedExecutionInstance>>,
    ) -> Self {
        self.resolved_override = Some(resolved);
        self
    }

    /// Whether the instance's opens report a passed native open (`FakeState::observed_open`).
    fn observed_open(&self, instance_alias: &str) -> bool {
        self.entries
            .get(instance_alias)
            .is_some_and(|entry| entry.state.observed_open.load(Ordering::Acquire))
    }
}

/// Workflow #369 (verify-666 L6): an opened fake backend with an unobserved report, or with a
/// passed native one (capture `adb_screencap` 1280x720; input `adb_shell_input` with its
/// geometry), so a preparation's self-check passes.
fn fake_opened_backend<T>(
    backend: T,
    entry: actingcommand_contract::BackendOpenEntry,
    observed: bool,
) -> actingcommand_device::OpenedBackend<T> {
    use actingcommand_contract::{BackendObservationStatus, BackendOpenReport, BackendOpenSource};
    if !observed {
        return actingcommand_device::OpenedBackend::unobserved(backend, entry);
    }
    let mut report = BackendOpenReport::unobserved(entry);
    report.source = BackendOpenSource::Native;
    report.status = BackendObservationStatus::Passed;
    report.connection = BackendObservationStatus::Passed;
    if entry == actingcommand_contract::BackendOpenEntry::Capture {
        report.requested = "adb_screencap".to_owned();
        report.selected = Some("adb_screencap".to_owned());
        report.capture_check = BackendObservationStatus::Passed;
        report.frame_width = Some(1280);
        report.frame_height = Some(720);
    } else {
        report.requested = "adb_shell_input".to_owned();
        report.selected = Some("adb_shell_input".to_owned());
        report.input_check = BackendObservationStatus::Passed;
        report.input_geometry = Some(actingcommand_contract::BackendInputGeometryObservation {
            natural_max_x: 1280,
            natural_max_y: 720,
            rotation_degrees: 0,
        });
    }
    actingcommand_device::OpenedBackend::new(backend, report)
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        self.advertised_aliases
            .clone()
            .unwrap_or_else(|| self.entries.keys().cloned().collect())
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        let entry = self.entries.get(instance_alias)?;
        if entry.state.emulator_control.load(Ordering::Acquire) {
            return Some(fake_discovered_instance(
                entry.instance_id,
                entry.state.emulator.bound_port(),
            ));
        }
        if let Some(resolved) = self.resolved_for.get(instance_alias) {
            return Some(resolved.clone());
        }
        if let Some(resolved) = &self.resolved_override {
            return Some(
                resolved
                    .lock()
                    .expect("resolved instance override poisoned")
                    .clone(),
            );
        }
        Some(match self.provenance {
            ExecutionBackendProvenance::PhysicalDevice if self.device_endpoints => {
                let index = self
                    .entries
                    .keys()
                    .position(|alias| alias == instance_alias)
                    .and_then(|index| u16::try_from(index).ok())
                    .expect("a registered fake entry");
                let adb_port = FAKE_DEVICE_ADB_PORT_BASE + index;
                ResolvedExecutionInstance::new(entry.instance_id, format!("127.0.0.1:{adb_port}"))
                    .with_adb_endpoint(actingcommand_execution_kernel::ResolvedAdbEndpoint::new(
                        "127.0.0.1",
                        adb_port,
                        false,
                    ))
            }
            ExecutionBackendProvenance::PhysicalDevice => {
                ResolvedExecutionInstance::new(entry.instance_id, "127.0.0.1:16384")
            }
            ExecutionBackendProvenance::FixtureSimulation => {
                ResolvedExecutionInstance::fixture_simulation(entry.instance_id)
            }
        })
    }

    fn vision_provider(&self) -> Option<Arc<dyn VisionProvider>> {
        self.vision_provider.as_ref().map(Arc::clone)
    }

    fn open_input(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn InputBackend>>> {
        let open = || -> DeviceResult<Box<dyn InputBackend>> {
            let entry = self
                .entries
                .get(instance_alias)
                .ok_or_else(|| DeviceError::fatal("fake instance is not registered"))?;
            entry.state.open_count.fetch_add(1, Ordering::AcqRel);
            if let Some(error) = entry
                .state
                .input_open_error
                .lock()
                .expect("input open error")
                .clone()
            {
                return Err(error);
            }
            Ok(Box::new(FakeBackend {
                state: Arc::clone(&entry.state),
                close_outcome: None,
            }))
        };
        let result: DeviceResult<Box<dyn InputBackend>> = open();
        let observed = self.observed_open(instance_alias);
        result.map(|backend| {
            fake_opened_backend(
                backend,
                actingcommand_contract::BackendOpenEntry::Input,
                observed,
            )
        })
    }

    fn open_capture(
        &self,
        instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<actingcommand_device::OpenedBackend<Box<dyn CaptureBackend>>> {
        let open = || -> DeviceResult<Box<dyn CaptureBackend>> {
            let entry = self
                .entries
                .get(instance_alias)
                .ok_or_else(|| DeviceError::fatal("fake instance is not registered"))?;
            entry
                .state
                .capture_open_count
                .fetch_add(1, Ordering::AcqRel);
            if let Some(error) = entry
                .state
                .capture_open_error
                .lock()
                .expect("capture open error")
                .as_ref()
            {
                return Err(error.clone());
            }
            Ok(Box::new(FakeCapture {
                state: Arc::clone(&entry.state),
                provenance: self.provenance,
                audit_endpoint: self
                    .resolve(instance_alias)
                    .expect("resolved fake capture instance")
                    .audit_endpoint()
                    .to_owned(),
                close_outcome: None,
            }))
        };
        let result: DeviceResult<Box<dyn CaptureBackend>> = open();
        let observed = self.observed_open(instance_alias);
        result.map(|backend| {
            fake_opened_backend(
                backend,
                actingcommand_contract::BackendOpenEntry::Capture,
                observed,
            )
        })
    }

    fn observe_monitor(
        &self,
        instance_alias: &str,
        expected_page: &str,
        _frame: &Frame,
    ) -> actingcommand_execution_kernel::ExecutionKernelResult<MonitorObservation> {
        let entry = self
            .entries
            .get(instance_alias)
            .expect("resolved fake monitor instance");
        entry
            .state
            .monitor_observation_count
            .fetch_add(1, Ordering::AcqRel);
        let observation = match entry.state.monitor_mode.load(Ordering::Acquire) {
            0 => MonitorObservation::new(
                MonitorDiagnosis::Healthy,
                expected_page,
                Some(expected_page.to_string()),
            ),
            1 => MonitorObservation::new(MonitorDiagnosis::Standby, expected_page, None),
            2 => MonitorObservation::new(
                MonitorDiagnosis::UnexpectedPage,
                expected_page,
                Some("unexpected".to_string()),
            ),
            3 => MonitorObservation::new(
                MonitorDiagnosis::CaptureStaleSuspected,
                expected_page,
                None,
            ),
            _ => MonitorObservation::new(
                MonitorDiagnosis::Healthy,
                "wrong-policy-page",
                Some("wrong-policy-page".to_string()),
            ),
        }
        .expect("fake monitor observation must be valid");
        Ok(observation)
    }

    // Workflow #398 RD-2: a provider with handsets opens a watch that reports nothing by itself.
    fn open_device_link_watch(
        &self,
    ) -> DeviceResult<Option<Box<dyn actingcommand_execution_kernel::DeviceLinkWatch>>> {
        let Some(watches) = &self.link_watches else {
            return Ok(None);
        };
        watches.opened.fetch_add(1, Ordering::AcqRel);
        Ok(Some(Box::new(FakeLinkWatch {
            watches: Arc::clone(watches),
            stopped: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
        })))
    }

    fn reconnect_handset(&self, instance_alias: &str) -> DeviceResult<()> {
        let Some(watches) = &self.link_watches else {
            return Err(DeviceError::fatal("the fake provider has no handset"));
        };
        watches
            .reconnects
            .lock()
            .expect("fake reconnects")
            .push(instance_alias.to_owned());
        Ok(())
    }

    // Workflow #387 D1c-G: adbd answers every probe unless the instance's `adb_probe_error` is
    // set.
    fn probe_adb_baseline(&self, instance_alias: &str) -> DeviceResult<()> {
        let error = self.entries.get(instance_alias).and_then(|entry| {
            entry
                .state
                .adb_probe_error
                .lock()
                .expect("adb probe error")
                .clone()
        });
        error.map_or(Ok(()), Err)
    }

    // Slice #316-B3: the fake device always reports its assigned application in the
    // foreground, so the foreground gate passes exactly as before the gate existed.
    // Workflow #369 review C-2: an emulator-controlled fake held "not booted" after its Start
    // reports no foreground activity. Workflow #387 D1c-G: a set `foreground_error_once` is the
    // next query's answer.
    fn observe_foreground_application(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<crate::ForegroundApplicationObservation> {
        if let Some(error) = self.entries.get(instance_alias).and_then(|entry| {
            entry
                .state
                .foreground_error_once
                .lock()
                .expect("one-shot foreground error")
                .take()
        }) {
            return Err(error);
        }
        if let Some(entry) = self
            .entries
            .get(instance_alias)
            .filter(|entry| entry.state.emulator_control.load(Ordering::Acquire))
        {
            entry
                .state
                .emulator
                .note(emulator_controller::EmulatorCall::ObserveForeground);
        }
        let android_down = self
            .entries
            .get(instance_alias)
            .is_some_and(|entry| entry.state.android_down.load(Ordering::Acquire));
        Ok(crate::ForegroundApplicationObservation {
            foreground: (!android_down).then(|| "neutral.application".to_owned()),
            assigned: "neutral.application".to_owned(),
        })
    }

    /// Workflow #369 review C-2: Stop and Start of a fake bound with `bind_fake_emulator`; any
    /// other instance keeps the provider's typed refusal. Workflow #381 B: the result comes from
    /// the scripted controller (HOST-I1) unless `fail_emulator_start` fails the Start.
    fn control_instance(
        &self,
        instance_alias: &str,
        action: actingcommand_contract::EmulatorInstanceAction,
    ) -> actingcommand_device::EmulatorControlResult<actingcommand_device::EmulatorControlOutcome>
    {
        let Some(entry) = self
            .entries
            .get(instance_alias)
            .filter(|entry| entry.state.emulator_control.load(Ordering::Acquire))
        else {
            return Err(
                actingcommand_device::EmulatorControlFailure::without_output(
                    DeviceError::fatal("emulator control unsupported by this provider")
                        .with_diagnostic(
                            DeviceErrorCategory::Protocol,
                            actingcommand_execution_kernel::ExecutionKernelLocation::EmulatorControlSupport.location(),
                        ),
                    0,
                ),
            );
        };
        entry
            .state
            .emulator_actions
            .lock()
            .expect("fake emulator actions lock")
            .push(action);
        let running = action != actingcommand_contract::EmulatorInstanceAction::Stop;
        if running {
            entry
                .state
                .emulator_start_entered
                .store(true, Ordering::Release);
            while entry.state.block_emulator_start.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
            if entry.state.fail_emulator_start.load(Ordering::Acquire) {
                entry
                    .state
                    .emulator
                    .note(emulator_controller::EmulatorCall::Control(action));
                return Err(
                    actingcommand_device::EmulatorControlFailure::without_output(
                        DeviceError::fatal("fixture emulator readiness wait timed out")
                            .with_diagnostic(
                                DeviceErrorCategory::ChildExit,
                                actingcommand_device::codes::DeviceStage::MumuManagerWaitTimeout.location(),
                            ),
                        0,
                    ),
                );
            }
        } else {
            while entry.state.block_emulator_stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(5));
            }
        }
        let outcome = entry.state.emulator.control(FAKE_EMULATOR_INDEX, action)?;
        entry.state.android_down.store(
            !outcome.running || entry.state.hold_android_boot.load(Ordering::Acquire),
            Ordering::Release,
        );
        Ok(outcome)
    }

    /// Workflow #381 B (review M3): the state read of a fake bound with `bind_fake_emulator`
    /// comes from the scripted controller; any other instance keeps the typed refusal.
    fn read_instance_state(&self, instance_alias: &str) -> DeviceResult<crate::InstanceState> {
        let entry = self
            .entries
            .get(instance_alias)
            .filter(|entry| entry.state.emulator_control.load(Ordering::Acquire))
            .ok_or_else(|| {
                DeviceError::fatal("emulator state read unsupported by this provider")
                    .with_diagnostic(
                        DeviceErrorCategory::Protocol,
                        actingcommand_execution_kernel::ExecutionKernelLocation::EmulatorControlSupport.location(),
                    )
            })?;
        entry.state.emulator.read_state()
    }

    /// Workflow #383 AO-4q: the scripted discovery answer, else the typed refusal of a provider
    /// without a discovery surface.
    fn discover_instances(
        &self,
    ) -> Result<ProviderInstanceDiscovery, Box<InstanceDiscoveryFailure>> {
        self.discovery.clone().ok_or_else(|| {
            Box::new(InstanceDiscoveryFailure {
                code: actingcommand_contract::codes::SharedCode::InstanceDiscoveryUnavailable
                    .as_str(),
                error: DeviceError::fatal("instance discovery unsupported by this provider")
                    .with_diagnostic(
                        DeviceErrorCategory::Protocol,
                        actingcommand_contract::codes::ContractLocation::InstanceDiscoveryUnsupported,
                    ),
            })
        })
    }

    /// Workflow #369 review C-2: `Some(port)` binds the fake's discovered endpoint again,
    /// `None` returns it to pending.
    fn rebind_discovered_endpoint(
        &self,
        instance_alias: &str,
        adb_port: Option<u16>,
    ) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .filter(|entry| entry.state.emulator_control.load(Ordering::Acquire))
            .ok_or_else(|| {
                DeviceError::fatal("emulator control unsupported by this provider").with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    actingcommand_execution_kernel::ExecutionKernelLocation::EmulatorControlSupport.location(),
                )
            })?;
        entry
            .state
            .emulator_stopped
            .store(adb_port.is_none(), Ordering::Release);
        entry.state.emulator.rebind(adb_port);
        Ok(())
    }

    fn control_application(
        &self,
        _witness: &actingcommand_contract::FencedWrite,
        instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .expect("resolved fake application instance");
        entry.state.application_count.fetch_add(1, Ordering::AcqRel);
        if let Some(error) = entry
            .state
            .application_error
            .lock()
            .expect("application error")
            .take()
        {
            return Err(error);
        }
        if entry.state.fail_application.load(Ordering::Acquire) {
            Err(DeviceError::fatal("private application failure"))
        } else {
            Ok(())
        }
    }
}
