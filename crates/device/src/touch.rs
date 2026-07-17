// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    Adb, AdbConfig, DeviceError, DeviceInfo, DeviceResult, DeviceTarget, HandshakeInfo,
    InputBackend, MaaTouchBackend, MaaTouchConfig, MinitouchBackend, MinitouchConfig,
};
use std::time::{Duration, Instant};

const MAX_ADB_INPUT_GESTURE_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchBackendName {
    MaaTouch,
    Minitouch,
    AdbShellInput,
}

impl TouchBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
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
    MaaTouch,
    Minitouch,
    AdbShellInput,
}

impl TouchBackendChoice {
    pub fn parse(value: &str) -> DeviceResult<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "auto-fastest" | "auto_fastest" => Ok(Self::AutoFastest),
            "maatouch" | "maa_touch" => Ok(Self::MaaTouch),
            "minitouch" | "mini_touch" => Ok(Self::Minitouch),
            "adb" | "adb_input" | "adb-input" | "adb_shell_input" | "adb-shell-input" => {
                Ok(Self::AdbShellInput)
            }
            other => Err(DeviceError::fatal(format!(
                "unknown touch backend '{other}', expected auto, auto-fastest, maatouch, minitouch, or adb_shell_input"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AutoFastest => "auto-fastest",
            Self::MaaTouch => "maatouch",
            Self::Minitouch => "minitouch",
            Self::AdbShellInput => "adb_shell_input",
        }
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
    active: ConnectedTouchBackend,
    remaining: Vec<Box<dyn TouchBackendFactory>>,
    diagnostics: TouchBackendDiagnostics,
}

impl SelectedTouchBackend {
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
        mut run: impl FnMut(&mut dyn InputBackend) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        self.validate_action_points(action, points)?;

        let active_started = Instant::now();
        match run(self.active.backend.as_mut()) {
            Ok(()) => return Ok(()),
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
            }
        }

        while !self.remaining.is_empty() {
            let factory = self.remaining.remove(0);
            let backend_name = factory.name();
            let started = Instant::now();
            match factory.connect() {
                Ok(mut connected) => match run(connected.backend.as_mut()) {
                    Ok(()) => {
                        let elapsed_ms = started.elapsed().as_millis();
                        self.diagnostics
                            .push_success(connected.name, elapsed_ms, action, true);
                        if let Err(err) = self.active.backend.close() {
                            self.diagnostics.warnings.push(format!(
                                "WARNING failed to close previous touch backend {} after fallback: {}",
                                self.active.name.as_str(),
                                err
                            ));
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
                        self.diagnostics.warnings.push(format!(
                            "WARNING touch backend {} failed during {action}; fallback_backend={}; reason={reason}",
                            connected.name.as_str(),
                            fallback_backend.map(TouchBackendName::as_str).unwrap_or("none")
                        ));
                        if let Err(close_err) = connected.backend.close() {
                            self.diagnostics.warnings.push(format!(
                                "WARNING failed to close failed touch backend {}: {}",
                                connected.name.as_str(),
                                close_err
                            ));
                        }
                        if !err.is_fallback_eligible() {
                            return Err(self.chain_failed_error(action));
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
                    self.diagnostics.warnings.push(format!(
                        "WARNING touch backend {} could not be selected for {action}; fallback_backend={}; reason={reason}",
                        backend_name.as_str(),
                        fallback_backend.map(TouchBackendName::as_str).unwrap_or("none")
                    ));
                    if !err.is_fallback_eligible() {
                        return Err(self.chain_failed_error(action));
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
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.run_touch_action("tap", &[(x, y)], |backend| backend.tap(x, y))
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.run_touch_action("long_tap", &[(x, y)], |backend| {
            backend.long_tap(x, y, duration_ms)
        })
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        self.run_touch_action("swipe", &[(x1, y1), (x2, y2)], |backend| {
            backend.swipe(x1, y1, x2, y2, duration_ms)
        })
    }

    fn key(&mut self, key: &str) -> DeviceResult<()> {
        self.active.backend.key(key)
    }

    fn text(&mut self, text: &str) -> DeviceResult<()> {
        self.active.backend.text(text)
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.active.backend.reset()
    }

    fn close(&mut self) -> DeviceResult<()> {
        self.active.backend.close()
    }
}

pub fn create_touch_backend(config: TouchBackendConfig) -> DeviceResult<SelectedTouchBackend> {
    let requested = config.requested;
    let factories = default_touch_factories(config);
    match requested {
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
                if let Err(err) = connected.backend.close() {
                    diagnostics.warnings.push(format!(
                        "touch probe backend {} close failed: {}",
                        name.as_str(),
                        err
                    ));
                }
                diagnostics.push_success(name, elapsed_ms, "probe", false);
                successful.push((name, elapsed_ms));
            }
            Err(err) => diagnostics.push_failure(
                factory.name(),
                started.elapsed().as_millis(),
                err.to_string(),
                "probe",
                None,
            ),
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
                return Ok(SelectedTouchBackend {
                    active,
                    remaining: factories,
                    diagnostics,
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
                diagnostics.warnings.push(format!(
                    "WARNING touch backend {} unavailable during selection; fallback_backend={}; reason={reason}",
                    backend.as_str(),
                    fallback_backend.map(TouchBackendName::as_str).unwrap_or("none")
                ));
                if !err.is_fallback_eligible() {
                    return Err(DeviceError::fatal(format!(
                        "touch backend selection stopped on non-fallback error; diagnostics: {}",
                        format_touch_diagnostics(&diagnostics)
                    )));
                }
            }
        }
    }
    Err(DeviceError::fatal(format!(
        "touch backend selection failed; diagnostics: {}",
        format_touch_diagnostics(&diagnostics)
    )))
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
                diagnostics.warnings.push(format!(
                    "WARNING touch backend {} unavailable during fastest selection: {reason}",
                    backend.as_str()
                ));
                if !err.is_fallback_eligible() {
                    return Err(DeviceError::fatal(format!(
                        "touch backend fastest selection stopped on non-fallback error; diagnostics: {}",
                        format_touch_diagnostics(&diagnostics)
                    )));
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
        return Err(DeviceError::fatal(format!(
            "touch backend fastest selection failed; diagnostics: {}",
            format_touch_diagnostics(&diagnostics)
        )));
    };

    let (selected_factory_index, _elapsed_ms, active) = connected.remove(selected_pos);
    for (_index, _elapsed_ms, mut backend) in connected {
        if let Err(err) = backend.backend.close() {
            diagnostics.warnings.push(format!(
                "touch backend {} close failed after fastest probe: {}",
                backend.name.as_str(),
                err
            ));
        }
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
        active,
        remaining,
        diagnostics,
    })
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
}

impl TouchBackendFactory for AdbShellInputFactory {
    fn name(&self) -> TouchBackendName {
        TouchBackendName::AdbShellInput
    }

    fn connect(&self) -> DeviceResult<ConnectedTouchBackend> {
        let mut backend = AdbShellInputBackend::new(self.adb_config.clone(), self.target.clone());
        let device = backend.connect()?;
        Ok(ConnectedTouchBackend {
            name: TouchBackendName::AdbShellInput,
            backend: Box::new(backend),
            device,
            handshake: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AdbShellInputBackend {
    adb_config: AdbConfig,
    target: DeviceTarget,
    serial: String,
    bounds: Option<TouchBounds>,
    connected: bool,
}

impl AdbShellInputBackend {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        let serial = target.resolved_serial();
        Self {
            adb_config,
            target,
            serial,
            bounds: None,
            connected: false,
        }
    }

    pub fn connect(&mut self) -> DeviceResult<DeviceInfo> {
        let adb = Adb::new(self.adb_config.clone());
        let state = adb.ensure_device(&self.serial, self.target.connect)?;
        let screen_size = adb.screen_size(&self.serial)?;
        self.bounds = Some(touch_bounds_from_screen_size(&screen_size)?);
        self.connected = true;
        Ok(DeviceInfo {
            serial: self.serial.clone(),
            state,
            screen_size,
        })
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
}

impl InputBackend for AdbShellInputBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        let bounds = self.bounds()?;
        validate_touch_coordinate("tap x", x, bounds.max_x)?;
        validate_touch_coordinate("tap y", y, bounds.max_y)?;
        self.ensure_connected()?;
        let adb = Adb::new(self.adb_config.clone());
        adb.shell_input_tap(&self.serial, x, y)?;
        Ok(())
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.swipe(x, y, x, y, duration_ms)
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        let bounds = self.bounds()?;
        validate_touch_coordinate("swipe x1", x1, bounds.max_x)?;
        validate_touch_coordinate("swipe y1", y1, bounds.max_y)?;
        validate_touch_coordinate("swipe x2", x2, bounds.max_x)?;
        validate_touch_coordinate("swipe y2", y2, bounds.max_y)?;
        self.ensure_connected()?;
        let duration_ms = duration_ms.clamp(1, MAX_ADB_INPUT_GESTURE_MS);
        let adb = self.adb_for_duration(duration_ms);
        adb.shell_input_swipe(&self.serial, x1, y1, x2, y2, duration_ms)?;
        Ok(())
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "AdbShellInputBackend key input is outside A1 touch fallback scope",
        ))
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "AdbShellInputBackend text input is outside A1 touch fallback scope",
        ))
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.ensure_connected()
    }

    fn close(&mut self) -> DeviceResult<()> {
        self.connected = false;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TouchBounds {
    max_x: i32,
    max_y: i32,
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
        TouchBackendName::Minitouch => touch_bounds_from_screen_size(&device.screen_size),
        TouchBackendName::MaaTouch | TouchBackendName::AdbShellInput => {
            touch_bounds_from_device(handshake, device)
        }
    }
}

fn touch_bounds_from_screen_size(screen_size: &str) -> DeviceResult<TouchBounds> {
    let (_, dimensions) = screen_size.rsplit_once(':').unwrap_or(("", screen_size));
    let (width, height) = dimensions.trim().split_once('x').ok_or_else(|| {
        DeviceError::fatal(format!(
            "failed to parse touch screen bounds from adb wm size output: {screen_size}"
        ))
    })?;
    let max_x = width.trim().parse::<i32>().map_err(|err| {
        DeviceError::fatal(format!(
            "invalid touch screen width '{width}' in adb wm size output: {err}"
        ))
    })?;
    let max_y = height.trim().parse::<i32>().map_err(|err| {
        DeviceError::fatal(format!(
            "invalid touch screen height '{height}' in adb wm size output: {err}"
        ))
    })?;
    if max_x <= 0 || max_y <= 0 {
        return Err(DeviceError::fatal(format!(
            "touch screen bounds must be positive, got {max_x}x{max_y}"
        )));
    }
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
    use std::cell::RefCell;
    use std::rc::Rc;

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
        fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
            self.action_result.borrow_mut().remove(0)
        }

        fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
            self.action_result.borrow_mut().remove(0)
        }

        fn swipe(
            &mut self,
            _x1: i32,
            _y1: i32,
            _x2: i32,
            _y2: i32,
            _duration_ms: u64,
        ) -> DeviceResult<()> {
            self.action_result.borrow_mut().remove(0)
        }

        fn key(&mut self, _key: &str) -> DeviceResult<()> {
            Ok(())
        }

        fn text(&mut self, _text: &str) -> DeviceResult<()> {
            Ok(())
        }

        fn reset(&mut self) -> DeviceResult<()> {
            Ok(())
        }

        fn close(&mut self) -> DeviceResult<()> {
            self.closed = true;
            Ok(())
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

        selected.tap(10, 20).expect("fallback tap");

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

        let err = selected.tap(10, 20).expect_err("all failed");
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

        let err = selected.tap(10, 20).expect_err("fatal input error");

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

        selected.long_tap(10, 20, 100).expect("transient fallback");

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

        selected.swipe(10, 20, 30, 40, 100).expect("fallback swipe");

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

        let err = selected.tap(1281, 20).expect_err("out of bounds");

        assert!(err.message().contains("exceeds touch screen max"));
        assert_eq!(selected.backend_name(), TouchBackendName::MaaTouch);
        assert_eq!(selected.diagnostics().attempts.len(), 1);
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

        selected.tap(10, 20).expect("degraded to adb");

        assert_eq!(selected.backend_name(), TouchBackendName::AdbShellInput);
        assert!(selected.diagnostics().attempts.iter().any(|attempt| {
            attempt.backend == TouchBackendName::Minitouch
                && !attempt.ok
                && attempt.fallback_backend == Some(TouchBackendName::AdbShellInput)
        }));
    }
}
