// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    CommandPayloadDraft, CommandStage, EventActor, EventDraft, EventLinks, EventOrigin,
    EventSeverity, EventSource, EventType, FieldRedactor, PersistedEvent,
};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

fn config(temp: &TempDir, owner_id: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(temp.path(), owner_id)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn event(
    event_id: &str,
) -> actingcommand_contract::SanitizedEventDraft<actingcommand_contract::CommandPayload> {
    let payload =
        CommandPayloadDraft::new(CommandStage::Received, "runtime.start", vec![]).expect("payload");
    EventDraft::new(
        event_id,
        1_752_147_200_000,
        EventType::CommandReceived,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, "actingctl", EventActor::User).expect("origin"),
        EventLinks::default(),
        payload,
    )
    .sanitize(&Sha256FieldRedactor::new(b"test-private-salt").expect("redactor"))
    .expect("sanitize")
}

fn segment_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root.join("segments"))
        .expect("segments")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_events(root: &Path) -> Vec<PersistedEvent> {
    let mut events = Vec::new();
    for path in segment_paths(root) {
        let text = fs::read_to_string(path).expect("segment text");
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).expect("line JSON");
            events.push(serde_json::from_value(value["event"].clone()).expect("persisted event"));
        }
    }
    events
}

fn write_owner_metadata(root: &Path, active: bool, valid: bool) {
    fs::create_dir_all(root).expect("root");
    let content = if valid {
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 999_999_u32,
            "active": active,
            "started_at_unix_ms": 1_u64,
            "closed_at_unix_ms": Value::Null
        })
        .to_string()
    } else {
        "{not-json".to_string()
    };
    fs::write(root.join("writer.lock"), content).expect("owner metadata");
}

#[test]
fn sha256_redactor_requires_non_empty_private_salt() {
    let error = Sha256FieldRedactor::new([]).expect_err("empty salt must fail");

    assert_eq!(error.code(), "invalid_redactor_config");
}

#[test]
fn sha256_redactor_returns_fixed_lowercase_fingerprint() {
    let redactor = Sha256FieldRedactor::new(b"private-salt").expect("redactor");

    let fingerprint = redactor
        .fingerprint("token", "secret-value")
        .expect("fingerprint");

    assert!(fingerprint.starts_with("sha256:"));
    assert_eq!(fingerprint.len(), 71);
    assert!(
        fingerprint[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    );
    assert!(!fingerprint.contains("secret-value"));
}

#[test]
fn config_debug_hides_machine_path() {
    let temp = TempDir::new().expect("temp");
    let config = config(&temp, "writer-one");

    let debug = format!("{config:?}");

    assert!(!debug.contains(&temp.path().display().to_string()));
    assert!(debug.contains("<redacted-root>"));
}

#[test]
fn shutdown_waits_for_a_full_ingress_queue_to_drain() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let (prefill_response, _prefill_receiver) = mpsc::sync_channel(1);
    sender
        .send(WriterCommand::Shutdown {
            response: prefill_response,
        })
        .expect("fill queue");
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let _ = receiver.recv().expect("prefill");
        if let WriterCommand::Shutdown { response } = receiver.recv().expect("shutdown") {
            response.send(Ok(())).expect("shutdown response");
        }
        Ok(())
    });
    let ledger = GlobalLedger {
        sender: Some(sender),
        writer: Some(writer),
    };
    let (done_sender, done_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = done_sender.send(ledger.close());
    });

    let result = done_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("close must not deadlock");

    result.expect("close");
}

#[test]
fn second_writer_is_rejected_while_first_is_alive() {
    let temp = TempDir::new().expect("temp");
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first writer");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("second writer must fail");

    assert_eq!(error.code(), "writer_conflict");
    first.close().expect("close first");
}

#[test]
fn malformed_writer_metadata_is_fatal_without_path_disclosure() {
    let temp = TempDir::new().expect("temp");
    write_owner_metadata(temp.path(), true, false);

    let error =
        GlobalLedger::open(config(&temp, "writer-one")).expect_err("malformed metadata must fail");

    assert_eq!(error.code(), "malformed_owner_metadata");
    assert!(
        !error
            .to_string()
            .contains(&temp.path().display().to_string())
    );
}

#[test]
fn contradictory_writer_metadata_is_fatal() {
    let cases = [
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": true,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 11
        }),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": Value::Null
        }),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "previous-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 9
        }),
    ];
    for metadata in cases {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("writer.lock"), metadata.to_string()).expect("metadata");

        let error = GlobalLedger::open(config(&temp, "writer-new"))
            .expect_err("contradictory metadata must fail");

        assert_eq!(error.code(), "malformed_owner_metadata");
    }
}

