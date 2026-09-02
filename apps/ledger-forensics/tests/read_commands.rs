// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, DiagnosticCode, EffectDisposition, EventAction, EventActor,
    EventDraft, EventLinksDraft, EventOrigin, EventSeverity, EventSource, IdentifierIssuer,
    OriginModule,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::Command;

#[test]
fn actingledger_read_commands_are_thin_and_fail_loud() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let ledger_root = state_root.join("ledger");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let request_id = identifiers.mint_request_id().expect("request id");
    let request_text = serde_json::to_value(request_id.transport())
        .expect("request id JSON")
        .as_str()
        .expect("request id string")
        .to_owned();
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "cli-fixture"))
        .expect("open ledger");
    let event_id = identifiers.mint_event_id().expect("event id");
    let event = EventDraft::new(
        event_id,
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default().with_request_id(request_id),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"actingledger-test-salt").expect("fingerprinter"))
    .expect("sanitize");
    writer.append(event).expect("append event");
    writer.close().expect("close writer");

    let binary = env!("CARGO_BIN_EXE_actingledger");
    let commands = [
        vec!["open".to_owned()],
        vec!["events".to_owned()],
        vec!["chain".to_owned(), "--req".to_owned(), request_text],
        vec!["tail".to_owned()],
        vec!["repairs".to_owned()],
        vec!["export".to_owned()],
    ];
    for (index, command) in commands.iter().enumerate() {
        let first = invoke(binary, state_root, command);
        let second = invoke(binary, state_root, command);
        assert!(first.status.success(), "valid command failed: {first:?}");
        assert_eq!(first.stdout, second.stdout);
        assert!(first.stderr.is_empty());
        if index < 5 {
            let value: serde_json::Value =
                serde_json::from_slice(&first.stdout).expect("machine JSON");
            assert_eq!(value["command"], command[0]);
        } else {
            assert!(
                std::str::from_utf8(&first.stdout)
                    .expect("UTF-8 export")
                    .starts_with("ActingCommand ledger forensic export\n")
            );
        }
    }

    for invalid in [
        Vec::<&str>::new(),
        vec!["--state-root"],
        vec![
            "--state-root",
            state_root.to_str().expect("state root"),
            "unknown",
        ],
        vec![
            "--state-root",
            state_root.to_str().expect("state root"),
            "chain",
        ],
        vec![
            "--state-root",
            state_root.to_str().expect("state root"),
            "events",
            "--module",
            "runtime_host",
        ],
    ] {
        let output = Command::new(binary)
            .args(invalid)
            .output()
            .expect("run invalid command");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }

    let args = [
        OsString::from("--state-root"),
        state_root.as_os_str().to_owned(),
        OsString::from("open"),
    ];
    let error = actingledger::run(args, &mut FailingWriter).expect_err("output failure");
    assert_eq!(error.code(), "output_failed");

    let main_source = include_str!("../src/main.rs");
    assert_eq!(main_source.matches("actingledger::run_env()").count(), 1);
    for forbidden in ["GlobalLedger", "EventQuery", "serde_json"] {
        assert!(!main_source.contains(forbidden));
    }
    let cli_source = include_str!("../src/lib.rs");
    assert_eq!(
        cli_source
            .matches("actingcommand_ledger_forensics::run(request)")
            .count(),
        1
    );
    for forbidden in ["GlobalLedger", "EventQuery", "PersistedEvent"] {
        assert!(!cli_source.contains(forbidden));
    }
}

