// SPDX-License-Identifier: AGPL-3.0-only

use crate::capture::{DeviceRotation, display_size_from_natural, read_device_rotation};
use crate::{
    Adb, AdbBoundsAction, AdbBoundsCoordinate, AdbConfig, AdbInputBoundsContext,
    AdbInputConnectGeometry, DeviceCloseAuthority, DeviceError, DeviceErrorCategory,
    DeviceErrorDiagnosticMessage, DeviceErrorSensitivity, DeviceInfo, DeviceLocation,
    DeviceResourceCloseOutcome, DeviceResourceQuiescence, DeviceResult, DeviceTarget,
    HandshakeInfo, InputBackend, MaaTouchBackend, MaaTouchConfig, MinitouchBackend,
    MinitouchConfig, PreparedSegmentedSwipePlan, SegmentedSwipeEvent,
    segmented_swipe_capability_error,
};
use actingcommand_contract::FencedWrite;
use actingcommand_contract::outcome::Location;
use std::time::{Duration, Instant};

const MAX_ADB_INPUT_GESTURE_MS: u64 = 60_000;
/// Workflow #406 T3 (plan Q18): the lowest Android SDK level whose `input` command takes
/// `motionevent` (Android 6.0), from which the adb shell input backend performs segmented plans.
const ADB_MOTIONEVENT_MIN_SDK_LEVEL: u32 = 23;
/// Workflow #406 T3: the time each command of a motionevent script may take beyond the plan's
/// own waits (every `input` call starts a process on the device), counted into the timeout of
/// the script's one adb invocation so that a slow device is not cut off before the release.
const ADB_MOTIONEVENT_COMMAND_ALLOWANCE_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchBackendName {
    NemuIpc,
    MaaTouch,
    Minitouch,
    AdbShellInput,
}

impl TouchBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NemuIpc => "nemu_ipc",
            Self::MaaTouch => "maatouch",
            Self::Minitouch => "minitouch",
            Self::AdbShellInput => "adb_shell_input",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TouchBackendChoice {
    #[default]
    Auto,
    AutoFastest,
    NemuIpc,
    MaaTouch,
    Minitouch,
    AdbShellInput,
}

