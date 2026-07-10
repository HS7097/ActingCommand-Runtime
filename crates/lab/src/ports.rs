// SPDX-License-Identifier: AGPL-3.0-only

use crate::UserConfig;
use actingcommand_contract::{DriveRecord, LabResult, LedgerProjection};
use actingcommand_device::{
    CaptureBackend, CaptureBackendAttempt, CaptureBackendChoice, CaptureBackendConfig,
    CaptureBackendName, InputBackend, TouchBackendConfig,
};
use actingcommand_ledger::{
    LabLogError, LabLogResult, LastResortError, LedgerRead, LedgerRecord, LightEvent, SessionHeader,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct InputBackendRequest {
    pub config: TouchBackendConfig,
    pub observation: Option<InputBackendObservation>,
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

pub trait InputBackendFactory {
    fn open(&self, request: InputBackendRequest) -> LabResult<Box<dyn InputBackend>>;
}

pub struct CaptureBackendRequest {
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

pub struct RunLedgerSessionRequest {
    pub run_root: PathBuf,
    pub run_id: String,
    pub instance: String,
    pub header: SessionHeader,
}

pub trait LedgerSink {
    type RunSession;

    fn append_drive<T: Serialize>(&mut self, record: &DriveRecord<T>) -> LabResult<()>;

    fn finish<T: Serialize>(&mut self, response: &T) -> LabResult<LedgerProjection>;

    fn run_session(&mut self) -> Self::RunSession;

    fn start_run_session(
        _session: &mut Self::RunSession,
        _request: RunLedgerSessionRequest,
    ) -> LabLogResult<PathBuf> {
        Err(run_ledger_unavailable())
    }

    fn append_run_record(
        _session: &mut Self::RunSession,
        _record: LedgerRecord,
    ) -> LabLogResult<()> {
        Err(run_ledger_unavailable())
    }

    fn append_run_event(_session: &mut Self::RunSession, _event: LightEvent) -> LabLogResult<()> {
        Err(run_ledger_unavailable())
    }

    fn sync_run_session(_session: &Self::RunSession) -> LabLogResult<()> {
        Err(run_ledger_unavailable())
    }

    fn read_run_session(_session: &Self::RunSession) -> LabLogResult<LedgerRead> {
        Err(run_ledger_unavailable())
    }

    fn write_run_last_resort(
        _run_root: Option<&Path>,
        _error: &LastResortError,
    ) -> LabLogResult<PathBuf> {
        Err(run_ledger_unavailable())
    }
}

fn run_ledger_unavailable() -> LabLogError {
    LabLogError::InvalidInput("run ledger capability is unavailable".to_string())
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
    type CaptureFactory: CaptureBackendFactory;
    type Ledger: LedgerSink;
    type Time: Clock;
    type Config: ConfigSource;

    fn input_factory(&self) -> &Self::InputFactory;
    fn capture_factory(&self) -> &Self::CaptureFactory;
    fn ledger(&mut self) -> &mut Self::Ledger;
    fn clock(&self) -> &Self::Time;
    fn config(&self) -> &Self::Config;
}
