// SPDX-License-Identifier: AGPL-3.0-only

use crate::UserConfig;
use actingcommand_contract::{InputAction, LabError, LabResult, LeaseToken};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendConfig,
    CaptureBackendName, DeviceResult, TouchBackendConfig, combine_operation_and_close,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct InputBackendRequest {
    pub instance_alias: Option<String>,
    pub config: TouchBackendConfig,
    pub observation: Option<InputBackendObservation>,
    /// A lease the caller already holds on the port's Runtime connection. With `Some`, the
    /// port's input requests run under it and the port acquires, renews and releases no
    /// lease of its own; the holder keeps renewing and releasing it. With `None`, the port
    /// acquires its own lease as before.
    pub lease: Option<LeaseToken>,
}

#[derive(Debug, Clone, Default)]
pub struct InputBackendObservation {
    report: Arc<Mutex<Option<InputBackendReport>>>,
}

impl InputBackendObservation {
    pub fn record(&self, report: InputBackendReport) -> LabResult<()> {
        let mut slot = self.report.lock().map_err(|_| {
            actingcommand_contract::LabError::device("input backend observation lock poisoned")
        })?;
        *slot = Some(report);
        Ok(())
    }

    pub fn snapshot(&self) -> LabResult<InputBackendReport> {
        self.report
            .lock()
            .map_err(|_| {
                actingcommand_contract::LabError::device("input backend observation lock poisoned")
            })?
            .clone()
            .ok_or_else(|| {
                actingcommand_contract::LabError::device(
                    "input backend did not publish execution diagnostics",
                )
            })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InputBackendReport {
    pub backend: String,
    #[serde(rename = "touch_backend_requested")]
    pub requested_backend: String,
    pub adb_source: String,
    pub adb_warning: Option<String>,
    #[serde(rename = "touch_backend_attempts")]
    pub attempts: Vec<InputBackendAttemptReport>,
    #[serde(rename = "touch_backend_warnings")]
    pub warnings: Vec<String>,
    pub serial: String,
    pub device_state: String,
    pub screen_size: String,
    pub handshake: Option<InputHandshakeReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputBackendAttemptReport {
    pub attempt_id: u64,
    pub backend: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    pub action: Option<String>,
    pub fallback_backend: Option<String>,
    pub error_reason: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InputHandshakeReport {
    pub max_contacts: i32,
    pub max_x: i32,
    pub max_y: i32,
    pub max_pressure: i32,
    pub pid: String,
}

/// Lab's input port. Production implementations submit each action to the resident
/// Runtime, which admits it under its own lease and issues the device write witness on its
/// side of the proxy; Lab never holds a device backend or a witness.
pub trait LabInputPort {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()>;

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()>;

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()>;

    fn key(&mut self, key: &str) -> DeviceResult<()>;

    fn text(&mut self, text: &str) -> DeviceResult<()>;

    fn close(&mut self) -> DeviceResult<()>;
}

pub trait InputBackendFactory {
    fn open(&self, request: InputBackendRequest) -> LabResult<Box<dyn LabInputPort>>;
}

/// Temporary Lab client port. Production implementations submit to Runtime; sealed tests may fake it.
///
/// This is Lab's only input face: inside `crates/lab`, only its implementations call the
/// `LabInputPort` write methods (workspace guard).
pub trait SemanticInputExecutor {
    fn execute(&self, action: InputAction) -> LabResult<InputBackendReport>;
}

/// Lab's input face over an `InputBackendFactory` port: each action opens one port, performs
/// the action, closes the port and returns the report the port published.
pub(crate) struct PortSemanticInput<'a, F> {
    pub(crate) factory: &'a F,
    pub(crate) instance_alias: Option<String>,
    pub(crate) config: TouchBackendConfig,
    pub(crate) lease: Option<LeaseToken>,
}

type PortAction = Box<dyn FnOnce(&mut dyn LabInputPort) -> DeviceResult<()>>;

impl<F: InputBackendFactory> SemanticInputExecutor for PortSemanticInput<'_, F> {
    fn execute(&self, action: InputAction) -> LabResult<InputBackendReport> {
        let perform: PortAction = match action {
            InputAction::Tap { x, y } => {
                Box::new(move |port: &mut dyn LabInputPort| port.tap(x, y))
            }
            InputAction::LongTap { x, y, duration_ms } => {
                Box::new(move |port: &mut dyn LabInputPort| port.long_tap(x, y, duration_ms))
            }
            InputAction::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => {
                Box::new(move |port: &mut dyn LabInputPort| port.swipe(x1, y1, x2, y2, duration_ms))
            }
            InputAction::Key { key } => Box::new(move |port: &mut dyn LabInputPort| port.key(&key)),
            InputAction::Text { text } => {
                Box::new(move |port: &mut dyn LabInputPort| port.text(&text))
            }
            InputAction::SingleTouchDragWithVerticalBrakeV1 { .. } | InputAction::Reset => {
                return Err(LabError::usage(
                    "Lab input port performs only tap, long tap, swipe, key and text actions",
                ));
            }
        };
        let observation = InputBackendObservation::default();
        let mut port = self.factory.open(InputBackendRequest {
            instance_alias: self.instance_alias.clone(),
            config: self.config.clone(),
            observation: Some(observation.clone()),
            lease: self.lease.clone(),
        })?;
        let operation = perform(port.as_mut());
        let close = port.close();
        combine_operation_and_close(operation, close)
            .map_err(|error| LabError::device(error.to_string()))?;
        observation.snapshot()
    }
}

#[cfg(test)]
pub(crate) struct DisabledSemanticInput;

#[cfg(test)]
impl SemanticInputExecutor for DisabledSemanticInput {
    fn execute(&self, _action: InputAction) -> LabResult<InputBackendReport> {
        Err(actingcommand_contract::LabError::device(
            "semantic input must not execute in this test",
        ))
    }
}

pub struct CaptureBackendRequest {
    pub instance_alias: Option<String>,
    pub config: CaptureBackendConfig,
    pub observation: Option<CaptureBackendObservation>,
}

#[derive(Debug, Clone, Default)]
pub struct CaptureBackendObservation {
    report: Arc<Mutex<Option<CaptureBackendReport>>>,
}

impl CaptureBackendObservation {
    pub fn record(&self, report: CaptureBackendReport) -> LabResult<()> {
        let mut slot = self.report.lock().map_err(|_| {
            actingcommand_contract::LabError::device("capture backend observation lock poisoned")
        })?;
        *slot = Some(report);
        Ok(())
    }

    pub fn snapshot(&self) -> LabResult<CaptureBackendReport> {
        self.report
            .lock()
            .map_err(|_| {
                actingcommand_contract::LabError::device(
                    "capture backend observation lock poisoned",
                )
            })?
            .clone()
            .ok_or_else(|| {
                actingcommand_contract::LabError::device(
                    "capture backend did not publish selection diagnostics",
                )
            })
    }
}

#[derive(Debug, Clone)]
pub struct CaptureBackendReport {
    pub requested: CaptureBackendChoice,
    pub used: CaptureBackendName,
    pub attempts: Vec<CaptureBackendAttempt>,
}

pub trait CaptureBackendFactory {
    fn open(&self, request: CaptureBackendRequest) -> LabResult<Box<dyn CaptureBackend>>;
}

pub trait Clock {
    fn now_unix_ms(&self) -> LabResult<u64>;

    fn sleep(&self, duration: Duration);
}

pub trait ConfigSource {
    fn load(&self) -> LabResult<UserConfig>;

    fn state_root(&self) -> LabResult<PathBuf>;
}

pub trait LabPorts {
    type InputFactory: InputBackendFactory;
    type SemanticInput: SemanticInputExecutor;
    type CaptureFactory: CaptureBackendFactory;
    type Time: Clock;
    type Config: ConfigSource;

    fn input_factory(&self) -> &Self::InputFactory;
    fn semantic_input(&self) -> &Self::SemanticInput;
    fn capture_factory(&self) -> &Self::CaptureFactory;
    fn clock(&self) -> &Self::Time;
    fn config(&self) -> &Self::Config;
}
