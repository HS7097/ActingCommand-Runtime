// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    Adb, AdbConfig, DeviceError, DeviceInfo, DeviceResult, DeviceTarget, HandshakeInfo,
    InputBackend, MaaTouchBackend, MaaTouchConfig,
};
use std::time::{Duration, Instant};

const MAX_ADB_INPUT_GESTURE_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchBackendName {
    MaaTouch,
    AdbShellInput,
}

impl TouchBackendName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaaTouch => "maatouch",
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
    AdbShellInput,
}

impl TouchBackendChoice {
    pub fn parse(value: &str) -> DeviceResult<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "auto-fastest" | "auto_fastest" => Ok(Self::AutoFastest),
            "maatouch" | "maa_touch" => Ok(Self::MaaTouch),
            "adb" | "adb_input" | "adb-input" | "adb_shell_input" | "adb-shell-input" => {
                Ok(Self::AdbShellInput)
            }
            other => Err(DeviceError::fatal(format!(
                "unknown touch backend '{other}', expected auto, auto-fastest, maatouch, or adb_shell_input"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AutoFastest => "auto-fastest",
            Self::MaaTouch => "maatouch",
            Self::AdbShellInput => "adb_shell_input",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TouchBackendConfig {
    pub adb_config: AdbConfig,
    pub target: DeviceTarget,
    pub maatouch_config: MaaTouchConfig,
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
            requested: TouchBackendChoice::Auto,
        }
    }

    pub fn with_requested(mut self, requested: TouchBackendChoice) -> Self {
        self.requested = requested;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchBackendAttempt {
    pub backend: TouchBackendName,
    pub ok: bool,
    pub elapsed_ms: u128,
    pub error_reason: Option<String>,
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
        mut run: impl FnMut(&mut dyn InputBackend) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        match run(self.active.backend.as_mut()) {
            Ok(()) => return Ok(()),
            Err(err) => {
                self.record_runtime_failure(action, self.active.name, &err);
            }
        }

        while !self.remaining.is_empty() {
            let factory = self.remaining.remove(0);
            let started = Instant::now();
            match factory.connect() {
                Ok(mut connected) => match run(connected.backend.as_mut()) {
                    Ok(()) => {
                        let elapsed_ms = started.elapsed().as_millis();
                        self.diagnostics.attempts.push(TouchBackendAttempt {
                            backend: connected.name,
                            ok: true,
                            elapsed_ms,
                            error_reason: None,
                            selected: true,
                        });
                        if let Err(err) = self.active.backend.close() {
                            self.diagnostics.warnings.push(format!(
                                "failed to close previous touch backend {} after fallback: {}",
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
                        self.diagnostics.attempts.push(TouchBackendAttempt {
                            backend: connected.name,
                            ok: false,
                            elapsed_ms,
                            error_reason: Some(reason.clone()),
                            selected: false,
                        });
                        self.diagnostics.warnings.push(format!(
                            "touch backend {} failed during {action}: {reason}",
                            connected.name.as_str()
                        ));
                        if let Err(close_err) = connected.backend.close() {
                            self.diagnostics.warnings.push(format!(
                                "failed to close failed touch backend {}: {}",
                                connected.name.as_str(),
                                close_err
                            ));
                        }
                    }
                },
                Err(err) => {
                    let elapsed_ms = started.elapsed().as_millis();
                    let backend = factory.name();
                    let reason = err.to_string();
                    self.diagnostics.attempts.push(TouchBackendAttempt {
                        backend,
                        ok: false,
                        elapsed_ms,
                        error_reason: Some(reason.clone()),
                        selected: false,
                    });
                    self.diagnostics.warnings.push(format!(
                        "touch backend {} could not be selected for {action}: {reason}",
                        backend.as_str()
                    ));
                }
            }
        }

        Err(DeviceError::fatal(format!(
            "touch backend chain failed during {action}; diagnostics: {}",
            format_touch_diagnostics(&self.diagnostics)
        )))
    }

    fn record_runtime_failure(
        &mut self,
        action: &str,
        backend: TouchBackendName,
        err: &DeviceError,
    ) {
        let reason = err.to_string();
        self.diagnostics.attempts.push(TouchBackendAttempt {
            backend,
            ok: false,
            elapsed_ms: 0,
            error_reason: Some(reason.clone()),
            selected: false,
        });
        self.diagnostics.warnings.push(format!(
            "touch backend {} failed during {action}: {reason}",
            backend.as_str()
        ));
    }
}

impl InputBackend for SelectedTouchBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.run_touch_action("tap", |backend| backend.tap(x, y))
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.run_touch_action("long_tap", |backend| backend.long_tap(x, y, duration_ms))
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        self.run_touch_action("swipe", |backend| {
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
                diagnostics.attempts.push(TouchBackendAttempt {
                    backend: name,
                    ok: true,
                    elapsed_ms,
                    error_reason: None,
                    selected: false,
                });
                successful.push((name, elapsed_ms));
            }
            Err(err) => diagnostics.attempts.push(TouchBackendAttempt {
                backend: factory.name(),
                ok: false,
                elapsed_ms: started.elapsed().as_millis(),
                error_reason: Some(err.to_string()),
                selected: false,
            }),
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
        TouchBackendChoice::Auto => [TouchBackendName::MaaTouch, TouchBackendName::AdbShellInput]
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
                diagnostics.attempts.push(TouchBackendAttempt {
                    backend: active.name,
                    ok: true,
                    elapsed_ms: started.elapsed().as_millis(),
                    error_reason: None,
                    selected: true,
                });
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
                diagnostics.attempts.push(TouchBackendAttempt {
                    backend,
                    ok: false,
                    elapsed_ms: started.elapsed().as_millis(),
                    error_reason: Some(reason.clone()),
                    selected: false,
                });
                diagnostics.warnings.push(format!(
                    "touch backend {} unavailable during selection: {reason}",
                    backend.as_str()
                ));
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
                diagnostics.attempts.push(TouchBackendAttempt {
                    backend: backend.name,
                    ok: true,
                    elapsed_ms,
                    error_reason: None,
                    selected: false,
                });
                connected.push((index, elapsed_ms, backend));
            }
            Err(err) => {
                let backend = factory.name();
                let reason = err.to_string();
                diagnostics.attempts.push(TouchBackendAttempt {
                    backend,
                    ok: false,
                    elapsed_ms: started.elapsed().as_millis(),
                    error_reason: Some(reason.clone()),
                    selected: false,
                });
                diagnostics.warnings.push(format!(
                    "touch backend {} unavailable during fastest selection: {reason}",
                    backend.as_str()
                ));
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
                "{{backend:{}, ok:{}, elapsed_ms:{}, selected:{}, error_reason:{}}}",
                attempt.backend.as_str(),
                attempt.ok,
                attempt.elapsed_ms,
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
    connected: bool,
}

impl AdbShellInputBackend {
    pub fn new(adb_config: AdbConfig, target: DeviceTarget) -> Self {
        let serial = target.resolved_serial();
        Self {
            adb_config,
            target,
            serial,
            connected: false,
        }
    }

    pub fn connect(&mut self) -> DeviceResult<DeviceInfo> {
        let adb = Adb::new(self.adb_config.clone());
        let state = adb.ensure_device(&self.serial, self.target.connect)?;
        let screen_size = adb.screen_size(&self.serial)?;
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
}

impl InputBackend for AdbShellInputBackend {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        validate_adb_input_coordinate("tap x", x)?;
        validate_adb_input_coordinate("tap y", y)?;
        self.ensure_connected()?;
        let adb = Adb::new(self.adb_config.clone());
        adb.shell_input_tap(&self.serial, x, y)?;
        Ok(())
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.swipe(x, y, x, y, duration_ms)
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        validate_adb_input_coordinate("swipe x1", x1)?;
        validate_adb_input_coordinate("swipe y1", y1)?;
        validate_adb_input_coordinate("swipe x2", x2)?;
        validate_adb_input_coordinate("swipe y2", y2)?;
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

fn validate_adb_input_coordinate(label: &str, value: i32) -> DeviceResult<()> {
    if value < 0 {
        return Err(DeviceError::fatal(format!(
            "{label} must be non-negative for adb shell input, got {value}"
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
                    Err(DeviceError::fatal("maatouch write failed")),
                ),
                fake_factory(TouchBackendName::AdbShellInput, Ok(()), Ok(())),
            ],
        )
        .expect("selected");

        selected.tap(10, 20).expect("fallback tap");

        assert_eq!(selected.backend_name(), TouchBackendName::AdbShellInput);
        assert!(
            selected
                .diagnostics()
                .warnings
                .iter()
                .any(|warning| { warning.contains("maatouch") && warning.contains("tap") })
        );
        assert!(
            selected
                .diagnostics()
                .attempts
                .iter()
                .any(|attempt| attempt.backend == TouchBackendName::MaaTouch && !attempt.ok)
        );
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
                    Err(DeviceError::fatal("maatouch write failed")),
                ),
                fake_factory(
                    TouchBackendName::AdbShellInput,
                    Ok(()),
                    Err(DeviceError::fatal("adb input failed")),
                ),
            ],
        )
        .expect("selected");

        let err = selected.tap(10, 20).expect_err("all failed");
        assert!(err.to_string().contains("touch backend chain failed"));
        assert!(err.to_string().contains("adb input failed"));
    }

    #[test]
    fn touch_probe_report_uses_fake_backends_without_touch_actions() {
        let report = touch_probe_report_with_factories(
            TouchBackendChoice::Auto,
            vec![
                fake_factory(
                    TouchBackendName::MaaTouch,
                    Err(DeviceError::fatal("maatouch unavailable")),
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
}
