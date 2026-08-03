// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::Envelope;
use serde_json::Value;

use super::{CliError, ErrorKind, RUNTIME_VERSION, SCHEMA_VERSION};

pub(super) fn human_summary(command: &str, data: &Value) -> String {
    match data {
        Value::String(text) => text.clone(),
        _ => format!("{command} ok"),
    }
}

#[derive(Debug)]
pub(super) struct CliResult {
    pub(super) print_json: bool,
    pub(super) envelope: Envelope<Value>,
    human: String,
    exit_code: i32,
}

impl CliResult {
    pub(super) fn ok(command: String, data: Value, print_json: bool, human: String) -> Self {
        Self {
            print_json,
            envelope: Envelope::ok(
                SCHEMA_VERSION,
                env!("CARGO_PKG_VERSION"),
                RUNTIME_VERSION,
                command,
                data,
            ),
            human,
            exit_code: 0,
        }
    }

    pub(super) fn err(command: String, err: CliError, print_json: bool) -> Self {
        let exit_code = err.exit_code();
        let human = format!("{}: {}", err.code, err.message);
        Self {
            print_json,
            envelope: Envelope::err(
                SCHEMA_VERSION,
                env!("CARGO_PKG_VERSION"),
                RUNTIME_VERSION,
                command,
                err,
            ),
            human,
            exit_code,
        }
    }

    pub(super) fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub(super) fn envelope_json(&self) -> String {
        serde_json::to_string(&self.envelope).unwrap_or_else(|err| {
            format!(r#"{{"ok":false,"error":"json_serialize_failed:{err}"}}"#)
        })
    }

    pub(super) fn human_text(&self) -> String {
        self.human.clone()
    }
}

pub(super) trait CliErrorExitCode {
    fn exit_code(&self) -> i32;
}

impl CliErrorExitCode for CliError {
    fn exit_code(&self) -> i32 {
        match self.class {
            ErrorKind::UsageValidation => 2,
            ErrorKind::SafetyBlocked => 3,
            ErrorKind::DeviceInstance => 4,
            ErrorKind::RuntimeUnavailable => 5,
            ErrorKind::NotImplemented => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_summary_preserves_string_and_non_string_output() {
        assert_eq!(
            human_summary("ignored", &Value::String("ready".to_string())),
            "ready"
        );
        assert_eq!(human_summary("ignored", &Value::String(String::new())), "");
        assert_eq!(human_summary("status", &Value::Null), "status ok");
        assert_eq!(
            human_summary("session status!?  Mixed", &serde_json::json!({"ok": true})),
            "session status!?  Mixed ok"
        );
    }
}
