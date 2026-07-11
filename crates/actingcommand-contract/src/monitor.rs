// SPDX-License-Identifier: AGPL-3.0-only

//! Typed monitor vocabulary shared by Runtime and its disposable clients.

use crate::{
    ArtifactLinksDraft, EventLinksDraft, IdentifierIssuanceError, IdentifierIssuer,
    IssuedCorrelationId, IssuedFrameId, IssuedInstanceId, IssuedRecognitionId, IssuedRunId,
    IssuedTaskId, MAX_INSTANCE_ALIAS_BYTES, OwnerEpoch, RuntimeErrorCode,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

pub const MIN_RUNTIME_MONITOR_INTERVAL_MS: u64 = 100;
pub const MAX_RUNTIME_MONITOR_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_MONITOR_PAGE_BYTES: usize = 256;

pub type MonitorDecisionResult<T> = Result<T, MonitorDecisionError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorDecisionError {
    code: &'static str,
}

impl MonitorDecisionError {
    pub(crate) const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for MonitorDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "monitor contract error: {}", self.code)
    }
}

impl Error for MonitorDecisionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorDiagnosis {
    Healthy,
    Standby,
    UnexpectedPage,
    CaptureStaleSuspected,
    CaptureUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorObservation {
    diagnosis: MonitorDiagnosis,
    expected_page: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_page: Option<String>,
}

impl MonitorObservation {
    pub fn new(
        diagnosis: MonitorDiagnosis,
        expected_page: impl Into<String>,
        current_page: Option<String>,
    ) -> MonitorDecisionResult<Self> {
        let observation = Self {
            diagnosis,
            expected_page: expected_page.into(),
            current_page,
        };
        observation.validate()?;
        Ok(observation)
    }

    pub fn validate(&self) -> MonitorDecisionResult<()> {
        validate_text(
            &self.expected_page,
            MAX_MONITOR_PAGE_BYTES,
            "invalid_monitor_page",
        )?;
        if let Some(current_page) = &self.current_page {
            validate_text(current_page, MAX_MONITOR_PAGE_BYTES, "invalid_monitor_page")?;
        }
        let valid = match self.diagnosis {
            MonitorDiagnosis::Healthy => self.current_page.as_deref() == Some(&self.expected_page),
            MonitorDiagnosis::UnexpectedPage => self
                .current_page
                .as_deref()
                .is_some_and(|current| current != self.expected_page),
            MonitorDiagnosis::Standby
            | MonitorDiagnosis::CaptureStaleSuspected
            | MonitorDiagnosis::CaptureUnavailable => self.current_page.is_none(),
        };
        if !valid {
            return Err(MonitorDecisionError::new("invalid_monitor_observation"));
        }
        Ok(())
    }

    pub const fn diagnosis(&self) -> MonitorDiagnosis {
        self.diagnosis
    }

    pub fn expected_page(&self) -> &str {
        &self.expected_page
    }

