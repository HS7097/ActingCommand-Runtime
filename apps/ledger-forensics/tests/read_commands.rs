// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventSeverity, EventSource, IdentifierIssuer, OriginModule,
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
