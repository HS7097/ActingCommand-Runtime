// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CorrelationId, InstanceId, LeaseId, OwnerEpoch, OwnerResourceDisposition, RequestId, RunId,
};
use crate::{SanitizationError, TerminalEvent};
use serde::{Deserialize, Serialize};

pub const OWNER_JOURNAL_SCHEMA: &str = "actingcommand.runtime-owner.v2";
pub const OWNER_JOURNAL_LIMIT: u64 = 4 * 1024 * 1024;

/// The original retention closure identity. Run scopes deliberately omit request links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorEpochScope {
    pub owner: OwnerEpoch,
    pub instance: InstanceId,
    pub run: Option<RunId>,
    pub lease: Option<LeaseId>,
    pub request: Option<RequestId>,
    pub correlation: Option<CorrelationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerClosedRevision {
    pub revision: u64,
    pub active: bool,
    pub closed_at_unix_ms: Option<u64>,
    pub disposition: OwnerResourceDisposition,
}

/// A suffix of the complete, validated native journal, starting at its final positive
/// close revision for this epoch. Only the locked native reader can import this evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerEpochCloseEvidence {
    pub schema_version: String,
    pub subject: OwnerEpoch,
    pub pid: u32,
    pub started_at_unix_ms: u64,
    pub first_revision: u64,
    pub confirmed_revision: u64,
    pub final_revision: u64,
    pub journal_through_revision: u64,
    pub journal_bytes: u64,
    pub journal_sha256: String,
    pub suffix: Vec<OwnerClosedRevision>,
}

impl OwnerEpochCloseEvidence {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        let invalid =
            || SanitizationError::new("invalid_owner_close_evidence", "prior_epoch_close");
        if self.schema_version != OWNER_JOURNAL_SCHEMA
            || self.pid == 0
            || self.started_at_unix_ms == 0
            || self.first_revision == 0
            || self.first_revision > self.confirmed_revision
            || self.confirmed_revision > self.final_revision
            || self.final_revision > self.journal_through_revision
            || self.journal_bytes == 0
            || self.journal_bytes > OWNER_JOURNAL_LIMIT
            || self.journal_sha256.len() != 64
            || !self
                .journal_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.suffix.len() as u64 != self.final_revision - self.confirmed_revision + 1
            || self.suffix.len() as u64 > self.journal_bytes
        {
            return Err(invalid());
        }
        let mut inactive = false;
        for (offset, record) in self.suffix.iter().enumerate() {
            if record.revision != self.confirmed_revision + offset as u64
                || record.active == record.closed_at_unix_ms.is_some()
                || record
                    .closed_at_unix_ms
                    .is_some_and(|time| time < self.started_at_unix_ms)
                || inactive && record.active
                || if record.active {
                    record.disposition != OwnerResourceDisposition::ConfirmedClosed
                } else {
                    record.disposition != OwnerResourceDisposition::None
                }
                || offset == 0
                    && (!record.active
                        || record.disposition != OwnerResourceDisposition::ConfirmedClosed)
            {
                return Err(invalid());
            }
            inactive |= !record.active;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorEpochOwnerImport {
    pub evidence: OwnerEpochCloseEvidence,
    /// The authenticated prefix at first import, never refreshed by a later startup.
    pub through_sequence: u64,
    pub scope_upper_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriorEpochScopeClose {
    pub subject: PriorEpochScope,
    pub proof: TerminalEvent,
    pub scope_source: TerminalEvent,
    pub through_sequence: u64,
    pub scope_upper_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "record",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PriorEpochCloseFact {
    OwnerImported(PriorEpochOwnerImport),
    ScopeClosed(PriorEpochScopeClose),
}

impl PriorEpochCloseFact {
    pub fn public_summary(&self, writer: OwnerEpoch) -> PriorEpochCloseSummary {
        match self {
            Self::OwnerImported(record) => PriorEpochCloseSummary::OwnerImported {
                writer,
                subject: record.evidence.subject,
                confirmed_revision: record.evidence.confirmed_revision,
                final_revision: record.evidence.final_revision,
                journal_through_revision: record.evidence.journal_through_revision,
                journal_sha256: record.evidence.journal_sha256.clone(),
                through_sequence: record.through_sequence,
                scope_upper_sequence: record.scope_upper_sequence,
            },
            Self::ScopeClosed(record) => PriorEpochCloseSummary::ScopeClosed {
                writer,
                record: record.clone(),
            },
        }
    }

    pub fn validate(&self, writer: OwnerEpoch) -> Result<(), SanitizationError> {
        let invalid = || SanitizationError::new("invalid_prior_epoch_close", "prior_epoch_close");
        let (subject, through, upper) = match self {
            Self::OwnerImported(record) => {
                record.evidence.validate()?;
                (
                    record.evidence.subject,
                    record.through_sequence,
                    record.scope_upper_sequence,
                )
            }
            Self::ScopeClosed(record) => {
                let scope = &record.subject;
                if (scope.run.is_some() && (scope.request.is_some() || scope.correlation.is_some()))
                    || (scope.run.is_none()
                        && (scope.request.is_none() || scope.correlation.is_none()))
                    || record.proof.sequence <= record.through_sequence
                    || record.scope_source.sequence == 0
                    || record.scope_source.sequence > record.scope_upper_sequence
                {
                    return Err(invalid());
                }
                (
                    scope.owner,
                    record.through_sequence,
                    record.scope_upper_sequence,
                )
            }
        };
        if writer == subject || upper == 0 || upper > through {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PriorEpochCloseSummary {
    OwnerImported {
        writer: OwnerEpoch,
        subject: OwnerEpoch,
        confirmed_revision: u64,
        final_revision: u64,
        journal_through_revision: u64,
        journal_sha256: String,
        through_sequence: u64,
        scope_upper_sequence: u64,
    },
    ScopeClosed {
        writer: OwnerEpoch,
        record: PriorEpochScopeClose,
    },
}