impl TouchBackendChoice {
    pub fn parse(value: &str) -> DeviceResult<Self> {
        match value {
            "nemu_ipc" => Ok(Self::NemuIpc),
            "auto" => Ok(Self::Auto),
            "auto-fastest" | "auto_fastest" => Ok(Self::AutoFastest),
            "maatouch" | "maa_touch" => Ok(Self::MaaTouch),
            "minitouch" | "mini_touch" => Ok(Self::Minitouch),
            "adb" | "adb_input" | "adb-input" | "adb_shell_input" | "adb-shell-input" => {
                Ok(Self::AdbShellInput)
            }
            other => Err(DeviceError::fatal(format!(
                "unknown touch backend '{other}', expected auto, auto-fastest, nemu_ipc, maatouch, minitouch, or adb_shell_input"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NemuIpc => "nemu_ipc",
            Self::Auto => "auto",
            Self::AutoFastest => "auto-fastest",
            Self::MaaTouch => "maatouch",
            Self::Minitouch => "minitouch",
            Self::AdbShellInput => "adb_shell_input",
        }
    }

    /// #398 R1 §3.1 (RD-1b): a handset has no Nemu IPC input; every other choice stays open to
    /// both kinds (the automatic choices are refused elsewhere: every instance names its
    /// backend).
    pub fn allowed_on(self, kind: crate::DeviceKind) -> bool {
        !matches!((self, kind), (Self::NemuIpc, crate::DeviceKind::Handset))
    }
}

#[derive(Debug, Clone)]
pub struct TouchBackendConfig {
    pub adb_config: AdbConfig,
    pub target: DeviceTarget,
    pub maatouch_config: MaaTouchConfig,
    pub minitouch_config: MinitouchConfig,
    pub requested: TouchBackendChoice,
}

impl TouchBackendConfig {
    pub fn new(
        adb_config: AdbConfig,
        target: DeviceTarget,
        maatouch_config: MaaTouchConfig,
    ) -> Self {
        Self {
            adb_config,
            target,
            maatouch_config,
            minitouch_config: MinitouchConfig::default(),
            requested: TouchBackendChoice::Auto,
        }
    }

    pub fn with_minitouch_config(mut self, minitouch_config: MinitouchConfig) -> Self {
        self.minitouch_config = minitouch_config;
        self
    }

    pub fn with_requested(mut self, requested: TouchBackendChoice) -> Self {
        self.requested = requested;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchBackendAttempt {
    pub input_parameters: Option<actingcommand_contract::BackendInputParameterCheck>,
    pub attempt_id: u64,
    pub backend: TouchBackendName,
    pub ok: bool,
    pub elapsed_ms: u128,
    pub error_reason: Option<String>,
    pub action: Option<String>,
    pub fallback_backend: Option<TouchBackendName>,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchBackendDiagnostics {
    pub requested: TouchBackendChoice,
    pub selected: Option<TouchBackendName>,
    pub attempts: Vec<TouchBackendAttempt>,
    pub warnings: Vec<String>,
}

impl TouchBackendDiagnostics {
    fn record_input_parameters(
        &mut self,
        check: Option<&actingcommand_contract::BackendInputParameterCheck>,
    ) {
        self.attempts
            .last_mut()
            .expect("connection attempt recorded")
            .input_parameters = check.cloned();
    }
    fn new(requested: TouchBackendChoice) -> Self {
        Self {
            requested,
            selected: None,
            attempts: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn push_attempt(&mut self, mut attempt: TouchBackendAttempt) {
        attempt.attempt_id = self.attempts.len() as u64 + 1;
        self.attempts.push(attempt);
    }

    fn push_success(
        &mut self,
        backend: TouchBackendName,
        elapsed_ms: u128,
        action: &str,
        selected: bool,
    ) {
        self.push_attempt(TouchBackendAttempt {
            input_parameters: None,
            attempt_id: self.attempts.len() as u64 + 1,
            backend,
            ok: true,
            elapsed_ms,
            error_reason: None,
            action: Some(action.to_string()),
            fallback_backend: None,
            selected,
        });
    }

    fn push_failure(
        &mut self,
        backend: TouchBackendName,
        elapsed_ms: u128,
        error_reason: String,
        action: &str,
        fallback_backend: Option<TouchBackendName>,
    ) {
        self.push_attempt(TouchBackendAttempt {
            input_parameters: None,
            attempt_id: self.attempts.len() as u64 + 1,
            backend,
            ok: false,
            elapsed_ms,
            error_reason: Some(error_reason),
            action: Some(action.to_string()),
            fallback_backend,
            selected: false,
        });
    }
}

pub struct ConnectedTouchBackend {
    pub input_parameters: Option<actingcommand_contract::BackendInputParameterCheck>,
    pub name: TouchBackendName,
    pub backend: Box<dyn InputBackend>,
    pub device: DeviceInfo,
    pub handshake: Option<HandshakeInfo>,
}

pub trait TouchBackendFactory {
    fn name(&self) -> TouchBackendName;
    fn connect(&self) -> DeviceResult<ConnectedTouchBackend>;
}

pub struct SelectedTouchBackend {
    // Bounded by the original remaining factory chain; moved out by this action.
    action_open_observations: Vec<crate::BackendOpenObservation>,
    active: ConnectedTouchBackend,
    remaining: Vec<Box<dyn TouchBackendFactory>>,
    diagnostics: TouchBackendDiagnostics,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

impl SelectedTouchBackend {
    pub(crate) fn input_parameters(
        &self,
    ) -> Option<&actingcommand_contract::BackendInputParameterCheck> {
        self.active.input_parameters.as_ref()
    }
    pub(crate) fn opened_input_geometry(
        &self,
    ) -> Option<actingcommand_contract::BackendInputGeometryObservation> {
        self.active.backend.opened_geometry()
    }

    pub fn backend_name(&self) -> TouchBackendName {
        self.active.name
    }

    pub fn serial(&self) -> &str {
        &self.active.device.serial
    }

    pub fn device_info(&self) -> &DeviceInfo {
        &self.active.device
    }

    pub fn handshake_info(&self) -> Option<&HandshakeInfo> {
        self.active.handshake.as_ref()
    }

    pub fn diagnostics(&self) -> &TouchBackendDiagnostics {
        &self.diagnostics
    }

    fn set_selected(&mut self, name: TouchBackendName) {
        self.diagnostics.selected = Some(name);
        for attempt in &mut self.diagnostics.attempts {
            attempt.selected = attempt.backend == name && attempt.ok;
        }
    }

    fn run_touch_action(
        &mut self,
        action: &'static str,
        points: &[(i32, i32)],
        run: impl FnMut(&mut dyn InputBackend) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        self.run_touch_action_inner(action, points, run)
            .map_err(|mut error| {
                for observation in self.take_backend_open_observations() {
                    error = error.with_backend_open_observation(observation);
                }
                error
            })
    }

    fn run_touch_action_inner(
        &mut self,
        action: &'static str,
        points: &[(i32, i32)],
        mut run: impl FnMut(&mut dyn InputBackend) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        self.validate_action_points(action, points)?;

        let active_started = Instant::now();
        let active_failure = match run(self.active.backend.as_mut()) {
            Ok(()) => {
                // MaaTouch/Minitouch measure the local write+flush; AdbShellInput's child exit is the real ack.
                let elapsed_ms = active_started.elapsed().as_millis();
                self.diagnostics
                    .push_success(self.active.name, elapsed_ms, action, true);
                return Ok(());
            }
            Err(err) => {
                let elapsed_ms = active_started.elapsed().as_millis();
                let fallback_backend = err
                    .is_fallback_eligible()
                    .then(|| self.next_fallback_backend())
                    .flatten();
                self.record_runtime_failure(
                    action,
                    self.active.name,
                    &err,
                    elapsed_ms,
                    fallback_backend,
                );
                if !err.is_fallback_eligible() {
                    return Err(err);
                }
                err
            }
        };

        while !self.remaining.is_empty() {
            let factory = self.remaining.remove(0);
            let backend_name = factory.name();
            let started = Instant::now();
            let connected = factory.connect();
            self.action_open_observations.push(
                crate::backend_open::observe_touch_action_connection(
                    self.diagnostics.requested,
                    backend_name,
                    &connected,
                ),
            );
            match connected {
                Ok(mut connected) => match run(connected.backend.as_mut()) {
                    Ok(()) => {
                        let elapsed_ms = started.elapsed().as_millis();
                        self.diagnostics
                            .push_success(connected.name, elapsed_ms, action, true);
                        self.diagnostics
                            .record_input_parameters(connected.input_parameters.as_ref());
                        if let Err(cleanup) = self
                            .active
                            .backend
                            .close_once(DeviceCloseAuthority::LocalOnly)
                        {
                            let cleanup = cleanup.with_backend_resources();
                            self.close_result = Some(Err(cleanup.clone()));
                            let mut error = active_failure.clone().merge_resource_cleanup(cleanup);
                            match connected
                                .backend
                                .close_once(DeviceCloseAuthority::LocalOnly)
                            {
                                Ok(outcome) => {
                                    error = error
                                        .with_close_outcome(&outcome.with_operation_resources());
                                }
                                Err(new_cleanup) => {
                                    let unconfirmed = new_cleanup.resource_quiescence()
                                        == Some(DeviceResourceQuiescence::Unconfirmed);
                                    error = error.merge_resource_cleanup(
                                        new_cleanup.with_operation_resources(),
                                    );
                                    if unconfirmed {
                                        std::mem::forget(connected);
                                    }
                                }
                            }
                            return Err(error);
                        }
                        self.active = connected;
                        self.set_selected(self.active.name);
                        return Ok(());
                    }
                    Err(err) => {
                        let elapsed_ms = started.elapsed().as_millis();
                        let reason = err.to_string();
                        let fallback_backend = self.next_fallback_backend();
                        self.diagnostics.push_failure(
                            connected.name,
                            elapsed_ms,
                            reason.clone(),
                            action,
                            fallback_backend,
                        );
                        self.diagnostics
                            .record_input_parameters(connected.input_parameters.as_ref());
                        self.diagnostics.warnings.push(format!(
                            "WARNING touch backend {} failed during {action}; fallback_backend={}; reason={reason}",
                            connected.name.as_str(),
                            fallback_backend.map(TouchBackendName::as_str).unwrap_or("none")
                        ));
                        if let Err(close_err) = connected
                            .backend
                            .close_once(DeviceCloseAuthority::LocalOnly)
                        {
                            let unconfirmed = close_err.resource_quiescence()
                                == Some(DeviceResourceQuiescence::Unconfirmed);
                            let error = err.merge_resource_cleanup(close_err);
                            if unconfirmed {
                                std::mem::forget(connected);
                            }
                            return Err(error);
                        }
                        if !err.is_fallback_eligible() {
                            return Err(err);
                        }
                    }
                },
                Err(err) => {
                    let elapsed_ms = started.elapsed().as_millis();
                    let reason = err.to_string();
                    let fallback_backend = self.next_fallback_backend();
                    self.diagnostics.push_failure(
                        backend_name,
                        elapsed_ms,
                        reason.clone(),
                        action,
                        fallback_backend,
                    );
                    self.diagnostics
                        .record_input_parameters(err.input_parameters());
                    self.diagnostics.warnings.push(format!(
                        "WARNING touch backend {} could not be selected for {action}; fallback_backend={}; reason={reason}",
                        backend_name.as_str(),
                        fallback_backend.map(TouchBackendName::as_str).unwrap_or("none")
                    ));
                    if !err.is_fallback_eligible() {
                        return Err(err);
                    }
                }
            }
        }

        Err(self.chain_failed_error(action))
    }

    fn chain_failed_error(&self, action: &str) -> DeviceError {
        DeviceError::fatal(format!(
            "touch backend chain failed during {action}; diagnostics: {}",
            format_touch_diagnostics(&self.diagnostics)
        ))
    }

    fn record_runtime_failure(
        &mut self,
        action: &str,
        backend: TouchBackendName,
        err: &DeviceError,
        elapsed_ms: u128,
        fallback_backend: Option<TouchBackendName>,
    ) {
        let reason = err.to_string();
        self.diagnostics.push_failure(
            backend,
            elapsed_ms,
            reason.clone(),
            action,
            fallback_backend,
        );
        self.diagnostics.warnings.push(format!(
            "WARNING touch backend {} failed during {action}; fallback_backend={}; reason={reason}",
            backend.as_str(),
            fallback_backend
                .map(TouchBackendName::as_str)
                .unwrap_or("none")
        ));
    }

    fn next_fallback_backend(&self) -> Option<TouchBackendName> {
        self.remaining.first().map(|factory| factory.name())
    }

    fn validate_action_points(&self, action: &str, points: &[(i32, i32)]) -> DeviceResult<()> {
        if self.active.name == TouchBackendName::AdbShellInput {
            return Ok(());
        }
        let bounds = touch_bounds_for_backend(
            self.active.name,
            self.active.handshake.as_ref(),
            &self.active.device,
        )?;
        for (index, (x, y)) in points.iter().enumerate() {
            validate_touch_coordinate(&format!("{action} point {index} x"), *x, bounds.max_x)?;
            validate_touch_coordinate(&format!("{action} point {index} y"), *y, bounds.max_y)?;
        }
        Ok(())
    }
}

impl InputBackend for SelectedTouchBackend {
    fn take_backend_open_observations(&mut self) -> Vec<crate::BackendOpenObservation> {
        std::mem::take(&mut self.action_open_observations)
    }

    fn take_adb_recovery(&mut self) -> Option<crate::AdbTargetRecovery> {
        self.active.backend.take_adb_recovery()
    }

    fn selection_context(&self) -> Option<crate::InputSelectionContext> {
        Some(crate::InputSelectionContext {
            backend: self.backend_name(),
            serial: self.serial().to_owned(),
        })
    }

    fn tap(&mut self, witness: &FencedWrite, x: i32, y: i32) -> DeviceResult<()> {
        self.run_touch_action("tap", &[(x, y)], |backend| backend.tap(witness, x, y))
    }

    fn long_tap(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.run_touch_action("long_tap", &[(x, y)], |backend| {
            backend.long_tap(witness, x, y, duration_ms)
        })
    }

    fn swipe(
        &mut self,
        witness: &FencedWrite,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.run_touch_action("swipe", &[(x1, y1), (x2, y2)], |backend| {
            backend.swipe(witness, x1, y1, x2, y2, duration_ms)
        })
    }

    fn supports_segmented_swipe(&self) -> bool {
        self.active.backend.supports_segmented_swipe()
    }

    fn segmented_swipe_prepared(
        &mut self,
        witness: &FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
    ) -> DeviceResult<()> {
        let action = plan.action();
        action.validate()?;
        if !self.active.backend.supports_segmented_swipe() {
            return Err(DeviceError::fatal(format!(
                "touch backend {} does not support {}",
                self.active.name.as_str(),
                action.name()
            )));
        }
        self.validate_action_points(action.name(), &action.points())?;
        let started = Instant::now();
        match self.active.backend.segmented_swipe_prepared(witness, plan) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.record_runtime_failure(
                    action.name(),
                    self.active.name,
                    &err,
                    started.elapsed().as_millis(),
                    None,
                );
                Err(err)
            }
        }
    }

    fn key(&mut self, witness: &FencedWrite, key: &str) -> DeviceResult<()> {
        self.active.backend.key(witness, key)
    }

    fn text(&mut self, witness: &FencedWrite, text: &str) -> DeviceResult<()> {
        self.active.backend.text(witness, text)
    }

    fn reset(&mut self, witness: &FencedWrite) -> DeviceResult<()> {
        self.active.backend.reset(witness)
    }

    fn close_once(
        &mut self,
        authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let result = self
            .active
            .backend
            .close_once(authority)
            .map_err(DeviceError::with_backend_resources);
        self.close_result = Some(result.clone());
        result
    }
}

pub fn create_touch_backend(config: TouchBackendConfig) -> DeviceResult<SelectedTouchBackend> {
    let requested = config.requested;
    let factories = default_touch_factories(config);
    match requested {
        TouchBackendChoice::NemuIpc => Err(DeviceError::fatal(
            "Nemu input requires its paired ExecutionSession",
        )),
        TouchBackendChoice::Auto => select_fixed_priority(requested, factories),
        TouchBackendChoice::AutoFastest => select_fastest(requested, factories),
        TouchBackendChoice::MaaTouch => select_fixed_priority(
            requested,
            factories
                .into_iter()
                .filter(|factory| factory.name() == TouchBackendName::MaaTouch)
                .collect(),
        ),
        TouchBackendChoice::Minitouch => select_fixed_priority(
            requested,
            factories
                .into_iter()
                .filter(|factory| factory.name() == TouchBackendName::Minitouch)
                .collect(),
        ),
        TouchBackendChoice::AdbShellInput => select_fixed_priority(
            requested,
            factories
                .into_iter()
                .filter(|factory| factory.name() == TouchBackendName::AdbShellInput)
                .collect(),
        ),
    }
}

/// Runtime calls this only inside its admitted, fenced input operation.
/// Capture and backend probes use their existing factories.
pub fn create_touch_backend_for_fenced_input(
    config: TouchBackendConfig,
) -> DeviceResult<SelectedTouchBackend> {
    if config.requested != TouchBackendChoice::AdbShellInput {
        return create_touch_backend(config);
    }
    select_fixed_priority(
        config.requested,
        vec![Box::new(AdbShellInputFactory {
            adb_config: config.adb_config,
            target: config.target,
            recovery_allowed: true,
        })],
    )
}

pub fn touch_probe_report(config: TouchBackendConfig) -> TouchBackendDiagnostics {
    let requested = config.requested;
    let factories = default_touch_factories(config);
    touch_probe_report_with_factories(requested, factories)
}

fn touch_probe_report_with_factories(
    requested: TouchBackendChoice,
    factories: Vec<Box<dyn TouchBackendFactory>>,
) -> TouchBackendDiagnostics {
    let mut diagnostics = TouchBackendDiagnostics::new(requested);
    let mut successful = Vec::new();

    for factory in &factories {
        let started = Instant::now();
        match factory.connect() {
            Ok(mut connected) => {
                let elapsed_ms = started.elapsed().as_millis();
                let name = connected.name;
                if let Err(err) = connected
                    .backend
                    .close_once(DeviceCloseAuthority::LocalOnly)
                {
                    let unconfirmed =
                        err.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed);
                    diagnostics.push_failure(
                        name,
                        elapsed_ms,
                        err.to_string(),
                        "probe_close",
                        None,
                    );
                    diagnostics.record_input_parameters(connected.input_parameters.as_ref());
                    if unconfirmed {
                        std::mem::forget(connected);
                    }
                    continue;
                }
                diagnostics.push_success(name, elapsed_ms, "probe", false);
                diagnostics.record_input_parameters(connected.input_parameters.as_ref());
                successful.push((name, elapsed_ms));
            }
            Err(err) => {
                diagnostics.push_failure(
                    factory.name(),
                    started.elapsed().as_millis(),
                    err.to_string(),
                    "probe",
                    None,
                );
                diagnostics.record_input_parameters(err.input_parameters());
            }
        }
    }

    diagnostics.selected = selected_backend_from_probe(requested, &successful);
    if let Some(selected) = diagnostics.selected {
        for attempt in &mut diagnostics.attempts {
            attempt.selected = attempt.backend == selected && attempt.ok;
        }
    }
    diagnostics
}

fn selected_backend_from_probe(
    requested: TouchBackendChoice,
    successful: &[(TouchBackendName, u128)],
) -> Option<TouchBackendName> {
    match requested {
        TouchBackendChoice::NemuIpc => successful
            .iter()
            .find(|(backend, _)| *backend == TouchBackendName::NemuIpc)
            .map(|(backend, _)| *backend),
        TouchBackendChoice::Auto => [
            TouchBackendName::MaaTouch,
            TouchBackendName::Minitouch,
            TouchBackendName::AdbShellInput,
        ]
        .into_iter()
        .find(|name| successful.iter().any(|(backend, _)| backend == name)),
        TouchBackendChoice::AutoFastest => successful
            .iter()
            .min_by_key(|(_backend, elapsed_ms)| *elapsed_ms)
            .map(|(backend, _)| *backend),
        TouchBackendChoice::MaaTouch => successful
            .iter()
            .find(|(backend, _)| *backend == TouchBackendName::MaaTouch)
            .map(|(backend, _)| *backend),
        TouchBackendChoice::Minitouch => successful
            .iter()
            .find(|(backend, _)| *backend == TouchBackendName::Minitouch)
            .map(|(backend, _)| *backend),
        TouchBackendChoice::AdbShellInput => successful
            .iter()
            .find(|(backend, _)| *backend == TouchBackendName::AdbShellInput)
            .map(|(backend, _)| *backend),
    }
}

fn select_fixed_priority(
    requested: TouchBackendChoice,
    mut factories: Vec<Box<dyn TouchBackendFactory>>,
) -> DeviceResult<SelectedTouchBackend> {
    let mut diagnostics = TouchBackendDiagnostics::new(requested);
    while !factories.is_empty() {
        let factory = factories.remove(0);
        let started = Instant::now();
        match factory.connect() {
            Ok(active) => {
                diagnostics.push_success(
                    active.name,
                    started.elapsed().as_millis(),
                    "select",
                    true,
                );
                diagnostics.selected = Some(active.name);
                diagnostics.record_input_parameters(active.input_parameters.as_ref());
                return Ok(SelectedTouchBackend {
                    action_open_observations: Vec::new(),
                    active,
                    remaining: factories,
                    diagnostics,
                    close_result: None,
                });
            }
            Err(err) => {
                let backend = factory.name();
                let reason = err.to_string();
                let fallback_backend = factories.first().map(|factory| factory.name());
                diagnostics.push_failure(
                    backend,
                    started.elapsed().as_millis(),
                    reason.clone(),
                    "select",
                    fallback_backend,
                );
                diagnostics.record_input_parameters(err.input_parameters());
                diagnostics.warnings.push(format!(
                    "WARNING touch backend {} unavailable during selection; fallback_backend={}; reason={reason}",
                    backend.as_str(),
                    fallback_backend.map(TouchBackendName::as_str).unwrap_or("none")
                ));
                if !err.is_fallback_eligible() {
                    return Err(crate::observe_open_failure(diagnostics.open_report(), err));
                }
            }
        }
    }
    Err(crate::observe_open_failure(
        diagnostics.open_report(),
        DeviceError::fatal(format!(
            "touch backend selection failed; diagnostics: {}",
            format_touch_diagnostics(&diagnostics)
        )),
    ))
}

fn select_fastest(
    requested: TouchBackendChoice,
    factories: Vec<Box<dyn TouchBackendFactory>>,
) -> DeviceResult<SelectedTouchBackend> {
    let mut diagnostics = TouchBackendDiagnostics::new(requested);
    let mut connected = Vec::new();

    for (index, factory) in factories.iter().enumerate() {
        let started = Instant::now();
        match factory.connect() {
            Ok(backend) => {
                let elapsed_ms = started.elapsed().as_millis();
                diagnostics.push_success(backend.name, elapsed_ms, "select", false);
                diagnostics.record_input_parameters(backend.input_parameters.as_ref());
                connected.push((index, elapsed_ms, backend));
            }
            Err(err) => {
                let backend = factory.name();
                let reason = err.to_string();
                diagnostics.push_failure(
                    backend,
                    started.elapsed().as_millis(),
                    reason.clone(),
                    "select",
                    None,
                );
                diagnostics.record_input_parameters(err.input_parameters());
                diagnostics.warnings.push(format!(
                    "WARNING touch backend {} unavailable during fastest selection: {reason}",
                    backend.as_str()
                ));
                if !err.is_fallback_eligible() {
                    return Err(crate::observe_open_failure(
                        diagnostics.open_report(),
                        close_connected_touch_backends(connected, err),
                    ));
                }
            }
        }
    }

    let Some(selected_pos) = connected
        .iter()
        .enumerate()
        .min_by_key(|(_pos, (_index, elapsed_ms, _backend))| *elapsed_ms)
        .map(|(pos, _)| pos)
    else {
        return Err(crate::observe_open_failure(
            diagnostics.open_report(),
            DeviceError::fatal(format!(
                "touch backend fastest selection failed; diagnostics: {}",
                format_touch_diagnostics(&diagnostics)
            )),
        ));
    };

    let (selected_factory_index, _elapsed_ms, mut active) = connected.remove(selected_pos);
    let mut cleanup_error: Option<DeviceError> = None;
    for (_index, _elapsed_ms, mut backend) in connected {
        if let Err(error) = backend.backend.close_once(DeviceCloseAuthority::LocalOnly) {
            let unconfirmed =
                error.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed);
            cleanup_error = Some(match cleanup_error {
                Some(primary) => primary.merge_resource_cleanup(error),
                None => error,
            });
            if unconfirmed {
                std::mem::forget(backend);
            }
        }
    }
    if let Some(primary) = cleanup_error {
        return Err(crate::observe_open_failure(
            diagnostics.open_report(),
            match active.backend.close_once(DeviceCloseAuthority::LocalOnly) {
                Ok(_) => primary,
                Err(cleanup) => {
                    let unconfirmed = cleanup.resource_quiescence()
                        == Some(DeviceResourceQuiescence::Unconfirmed);
                    let error = primary.merge_resource_cleanup(cleanup);
                    if unconfirmed {
                        std::mem::forget(active);
                    }
                    error
                }
            },
        ));
    }

    diagnostics.selected = Some(active.name);
    for attempt in &mut diagnostics.attempts {
        attempt.selected = attempt.backend == active.name && attempt.ok;
    }

    let remaining = factories
        .into_iter()
        .enumerate()
        .filter_map(|(index, factory)| (index != selected_factory_index).then_some(factory))
        .collect::<Vec<_>>();

    Ok(SelectedTouchBackend {
        action_open_observations: Vec::new(),
        active,
        remaining,
        diagnostics,
        close_result: None,
    })
}

fn close_connected_touch_backends(
    connected: Vec<(usize, u128, ConnectedTouchBackend)>,
    primary: DeviceError,
) -> DeviceError {
    connected.into_iter().fold(
        primary,
        |primary, (_index, _elapsed_ms, mut backend)| match backend
            .backend
            .close_once(DeviceCloseAuthority::LocalOnly)
        {
            Ok(_) => primary,
            Err(cleanup) => {
                let unconfirmed =
                    cleanup.resource_quiescence() == Some(DeviceResourceQuiescence::Unconfirmed);
                let error = primary.merge_resource_cleanup(cleanup);
                if unconfirmed {
                    std::mem::forget(backend);
                }
                error
            }
        },
    )
}

fn default_touch_factories(config: TouchBackendConfig) -> Vec<Box<dyn TouchBackendFactory>> {
    vec![
        Box::new(MaaTouchFactory {
            adb_config: config.adb_config.clone(),
            target: config.target.clone(),
            maatouch_config: config.maatouch_config,
        }),
        Box::new(MinitouchFactory {
            adb_config: config.adb_config.clone(),
            target: config.target.clone(),
            minitouch_config: config.minitouch_config,
        }),
        Box::new(AdbShellInputFactory {
            adb_config: config.adb_config,
            target: config.target,
            recovery_allowed: false,
        }),
    ]
}

fn format_touch_diagnostics(diagnostics: &TouchBackendDiagnostics) -> String {
    let attempts = diagnostics
        .attempts
        .iter()
        .map(|attempt| {
            format!(
                "{{attempt_id:{}, backend:{}, ok:{}, elapsed_ms:{}, action:{}, fallback_backend:{}, selected:{}, error_reason:{}}}",
                attempt.attempt_id,
                attempt.backend.as_str(),
                attempt.ok,
                attempt.elapsed_ms,
                attempt.action.as_deref().unwrap_or("none"),
                attempt
                    .fallback_backend
                    .map(TouchBackendName::as_str)
                    .unwrap_or("none"),
                attempt.selected,
                attempt.error_reason.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let warnings = diagnostics.warnings.join(" | ");
    format!(
        "requested={}, selected={}, attempts=[{}], warnings=[{}]",
        diagnostics.requested.as_str(),
        diagnostics
            .selected
            .map(TouchBackendName::as_str)
            .unwrap_or("none"),
        attempts,
        warnings
    )
}

struct MaaTouchFactory {
    adb_config: AdbConfig,
    target: DeviceTarget,
    maatouch_config: MaaTouchConfig,
}

impl TouchBackendFactory for MaaTouchFactory {
    fn name(&self) -> TouchBackendName {
        TouchBackendName::MaaTouch
    }

    fn connect(&self) -> DeviceResult<ConnectedTouchBackend> {
        let mut backend = MaaTouchBackend::new(
            self.adb_config.clone(),
            self.target.clone(),
            self.maatouch_config.clone(),
        );
        let device = backend.connect()?;
        let handshake = backend.handshake_info().cloned();
        Ok(ConnectedTouchBackend {
            input_parameters: backend.input_parameters().cloned(),
            name: TouchBackendName::MaaTouch,
            backend: Box::new(backend),
            device,
            handshake,
        })
    }
}

struct MinitouchFactory {
    adb_config: AdbConfig,
    target: DeviceTarget,
    minitouch_config: MinitouchConfig,
}

impl TouchBackendFactory for MinitouchFactory {
    fn name(&self) -> TouchBackendName {
        TouchBackendName::Minitouch
    }

    fn connect(&self) -> DeviceResult<ConnectedTouchBackend> {
        let mut backend = MinitouchBackend::new(
            self.adb_config.clone(),
            self.target.clone(),
            self.minitouch_config.clone(),
        );
        let device = backend.connect()?;
        let handshake = backend.handshake_info().cloned();
        Ok(ConnectedTouchBackend {
            input_parameters: backend.input_parameters().cloned(),
            name: TouchBackendName::Minitouch,
            backend: Box::new(backend),
            device,
            handshake,
        })
    }
}

struct AdbShellInputFactory {
    adb_config: AdbConfig,
    target: DeviceTarget,
    recovery_allowed: bool,
}

impl TouchBackendFactory for AdbShellInputFactory {
    fn name(&self) -> TouchBackendName {
        TouchBackendName::AdbShellInput
    }

    fn connect(&self) -> DeviceResult<ConnectedTouchBackend> {
        let mut backend = AdbShellInputBackend::new(self.adb_config.clone(), self.target.clone());
        let device = if self.recovery_allowed {
            backend.connect_for_fenced_input()?
        } else {
            backend.connect()?
        };
        Ok(ConnectedTouchBackend {
            name: TouchBackendName::AdbShellInput,
            input_parameters: backend.input_parameters.clone(),
            backend: Box::new(backend),
            device,
            handshake: None,
        })
    }
}

#[derive(Clone)]
pub struct AdbShellInputBackend {
    input_parameters: Option<actingcommand_contract::BackendInputParameterCheck>,
    recovery: Option<crate::AdbTargetRecovery>,
    adb_config: AdbConfig,
    target: DeviceTarget,
    serial: String,
    bounds: Option<TouchBounds>,
    connect_geometry: Option<AdbInputConnectGeometry>,
    /// Workflow #406 T3: the Android SDK level read at connect (`input_sdk_level`); none when
    /// the device's answer was not a level.
    input_sdk_level: Option<u32>,
    connected: bool,
    close_result: Option<DeviceResult<DeviceResourceCloseOutcome>>,
}

/// Workflow #398 RD-1a (model-398 v1.1 §3.5 rule 1): `serial` holds the raw adb argument, so
/// `Debug` shows the target's label in its place, as [`crate::DeviceSerial`] does.
impl std::fmt::Debug for AdbShellInputBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdbShellInputBackend")
            .field("input_parameters", &self.input_parameters)
            .field("recovery", &self.recovery)
            .field("adb_config", &self.adb_config)
            .field("target", &self.target)
            .field("serial", &self.target.label())
            .field("bounds", &self.bounds)
            .field("connect_geometry", &self.connect_geometry)
            .field("input_sdk_level", &self.input_sdk_level)
            .field("connected", &self.connected)
            .field("close_result", &self.close_result)
            .finish()
    }
}

impl AdbShellInputBackend {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        let serial = target.adb_arg();
        Self {
            input_parameters: None,
            recovery: None,
            adb_config,
            target,
            serial,
            bounds: None,
            connect_geometry: None,
            input_sdk_level: None,
            connected: false,
            close_result: None,
        }
    }

    pub fn connect(&mut self) -> DeviceResult<DeviceInfo> {
        let adb = Adb::new(self.adb_config.clone());
        let serial = self.serial.clone();
        let connect = self.target.connect;
        let info = self.connect_with_steps(
            || adb.ensure_device(&serial, connect),
            || adb.screen_size(&serial),
            || read_device_rotation(&adb, &serial),
        )?;
        self.read_input_sdk_level(|| adb.sdk_level(&serial))?;
        Ok(info)
    }

    fn connect_for_fenced_input(&mut self) -> DeviceResult<DeviceInfo> {
        let adb = Adb::new(self.adb_config.clone());
        let serial = self.serial.clone();
        let ready = adb.ensure_input_device(&serial, self.target.connect);
        let recovery = ready.as_ref().ok().and_then(|ready| ready.recovery.clone());
        let result = self
            .connect_with_steps(
                || ready.map(|ready| ready.state),
                || adb.screen_size(&serial),
                || read_device_rotation(&adb, &serial),
            )
            .and_then(|info| {
                self.read_input_sdk_level(|| adb.sdk_level(&serial))
                    .map(|()| info)
            });
        match result {
            Ok(info) => {
                self.recovery = recovery;
                Ok(info)
            }
            Err(error) => Err(match recovery {
                Some(report) => error.with_adb_recovery(report),
                None => error,
            }),
        }
    }

    fn connect_with_steps(
        &mut self,
        ensure_device: impl FnOnce() -> DeviceResult<String>,
        screen_size: impl FnOnce() -> DeviceResult<String>,
        device_rotation: impl FnOnce() -> DeviceResult<DeviceRotation>,
    ) -> DeviceResult<DeviceInfo> {
        self.input_parameters = None;
        let state = ensure_device().map_err(|error| {
            adb_shell_input_connect_error(
                error,
                DeviceErrorCategory::Native,
                crate::codes::DeviceStage::AdbEnsureDeviceGetState.location(),
                DeviceLocation::EnsureDevice,
                DeviceLocation::EnsureDevice.location(),
                DeviceErrorDiagnosticMessage::AdbShellInputDeviceStateUnavailable,
            )
        })?;
        let screen_size = screen_size().map_err(|error| {
            adb_shell_input_connect_error(
                error,
                DeviceErrorCategory::Protocol,
                crate::codes::DeviceStage::AdbInputBoundsValidate.location(),
                DeviceLocation::ScreenSize,
                DeviceLocation::BoundsValidate.location(),
                DeviceErrorDiagnosticMessage::AdbShellInputBoundsUnavailableOrInvalid,
            )
        })?;
        let natural_bounds = touch_bounds_from_screen_size(&screen_size).map_err(|error| {
            adb_shell_input_connect_error(
                error.input_parameter_failure(),
                DeviceErrorCategory::Protocol,
                crate::codes::DeviceStage::AdbInputBoundsValidate.location(),
                DeviceLocation::BoundsConversion,
                DeviceLocation::BoundsValidate.location(),
                DeviceErrorDiagnosticMessage::AdbShellInputBoundsUnavailableOrInvalid,
            )
        })?;
        let rotation = device_rotation().map_err(|error| {
            adb_shell_input_connect_error(
                error,
                DeviceErrorCategory::Protocol,
                crate::codes::DeviceStage::AdbInputRotationResolve.location(),
                DeviceLocation::DeviceRotation,
                DeviceLocation::RotationResolve.location(),
                DeviceErrorDiagnosticMessage::AdbShellInputRotationUnavailable,
            )
        })?;
        let (max_x, max_y) = display_size_from_natural(
            natural_bounds.max_x as u32,
            natural_bounds.max_y as u32,
            rotation,
        );
        let bounds = TouchBounds {
            max_x: max_x as i32,
            max_y: max_y as i32,
        };
        self.bounds = Some(bounds);
        self.connect_geometry = Some(AdbInputConnectGeometry::new(
            natural_bounds.max_x,
            natural_bounds.max_y,
            match rotation {
                DeviceRotation::R0 => 0,
                DeviceRotation::R90 => 90,
                DeviceRotation::R180 => 180,
                DeviceRotation::R270 => 270,
            },
        ));
        self.connected = true;
        self.input_parameters = Some(actingcommand_contract::BackendInputParameterCheck {
            status: actingcommand_contract::BackendObservationStatus::Passed,
            input_geometry: self
                .connect_geometry
                .map(AdbInputConnectGeometry::open_observation),
            ..Default::default()
        });
        self.close_result = None;
        Ok(DeviceInfo {
            serial: self.target.label(),
            state,
            screen_size,
        })
    }

    /// Workflow #406 T3 (plan Q18; location `input_sdk_level`): the connect step after the
    /// rotation, reading the device's Android SDK level. An adb failure fails the connect as the
    /// earlier steps do and leaves the backend closed; an answer that is not a decimal level
    /// leaves the level unread. Segmented plans are performed from level 23 on.
    fn read_input_sdk_level(
        &mut self,
        sdk_level: impl FnOnce() -> DeviceResult<String>,
    ) -> DeviceResult<()> {
        self.input_sdk_level = None;
        match sdk_level() {
            Ok(answer) => {
                self.input_sdk_level = answer.trim().parse::<u32>().ok();
                Ok(())
            }
            Err(error) => {
                self.connected = false;
                self.bounds = None;
                self.connect_geometry = None;
                self.input_parameters = None;
                let stage = crate::codes::DeviceStage::InputSdkLevel;
                let severity = error.severity();
                let source_error = error.to_string();
                Err(error
                    .with_severity_and_message(
                        severity,
                        format!(
                            "adb shell input connect failed; child_operation={stage}; source_error={source_error}"
                        ),
                    )
                    .with_diagnostic_if_absent(DeviceErrorCategory::Protocol, stage.location())
                    .with_diagnostic_context_if_absent(
                        "adb_shell_input",
                        stage.location(),
                        DeviceErrorSensitivity::Internal,
                    ))
            }
        }
    }

    /// Workflow #406 T3 (plan Q18): the segmented plan `plan` as one `adb shell` invocation of
    /// `input motionevent` commands, handed to `child` with the invocation's timeout. Refused
    /// with the segmented swipe capability error, before any command, when the device's SDK
    /// level is below 23 or was not read; every touch point passes the backend's bounds check
    /// first, as a swipe's points do.
    fn segmented_swipe_with_child(
        &mut self,
        plan: &PreparedSegmentedSwipePlan,
        child: impl FnOnce(Duration, &str) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        plan.action().validate()?;
        if !self.supports_segmented_swipe() {
            return Err(segmented_swipe_capability_error());
        }
        let bounds = self.bounds()?;
        let mut previous: Option<(i32, i32)> = None;
        for event in plan.events() {
            let (SegmentedSwipeEvent::Down(point) | SegmentedSwipeEvent::Move { point, .. }) =
                event
            else {
                continue;
            };
            let (x1, y1) = previous.unwrap_or(*point);
            let action = AdbBoundsAction::Swipe {
                x1,
                y1,
                x2: point.0,
                y2: point.1,
            };
            let (label_x, label_y, coordinate_x, coordinate_y) = if previous.is_none() {
                (
                    "motionevent down x",
                    "motionevent down y",
                    AdbBoundsCoordinate::StartX,
                    AdbBoundsCoordinate::StartY,
                )
            } else {
                (
                    "motionevent move x",
                    "motionevent move y",
                    AdbBoundsCoordinate::EndX,
                    AdbBoundsCoordinate::EndY,
                )
            };
            validate_touch_coordinate(label_x, point.0, bounds.max_x)
                .map_err(|error| self.bounds_failure(error, action, coordinate_x, bounds))?;
            validate_touch_coordinate(label_y, point.1, bounds.max_y)
                .map_err(|error| self.bounds_failure(error, action, coordinate_y, bounds))?;
            previous = Some(*point);
        }
        self.ensure_connected()?;
        let script = motionevent_script(plan.events());
        let timeout_ms = script
            .wait_ms
            .saturating_add(
                script
                    .commands
                    .saturating_mul(ADB_MOTIONEVENT_COMMAND_ALLOWANCE_MS),
            )
            .saturating_add(2_000);
        let timeout = Duration::from_millis(timeout_ms).max(self.adb_config.command_timeout);
        child(timeout, &script.text)
    }

    fn ensure_connected(&self) -> DeviceResult<()> {
        if self.connected {
            Ok(())
        } else {
            Err(DeviceError::fatal("AdbShellInputBackend is not connected"))
        }
    }

    fn adb_for_duration(&self, duration_ms: u64) -> Adb {
        let mut config = self.adb_config.clone();
        let min_timeout = Duration::from_millis(duration_ms.saturating_add(2_000));
        if config.command_timeout < min_timeout {
            config.command_timeout = min_timeout;
        }
        Adb::new(config)
    }

    fn bounds(&self) -> DeviceResult<TouchBounds> {
        self.bounds
            .ok_or_else(|| DeviceError::fatal("AdbShellInputBackend screen bounds are unavailable"))
    }

    fn tap_with_child(
        &mut self,
        x: i32,
        y: i32,
        child: impl FnOnce(i32, i32) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        let bounds = self.bounds()?;
        let action = AdbBoundsAction::Tap { x, y };
        validate_touch_coordinate("tap x", x, bounds.max_x).map_err(|error| {
            self.bounds_failure(error, action, AdbBoundsCoordinate::PointX, bounds)
        })?;
        validate_touch_coordinate("tap y", y, bounds.max_y).map_err(|error| {
            self.bounds_failure(error, action, AdbBoundsCoordinate::PointY, bounds)
        })?;
        self.ensure_connected()?;
        child(x, y)
    }

    fn bounds_failure(
        &self,
        error: DeviceError,
        action: AdbBoundsAction,
        rejected: AdbBoundsCoordinate,
        bounds: TouchBounds,
    ) -> DeviceError {
        error
            .with_adb_input_bounds_context_if_absent(AdbInputBoundsContext::new(
                action,
                rejected,
                (bounds.max_x, bounds.max_y),
                self.connect_geometry,
            ))
            .with_diagnostic_if_absent(
                DeviceErrorCategory::Protocol,
                crate::codes::DeviceStage::AdbInputBoundsValidate.location(),
            )
            .with_diagnostic_context_if_absent(
                "adb_shell_input",
                action.operation(),
                DeviceErrorSensitivity::Sensitive,
            )
    }
}

/// Workflow #406 T3: a segmented plan as one shell script of `input motionevent` commands.
struct MotioneventScript {
    /// `input motionevent DOWN|MOVE|UP x y` and `sleep s.mmm`, joined by `;`.
    text: String,
    /// The plan's waits, in milliseconds.
    wait_ms: u64,
    /// The `input motionevent` commands.
    commands: u64,
}

/// Workflow #406 T3 (plan Q18): `events` as motionevent commands: the press `DOWN`, each move
/// `MOVE` after a `sleep` of its wait, each hold a `sleep`, and the release `UP` at the last
/// point the touch reached. A wait of 0 sleeps nowhere.
fn motionevent_script(events: &[SegmentedSwipeEvent]) -> MotioneventScript {
    fn sleep(lines: &mut Vec<String>, wait_ms: &mut u64, milliseconds: u64) {
        if milliseconds > 0 {
            *wait_ms = wait_ms.saturating_add(milliseconds);
            lines.push(format!(
                "sleep {}.{:03}",
                milliseconds / 1_000,
                milliseconds % 1_000
            ));
        }
    }
    let mut lines = Vec::with_capacity(events.len() * 2);
    let mut wait_ms = 0_u64;
    let mut commands = 0_u64;
    let mut current = (0, 0);
    for event in events {
        match event {
            SegmentedSwipeEvent::Down(point) => {
                current = *point;
                commands += 1;
                lines.push(format!("input motionevent DOWN {} {}", point.0, point.1));
            }
            SegmentedSwipeEvent::Move {
                point,
                delay_before_ms,
            } => {
                sleep(&mut lines, &mut wait_ms, *delay_before_ms);
                current = *point;
                commands += 1;
                lines.push(format!("input motionevent MOVE {} {}", point.0, point.1));
            }
            SegmentedSwipeEvent::Hold(duration_ms) => {
                sleep(&mut lines, &mut wait_ms, *duration_ms);
            }
            SegmentedSwipeEvent::Up => {
                commands += 1;
                lines.push(format!("input motionevent UP {} {}", current.0, current.1));
            }
        }
    }
    MotioneventScript {
        text: lines.join(";"),
        wait_ms,
        commands,
    }
}

fn adb_shell_input_connect_error(
    error: DeviceError,
    category: DeviceErrorCategory,
    stage: Location,
    child_operation: DeviceLocation,
    operation: Location,
    diagnostic_message: DeviceErrorDiagnosticMessage,
) -> DeviceError {
    let severity = error.severity();
    let source_error = error.to_string();
    let producer_message = error.diagnostic_message().is_some();
    let error = error
        .with_severity_and_message(
            severity,
            format!(
                "adb shell input connect failed; child_operation={child_operation}; source_error={source_error}"
            ),
        )
        .with_diagnostic_if_absent(category, stage)
        .with_diagnostic_context_if_absent(
            "adb_shell_input",
            operation,
            DeviceErrorSensitivity::Internal,
        );
    if producer_message {
        error
    } else {
        error.with_diagnostic_message(diagnostic_message)
    }
}

impl InputBackend for AdbShellInputBackend {
    fn opened_geometry(&self) -> Option<actingcommand_contract::BackendInputGeometryObservation> {
        self.connect_geometry
            .map(AdbInputConnectGeometry::open_observation)
    }

    fn take_adb_recovery(&mut self) -> Option<crate::AdbTargetRecovery> {
        self.recovery.take()
    }

    fn tap(&mut self, witness: &FencedWrite, x: i32, y: i32) -> DeviceResult<()> {
        let adb_config = self.adb_config.clone();
        let serial = self.serial.clone();
        self.tap_with_child(x, y, move |x, y| {
            Adb::new(adb_config)
                .shell_input_tap(witness, &serial, x, y)
                .map(|_| ())
        })
    }

    fn long_tap(
        &mut self,
        witness: &FencedWrite,
        x: i32,
        y: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        self.swipe(witness, x, y, x, y, duration_ms)
    }

    fn swipe(
        &mut self,
        witness: &FencedWrite,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: u64,
    ) -> DeviceResult<()> {
        let bounds = self.bounds()?;
        let action = AdbBoundsAction::Swipe { x1, y1, x2, y2 };
        validate_touch_coordinate("swipe x1", x1, bounds.max_x).map_err(|error| {
            self.bounds_failure(error, action, AdbBoundsCoordinate::StartX, bounds)
        })?;
        validate_touch_coordinate("swipe y1", y1, bounds.max_y).map_err(|error| {
            self.bounds_failure(error, action, AdbBoundsCoordinate::StartY, bounds)
        })?;
        validate_touch_coordinate("swipe x2", x2, bounds.max_x).map_err(|error| {
            self.bounds_failure(error, action, AdbBoundsCoordinate::EndX, bounds)
        })?;
        validate_touch_coordinate("swipe y2", y2, bounds.max_y).map_err(|error| {
            self.bounds_failure(error, action, AdbBoundsCoordinate::EndY, bounds)
        })?;
        self.ensure_connected()?;
        let duration_ms = duration_ms.clamp(1, MAX_ADB_INPUT_GESTURE_MS);
        let adb = self.adb_for_duration(duration_ms);
        adb.shell_input_swipe(witness, &self.serial, x1, y1, x2, y2, duration_ms)?;
        Ok(())
    }

    /// Workflow #406 T3 (plan Q18): from Android SDK level 23 (`input motionevent`), read at
    /// connect.
    fn supports_segmented_swipe(&self) -> bool {
        self.input_sdk_level
            .is_some_and(|level| level >= ADB_MOTIONEVENT_MIN_SDK_LEVEL)
    }

    /// Workflow #406 T3 (plan Q18): the path drag and both brake drags as one `adb shell`
    /// invocation of `input motionevent` commands, the plan's waits as `sleep`s between them.
    fn segmented_swipe_prepared(
        &mut self,
        witness: &FencedWrite,
        plan: &PreparedSegmentedSwipePlan,
    ) -> DeviceResult<()> {
        let mut config = self.adb_config.clone();
        let serial = self.serial.clone();
        self.segmented_swipe_with_child(plan, move |timeout, script| {
            config.command_timeout = timeout;
            Adb::new(config)
                .shell_input_motionevents(witness, &serial, script)
                .map(|_| ())
        })
    }

    fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "AdbShellInputBackend key input is outside A1 touch fallback scope",
        ))
    }

    fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "AdbShellInputBackend text input is outside A1 touch fallback scope",
        ))
    }

    fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
        self.ensure_connected()
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        if let Some(result) = &self.close_result {
            return result.clone();
        }
        let resource_count = u16::from(self.connected);
        self.connected = false;
        let result = Ok(DeviceResourceCloseOutcome::confirmed(resource_count));
        self.close_result = Some(result.clone());
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TouchBounds {
    pub(crate) max_x: i32,
    pub(crate) max_y: i32,
}

