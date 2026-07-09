// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{LabError, LabResult, LedgerProjection};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub fn project_semantic_payload<T>(payload: T, ledger: LedgerProjection) -> LabResult<T>
where
    T: Serialize + DeserializeOwned,
{
    let mut projected = serde_json::to_value(payload).map_err(|error| {
        LabError::device(format!(
            "failed to serialize semantic ledger projection: {error}"
        ))
    })?;
    let Some(object) = projected.as_object_mut() else {
        return Err(LabError::device(
            "semantic ledger projection returned non-object",
        ));
    };
    let ledger_value = serde_json::to_value(&ledger).map_err(|error| {
        LabError::device(format!("failed to serialize ledger summary: {error}"))
    })?;
    object.insert("ledger".to_string(), ledger_value);
    object.insert(
        "projection_source".to_string(),
        json!({
            "kind": "runtime_ledger",
            "record_kind": "receipt",
            "path": ledger.path,
            "req_id": object.get("req_id").cloned().unwrap_or(Value::Null)
        }),
    );
    serde_json::from_value(projected).map_err(|error| {
        LabError::device(format!(
            "failed to deserialize semantic ledger projection: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projection_preserves_current_flat_payload_shape() {
        let projected = project_semantic_payload(
            json!({"req_id": "req-1", "status": "ok"}),
            LedgerProjection::written("runs/ledger.jsonl"),
        )
        .expect("projection");

        assert_eq!(projected["status"], "ok");
        assert_eq!(projected["ledger"]["written"], true);
        assert_eq!(projected["projection_source"]["req_id"], "req-1");
    }

    #[test]
    fn projection_rejects_non_object_payload() {
        let error = project_semantic_payload(
            vec!["not".to_string(), "an".to_string(), "object".to_string()],
            LedgerProjection::skipped("not_configured"),
        )
        .expect_err("array must fail");
        assert_eq!(error.code, "device_error");
    }
}
