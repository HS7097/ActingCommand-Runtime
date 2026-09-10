// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

/// The imported prefix is immutable; the completion itself is the next Ledger fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMigrationRecord {
    pub migration_id: String,
    pub source_sha256: String,
    pub backup_sha256: String,
    pub source_event_count: u64,
    pub source_first_sequence: u64,
    pub source_last_sequence: u64,
    pub source_head_sha256: String,
    pub imported_content_sha256: String,
    pub state_sha256: String,
    pub cutover_sequence: u64,
    pub phase: LedgerMigrationPhase,
    pub result: LedgerMigrationResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMigrationPhase {
    Cutover,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMigrationResult {
    Committed,
}

impl LedgerMigrationRecord {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let valid_hash = |value: &str, prefix: &str| {
            value.strip_prefix(prefix).is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        };
        if !valid_hash(&self.migration_id, "migration:")
            || [
                &self.source_sha256,
                &self.backup_sha256,
                &self.source_head_sha256,
                &self.imported_content_sha256,
                &self.state_sha256,
            ]
            .iter()
            .any(|value| !valid_hash(value, "sha256:"))
            || self.source_last_sequence != self.source_event_count
            || self.source_first_sequence != u64::from(self.source_event_count != 0)
            || self.source_last_sequence.checked_add(1) != Some(self.cutover_sequence)
        {
            return Err(SanitizationError::new(
                "invalid_ledger_migration",
                "ledger_migration",
            ));
        }
        Ok(())
    }
}
