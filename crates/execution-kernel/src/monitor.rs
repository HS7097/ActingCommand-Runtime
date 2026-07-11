// SPDX-License-Identifier: AGPL-3.0-only

//! Pure monitor decisions over already-classified observations.

pub use actingcommand_contract::{
    MonitorDecision, MonitorDecisionError, MonitorDecisionResult, MonitorDiagnosis,
    MonitorDisposition, MonitorObservation, MonitorPolicy, MonitorRecoveryKind,
};

pub fn decide_monitor(
    policy: MonitorPolicy,
    observation: &MonitorObservation,
) -> MonitorDecisionResult<MonitorDecision> {
    observation.validate()?;
    let (disposition, recovery) = match observation.diagnosis() {
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
    MonitorDecision::new(observation.diagnosis(), disposition, recovery)
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
