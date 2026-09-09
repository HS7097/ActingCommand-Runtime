// SPDX-License-Identifier: AGPL-3.0-only

#[derive(Default)]
struct FakeState {
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
    block_input: AtomicBool,
    input_started: AtomicBool,
    capture_open_count: AtomicUsize,
    capture_open_error: std::sync::Mutex<Option<DeviceError>>,
    capture_count: AtomicUsize,
    capture_delay_ms: AtomicU64,
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
    input_actions: std::sync::Mutex<Vec<InputAction>>,
    segmented_swipe_plans: std::sync::Mutex<Vec<PreparedSegmentedSwipePlan>>,
}

struct FakeBackend {
    state: Arc<FakeState>,
    close_outcome: Option<DeviceResult<actingcommand_device::DeviceResourceCloseOutcome>>,
}

struct FakeCapture {
    state: Arc<FakeState>,
    provenance: ExecutionBackendProvenance,
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
        if self.state.fail_input.load(Ordering::Acquire) {
            return Err(DeviceError::fatal("injected backend failure")
                .with_diagnostic(
                    DeviceErrorCategory::Native,
                    "device_registry.input.operation",
                )
                .with_diagnostic_context(
                    "adb_shell_input",
                    "reset",
                    DeviceErrorSensitivity::Sensitive,
                ));
        }
        self.state.input_count.fetch_add(1, Ordering::AcqRel);
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

    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.input(InputAction::Tap { x, y })
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.input(InputAction::LongTap { x, y, duration_ms })
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
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

    fn segmented_swipe_prepared(&mut self, plan: &PreparedSegmentedSwipePlan) -> DeviceResult<()> {
        self.state
            .segmented_swipe_plans
            .lock()
            .expect("fake segmented swipe plans lock")
            .push(plan.clone());
        self.complete_input()
    }

    fn key(&mut self, key: &str) -> DeviceResult<()> {
        self.input(InputAction::Key {
            key: key.to_string(),
        })
    }

    fn text(&mut self, text: &str) -> DeviceResult<()> {
        self.input(InputAction::Text {
            text: text.to_string(),
        })
    }

    fn reset(&mut self) -> DeviceResult<()> {
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
                    "device_registry.capture.operation",
                )
                .with_diagnostic_context(
                    "nemu_ipc",
                    "capture",
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
        let mut frame = Frame::from_pixels(
            2,
            1,
            [first.as_slice(), guard.as_slice()].concat(),
            PixelFormat::Rgb8,
            match self.provenance {
                ExecutionBackendProvenance::PhysicalDevice => CaptureBackendName::AdbScreencap,
                ExecutionBackendProvenance::FixtureSimulation => {
                    CaptureBackendName::FixtureSimulation
                }
            },
        )?;
        frame.selection = self
            .state
            .capture_selection
            .lock()
            .expect("capture selection")
            .clone()
            .map(Arc::new);
        Ok(frame)
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
            && authority != actingcommand_device::DeviceCloseAuthority::FencedDeviceWrite
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

struct FakeProvider {
    entries: BTreeMap<String, FakeEntry>,
    advertised_aliases: Option<Vec<String>>,
    provenance: ExecutionBackendProvenance,
    vision_provider: Option<Arc<dyn VisionProvider>>,
    resolved_override: Option<Arc<std::sync::Mutex<ResolvedExecutionInstance>>>,
}

impl FakeProvider {
    fn one(alias: &str, instance_id: InstanceId, state: Arc<FakeState>) -> Self {
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
        }
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
}

impl ExecutionBackendProvider for FakeProvider {
    fn instance_aliases(&self) -> Vec<String> {
        self.advertised_aliases
            .clone()
            .unwrap_or_else(|| self.entries.keys().cloned().collect())
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        let entry = self.entries.get(instance_alias)?;
        if let Some(resolved) = &self.resolved_override {
            return Some(
                resolved
                    .lock()
                    .expect("resolved instance override poisoned")
                    .clone(),
            );
        }
        Some(match self.provenance {
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

    fn open_input(&self, instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
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
    }

    fn open_capture(&self, instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
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
            close_outcome: None,
        }))
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

    fn control_application(
        &self,
        instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        let entry = self
            .entries
            .get(instance_alias)
            .expect("resolved fake application instance");
        entry.state.application_count.fetch_add(1, Ordering::AcqRel);
        if entry.state.fail_application.load(Ordering::Acquire) {
            Err(DeviceError::fatal("private application failure"))
        } else {
            Ok(())
        }
    }
}
