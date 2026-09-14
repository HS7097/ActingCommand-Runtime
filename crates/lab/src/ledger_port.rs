// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{LabError, LabResult};
use actingcommand_ledger::LedgerRecord;
use serde::{Serialize, de::DeserializeOwned};

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

    #[cfg(test)]
    pub(crate) fn storage(&self) -> &LedgerRecord {
        &self.0
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
