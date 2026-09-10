// SPDX-License-Identifier: AGPL-3.0-only

use crate::{EventId, LifecycleNativeDetail, OwnerEpoch, SanitizationError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityPurpose {
    Installation,
    State,
    Artifact,
    ArtifactStaging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityThresholds {
    pub hard_bytes: u64,
    pub soft_bytes: u64,
}

impl Default for CapacityThresholds {
    fn default() -> Self {
        Self {
            hard_bytes: 512 * 1024 * 1024,
            soft_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

impl CapacityThresholds {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.hard_bytes == 0 || self.hard_bytes >= self.soft_bytes {
            return Err(SanitizationError::new(
                "invalid_capacity_thresholds",
                "capacity",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityState {
    Sufficient,
    SoftPressure,
    HardPressure,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityNativeCause {
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_os_error: Option<i32>,
    pub detail: LifecycleNativeDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityVolumeSample {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_id: Option<String>,
    pub purposes: Vec<CapacityPurpose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_bytes: Option<u64>,
    pub state: CapacityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<CapacityNativeCause>,
}

/// B3 observation time is captured before the OS query, never at queue/append time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceCapacitySample {
    pub owner_epoch: OwnerEpoch,
    pub observed_at_unix_ms: u64,
    pub observed_at_monotonic_ms: u64,
    pub freshness_ms: u64,
    pub thresholds: CapacityThresholds,
    pub volumes: Vec<CapacityVolumeSample>,
}

impl PerformanceCapacitySample {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        self.thresholds.validate()?;
        let invalid = || SanitizationError::new("invalid_capacity_sample", "capacity");
        if self.observed_at_unix_ms == 0
            || !(2_000..=10_000).contains(&self.freshness_ms)
            || self.volumes.is_empty()
            || self.volumes.len() > 4
        {
            return Err(invalid());
        }
        let mut purposes = BTreeSet::new();
        let mut volumes = BTreeSet::new();
        for volume in &self.volumes {
            if volume.purposes.is_empty()
                || volume
                    .purposes
                    .iter()
                    .any(|purpose| !purposes.insert(*purpose))
            {
                return Err(invalid());
            }
            if let Some(id) = &volume.volume_id
                && (id.is_empty()
                    || id.len() > 128
                    || id.chars().any(char::is_control)
                    || !volumes.insert(id))
            {
                return Err(invalid());
            }
            match (volume.available_bytes, volume.state, &volume.cause) {
                (Some(bytes), state, None) if volume.volume_id.is_some() => {
                    let expected = if bytes < self.thresholds.hard_bytes {
                        CapacityState::HardPressure
                    } else if bytes < self.thresholds.soft_bytes {
                        CapacityState::SoftPressure
                    } else {
                        CapacityState::Sufficient
                    };
                    if state != expected {
                        return Err(invalid());
                    }
                }
                (None, CapacityState::Unknown, Some(cause)) => {
                    if cause.operation.is_empty()
                        || cause.operation.len() > 128
                        || cause.operation.chars().any(char::is_control)
                    {
                        return Err(invalid());
                    }
                    cause.detail.validate()?;
                }
                _ => return Err(invalid()),
            }
        }
        if purposes.len() != 4 {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityFactReference {
    pub event_id: EventId,
    pub sequence: u64,
    pub owner_epoch: OwnerEpoch,
    pub observed_at_unix_ms: u64,
    pub observed_at_monotonic_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityAdmissionOutcome {
    Allowed,
    SoftPressure,
    HardPressure,
    Unknown,
    RequiredBytesOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityAdmissionReason {
    FreshSample,
    NoCommittedFact,
    OwnerChanged,
    OutsideFreshness,
    BindingChanged,
    SampleUnavailable,
    HardThreshold,
    KnownBytesOverflow,
}

impl CapacityAdmissionOutcome {
    pub const fn allows(self) -> bool {
        matches!(self, Self::Allowed | Self::SoftPressure)
    }
}

/// References committed facts; this result neither reserves space nor diagnoses an I/O failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityDecision {
    pub owner_epoch: OwnerEpoch,
    pub decided_at_unix_ms: u64,
    pub decided_at_monotonic_ms: u64,
    pub requested_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_volume: Option<String>,
    pub outcome: CapacityAdmissionOutcome,
    pub reason: CapacityAdmissionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact: Option<CapacityFactReference>,
}

impl CapacityDecision {
    pub fn validate(&self) -> Result<(), SanitizationError> {
        if self.decided_at_unix_ms == 0
            || self.target_volume.as_ref().is_some_and(|id| {
                id.is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            })
            || self
                .fact
                .as_ref()
                .is_some_and(|fact| fact.sequence == 0 || fact.observed_at_unix_ms == 0)
            || (self.outcome.allows()
                && self.fact.as_ref().is_none_or(|fact| {
                    fact.owner_epoch != self.owner_epoch
                        || fact.observed_at_unix_ms > self.decided_at_unix_ms
                        || fact.observed_at_monotonic_ms > self.decided_at_monotonic_ms
                }))
        {
            return Err(SanitizationError::new(
                "invalid_capacity_decision",
                "capacity",
            ));
        }
        Ok(())
    }
}
