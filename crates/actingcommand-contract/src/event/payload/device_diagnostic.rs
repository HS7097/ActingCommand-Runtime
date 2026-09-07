// SPDX-License-Identifier: AGPL-3.0-only

use super::{DiagnosticDetailRecord, EventId, SanitizationError, Sensitivity};
use crate::{OriginModule, OwnerEpoch};
use serde::{Deserialize, Serialize};

pub const DEVICE_DIAGNOSTIC_DETAIL_LIMIT: u8 = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceDiagnosticMode {
    #[default]
    Shadow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceDiagnosticConfig {
    pub mode: DeviceDiagnosticMode,
    pub detail_limit: u8,
}

impl DeviceDiagnosticConfig {
    pub const fn new(mode: DeviceDiagnosticMode) -> Self {
        Self {
            mode,
            detail_limit: DEVICE_DIAGNOSTIC_DETAIL_LIMIT,
        }
    }

    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.detail_limit != DEVICE_DIAGNOSTIC_DETAIL_LIMIT {
            return Err(invalid_budget());
        }
        Ok(())
    }
}

/// Identifies the exact typed field in the source event, not an occurrence identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceDiagnosticSourceField {
    Detail,
    CleanupDetail,
    LifecyclePrimaryDetail,
    LifecycleCleanupDetail,
    LifecycleCauseDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceDiagnosticSample {
    pub source_event_id: EventId,
    pub source_sequence: u64,
    pub source_module: OriginModule,
    pub source_field: DeviceDiagnosticSourceField,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<DiagnosticDetailRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceDiagnosticBudgetRecord {
    pub owner_epoch: OwnerEpoch,
    pub configuration: DeviceDiagnosticConfig,
    pub emitted_count: u8,
    pub folded_count: u64,
    pub declared_sensitivity: Sensitivity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<DeviceDiagnosticSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<DeviceDiagnosticSample>,
}

impl DeviceDiagnosticBudgetRecord {
    pub fn new(owner_epoch: OwnerEpoch, mode: DeviceDiagnosticMode) -> Self {
        Self {
            owner_epoch,
            configuration: DeviceDiagnosticConfig::new(mode),
            emitted_count: 0,
            folded_count: 0,
            declared_sensitivity: Sensitivity::Internal,
            first: None,
            last: None,
        }
    }

    pub(super) fn validate(&self, summary: bool) -> Result<(), SanitizationError> {
        self.configuration.validate()?;
        let total = u64::from(self.emitted_count)
            .checked_add(self.folded_count)
            .ok_or_else(invalid_budget)?;
        if self.emitted_count > DEVICE_DIAGNOSTIC_DETAIL_LIMIT
            || (self.folded_count > 0 && self.emitted_count != DEVICE_DIAGNOSTIC_DETAIL_LIMIT)
            || self.declared_sensitivity < Sensitivity::Internal
            || (!summary && (total == 0 || self.folded_count != 0))
        {
            return Err(invalid_budget());
        }
        match (&self.first, &self.last) {
            (None, None) if total == 0 => {}
            (Some(first), Some(last)) if total > 0 => {
                if first.source_sequence == 0
                    || first.source_sequence > last.source_sequence
                    || (total == 1 && first != last)
                {
                    return Err(invalid_budget());
                }
                for sample in [first, last] {
                    let detail = sample.detail.as_ref().ok_or_else(invalid_budget)?;
                    detail.validate()?;
                    if detail.declared_sensitivity() > self.declared_sensitivity {
                        return Err(invalid_budget());
                    }
                }
            }
            _ => return Err(invalid_budget()),
        }
        Ok(())
    }

    pub(super) fn public_summary(&self) -> Self {
        let mut summary = self.clone();
        for sample in [&mut summary.first, &mut summary.last]
            .into_iter()
            .flatten()
        {
            sample.detail = None;
        }
        summary
    }
}

fn invalid_budget() -> SanitizationError {
    SanitizationError::new(
        "invalid_device_diagnostic_budget",
        "device_diagnostic_budget",
    )
}
