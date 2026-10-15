// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{
    AuditInput, CommandPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft,
    EventOrigin, EventSeverity, EventSource, IdentifierIssuer, OriginModule,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_TEST_REPAIR_ROOT";
const FAILPOINT_ENV: &str = "ACTINGCOMMAND_TEST_REPAIR_FAILPOINT";
const READY_PATH_ENV: &str = "ACTINGCOMMAND_TEST_REPAIR_READY";
const CHILD_TEST: &str = "global::recovery_tests::repair_process_child";
const TAIL: &[u8] = br#"{"incomplete":"tail""#;

// Workflow #382 L-4 (test plan §4 #15): a child process entry, not a test of its own. It is
// ignored in an ordinary run; `kill_at_repair_stage` runs it with `--ignored --exact`, and only
// the journal check below still stops it (after its prepare). The Segment repair writer's crash
// tests that stopped it at every stage are retired (test plan §4 #16, LED-R1); their replacement
// is the read-only journal state table in read_only_tests.rs and, on the formal store, L-3 G5
// (tests/gate_l3_g5.rs).
#[test]
#[ignore = "child process entry: its parent test runs it with --ignored --exact"]
fn repair_process_child() {
    let Ok(root) = std::env::var(CHILD_ROOT_ENV) else {
        return;
    };
    let ledger = GlobalLedger::open(test_config(Path::new(&root), "repair-child"))
        .expect("child opens ledger until repair barrier");
    ledger.close().expect("child closes ledger");
}

#[test]
fn successful_open_has_no_unresolved_prepared_repair() {
    let temp = seeded_truncated_root();

    GlobalLedger::open(test_config(temp.path(), "normal-repair"))
        .expect("repair open")
        .close()
        .expect("repair close");

    assert_journal_completed(temp.path());
}

#[test]
fn repair_journal_rejects_unknown_schema_duplicate_keys_and_invalid_transitions() {
    for (case, expected_code) in [
        ("unknown_schema", "unsupported_repair_schema"),
        ("duplicate_key", "corrupt_repair_journal"),
        ("completed_without_prepare", "repair_state_inconsistent"),
    ] {
        let temp = seeded_truncated_root();
        kill_at_repair_stage(temp.path(), "after_prepare");
        let journal = temp.path().join("repair-journal.jsonl");
        let prepared = fs::read_to_string(&journal).expect("prepared journal");
        let corrupted = match case {
            "unknown_schema" => prepared.replace(
                "actingcommand.ledger-repair.v1",
                "actingcommand.ledger-repair.v0",
            ),
            "duplicate_key" => prepared.replacen(
                "\"repair_id\":",
                "\"repair_id\":\"sha256:0000000000000000000000000000000000000000000000000000000000000000\",\"repair_id\":",
                1,
            ),
            "completed_without_prepare" => {
                let mut value: Value = serde_json::from_str(prepared.trim()).expect("record");
                value["state"] = Value::String("completed".to_string());
                format!("{}\n", serde_json::to_string(&value).expect("completed record"))
            }
            _ => unreachable!(),
        };
        fs::write(journal, corrupted).expect("corrupt repair journal");

        let error = GlobalLedger::open(test_config(temp.path(), "journal-check"))
            .expect_err("invalid repair journal must fail");
        assert_eq!(error.code(), expected_code, "case {case}");
    }
}

#[test]
fn legacy_writer_v1_is_rejected_explicitly() {
    let temp = TempDir::new().expect("temp");
    fs::write(
        temp.path().join("writer.lock"),
        serde_json::json!({
            "schema_version": "actingcommand.ledger-writer.v1",
            "owner_id": "legacy-owner",
            "pid": 42,
            "active": false,
            "started_at_unix_ms": 10,
            "closed_at_unix_ms": 9
        })
        .to_string(),
    )
    .expect("legacy writer metadata");

    let error = GlobalLedger::open(test_config(temp.path(), "writer-v2"))
        .expect_err("v1 writer metadata must be rejected");

    assert_eq!(error.code(), "unsupported_writer_schema");
}

fn seeded_truncated_root() -> TempDir {
    let temp = TempDir::new().expect("temp");
    let ledger = GlobalLedger::open(test_config(temp.path(), "seed-writer")).expect("seed ledger");
    ledger.append(seed_event()).expect("append seed event");
    ledger.close().expect("close seed ledger");
    let segment = final_segment(temp.path());
    let mut file = OpenOptions::new()
        .append(true)
        .open(segment)
        .expect("open final segment");
    file.write_all(TAIL).expect("append incomplete tail");
    file.sync_all().expect("sync incomplete tail");
    temp
}

fn seed_event() -> actingcommand_contract::SanitizedEventDraft {
    let identifiers = IdentifierIssuer::new().expect("identifier issuer");
    EventDraft::new(
        identifiers.mint_event_id().expect("event id"),
        1,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        EventLinksDraft::default(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
    .sanitize(&Sha256SecretFingerprinter::new(b"repair-test-salt").expect("fingerprinter"))
    .expect("sanitize seed event")
}

fn test_config(root: &Path, owner: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(root, owner)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn final_segment(root: &Path) -> PathBuf {
    let mut segments = fs::read_dir(root.join("segments"))
        .expect("segments")
        .map(|entry| entry.expect("segment entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    segments.sort();
    segments.pop().expect("final segment")
}

fn kill_at_repair_stage(root: &Path, stage: &str) {
    let ready = root.join(format!("repair-ready-{stage}"));
    let mut child = spawn_repair_child(root, stage, &ready);
    wait_for_barrier(&mut child, &ready, stage);
    child.kill().expect("kill repair child");
    child.wait().expect("wait for repair child");
    let _ = fs::remove_file(ready);
}

fn spawn_repair_child(root: &Path, stage: &str, ready: &Path) -> Child {
    Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--ignored")
        .arg("--exact")
        .arg(CHILD_TEST)
        .arg("--nocapture")
        .env(CHILD_ROOT_ENV, root)
        .env(FAILPOINT_ENV, stage)
        .env(READY_PATH_ENV, ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn repair child")
}

pub(super) fn wait_for_barrier(child: &mut Child, ready: &Path, stage: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready.exists() {
            return;
        }
        if let Some(status) = child.try_wait().expect("poll repair child") {
            panic!("repair child exited before {stage} barrier: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("repair child did not reach {stage} barrier");
}

fn assert_journal_completed(root: &Path) {
    let text = fs::read_to_string(root.join("repair-journal.jsonl")).expect("repair journal");
    let mut states = BTreeMap::new();
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).expect("repair journal record");
        let repair_id = value["repair_id"].as_str().expect("repair id").to_string();
        let state = value["state"].as_str().expect("repair state").to_string();
        states.insert(repair_id, state);
    }
    assert!(!states.is_empty());
    assert!(states.values().all(|state| state == "completed"));
}
