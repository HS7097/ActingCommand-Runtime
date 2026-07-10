// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{LabError, LabResult};
use actingcommand_ledger::{LastResortError, LedgerRead, LedgerRecord, LightEvent, SessionHeader};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LedgerSessionHeader(SessionHeader);

impl LedgerSessionHeader {
    pub fn from_json(encoded: &str) -> LabResult<Self> {
        decode_json(encoded, "ledger session header").map(Self)
    }

    pub fn encoded_json(&self) -> LabResult<String> {
        encode_json(&self.0, "ledger session header")
    }

    pub(crate) fn from_storage(header: SessionHeader) -> Self {
        Self(header)
    }

    fn into_storage(self) -> SessionHeader {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct LedgerRecordEntry(LedgerRecord);

impl LedgerRecordEntry {
    pub fn from_json(encoded: &str) -> LabResult<Self> {
        decode_json(encoded, "ledger record").map(Self)
    }

    pub fn encoded_json(&self) -> LabResult<String> {
        encode_json(&self.0, "ledger record")
    }

    pub(crate) fn from_storage(record: LedgerRecord) -> Self {
        Self(record)
    }

    pub(crate) fn into_storage(self) -> LedgerRecord {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn storage(&self) -> &LedgerRecord {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct LedgerEventEntry(LightEvent);

impl LedgerEventEntry {
    pub fn from_json(encoded: &str) -> LabResult<Self> {
        decode_json(encoded, "ledger event").map(Self)
    }

    pub fn encoded_json(&self) -> LabResult<String> {
        encode_json(&self.0, "ledger event")
    }

    pub(crate) fn from_storage(event: LightEvent) -> Self {
        Self(event)
    }

    pub(crate) fn into_storage(self) -> LightEvent {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct LedgerLastResort(LastResortError);

impl LedgerLastResort {
    pub fn from_json(encoded: &str) -> LabResult<Self> {
        decode_json(encoded, "last-resort ledger error").map(Self)
    }

    pub fn encoded_json(&self) -> LabResult<String> {
        encode_json(&self.0, "last-resort ledger error")
    }

    pub(crate) fn from_storage(error: LastResortError) -> Self {
        Self(error)
    }

    #[cfg(test)]
    pub(crate) fn storage(&self) -> &LastResortError {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct LedgerReadback(LedgerRead);

impl LedgerReadback {
    pub fn new(
        header: Option<LedgerSessionHeader>,
        events: Vec<LedgerEventEntry>,
        records: Vec<LedgerRecordEntry>,
        skipped_corrupt_lines: usize,
    ) -> Self {
        Self(LedgerRead {
            header: header.map(LedgerSessionHeader::into_storage),
            events: events
                .into_iter()
                .map(LedgerEventEntry::into_storage)
                .collect(),
            records: records
                .into_iter()
                .map(LedgerRecordEntry::into_storage)
                .collect(),
            skipped_corrupt_lines,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_storage(readback: LedgerRead) -> Self {
        Self(readback)
    }

    pub(crate) fn storage(&self) -> &LedgerRead {
        &self.0
    }
}

pub struct RunLedgerSessionRequest {
    run_root: PathBuf,
    run_id: String,
    instance: String,
    header: LedgerSessionHeader,
}

impl RunLedgerSessionRequest {
    pub fn new(
        run_root: PathBuf,
        run_id: impl Into<String>,
        instance: impl Into<String>,
        header: LedgerSessionHeader,
    ) -> Self {
        Self {
            run_root,
            run_id: run_id.into(),
            instance: instance.into(),
            header,
        }
    }

    pub fn run_root(&self) -> &Path {
        &self.run_root
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn instance(&self) -> &str {
        &self.instance
    }

    pub fn header(&self) -> &LedgerSessionHeader {
        &self.header
    }
}

fn encode_json<T: Serialize>(value: &T, label: &str) -> LabResult<String> {
    serde_json::to_string(value)
        .map_err(|error| LabError::package_invalid(format!("failed to encode {label}: {error}")))
}

fn decode_json<T: DeserializeOwned>(encoded: &str, label: &str) -> LabResult<T> {
    serde_json::from_str(encoded)
        .map_err(|error| LabError::package_invalid(format!("failed to decode {label}: {error}")))
}
