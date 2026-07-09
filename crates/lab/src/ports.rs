// SPDX-License-Identifier: AGPL-3.0-only

use crate::UserConfig;
use actingcommand_contract::{DriveRecord, LabResult, LedgerProjection};
use actingcommand_device::{
    CaptureBackend, CaptureBackendConfig, InputBackend, TouchBackendConfig,
};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

pub struct InputBackendRequest {
    pub config: TouchBackendConfig,
}

pub trait InputBackendFactory {
    fn open(&self, request: InputBackendRequest) -> LabResult<Box<dyn InputBackend>>;
}

pub struct CaptureBackendRequest {
    pub config: CaptureBackendConfig,
}

pub trait CaptureBackendFactory {
    fn open(&self, request: CaptureBackendRequest) -> LabResult<Box<dyn CaptureBackend>>;
}

pub trait LedgerSink {
    fn append_drive<T: Serialize>(&mut self, record: &DriveRecord<T>) -> LabResult<()>;

    fn finish<T: Serialize>(&mut self, response: &T) -> LabResult<LedgerProjection>;
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