#[test]
fn events_cli_parses_bounded_filters_and_reports_next_cursor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_root = temp.path();
    let ledger_root = state_root.join("ledger");
    let identifiers = IdentifierIssuer::new().expect("identifiers");
    let correlation_id = identifiers.mint_correlation_id().expect("correlation id");
    let correlation_text = serde_json::to_value(correlation_id.transport())
        .expect("correlation JSON")
        .as_str()
        .expect("correlation string")
        .to_owned();
    let writer = GlobalLedger::open(GlobalLedgerConfig::new(&ledger_root, "cli-filter-fixture"))
        .expect("open ledger");
    for index in 0..2 {
        let event = EventDraft::new(
            identifiers.mint_event_id().expect("event id"),
            1_752_147_200_000 + index,
            EventSeverity::Error,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            EventLinksDraft::default().with_correlation_id(correlation_id),
            CommandPayloadDraft::rejected(
                EventAction::RuntimeStart,
                DiagnosticCode::CommandRejected,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            )
            .into(),
        )
        .sanitize(
            &Sha256SecretFingerprinter::new(b"actingledger-filter-test-salt")
                .expect("fingerprinter"),
        )
        .expect("sanitize");
        writer.append(event).expect("append event");
    }
    writer.close().expect("close writer");

    let binary = env!("CARGO_BIN_EXE_actingledger");
    let command = vec![
        "events".to_owned(),
        "--after".to_owned(),
        "0".to_owned(),
        "--through".to_owned(),
        "2".to_owned(),
        "--limit".to_owned(),
        "1".to_owned(),
        "--origin-module".to_owned(),
        "runtime".to_owned(),
        "--diagnostic-code".to_owned(),
        "command.rejected".to_owned(),
        "--severity".to_owned(),
        "error".to_owned(),
        "--correlation-id".to_owned(),
        correlation_text.clone(),
    ];
    let first = invoke(binary, state_root, &command);
    assert!(first.status.success(), "filtered events failed: {first:?}");
    assert!(first.stderr.is_empty());
    let first: serde_json::Value = serde_json::from_slice(&first.stdout).expect("events JSON");
    assert_eq!(first["command"], "events");
    assert_eq!(first["data"]["filter"]["origin_module"], "runtime");
    assert_eq!(
        first["data"]["filter"]["diagnostic_code"],
        "command.rejected"
    );
    assert_eq!(first["data"]["filter"]["severity"], "error");
    assert_eq!(first["data"]["filter"]["correlation_id"], correlation_text);
    assert_eq!(first["data"]["after_sequence"], 0);
    assert_eq!(first["data"]["through_sequence"], 2);
    assert_eq!(first["data"]["limit"], 1);
    assert_eq!(first["data"]["events"][0]["sequence"], 1);
    assert_eq!(first["data"]["next_after_sequence"], 1);

    let continuation = invoke(
        binary,
        state_root,
        &[
            "events".to_owned(),
            "--after".to_owned(),
            "1".to_owned(),
            "--through".to_owned(),
            "2".to_owned(),
            "--limit".to_owned(),
            "1".to_owned(),
            "--origin-module".to_owned(),
            "runtime".to_owned(),
            "--diagnostic-code".to_owned(),
            "command.rejected".to_owned(),
            "--severity".to_owned(),
            "error".to_owned(),
            "--correlation-id".to_owned(),
            correlation_text,
        ],
    );
    assert!(
        continuation.status.success(),
        "filtered continuation failed: {continuation:?}"
    );
    let continuation: serde_json::Value =
        serde_json::from_slice(&continuation.stdout).expect("continuation JSON");
    assert_eq!(continuation["data"]["events"][0]["sequence"], 2);
    assert!(continuation["data"].get("next_after_sequence").is_none());

    let missing_root = state_root.join("does-not-exist");
    let invalid_commands = [
        vec!["events", "--limit", "1", "--limit", "2"],
        vec!["events", "--unknown", "value"],
        vec!["events", "--limit"],
        vec!["events", "--severity", "urgent"],
        vec!["events", "--correlation-id", "correlation_not_hex"],
        vec!["events", "--after", "not-a-number"],
        vec!["events", "--limit", "0"],
        vec!["events", "--limit", "1025"],
        vec!["events", "--after", "3", "--through", "2"],
    ];
    for command in invalid_commands {
        let command = command.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let output = invoke(binary, &missing_root, &command);
        assert!(
            !output.status.success(),
            "invalid command passed: {command:?}"
        );
        assert!(output.stdout.is_empty());
        assert!(
            std::str::from_utf8(&output.stderr)
                .expect("UTF-8 stderr")
                .contains("invalid_arguments"),
            "unexpected invalid-command error: {output:?}"
        );
    }
}

fn invoke(binary: &str, state_root: &std::path::Path, command: &[String]) -> std::process::Output {
    Command::new(binary)
        .arg("--state-root")
        .arg(state_root)
        .args(command)
        .output()
        .expect("run actingledger")
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed output"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
