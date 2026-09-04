use crate::{CliOutcome, env_detection};
use actingcommand_lab::SemanticLedgerContext;
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn env_resolved_json(values: &[env_detection::ResolvedEnvValue]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| {
                json!({
                    "key": value.key,
                    "value": value.value,
                    "confidence": value.confidence,
                    "source": value.source,
                    "detector_id": value.detector_id,
                    "source_result": value.source_result
                })
            })
            .collect(),
    )
}

pub(crate) fn attach_env_resolved(payload: &mut Value, values: &[env_detection::ResolvedEnvValue]) {
    if values.is_empty() {
        return;
    }
    payload["env_resolved"] = env_resolved_json(values);
}

pub(crate) fn env_needs_detection_json(
    command: &str,
    reason: &str,
    subject: &str,
    values: &[env_detection::ResolvedEnvValue],
) -> Option<Value> {
    if values.is_empty() {
        return None;
    }
    let detector_ids = values
        .iter()
        .map(|value| value.detector_id.clone())
        .collect::<BTreeSet<_>>();
    Some(json!({
        "status": "needs_detection",
        "reason": reason,
        "command": command,
        "subject": subject,
        "detector_ids": detector_ids.into_iter().collect::<Vec<_>>(),
        "keys": env_resolved_json(values),
        "recommended_action": "run_detect"
    }))
}

pub(crate) fn record_env_needs_detection(
    ledger: &mut SemanticLedgerContext,
    command: &str,
    reason: &str,
    subject: &str,
    values: &[env_detection::ResolvedEnvValue],
) -> CliOutcome<()> {
    if let Some(needs_detection) = env_needs_detection_json(command, reason, subject, values) {
        ledger.record_drive(json!({
            "stage": "env_needs_detection",
            "command": command,
            "needs_detection": needs_detection
        }))?;
    }
    Ok(())
}

pub(crate) fn record_env_resolved(
    ledger: &mut SemanticLedgerContext,
    command: &str,
    values: &[env_detection::ResolvedEnvValue],
) -> CliOutcome<()> {
    if values.is_empty() {
        return Ok(());
    }
    ledger.record_drive(json!({
        "stage": "env_resolved",
        "command": command,
        "keys": env_resolved_json(values)
    }))?;
    Ok(())
}