#[test]
fn stale_active_owner_is_recovered_explicitly() {
    let temp = TempDir::new().expect("temp");
    write_owner_metadata(temp.path(), true, true);

    GlobalLedger::open(config(&temp, "writer-new"))
        .expect("recover writer")
        .close()
        .expect("close");

    let events = read_events(temp.path());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::LedgerRecovered);
    assert_eq!(events[0].sequence, 1);
}

#[test]
fn append_assigns_contiguous_sequences_across_reopen() {
    let temp = TempDir::new().expect("temp");
    let first = GlobalLedger::open(config(&temp, "writer-one")).expect("first");
    assert_eq!(
        first.append(event("evt-one")).expect("append one").sequence,
        1
    );
    first.close().expect("close first");

    let second = GlobalLedger::open(config(&temp, "writer-two")).expect("second");
    assert_eq!(
        second
            .append(event("evt-two"))
            .expect("append two")
            .sequence,
        2
    );
    second.close().expect("close second");

    let sequences = read_events(temp.path())
        .into_iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    assert_eq!(sequences, vec![1, 2]);
}

#[test]
fn truncated_final_tail_is_quarantined_and_reported() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment")
        .write_all(br#"{"line_type":"event""#)
        .expect("write tail");

    let recovered = GlobalLedger::open(config(&temp, "writer-two")).expect("recover");
    assert_eq!(
        recovered
            .append(event("evt-after-recovery"))
            .expect("append after recovery")
            .sequence,
        3
    );
    recovered.close().expect("close recovered");

    let events = read_events(temp.path());
    assert_eq!(events[0].event_id, "evt-one");
    assert_eq!(events[1].event_type, EventType::LedgerRecovered);
    assert_eq!(events[2].event_id, "evt-after-recovery");
    let quarantine_count = fs::read_dir(temp.path().join("quarantine"))
        .expect("quarantine")
        .count();
    assert_eq!(quarantine_count, 1);
}

#[test]
fn complete_corrupt_line_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    let mut file = OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment");
    file.write_all(b"not-json\n").expect("write corrupt line");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("complete corruption must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn complete_blank_line_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open segment")
        .write_all(b"\n")
        .expect("write blank record");

    let error = GlobalLedger::open(config(&temp, "writer-two"))
        .expect_err("blank complete record must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn duplicate_json_key_is_fatal_without_disclosing_the_hidden_value() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-one")).expect("append");
    ledger.close().expect("close");
    let segment = segment_paths(temp.path()).pop().expect("segment");
    let secret = r"C:\hidden\duplicate-subject";
    let text = fs::read_to_string(&segment).expect("segment text");
    let encoded_secret = serde_json::to_string(secret).expect("encode secret");
    let forged = text.replacen(
        r#""subject":"runtime.start""#,
        &format!(r#""subject":{encoded_secret},"subject":"runtime.start""#),
        1,
    );
    assert_ne!(forged, text);
    fs::write(segment, forged).expect("write duplicate-key line");

    let error =
        GlobalLedger::open(config(&temp, "writer-two")).expect_err("duplicate key must fail");

    assert_eq!(error.code(), "corrupt_segment");
    assert!(!error.to_string().contains(secret));
}

#[test]
fn non_final_segment_corruption_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let small = GlobalLedgerConfig::new(temp.path(), "writer-one")
        .with_segment_max_bytes(256)
        .with_ingress_capacity(8);
    let ledger = GlobalLedger::open(small).expect("ledger");
    ledger.append(event("evt-one")).expect("one");
    ledger.append(event("evt-two")).expect("two");
    ledger.append(event("evt-three")).expect("three");
    ledger.close().expect("close");
    let segments = segment_paths(temp.path());
    assert!(segments.len() >= 2);
    OpenOptions::new()
        .append(true)
        .open(&segments[0])
        .expect("first segment")
        .write_all(b"truncated")
        .expect("write corrupt tail");

    let error = GlobalLedger::open(config(&temp, "writer-two"))
        .expect_err("non-final corruption must fail");

    assert_eq!(error.code(), "corrupt_segment");
}

#[test]
fn duplicate_event_id_is_fatal() {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(config(&temp, "writer-one")).expect("ledger");
    ledger.append(event("evt-duplicate")).expect("first");

    let error = ledger
        .append(event("evt-duplicate"))
        .expect_err("duplicate event must fail");

    assert_eq!(error.code(), "duplicate_event_id");
    ledger.close().expect("close");
}
