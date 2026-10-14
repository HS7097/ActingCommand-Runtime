// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-1, the acceptance row of additions-v012 §8.3 ("片级前移", the dump import):
// one test drives it end to end through the Runtime. A forced write failure produces a dump and
// stops the Runtime through its sink; the restart imports the dump before it opens, the record
// the dump shares with the ledger is not written twice, and the dump is deleted
// (`contracts/ledger-observer.md` §9.2, §9.4).
//
// The write's two attempts (`ledger_observer.write_attempts` 2, no wait between them) fail by the
// write fault gate: the first before it commits, the second after it committed with its result
// lost, so the record is both in the ledger and in the dump. `GATE_WAIT` only bounds a broken
// run.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{FactContent, FactRecord, FactScope, RuntimeConfigManifest};
use actingcommand_ledger::{
    LEDGER_DUMP_FILE_NAME, LedgerCode, LedgerFailureSink, LedgerWriteFaultGate,
};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};
use std::sync::Mutex;

const GATE_WAIT: Duration = Duration::from_secs(30);

// The Runtime's stop, as actingd's sink would end the process.
struct Stops(Mutex<mpsc::Sender<Outcome>>);

impl LedgerFailureSink for Stops {
    fn write_retried(&self, _outcome: &Outcome) {}

    fn stop(&self, outcome: &Outcome) {
        let _ = self.0.lock().expect("stops").send(outcome.clone());
    }
}

// The configuration a start reads: two write attempts with no wait between them.
fn manifest() -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({"ledger_observer": {"write_attempts": 2, "write_retry_backoff_ms": [0, 0]}}),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn start(root: &TempDir, sink: &Arc<dyn LedgerFailureSink>) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_config_manifest(manifest())
            .with_ledger_failure_sink(Arc::clone(sink)),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

// A new server-scoped observation for the instance fact store.
fn fact() -> FactRecord {
    FactRecord {
        scope: FactScope::Server {
            server_id: "fixture-server".to_owned(),
        },
        key: "env.fixture_flag".to_owned(),
        content: FactContent::Inline {
            value: actingcommand_contract::FactValue::Boolean(true),
        },
        observed_at_unix_ms: 1_000,
        expires_at_unix_ms: None,
        ttl_policy: None,
        confidence_milli: 1_000,
        source_detector: "detector.fixture".to_owned(),
        source_snapshot_id: "snapshot:ao1-acceptance".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// The event ids of the dump's records, in capture order.
fn dumped_ids(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("the dump file")
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line: Value = serde_json::from_str(line).expect("a JSON line");
            line.get("record")
                .map(|record| record["draft"]["event_id"].clone())
        })
        .collect()
}

#[test]
fn ao1_acceptance_a_forced_write_failure_dumps_and_the_restart_imports_it_once() {
    let root = TempDir::new().expect("tempdir");
    let dump_path = root
        .path()
        .canonicalize()
        .expect("state root")
        .join(LEDGER_DUMP_FILE_NAME);
    let (stops, stopped) = mpsc::channel();
    let sink: Arc<dyn LedgerFailureSink> = Arc::new(Stops(Mutex::new(stops)));
    let host = start(&root, &sink);
    let faults = LedgerWriteFaultGate::arm(root.path()).expect("write fault gate");
    faults.fail_writes(1);
    faults.commit_then_fail(1);

    host.publish_fact(fact())
        .expect_err("the fact's record gives up after its two attempts");

    let stop = stopped
        .recv_timeout(GATE_WAIT)
        .expect("the Runtime is stopped through its sink");
    assert!(stop.is(LedgerCode::LedgerWriteFailed));
    assert_eq!(
        stop.value("path"),
        Some(&json!(dump_path.to_string_lossy()))
    );
    let ids = dumped_ids(&dump_path);
    assert_eq!(ids.len(), 2, "the fact's record and the fatal record");
    drop(faults);
    assert!(
        host.close().is_err(),
        "the Runtime ended with its ledger failure"
    );

    let host = start(&root, &sink);
    let records = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger records");
    for id in &ids {
        assert_eq!(
            records
                .iter()
                .filter(|record| serde_json::to_value(record.event_id()).ok().as_ref() == Some(id))
                .count(),
            1,
            "each dumped record is in the ledger once"
        );
    }
    let imported = records
        .iter()
        .find(|record| record.event_type() == EventType::LedgerDumpImported)
        .expect("the import's record");
    let outcome = imported.payload().outcome().expect("the import's outcome");
    assert_eq!(outcome.value("count"), Some(&json!(1)));
    assert_eq!(outcome.value("duplicate_count"), Some(&json!(1)));
    assert_eq!(outcome.value("dump_sealed"), Some(&json!(true)));
    assert!(!dump_path.exists(), "the dump is deleted");
    host.close().expect("close the restarted host");
}