fn touch_bounds_from_device(
    handshake: Option<&HandshakeInfo>,
    device: &DeviceInfo,
) -> DeviceResult<TouchBounds> {
    if let Some(handshake) = handshake {
        return Ok(TouchBounds {
            max_x: handshake.max_x,
            max_y: handshake.max_y,
        });
    }
    touch_bounds_from_screen_size(&device.screen_size)
}

fn touch_bounds_for_backend(
    backend: TouchBackendName,
    handshake: Option<&HandshakeInfo>,
    device: &DeviceInfo,
) -> DeviceResult<TouchBounds> {
    match backend {
        TouchBackendName::NemuIpc => Err(DeviceError::fatal(
            "Nemu input bounds require its committed frame geometry",
        )),
        TouchBackendName::Minitouch => touch_bounds_from_screen_size(&device.screen_size),
        TouchBackendName::MaaTouch | TouchBackendName::AdbShellInput => {
            touch_bounds_from_device(handshake, device)
        }
    }
}

pub(crate) fn touch_bounds_from_screen_size(screen_size: &str) -> DeviceResult<TouchBounds> {
    let size = crate::adb::parse_wm_size(screen_size)?;
    let max_x = i32::try_from(size.width).map_err(|_| {
        DeviceError::fatal(format!(
            "touch screen width {} does not fit i32 in adb wm size output: {screen_size}",
            size.width
        ))
    })?;
    let max_y = i32::try_from(size.height).map_err(|_| {
        DeviceError::fatal(format!(
            "touch screen height {} does not fit i32 in adb wm size output: {screen_size}",
            size.height
        ))
    })?;
    Ok(TouchBounds { max_x, max_y })
}

