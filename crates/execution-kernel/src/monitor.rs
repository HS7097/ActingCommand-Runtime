// SPDX-License-Identifier: AGPL-3.0-only

//! Pure monitor decisions over already-classified observations.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

const MAX_MONITOR_PAGE_BYTES: usize = 256;

pub type MonitorDecisionResult<T> = Result<T, MonitorDecisionError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorDecisionError {
    code: &'static str,
}

impl MonitorDecisionError {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for MonitorDecisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "monitor decision error: {}", self.code)
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
        validate_page(&self.expected_page)?;
        if let Some(current_page) = &self.current_page {
            validate_page(current_page)?;
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
    fn new(
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

pub fn decide_monitor(
    policy: MonitorPolicy,
    observation: &MonitorObservation,
) -> MonitorDecisionResult<MonitorDecision> {
    observation.validate()?;
    let (disposition, recovery) = match observation.diagnosis {
        MonitorDiagnosis::Healthy => (MonitorDisposition::Healthy, None),
        MonitorDiagnosis::CaptureUnavailable => (MonitorDisposition::Blocked, None),
        _ if !policy.recovery_enabled() => (MonitorDisposition::ObserveOnly, None),
        MonitorDiagnosis::Standby => (
            MonitorDisposition::RecoveryRequested,
            Some(MonitorRecoveryKind::WakeStandby),
        ),
        MonitorDiagnosis::UnexpectedPage => (
            MonitorDisposition::RecoveryRequested,
            Some(MonitorRecoveryKind::ReturnToExpectedPage),
        ),
        MonitorDiagnosis::CaptureStaleSuspected => (
            MonitorDisposition::RecoveryRequested,
            Some(MonitorRecoveryKind::RefreshCapture),
        ),
    };
    MonitorDecision::new(observation.diagnosis, disposition, recovery)
}

fn validate_page(value: &str) -> MonitorDecisionResult<()> {
    if value.is_empty()
        || value.len() > MAX_MONITOR_PAGE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(MonitorDecisionError::new("invalid_monitor_page"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_decisions_are_closed_and_policy_driven() {
        let healthy = observation(MonitorDiagnosis::Healthy, Some("home"));
        assert_eq!(
            decide_monitor(MonitorPolicy::new(true), &healthy)
                .expect("healthy decision")
                .disposition(),
            MonitorDisposition::Healthy
        );

        let cases = [
            (
                MonitorDiagnosis::Standby,
                None,
                MonitorRecoveryKind::WakeStandby,
            ),
            (
                MonitorDiagnosis::UnexpectedPage,
                Some("campaign"),
                MonitorRecoveryKind::ReturnToExpectedPage,
            ),
            (
                MonitorDiagnosis::CaptureStaleSuspected,
                None,
                MonitorRecoveryKind::RefreshCapture,
            ),
        ];
        for (diagnosis, current, expected_recovery) in cases {
            let observation = observation(diagnosis, current);
            let read_only = decide_monitor(MonitorPolicy::new(false), &observation)
                .expect("read-only decision");
            assert_eq!(read_only.disposition(), MonitorDisposition::ObserveOnly);
            assert_eq!(read_only.recovery(), None);

            let recovery =
                decide_monitor(MonitorPolicy::new(true), &observation).expect("recovery decision");
            assert_eq!(
                recovery.disposition(),
                MonitorDisposition::RecoveryRequested
            );
            assert_eq!(recovery.recovery(), Some(expected_recovery));
        }

        let unavailable = observation(MonitorDiagnosis::CaptureUnavailable, None);
        let blocked = decide_monitor(MonitorPolicy::new(true), &unavailable)
            .expect("capture unavailable decision");
        assert_eq!(blocked.disposition(), MonitorDisposition::Blocked);
        assert_eq!(blocked.recovery(), None);
    }

    #[test]
    fn monitor_models_reject_incoherent_or_unknown_state() {
        assert_eq!(
            MonitorObservation::new(MonitorDiagnosis::Healthy, "home", Some("campaign".into()))
                .expect_err("healthy page mismatch")
                .code(),
            "invalid_monitor_observation"
        );
        assert_eq!(
            MonitorObservation::new(MonitorDiagnosis::UnexpectedPage, "home", None)
                .expect_err("unexpected page without current page")
                .code(),
            "invalid_monitor_observation"
        );

        let mut value = serde_json::to_value(observation(
            MonitorDiagnosis::UnexpectedPage,
            Some("campaign"),
        ))
        .expect("observation JSON");
        value["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MonitorObservation>(value).is_err());

        let invalid = serde_json::json!({
            "diagnosis": "healthy",
            "disposition": "recovery_requested",
            "recovery": "wake_standby"
        });
        let invalid: MonitorDecision =
            serde_json::from_value(invalid).expect("structurally valid decision");
        assert_eq!(
            invalid.validate().expect_err("incoherent decision").code(),
            "invalid_monitor_decision"
        );
    }

    fn observation(diagnosis: MonitorDiagnosis, current: Option<&str>) -> MonitorObservation {
        MonitorObservation::new(diagnosis, "home", current.map(str::to_string))
            .expect("monitor observation")
    }
}
