// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_ledger::LedgerRecord;

#[derive(Debug, Clone)]
pub struct LedgerRecordEntry(LedgerRecord);

impl LedgerRecordEntry {
    pub(crate) fn from_storage(record: LedgerRecord) -> Self {
        Self(record)
    }

    #[cfg(test)]
    pub(crate) fn storage(&self) -> &LedgerRecord {
        &self.0
    }
}
