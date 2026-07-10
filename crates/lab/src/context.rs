// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{LabError, LabResult};
use actingcommand_ledger::{IdIssuer, IdKind, LedgerRecord, LedgerRecordKind};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UserConfig {
    pub adb_path: Option<String>,
    pub runtime_endpoint: Option<String>,
    pub run_root: Option<String>,
    pub resource_root: Option<String>,
    #[serde(default)]
    pub instances: std::collections::BTreeMap<String, InstanceConfig>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InstanceConfig {
    pub serial: Option<String>,
    pub game: Option<String>,
    pub server: Option<String>,
    pub package: Option<String>,
    pub adb_path: Option<String>,
    pub capture_backend: Option<String>,
    pub touch_backend: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SemanticRequestContext {
    pub command: String,
    pub instance: String,
    pub arguments: Vec<String>,
    pub dry_run: bool,
}

pub struct SemanticLedgerContext {
    pub command: String,
    pub instance: String,
    pub req_id: String,
    issuer: IdIssuer,
    records: Vec<LedgerRecord>,
}

impl SemanticLedgerContext {
    pub fn new(request: SemanticRequestContext) -> Self {
        let issuer = IdIssuer::new();
        let req_id = issuer.issue(IdKind::Req).value;
        let records = vec![LedgerRecord::new(
            LedgerRecordKind::Dispatch,
            Some(req_id.clone()),
            json!({
                "stage": "request",
                "command": request.command,
                "instance": request.instance,
                "args": request.arguments,
                "args_count": request.arguments.len(),
                "dry_run": request.dry_run
            }),
        )];
        Self {
            command: request.command,
            instance: request.instance,
            req_id,
            issuer,
            records,
        }
    }

    pub fn issue(&self, kind: IdKind) -> String {
        self.issuer.issue(kind).value
    }

    pub fn record_drive<T: Serialize>(&mut self, payload: T) -> LabResult<()> {
        let payload = serde_json::to_value(payload).map_err(|error| {
            LabError::device(format!(
                "failed to serialize semantic drive record: {error}"
            ))
        })?;
        self.records.push(LedgerRecord::new(
            LedgerRecordKind::Drive,
            Some(self.req_id.clone()),
            payload,
        ));
        Ok(())
    }

    pub fn take_records(&mut self) -> Vec<crate::LedgerRecordEntry> {
        std::mem::take(&mut self.records)
            .into_iter()
            .map(crate::LedgerRecordEntry::from_storage)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_context_records_typed_request_and_drive() {
        let mut context = SemanticLedgerContext::new(SemanticRequestContext {
            command: "detect-page".to_string(),
            instance: "fixture".to_string(),
            arguments: vec!["--scene".to_string(), "fixture.png".to_string()],
            dry_run: true,
        });
        context
            .record_drive(&serde_json::json!({"stage": "recognition"}))
            .expect("drive");
        let records = context.take_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].storage().payload["args_count"], 2);
        assert_eq!(records[1].storage().payload["stage"], "recognition");
    }
}