    pub fn current_page(&self) -> Option<&str> {
        self.current_page.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorPolicy {
    recovery_enabled: bool,
}

impl MonitorPolicy {
    pub const fn new(recovery_enabled: bool) -> Self {
        Self { recovery_enabled }
    }

    pub const fn recovery_enabled(self) -> bool {
        self.recovery_enabled
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorDisposition {
    Healthy,
    ObserveOnly,
    RecoveryRequested,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorRecoveryKind {
    WakeStandby,
    ReturnToExpectedPage,
    RefreshCapture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorDecision {
    diagnosis: MonitorDiagnosis,
    disposition: MonitorDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<MonitorRecoveryKind>,
}

impl MonitorDecision {
    pub fn new(
        diagnosis: MonitorDiagnosis,
        disposition: MonitorDisposition,
        recovery: Option<MonitorRecoveryKind>,
    ) -> MonitorDecisionResult<Self> {
        let decision = Self {
            diagnosis,
            disposition,
            recovery,
        };
        decision.validate()?;
        Ok(decision)
    }

    pub fn validate(&self) -> MonitorDecisionResult<()> {
        let valid = matches!(
            (self.diagnosis, self.disposition, self.recovery),
            (MonitorDiagnosis::Healthy, MonitorDisposition::Healthy, None)
                | (
                    MonitorDiagnosis::Standby
                        | MonitorDiagnosis::UnexpectedPage
                        | MonitorDiagnosis::CaptureStaleSuspected,
                    MonitorDisposition::ObserveOnly,
                    None
                )
                | (
                    MonitorDiagnosis::Standby,
                    MonitorDisposition::RecoveryRequested,
                    Some(MonitorRecoveryKind::WakeStandby)
                )
                | (
                    MonitorDiagnosis::UnexpectedPage,
                    MonitorDisposition::RecoveryRequested,
                    Some(MonitorRecoveryKind::ReturnToExpectedPage)
                )
                | (
                    MonitorDiagnosis::CaptureStaleSuspected,
                    MonitorDisposition::RecoveryRequested,
                    Some(MonitorRecoveryKind::RefreshCapture)
                )
                | (
                    MonitorDiagnosis::CaptureUnavailable,
                    MonitorDisposition::Blocked,
                    None
                )
        );
        if !valid {
            return Err(MonitorDecisionError::new("invalid_monitor_decision"));
        }
        Ok(())
    }

    pub const fn diagnosis(&self) -> MonitorDiagnosis {
        self.diagnosis
    }

    pub const fn disposition(&self) -> MonitorDisposition {
        self.disposition
    }

    pub const fn recovery(&self) -> Option<MonitorRecoveryKind> {
        self.recovery
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorPolicy {
    interval_ms: u64,
    expected_page: String,
    recovery_enabled: bool,
}

impl RuntimeMonitorPolicy {
    pub fn new(
        interval_ms: u64,
        expected_page: impl Into<String>,
        recovery_enabled: bool,
    ) -> MonitorDecisionResult<Self> {
        let policy = Self {
            interval_ms,
            expected_page: expected_page.into(),
            recovery_enabled,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> MonitorDecisionResult<()> {
        if !(MIN_RUNTIME_MONITOR_INTERVAL_MS..=MAX_RUNTIME_MONITOR_INTERVAL_MS)
            .contains(&self.interval_ms)
        {
            return Err(MonitorDecisionError::new(
                "invalid_runtime_monitor_interval",
            ));
        }
        validate_text(
            &self.expected_page,
            MAX_MONITOR_PAGE_BYTES,
            "invalid_monitor_page",
        )
    }

    pub const fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    pub fn expected_page(&self) -> &str {
        &self.expected_page
    }

    pub const fn recovery_enabled(&self) -> bool {
        self.recovery_enabled
    }

    pub const fn decision_policy(&self) -> MonitorPolicy {
        MonitorPolicy::new(self.recovery_enabled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorState {
    next_due_unix_ms: u64,
    run_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_started_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_completed_at_unix_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_decision: Option<MonitorDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<RuntimeErrorCode>,
}

impl RuntimeMonitorState {
    pub fn scheduled(next_due_unix_ms: u64) -> MonitorDecisionResult<Self> {
        let state = Self {
            next_due_unix_ms,
            run_count: 0,
            last_started_at_unix_ms: None,
            last_completed_at_unix_ms: None,
            last_decision: None,
            last_error: None,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(&self) -> MonitorDecisionResult<()> {
        if self.next_due_unix_ms == 0 {
            return Err(MonitorDecisionError::new("invalid_runtime_monitor_state"));
        }
        if let Some(decision) = &self.last_decision {
            decision.validate()?;
        }
        let no_previous_run = self.run_count == 0
            && self.last_started_at_unix_ms.is_none()
            && self.last_completed_at_unix_ms.is_none()
            && self.last_decision.is_none()
            && self.last_error.is_none();
        let completed_run = self.run_count > 0
            && self
                .last_started_at_unix_ms
                .zip(self.last_completed_at_unix_ms)
                .is_some_and(|(started, completed)| started > 0 && completed >= started)
            && (self.last_decision.is_some() ^ self.last_error.is_some());
        if !no_previous_run && !completed_run {
            return Err(MonitorDecisionError::new("invalid_runtime_monitor_state"));
        }
        Ok(())
    }

    pub fn completed(
        &self,
        interval_ms: u64,
        started_at_unix_ms: u64,
        completed_at_unix_ms: u64,
        decision: MonitorDecision,
    ) -> MonitorDecisionResult<Self> {
        decision.validate()?;
        self.advance(
            interval_ms,
            started_at_unix_ms,
            completed_at_unix_ms,
            Some(decision),
            None,
        )
    }

    pub fn failed(
        &self,
        interval_ms: u64,
        started_at_unix_ms: u64,
        completed_at_unix_ms: u64,
        error: RuntimeErrorCode,
    ) -> MonitorDecisionResult<Self> {
        self.advance(
            interval_ms,
            started_at_unix_ms,
            completed_at_unix_ms,
            None,
            Some(error),
        )
    }

    pub const fn next_due_unix_ms(&self) -> u64 {
        self.next_due_unix_ms
    }

    pub const fn run_count(&self) -> u64 {
        self.run_count
    }

    pub const fn last_started_at_unix_ms(&self) -> Option<u64> {
        self.last_started_at_unix_ms
    }

    pub const fn last_completed_at_unix_ms(&self) -> Option<u64> {
        self.last_completed_at_unix_ms
    }

    pub const fn last_decision(&self) -> Option<&MonitorDecision> {
        self.last_decision.as_ref()
    }

    pub const fn last_error(&self) -> Option<RuntimeErrorCode> {
        self.last_error
    }

    fn advance(
        &self,
        interval_ms: u64,
        started_at_unix_ms: u64,
        completed_at_unix_ms: u64,
        last_decision: Option<MonitorDecision>,
        last_error: Option<RuntimeErrorCode>,
    ) -> MonitorDecisionResult<Self> {
        self.validate()?;
        if !(MIN_RUNTIME_MONITOR_INTERVAL_MS..=MAX_RUNTIME_MONITOR_INTERVAL_MS)
            .contains(&interval_ms)
            || started_at_unix_ms < self.next_due_unix_ms
            || completed_at_unix_ms < started_at_unix_ms
        {
            return Err(MonitorDecisionError::new("invalid_runtime_monitor_state"));
        }
        let state = Self {
            next_due_unix_ms: completed_at_unix_ms
                .checked_add(interval_ms)
                .ok_or_else(|| MonitorDecisionError::new("runtime_monitor_time_overflow"))?,
            run_count: self
                .run_count
                .checked_add(1)
                .ok_or_else(|| MonitorDecisionError::new("runtime_monitor_count_overflow"))?,
            last_started_at_unix_ms: Some(started_at_unix_ms),
            last_completed_at_unix_ms: Some(completed_at_unix_ms),
            last_decision,
            last_error,
        };
        state.validate()?;
        Ok(state)
    }
}

/// Producer capability for one resident monitor probe and its artifact correlations.
#[derive(Clone, Copy)]
pub struct IssuedMonitorProbe {
    instance_id: IssuedInstanceId,
    correlation_id: IssuedCorrelationId,
    task_id: IssuedTaskId,
    run_id: IssuedRunId,
    frame_id: IssuedFrameId,
    recognition_id: IssuedRecognitionId,
}

impl IssuedMonitorProbe {
    pub fn event_links(&self) -> EventLinksDraft {
        EventLinksDraft::default()
            .with_instance_id(self.instance_id)
            .with_correlation_id(self.correlation_id)
            .with_task_id(self.task_id)
            .with_run_id(self.run_id)
            .with_frame_id(self.frame_id)
            .with_recognition_id(self.recognition_id)
    }

    pub fn artifact_links(&self) -> ArtifactLinksDraft {
        ArtifactLinksDraft::default()
            .with_run_id(self.run_id)
            .with_frame_id(self.frame_id)
            .with_correlation_id(self.correlation_id)
    }
}

impl IdentifierIssuer {
    pub fn issue_monitor_probe(
        &self,
        instance_id: crate::InstanceId,
    ) -> Result<IssuedMonitorProbe, IdentifierIssuanceError> {
        Ok(IssuedMonitorProbe {
            instance_id: IssuedInstanceId::from_verified_transport(instance_id),
            correlation_id: self.mint_correlation_id()?,
            task_id: self.mint_task_id()?,
            run_id: self.mint_run_id()?,
            frame_id: self.mint_frame_id()?,
            recognition_id: self.mint_recognition_id()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorInstanceStatus {
    instance_alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<RuntimeMonitorPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<RuntimeMonitorState>,
}

impl RuntimeMonitorInstanceStatus {
    pub fn configured(
        instance_alias: impl Into<String>,
        policy: RuntimeMonitorPolicy,
        state: RuntimeMonitorState,
    ) -> MonitorDecisionResult<Self> {
        Self::new(instance_alias.into(), Some(policy), Some(state))
    }

    pub fn unconfigured(instance_alias: impl Into<String>) -> MonitorDecisionResult<Self> {
        Self::new(instance_alias.into(), None, None)
    }

    fn new(
        instance_alias: String,
        policy: Option<RuntimeMonitorPolicy>,
        state: Option<RuntimeMonitorState>,
    ) -> MonitorDecisionResult<Self> {
        let status = Self {
            instance_alias,
            policy,
            state,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> MonitorDecisionResult<()> {
        validate_text(
            &self.instance_alias,
            MAX_INSTANCE_ALIAS_BYTES,
            "invalid_instance_alias",
        )?;
        match (&self.policy, &self.state) {
            (Some(policy), Some(state)) => {
                policy.validate()?;
                state.validate()
            }
            (None, None) => Ok(()),
            _ => Err(MonitorDecisionError::new(
                "invalid_runtime_monitor_instance_status",
            )),
        }
    }

    pub fn instance_alias(&self) -> &str {
        &self.instance_alias
    }

    pub const fn policy(&self) -> Option<&RuntimeMonitorPolicy> {
        self.policy.as_ref()
    }

    pub const fn state(&self) -> Option<&RuntimeMonitorState> {
        self.state.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMonitorRegistryStatus {
    owner_epoch: OwnerEpoch,
    instances: Vec<RuntimeMonitorInstanceStatus>,
}

impl RuntimeMonitorRegistryStatus {
    pub fn new(
        owner_epoch: OwnerEpoch,
        mut instances: Vec<RuntimeMonitorInstanceStatus>,
    ) -> MonitorDecisionResult<Self> {
        instances.sort_by(|left, right| left.instance_alias.cmp(&right.instance_alias));
        let status = Self {
            owner_epoch,
            instances,
        };
        status.validate()?;
        Ok(status)
    }

    pub fn validate(&self) -> MonitorDecisionResult<()> {
        let mut aliases = BTreeSet::new();
        let mut previous_alias = None;
        for instance in &self.instances {
            instance.validate()?;
            if !aliases.insert(instance.instance_alias.as_str())
                || previous_alias.is_some_and(|previous| previous >= instance.instance_alias())
            {
                return Err(MonitorDecisionError::new(
                    "invalid_runtime_monitor_registry",
                ));
            }
            previous_alias = Some(instance.instance_alias());
        }
        Ok(())
    }

    pub const fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub fn instances(&self) -> &[RuntimeMonitorInstanceStatus] {
        &self.instances
    }
}

fn validate_text(value: &str, max_bytes: usize, code: &'static str) -> MonitorDecisionResult<()> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(MonitorDecisionError::new(code));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IdentifierIssuer;

    #[test]
    fn runtime_monitor_policy_state_and_registry_are_strict() {
        let issuer = IdentifierIssuer::new().expect("issuer");
        let owner_epoch = *issuer.mint_owner_epoch().expect("owner epoch").transport();
        let policy = RuntimeMonitorPolicy::new(1_000, "home", true).expect("policy");
        let state = RuntimeMonitorState::scheduled(10).expect("state");
        let configured =
            RuntimeMonitorInstanceStatus::configured("ak.cn", policy, state).expect("configured");
        let unconfigured =
            RuntimeMonitorInstanceStatus::unconfigured("ba.jp").expect("unconfigured");
        let status = RuntimeMonitorRegistryStatus::new(
            owner_epoch,
            vec![unconfigured.clone(), configured.clone()],
        )
        .expect("registry");

        assert_eq!(status.instances()[0], configured);
        assert_eq!(status.instances()[1], unconfigured);
        assert!(status.instances()[0].policy().is_some());
        assert!(status.instances()[1].policy().is_none());

        let mut value = serde_json::to_value(&status).expect("status JSON");
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RuntimeMonitorRegistryStatus>(value).is_err());
        assert_eq!(
            RuntimeMonitorPolicy::new(0, "home", false)
                .expect_err("zero interval")
                .code(),
            "invalid_runtime_monitor_interval"
        );
    }

    #[test]
    fn runtime_monitor_state_advances_only_from_due_completed_runs() {
        let scheduled = RuntimeMonitorState::scheduled(100).expect("scheduled state");
        let decision =
            MonitorDecision::new(MonitorDiagnosis::Healthy, MonitorDisposition::Healthy, None)
                .expect("decision");
        let completed = scheduled
            .completed(1_000, 100, 125, decision.clone())
            .expect("completed state");
        assert_eq!(completed.next_due_unix_ms(), 1_125);
        assert_eq!(completed.run_count(), 1);
        assert_eq!(completed.last_decision(), Some(&decision));
        assert_eq!(completed.last_error(), None);

        let failed = completed
            .failed(1_000, 1_125, 1_130, RuntimeErrorCode::CaptureFailed)
            .expect("failed state");
        assert_eq!(failed.run_count(), 2);
        assert_eq!(failed.last_decision(), None);
        assert_eq!(failed.last_error(), Some(RuntimeErrorCode::CaptureFailed));

        assert_eq!(
            scheduled
                .completed(1_000, 99, 100, decision)
                .expect_err("early run")
                .code(),
            "invalid_runtime_monitor_state"
        );
    }
}