fn validate_touch_coordinate(label: &str, value: i32, max: i32) -> DeviceResult<()> {
    if value < 0 {
        return Err(DeviceError::fatal(format!(
            "{label} must be non-negative for touch input, got {value}"
        )));
    }
    if value > max {
        return Err(DeviceError::fatal(format!(
            "{label} {value} exceeds touch screen max {max}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceErrorCategory, DeviceErrorSensitivity, DeviceErrorSeverity};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// A scheduler-shaped step witness for driving a write method directly.
    fn step_witness() -> FencedWrite {
        let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
        actingcommand_contract::issue_fenced_write(
            actingcommand_contract::LeaseToken::new(
                *issuer.mint_owner_epoch().expect("epoch").transport(),
                *issuer.mint_lease_id().expect("lease").transport(),
                *issuer.mint_instance_id().expect("instance").transport(),
                *issuer.mint_holder_id().expect("holder").transport(),
                100,
            )
            .expect("test step token"),
            1,
            std::num::NonZeroU64::new(1).expect("step"),
            actingcommand_contract::FencedWritePurpose::Business,
        )
    }

    fn adb_shell_input_test_backend() -> AdbShellInputBackend {
        AdbShellInputBackend::new(AdbConfig::default(), DeviceTarget::default())
    }

    #[test]
    fn adb_shell_input_connect_identifies_child_operation_and_preserves_device_error() {
        let mut ensure_backend = adb_shell_input_test_backend();
        let ensure_source = DeviceError::transient(
            "adb -s neutral:16384 get-state failed with exit code 17\nstdout:\nstate-out\nstderr:\nstate-err",
        );
        let ensure_error = ensure_backend
            .connect_with_steps(
                || Err(ensure_source),
                || -> DeviceResult<String> { panic!("screen size must not run") },
                || -> DeviceResult<DeviceRotation> { panic!("rotation must not run") },
            )
            .expect_err("ensure device failure");
        assert_eq!(ensure_error.severity(), DeviceErrorSeverity::Transient);
        assert!(
            ensure_error
                .message()
                .contains("child_operation=ensure_device")
        );
        assert!(ensure_error.message().contains("state-out"));
        assert_eq!(
            ensure_error.diagnostic_message(),
            Some("adb shell input device state is unavailable")
        );
        let ensure_diagnostic = ensure_error.diagnostic().expect("state diagnostic");
        assert_eq!(ensure_diagnostic.category(), DeviceErrorCategory::Native);
        assert_eq!(
            ensure_diagnostic.stage().as_str(),
            "adb_ensure_device_get_state"
        );
        let ensure_context = ensure_error
            .diagnostic_context()
            .expect("state diagnostic context");
        assert_eq!(ensure_context.backend(), "adb_shell_input");
        assert_eq!(ensure_context.operation().as_str(), "ensure_device");
        assert_eq!(
            ensure_context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );

        let mut screen_backend = adb_shell_input_test_backend();
        let screen_source = DeviceError::transient(
            "adb -s neutral:16384 shell wm size failed with exit code 18\nstdout:\nsize-out\nstderr:\nsize-err",
        );
        let screen_error = screen_backend
            .connect_with_steps(
                || Ok("device".to_string()),
                || Err(screen_source),
                || -> DeviceResult<DeviceRotation> { panic!("rotation must not run") },
            )
            .expect_err("screen size failure");
        assert_eq!(screen_error.severity(), DeviceErrorSeverity::Transient);
        assert!(
            screen_error
                .message()
                .contains("child_operation=screen_size")
        );
        assert!(screen_error.message().contains("size-out"));
        assert_eq!(
            screen_error.diagnostic_message(),
            Some("adb shell input bounds are unavailable or invalid")
        );
        let screen_diagnostic = screen_error.diagnostic().expect("size diagnostic");
        assert_eq!(screen_diagnostic.category(), DeviceErrorCategory::Protocol);
        assert_eq!(
            screen_diagnostic.stage().as_str(),
            "adb_input_bounds_validate"
        );
        let screen_context = screen_error
            .diagnostic_context()
            .expect("size diagnostic context");
        assert_eq!(screen_context.backend(), "adb_shell_input");
        assert_eq!(screen_context.operation().as_str(), "bounds_validate");
        assert_eq!(
            screen_context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );

        let invalid_screen_size = "unrecognized-size";
        let mut bounds_backend = adb_shell_input_test_backend();
        let bounds_error = bounds_backend
            .connect_with_steps(
                || Ok("device".to_string()),
                || Ok(invalid_screen_size.to_string()),
                || -> DeviceResult<DeviceRotation> { panic!("rotation must not run") },
            )
            .expect_err("bounds conversion failure");
        assert_eq!(bounds_error.severity(), DeviceErrorSeverity::Fatal);
        assert!(
            bounds_error
                .message()
                .contains("child_operation=bounds_conversion")
        );
        assert!(bounds_error.message().contains(invalid_screen_size));
        assert_eq!(
            bounds_error.diagnostic_message(),
            Some("adb shell input bounds are unavailable or invalid")
        );
        let bounds_diagnostic = bounds_error.diagnostic().expect("bounds diagnostic");
        assert_eq!(bounds_diagnostic.category(), DeviceErrorCategory::Protocol);
        assert_eq!(
            bounds_diagnostic.stage().as_str(),
            "adb_input_bounds_validate"
        );
        let bounds_context = bounds_error
            .diagnostic_context()
            .expect("bounds diagnostic context");
        assert_eq!(bounds_context.backend(), "adb_shell_input");
        assert_eq!(bounds_context.operation().as_str(), "bounds_validate");
        assert_eq!(
            bounds_context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );

        let mut rotation_backend = adb_shell_input_test_backend();
        let rotation_source = DeviceError::transient(
            "adb -s neutral:16384 shell dumpsys display failed with exit code 19",
        );
        let rotation_error = rotation_backend
            .connect_with_steps(
                || Ok("device".to_string()),
                || Ok("Physical size: 720x1280".to_string()),
                || Err(rotation_source),
            )
            .expect_err("rotation failure");
        assert_eq!(rotation_error.severity(), DeviceErrorSeverity::Transient);
        assert!(
            rotation_error
                .message()
                .contains("child_operation=device_rotation")
        );
        assert!(rotation_error.message().contains("neutral:16384"));
        assert_eq!(
            rotation_error.diagnostic_message(),
            Some("adb shell input rotation is unavailable")
        );
        let rotation_diagnostic = rotation_error.diagnostic().expect("rotation diagnostic");
        assert_eq!(
            rotation_diagnostic.category(),
            DeviceErrorCategory::Protocol
        );
        assert_eq!(
            rotation_diagnostic.stage().as_str(),
            "adb_input_rotation_resolve"
        );
        let rotation_context = rotation_error
            .diagnostic_context()
            .expect("rotation diagnostic context");
        assert_eq!(rotation_context.backend(), "adb_shell_input");
        assert_eq!(rotation_context.operation().as_str(), "rotation_resolve");
        assert_eq!(
            rotation_context.declared_sensitivity(),
            DeviceErrorSensitivity::Internal
        );
        assert!(!rotation_backend.connected);
        assert_eq!(rotation_backend.bounds, None);

        let child_calls = Cell::new(0);
        rotation_backend
            .tap_with_child(10, 20, |_, _| {
                child_calls.set(child_calls.get() + 1);
                Ok(())
            })
            .expect_err("rotation failure must leave input closed");
        assert_eq!(child_calls.get(), 0);
    }

    #[test]
    fn adb_input_bounds_diagnostic_preserves_actual_geometry() {
        let mut backend = adb_shell_input_test_backend();
        backend.adb_config.adb_path = std::env::temp_dir()
            .join("c1b6-unavailable-adb.exe")
            .to_string_lossy()
            .into_owned();
        let connect_calls = RefCell::new(Vec::new());
        backend
            .connect_with_steps(
                || {
                    connect_calls.borrow_mut().push("state");
                    Ok("device".to_owned())
                },
                || {
                    connect_calls.borrow_mut().push("size");
                    Ok("Physical size: 720x1280".to_owned())
                },
                || {
                    connect_calls.borrow_mut().push("rotation");
                    Ok(DeviceRotation::R90)
                },
            )
            .expect("synthetic connect");
        assert_eq!(*connect_calls.borrow(), ["state", "size", "rotation"]);
        let observed = AdbInputConnectGeometry::new(720, 1280, 90);
        assert_eq!(backend.connect_geometry, Some(observed));
        assert_eq!(
            backend.bounds,
            Some(TouchBounds {
                max_x: 1280,
                max_y: 720
            })
        );
        backend.bounds = Some(TouchBounds {
            max_x: 100,
            max_y: 200,
        });
        let child_calls = Cell::new(0);
        for (x, y, rejected, human) in [
            (
                101,
                -1,
                AdbBoundsCoordinate::PointX,
                "tap x 101 exceeds touch screen max 100",
            ),
            (
                -1,
                201,
                AdbBoundsCoordinate::PointX,
                "tap x must be non-negative for touch input, got -1",
            ),
            (
                50,
                201,
                AdbBoundsCoordinate::PointY,
                "tap y 201 exceeds touch screen max 200",
            ),
        ] {
            let error = backend
                .tap_with_child(x, y, |_, _| {
                    child_calls.set(child_calls.get() + 1);
                    Ok(())
                })
                .expect_err("coordinate rejection");
            assert_eq!(error.message(), human);
            assert_eq!(error.severity(), crate::DeviceErrorSeverity::Fatal);
            assert!(!error.is_fallback_eligible());
            assert_eq!(
                error.adb_input_bounds_context(),
                Some(AdbInputBoundsContext::new(
                    AdbBoundsAction::Tap { x, y },
                    rejected,
                    (100, 200),
                    Some(observed),
                ))
            );
            let detail = error.diagnostic_message().expect("bounded geometry");
            assert!(detail.contains("validation_max_x=100 validation_max_y=200"));
            assert!(detail.contains("connect_observation=connect_time"));
            assert!(detail.contains(
                "connect_natural_max_x=720 connect_natural_max_y=1280 connect_rotation_degrees=90"
            ));
            assert!(detail.len() <= 1_024);
            assert!(!detail.contains(['/', '\\', ':']));
            assert!(!detail.chars().any(char::is_control));
            assert_eq!(
                error.diagnostic().expect("diagnostic").stage().as_str(),
                "adb_input_bounds_validate"
            );
            assert_eq!(
                error
                    .diagnostic_context()
                    .expect("context")
                    .declared_sensitivity(),
                DeviceErrorSensitivity::Sensitive
            );
        }
        assert_eq!(child_calls.get(), 0);
        backend
            .tap_with_child(100, 200, |x, y| {
                assert_eq!((x, y), (100, 200));
                child_calls.set(child_calls.get() + 1);
                Ok(())
            })
            .expect("inclusive bounds retained");
        assert_eq!(child_calls.get(), 1);
        for (points, rejected, human) in [
            (
                (-1, 201, 101, 201),
                AdbBoundsCoordinate::StartX,
                "swipe x1 must be non-negative for touch input, got -1",
            ),
            (
                (0, 201, 101, 201),
                AdbBoundsCoordinate::StartY,
                "swipe y1 201 exceeds touch screen max 200",
            ),
            (
                (0, 0, 101, 201),
                AdbBoundsCoordinate::EndX,
                "swipe x2 101 exceeds touch screen max 100",
            ),
            (
                (0, 0, 100, 201),
                AdbBoundsCoordinate::EndY,
                "swipe y2 201 exceeds touch screen max 200",
            ),
        ] {
            let (x1, y1, x2, y2) = points;
            let error = backend
                .swipe(&step_witness(), x1, y1, x2, y2, 500)
                .expect_err("swipe rejection before child");
            assert_eq!(error.message(), human);
            assert_eq!(
                error.adb_input_bounds_context(),
                Some(AdbInputBoundsContext::new(
                    AdbBoundsAction::Swipe { x1, y1, x2, y2 },
                    rejected,
                    (100, 200),
                    Some(observed),
                ))
            );
            assert_eq!(
                error
                    .diagnostic_context()
                    .expect("context")
                    .operation()
                    .as_str(),
                "input_swipe"
            );
        }
        let long_tap = backend
            .long_tap(&step_witness(), 101, 50, 500)
            .expect_err("delegated long tap rejection");
        assert_eq!(
            long_tap.adb_input_bounds_context(),
            Some(AdbInputBoundsContext::new(
                AdbBoundsAction::Swipe {
                    x1: 101,
                    y1: 50,
                    x2: 101,
                    y2: 50
                },
                AdbBoundsCoordinate::StartX,
                (100, 200),
                Some(observed),
            ))
        );
        backend.close().expect("close");
        assert!(!backend.connected);
        assert_eq!(backend.connect_geometry, Some(observed));
        let closed = backend
            .tap_with_child(100, 200, |_, _| {
                child_calls.set(child_calls.get() + 1);
                Ok(())
            })
            .expect_err("connected check retained after coordinate validation");
        assert_eq!(closed.message(), "AdbShellInputBackend is not connected");
        assert_eq!(closed.adb_input_bounds_context(), None);
        assert_eq!(child_calls.get(), 1);

        let mut unavailable = adb_shell_input_test_backend();
        unavailable.bounds = Some(TouchBounds {
            max_x: 10,
            max_y: 20,
        });
        let error = unavailable
            .tap_with_child(11, 0, |_, _| panic!("no rejected input child"))
            .expect_err("unobserved connect geometry");
        assert_eq!(
            error.adb_input_bounds_context(),
            Some(AdbInputBoundsContext::new(
                AdbBoundsAction::Tap { x: 11, y: 0 },
                AdbBoundsCoordinate::PointX,
                (10, 20),
                None,
            ))
        );
        assert!(error.diagnostic_message().expect("message").contains("connect_observation=unavailable connect_natural_max_x=unavailable connect_natural_max_y=unavailable connect_rotation_degrees=unavailable"));
        let lower = DeviceError::transient("complete lower geometry error")
            .with_diagnostic(
                DeviceErrorCategory::Native,
                crate::codes::DeviceStage::AdbEnsureDeviceGetState.location(),
            )
            .with_diagnostic_context(
                "adb_shell_input",
                crate::codes::DeviceLocation::EnsureDevice,
                DeviceErrorSensitivity::Sensitive,
            );
        let preserved = unavailable.bounds_failure(
            lower.clone(),
            AdbBoundsAction::Tap { x: 11, y: 0 },
            AdbBoundsCoordinate::PointX,
            TouchBounds {
                max_x: 10,
                max_y: 20,
            },
        );
        assert_eq!(preserved, lower);
        assert_eq!(preserved.diagnostic_context(), lower.diagnostic_context());
        assert_eq!(preserved.diagnostic_message(), None);
    }

    #[test]
    fn adb_shell_input_connect_uses_current_logical_orientation_bounds() {
        for (rotation, expected_bounds) in [
            (
                DeviceRotation::R0,
                TouchBounds {
                    max_x: 720,
                    max_y: 1280,
                },
            ),
            (
                DeviceRotation::R180,
                TouchBounds {
                    max_x: 720,
                    max_y: 1280,
                },
            ),
            (
                DeviceRotation::R90,
                TouchBounds {
                    max_x: 1280,
                    max_y: 720,
                },
            ),
            (
                DeviceRotation::R270,
                TouchBounds {
                    max_x: 1280,
                    max_y: 720,
                },
            ),
        ] {
            let mut backend = adb_shell_input_test_backend();
            let device = backend
                .connect_with_steps(
                    || Ok("device".to_string()),
                    || Ok("Physical size: 720x1280".to_string()),
                    || Ok(rotation),
                )
                .expect("connect");

            assert!(backend.connected);
            assert_eq!(device.state, "device");
            assert_eq!(device.screen_size, "Physical size: 720x1280");
            assert_eq!(backend.bounds, Some(expected_bounds));
        }
    }

    #[test]
    fn adb_shell_input_landscape_points_reach_one_child_and_out_of_range_reaches_none() {
        for rotation in [DeviceRotation::R90, DeviceRotation::R270] {
            for point in [(775, 691), (722, 615)] {
                let mut backend = adb_shell_input_test_backend();
                backend
                    .connect_with_steps(
                        || Ok("device".to_string()),
                        || Ok("Physical size: 720x1280".to_string()),
                        || Ok(rotation),
                    )
                    .expect("landscape connect");
                let calls = RefCell::new(Vec::new());

                backend
                    .tap_with_child(point.0, point.1, |x, y| {
                        calls.borrow_mut().push((x, y));
                        Ok(())
                    })
                    .expect("landscape point");

                assert_eq!(calls.borrow().as_slice(), &[point]);
            }
        }

        let mut backend = adb_shell_input_test_backend();
        backend
            .connect_with_steps(
                || Ok("device".to_string()),
                || Ok("Physical size: 720x1280".to_string()),
                || Ok(DeviceRotation::R90),
            )
            .expect("landscape connect");
        let child_calls = Cell::new(0);

        let error = backend
            .tap_with_child(1281, 691, |_, _| {
                child_calls.set(child_calls.get() + 1);
                Ok(())
            })
            .expect_err("out-of-range point");

        assert!(error.message().contains("exceeds touch screen max 1280"));
        assert_eq!(child_calls.get(), 0);
    }

    // Workflow #406 T3: an adb shell input backend connected to a 1080 × 1920 portrait device
    // whose Android SDK level reads as `sdk_level`, the connect's last step.
    fn adb_backend_with_sdk_level(sdk_level: &str) -> AdbShellInputBackend {
        let mut backend = adb_shell_input_test_backend();
        backend
            .connect_with_steps(
                || Ok("device".to_string()),
                || Ok("Physical size: 1080x1920".to_string()),
                || Ok(DeviceRotation::R0),
            )
            .expect("connect");
        backend
            .read_input_sdk_level(|| Ok(sdk_level.to_string()))
            .expect("the SDK level step");
        backend
    }

    // Workflow #406 T3 (plan Q18), written from the requirement: a plan's events as the one
    // shell script adb runs, `;`-separated: the press `input motionevent DOWN x y`, each move
    // `input motionevent MOVE x y` after `sleep` of its wait in seconds with three decimals, each
    // hold a `sleep`, the release `input motionevent UP x y` at the last point; no sleep for a
    // wait of 0.
    fn expected_motionevents(events: &[SegmentedSwipeEvent]) -> String {
        let seconds = |ms: u64| format!("sleep {}.{:03}", ms / 1000, ms % 1000);
        let mut commands = Vec::new();
        let mut last = None;
        for event in events {
            match event {
                SegmentedSwipeEvent::Down((x, y)) => {
                    commands.push(format!("input motionevent DOWN {x} {y}"));
                    last = Some((*x, *y));
                }
                SegmentedSwipeEvent::Move {
                    point: (x, y),
                    delay_before_ms,
                } => {
                    if *delay_before_ms > 0 {
                        commands.push(seconds(*delay_before_ms));
                    }
                    commands.push(format!("input motionevent MOVE {x} {y}"));
                    last = Some((*x, *y));
                }
                SegmentedSwipeEvent::Hold(ms) => {
                    if *ms > 0 {
                        commands.push(seconds(*ms));
                    }
                }
                SegmentedSwipeEvent::Up => {
                    let (x, y) = last.expect("a release after a press");
                    commands.push(format!("input motionevent UP {x} {y}"));
                }
            }
        }
        commands.join(";")
    }

    // The scripts `backend` hands its adb child for `plan`, with their timeouts.
    fn motionevent_runs(
        backend: &mut AdbShellInputBackend,
        plan: &PreparedSegmentedSwipePlan,
    ) -> (DeviceResult<()>, Vec<(Duration, String)>) {
        let runs = RefCell::new(Vec::new());
        let result = backend.segmented_swipe_with_child(plan, |timeout, script| {
            runs.borrow_mut().push((timeout, script.to_owned()));
            Ok(())
        });
        (result, runs.into_inner())
    }

    // The plan's waits, the least time the one shell invocation takes.
    fn plan_wait_ms(plan: &PreparedSegmentedSwipePlan) -> u64 {
        plan.events()
            .iter()
            .map(|event| match event {
                SegmentedSwipeEvent::Move {
                    delay_before_ms, ..
                } => *delay_before_ms,
                SegmentedSwipeEvent::Hold(ms) => *ms,
                SegmentedSwipeEvent::Down(_) | SegmentedSwipeEvent::Up => 0,
            })
            .sum()
    }

    // Workflow #406 T3 (brief T3 requirement 4, plan Q18; exec key 111): from Android SDK level
    // 23 the adb shell input backend performs a path drag as one shell of `input motionevent`
    // commands, the press, the moves with the plan's waits as sleeps, the holds and the release,
    // and never as `input swipe`; at level 22 or with an answer that is no level it supports no
    // segmented plan and refuses one with the segmented swipe capability error before any
    // command.
    #[test]
    fn t3_adb_sends_one_shell_of_motionevents_and_refuses_below_sdk_23() {
        let plan = crate::prepare_path_drag(crate::PathDragAction {
            points: vec![(100, 200), (600, 260), (220, 900), (800, 1500)],
            segment_duration_ms: vec![200, 300, 160],
            corner_hold_ms: 100,
        })
        .expect("a valid path drag");
        for level in ["23", "34"] {
            let mut backend = adb_backend_with_sdk_level(level);
            assert!(backend.supports_segmented_swipe(), "SDK {level}");
            let (result, runs) = motionevent_runs(&mut backend, &plan);
            result.unwrap_or_else(|error| panic!("SDK {level}: {error}"));
            let [(timeout, script)] = runs.as_slice() else {
                panic!("SDK {level}: one shell invocation, got {runs:?}");
            };
            assert_eq!(*script, expected_motionevents(plan.events()), "SDK {level}");
            let commands = script.split(';').collect::<Vec<_>>();
            assert_eq!(commands.first(), Some(&"input motionevent DOWN 100 200"));
            assert_eq!(commands.last(), Some(&"input motionevent UP 800 1500"));
            assert_eq!(
                commands
                    .iter()
                    .filter(|command| **command == "sleep 0.100")
                    .count(),
                2,
                "a hold at each inner corner: {script}"
            );
            assert!(
                commands
                    .iter()
                    .all(|command| command.starts_with("input motionevent ")
                        || command.starts_with("sleep ")),
                "{script}"
            );
            assert!(!script.contains("input swipe"), "{script}");
            assert!(
                *timeout >= Duration::from_millis(plan_wait_ms(&plan)),
                "SDK {level}: {timeout:?}"
            );
        }
        for level in ["22", "", "unknown"] {
            let mut backend = adb_backend_with_sdk_level(level);
            assert!(!backend.supports_segmented_swipe(), "SDK {level:?}");
            let (result, runs) = motionevent_runs(&mut backend, &plan);
            let error = result.expect_err("refused without motionevent");
            assert_eq!(
                error.diagnostic().expect("diagnostic").stage().as_str(),
                "input_segmented_swipe_capability",
                "SDK {level:?}"
            );
            assert!(runs.is_empty(), "SDK {level:?}: no command, got {runs:?}");
        }
    }

    // Workflow #406 T3 (brief T3 requirement 4, decision Q18 (a); host key 124): the
    // vertical-brake drag, and the drag with a pause and a brake, run on adb shell input from SDK
    // level 23 as one shell of motionevents, with the corner hold and the pause as sleeps.
    #[test]
    fn t3_a_vertical_brake_drag_runs_on_adb_through_motionevents() {
        let vertical = crate::prepare_segmented_swipe(crate::SegmentedSwipeAction {
            points: [(900, 1500), (200, 1505), (200, 1405)],
            horizontal_duration_ms: 200,
            corner_hold_ms: 150,
            brake_distance_px: 100,
            brake_duration_ms: 200,
            slope_in: 2,
            slope_out: 0,
        })
        .expect("a vertical-brake drag");
        let paused = crate::prepare_drag_with_pause_and_brake(crate::DragWithPauseAndBrakeAction {
            points: [(900, 1000), (200, 1000), (200, 900)],
            drag_duration_ms: 400,
            pause_ms: 150,
            brake_duration_ms: 200,
        })
        .expect("a drag with a pause and a brake");
        for (plan, release) in [
            (&vertical, "input motionevent UP 200 1405"),
            (&paused, "input motionevent UP 200 900"),
        ] {
            let mut backend = adb_backend_with_sdk_level("30");
            assert!(backend.supports_segmented_swipe());
            let (result, runs) = motionevent_runs(&mut backend, plan);
            result.unwrap_or_else(|error| panic!("{}: {error}", plan.action().name()));
            let [(_, script)] = runs.as_slice() else {
                panic!("one shell invocation, got {runs:?}");
            };
            assert_eq!(*script, expected_motionevents(plan.events()));
            let commands = script.split(';').collect::<Vec<_>>();
            assert_eq!(
                commands
                    .iter()
                    .filter(|command| command.starts_with("input motionevent DOWN "))
                    .count(),
                1,
                "{script}"
            );
            assert_eq!(commands.last(), Some(&release), "{script}");
            assert!(commands.contains(&"sleep 0.150"), "{script}");
            assert!(!script.contains("input swipe"), "{script}");
        }
    }

    #[derive(Clone)]
    struct FakeFactory {
        name: TouchBackendName,
        connect_result: Rc<RefCell<Vec<DeviceResult<()>>>>,
        action_result: Rc<RefCell<Vec<DeviceResult<()>>>>,
    }

    impl TouchBackendFactory for FakeFactory {
        fn name(&self) -> TouchBackendName {
            self.name
        }

        fn connect(&self) -> DeviceResult<ConnectedTouchBackend> {
            let result = self.connect_result.borrow_mut().remove(0);
            result?;
            Ok(ConnectedTouchBackend {
                name: self.name,
                input_parameters: None,
                backend: Box::new(FakeBackend {
                    action_result: self.action_result.clone(),
                    closed: false,
                }),
                device: DeviceInfo {
                    serial: "fake".to_string(),
                    state: "device".to_string(),
                    screen_size: "Physical size: 1280x720".to_string(),
                },
                handshake: None,
            })
        }
    }

    struct FakeBackend {
        action_result: Rc<RefCell<Vec<DeviceResult<()>>>>,
        closed: bool,
    }

    impl InputBackend for FakeBackend {
        fn tap(&mut self, _witness: &FencedWrite, _x: i32, _y: i32) -> DeviceResult<()> {
            self.action_result.borrow_mut().remove(0)
        }

        fn long_tap(
            &mut self,
            _witness: &FencedWrite,
            _x: i32,
            _y: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.action_result.borrow_mut().remove(0)
        }

        fn swipe(
            &mut self,
            _witness: &FencedWrite,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.action_result.borrow_mut().remove(0)
        }

        fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
            Ok(())
        }

        fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
            Ok(())
        }

        fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
            Ok(())
        }

        fn close_once(
            &mut self,
            _authority: DeviceCloseAuthority,
        ) -> DeviceResult<DeviceResourceCloseOutcome> {
            self.closed = true;
            Ok(DeviceResourceCloseOutcome::confirmed(1))
        }
    }

    fn fake_factory(
        name: TouchBackendName,
        connect: DeviceResult<()>,
        action: DeviceResult<()>,
    ) -> Box<dyn TouchBackendFactory> {
        Box::new(FakeFactory {
            name,
            connect_result: Rc::new(RefCell::new(vec![connect])),
            action_result: Rc::new(RefCell::new(vec![action])),
        })
    }

    // Defect regressions D04/D09: PR298 review 5120590779, Workflow #257 C1B9 v16.
    #[test]
    fn c1b9_d04_factory_quiescence() {
        let original = DeviceError::transient("acquisition primary").with_resource_close_cause(
            crate::DeviceResourceKind::InputBackend,
            crate::DeviceResourceClosePhase::AcquisitionCleanup,
            "maatouch",
            None,
            None,
            DeviceResourceQuiescence::Unconfirmed,
            1,
        );
        for requested in [TouchBackendChoice::AutoFastest, TouchBackendChoice::Auto] {
            let factories = vec![
                fake_factory(TouchBackendName::MaaTouch, Err(original.clone()), Ok(())),
                fake_factory(TouchBackendName::Minitouch, Ok(()), Ok(())),
            ];
            let error = match requested {
                TouchBackendChoice::AutoFastest => select_fastest(requested, factories),
                _ => select_fixed_priority(requested, factories),
            }
            .err()
            .expect("unconfirmed acquisition stops fallback");
            assert_eq!(error, original);
            assert!(std::sync::Arc::ptr_eq(
                error.resource_close_causes()[0].occurrence(),
                original.resource_close_causes()[0].occurrence()
            ));
        }

        // Workflow #342 F2, first red issuecomment-5975166569: active close and
        // untransferred candidate cleanup keep separate scopes and shared receipts.
        let active = original.with_backend_resources();
        let candidate = DeviceResourceCloseOutcome::confirmed(2).with_operation_resources();
        let mixed = active.clone().with_close_outcome(&candidate);
        assert_eq!(mixed.resource_count(), 3);
        assert_eq!(
            mixed.resource_quiescence(),
            Some(DeviceResourceQuiescence::Unconfirmed)
        );
        let scopes = mixed
            .resource_contributions()
            .map(|(_, quiescence, count, scope)| (scope, count, quiescence))
            .collect::<Vec<_>>();
        assert_eq!(
            scopes,
            [
                (
                    Some(actingcommand_contract::ResourceDispositionScope::SessionBackends),
                    1,
                    DeviceResourceQuiescence::Unconfirmed
                ),
                (
                    Some(actingcommand_contract::ResourceDispositionScope::OperationResources),
                    2,
                    DeviceResourceQuiescence::Confirmed
                ),
            ]
        );
        assert_eq!(
            mixed
                .merge_resource_cleanup(active)
                .with_close_outcome(&candidate)
                .resource_count(),
            3
        );
    }

    #[test]
    fn c1b9_d09_adb_close_terminal_cache() {
        let mut backend = AdbShellInputBackend::new(AdbConfig::default(), DeviceTarget::default());
        backend
            .connect_with_steps(
                || Ok("device".to_string()),
                || Ok("Physical size: 720x1280".to_string()),
                || Ok(DeviceRotation::R0),
            )
            .expect("simulated connect");
        let first = backend
            .close_once(DeviceCloseAuthority::LocalOnly)
            .expect("close");
        let second = backend
            .close_once(DeviceCloseAuthority::LocalOnly)
            .expect("cached close");
        assert_eq!(first, second);
        assert_eq!(first.resource_count(), 1);
    }

    #[test]
    fn fixed_priority_falls_back_after_selection_failure() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Ok(()),
                    Err(DeviceError::transient("maatouch write failed")),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        selected.tap(&step_witness(), 10, 20).expect("fallback tap");

        assert_eq!(selected.backend_name(), TouchBackendName::AdbShellInput);
        assert!(selected.diagnostics().warnings.iter().any(|warning| {
            warning.contains("WARNING") && warning.contains("maatouch") && warning.contains("tap")
        }));
        assert!(selected.diagnostics().attempts.iter().any(|attempt| {
            attempt.backend == TouchBackendName::MaaTouch
                && !attempt.ok
                && attempt.action.as_deref() == Some("tap")
                && attempt.fallback_backend == Some(TouchBackendName::AdbShellInput)
                && attempt.attempt_id > 0
        }));
        assert!(
            selected
                .diagnostics()
                .attempts
                .iter()
                .any(|attempt| attempt.backend == TouchBackendName::AdbShellInput
                    && attempt.ok
                    && attempt.selected)
        );
    }

    #[test]
    fn fixed_priority_fails_loud_when_all_backends_fail() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Ok(()),
                    Err(DeviceError::transient("maatouch write failed")),
                ),
                fake_factory(
                    TouchBackendName::AdbShellInput,
                    Ok(()),
                    Err(DeviceError::transient("adb input failed")),
                ),
            ],
        )
        .expect("selected");

        let err = selected
            .tap(&step_witness(), 10, 20)
            .expect_err("all failed");
        assert!(err.to_string().contains("touch backend chain failed"));
        assert!(err.to_string().contains("adb input failed"));
    }

    #[test]
    fn fallback_skipped_on_serious_input_error() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Ok(()),
                    Err(DeviceError::fatal("serious input error")),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        let err = selected
            .tap(&step_witness(), 10, 20)
            .expect_err("fatal input error");

        assert_eq!(err.message(), "serious input error");
        assert_eq!(selected.backend_name(), TouchBackendName::MaaTouch);
        assert!(
            !selected
                .diagnostics()
                .attempts
                .iter()
                .any(|attempt| attempt.backend == TouchBackendName::AdbShellInput && attempt.ok)
        );
    }

    #[test]
    fn fallback_on_transient_backend_failure() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Ok(()),
                    Err(DeviceError::transient("temporary write failed")),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        selected
            .long_tap(&step_witness(), 10, 20, 100)
            .expect("transient fallback");

        assert_eq!(selected.backend_name(), TouchBackendName::AdbShellInput);
    }

    #[test]
    fn fallback_records_full_context() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Ok(()),
                    Err(DeviceError::transient("socket write failed")),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        selected
            .swipe(&step_witness(), 10, 20, 30, 40, 100)
            .expect("fallback swipe");

        let attempt = selected
            .diagnostics()
            .attempts
            .iter()
            .find(|attempt| attempt.backend == TouchBackendName::MaaTouch && !attempt.ok)
            .expect("maatouch failure attempt");
        assert_eq!(attempt.action.as_deref(), Some("swipe"));
        assert_eq!(
            attempt.fallback_backend,
            Some(TouchBackendName::AdbShellInput)
        );
        assert!(attempt.error_reason.as_deref().is_some_and(|reason| {
            reason.contains("Transient") && reason.contains("socket write failed")
        }));
        assert!(attempt.attempt_id > 0);
        assert!(
            selected
                .diagnostics()
                .warnings
                .iter()
                .any(|warning| warning.contains("WARNING")
                    && warning.contains("fallback_backend=adb_shell_input"))
        );
    }

    #[test]
    fn shared_input_validation_blocks_fallback_on_out_of_bounds() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(TouchBackendName::MaaTouch, Ok(()), Ok(())),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        let err = selected
            .tap(&step_witness(), 1281, 20)
            .expect_err("out of bounds");

        assert!(err.message().contains("exceeds touch screen max"));
        assert_eq!(selected.backend_name(), TouchBackendName::MaaTouch);
        assert_eq!(selected.diagnostics().attempts.len(), 1);
    }

    #[test]
    fn selected_adb_shell_input_defers_stale_natural_bounds_to_concrete_backend() {
        let actions = Rc::new(RefCell::new(vec![Ok(())]));
        let mut selected = SelectedTouchBackend {
            action_open_observations: Vec::new(),
            active: ConnectedTouchBackend {
                name: TouchBackendName::AdbShellInput,
                input_parameters: None,
                backend: Box::new(FakeBackend {
                    action_result: Rc::clone(&actions),
                    closed: false,
                }),
                device: DeviceInfo {
                    serial: "fake".to_string(),
                    state: "device".to_string(),
                    screen_size: "Physical size: 720x1280".to_string(),
                },
                handshake: None,
            },
            remaining: Vec::new(),
            diagnostics: TouchBackendDiagnostics::new(TouchBackendChoice::AdbShellInput),
            close_result: None,
        };

        selected
            .tap(&step_witness(), 775, 691)
            .expect("selected backend must defer adb bounds");

        assert!(actions.borrow().is_empty());
    }

    #[test]
    fn touch_probe_report_uses_fake_backends_without_touch_actions() {
        let report = touch_probe_report_with_factories(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Err(DeviceError::transient("maatouch unavailable")),
                    Ok(()),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        );

        assert_eq!(report.selected, Some(TouchBackendName::AdbShellInput));
        assert_eq!(report.attempts.len(), 2);
        assert!(
            report
                .attempts
                .iter()
                .any(|attempt| attempt.backend == TouchBackendName::MaaTouch && !attempt.ok)
        );
        assert!(
            report
                .attempts
                .iter()
                .any(|attempt| attempt.backend == TouchBackendName::AdbShellInput
                    && attempt.ok
                    && attempt.selected)
        );
    }

    #[test]
    fn fastest_selection_removes_selected_factory_from_remaining() {
        let selected = select_fastest(
            TouchBackendChoice::AutoFastest,
            vec![
                fake_factory(TouchBackendName::MaaTouch, Ok(()), Ok(())),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        let selected_name = selected.backend_name();
        let expected_remaining = match selected_name {
            TouchBackendName::MaaTouch => TouchBackendName::AdbShellInput,
            TouchBackendName::AdbShellInput => TouchBackendName::MaaTouch,
            unexpected => panic!("unexpected selected backend: {unexpected:?}"),
        };
        assert_eq!(selected.remaining.len(), 1);
        assert_eq!(selected.remaining[0].name(), expected_remaining);

        let diagnostics = selected.diagnostics();
        assert_eq!(diagnostics.attempts.len(), 2);
        for backend in [TouchBackendName::MaaTouch, TouchBackendName::AdbShellInput] {
            let attempt = diagnostics
                .attempts
                .iter()
                .find(|attempt| attempt.backend == backend)
                .expect("successful connection diagnostic");
            assert!(attempt.ok);
            assert_eq!(attempt.action.as_deref(), Some("select"));
            assert_eq!(attempt.selected, backend == selected_name);
        }
    }

    #[test]
    fn minitouch_in_priority_chain() {
        let selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Err(DeviceError::transient("maatouch unavailable")),
                    Ok(()),
                ),
                fake_factory(TouchBackendName::Minitouch, Ok(()), Ok(())),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        assert_eq!(selected.backend_name(), TouchBackendName::Minitouch);
        assert_eq!(
            selected.diagnostics().selected,
            Some(TouchBackendName::Minitouch)
        );
        assert!(selected.diagnostics().attempts.iter().any(|attempt| {
            attempt.backend == TouchBackendName::Minitouch && attempt.ok && attempt.selected
        }));
    }

    #[test]
    fn minitouch_transient_failure_degrades() {
        let mut selected = select_fixed_priority(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::Minitouch,
                    Ok(()),
                    Err(DeviceError::transient("minitouch socket write failed")),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        selected
            .tap(&step_witness(), 10, 20)
            .expect("degraded to adb");

        assert_eq!(selected.backend_name(), TouchBackendName::AdbShellInput);
        assert!(selected.diagnostics().attempts.iter().any(|attempt| {
            attempt.backend == TouchBackendName::Minitouch
                && !attempt.ok
                && attempt.fallback_backend == Some(TouchBackendName::AdbShellInput)
        }));
    }
}
