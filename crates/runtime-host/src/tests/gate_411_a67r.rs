// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A67-R (A67 R5's remainder; the 10-24 production start whose frame store sampled no
//! budget after the OS reserve): a start whose memory sample leaves no budget after the reserve
//! proceeds, and its frame stores' floor warning `frame_store_budget_floor_applied` is recorded
//! once in the daemon's log and once in the ledger, with the sampled total and available
//! memory, the reserve and the floor.
//!
//! Every capture frame store reads the fake sample (`with_capture_memory_sample_for_test`).
//! Two instances are prepared at the start, so two stores open on the floor: the budget enters
//! the floor once.

use super::*;
use actingcommand_artifact_store::MemorySample;
use actingcommand_contract::RuntimePayload;
use actingcommand_contract::outcome::Outcome;
use std::sync::Mutex;

const GIB: u64 = 1024 * 1024 * 1024;
/// The frame store's OS reserve (`os_reserve_bytes`, default 2 GiB).
const RESERVE_BYTES: u64 = 2 * GIB;
/// The frame store's floor (`frame_store.floor_mem_bytes`, default 256 MiB).
const FLOOR_BYTES: u64 = 256 * 1024 * 1024;
/// A sample whose available memory is below the reserve: the budget after it is zero.
const SAMPLE: MemorySample = MemorySample {
    total_bytes: 32 * GIB,
    available_bytes: GIB,
};

/// The daemon's log, as actingd's reporter writes it: the warnings the host records.
#[derive(Default)]
struct DaemonLog {
    warnings: Mutex<Vec<Outcome>>,
}

impl RuntimeDaemonReporter for DaemonLog {
    fn fatal_latched(&self, _outcome: &Outcome) {}

    fn step_skipped(&self, _outcome: &Outcome) {}

    fn warning_recorded(&self, outcome: &Outcome) {
        self.warnings
            .lock()
            .expect("daemon log")
            .push(outcome.clone());
    }
}

/// A fake device whose opens pass, so its start preparation succeeds.
fn reachable_device() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    state.observed_open.store(true, Ordering::Release);
    state
}

fn assert_floor_values(warning: &Outcome, place: &str) {
    assert_eq!(
        warning.code().as_str(),
        "frame_store_budget_floor_applied",
        "{place}: {warning:?}"
    );
    for (key, expected) in [
        ("memory_total_bytes", SAMPLE.total_bytes),
        ("memory_available_bytes", SAMPLE.available_bytes),
        ("memory_reserve_bytes", RESERVE_BYTES),
        ("memory_floor_bytes", FLOOR_BYTES),
    ] {
        assert_eq!(
            warning.value(key).and_then(serde_json::Value::as_u64),
            Some(expected),
            "{place}: {key} in {warning:?}"
        );
    }
}

#[test]
fn a67r_a_start_on_the_floor_records_its_warning_once_in_the_log_and_the_ledger() {
    let root = TempDir::new().expect("tempdir");
    let log = Arc::new(DaemonLog::default());

    let host = RuntimeHost::start(
        config(&root)
            .with_daemon_reporter(log.clone())
            .with_capture_memory_sample_for_test(SAMPLE),
        Arc::new(
            FakeProvider::from_entries([
                ("node.a".to_owned(), instance_id(), reachable_device()),
                ("node.b".to_owned(), instance_id(), reachable_device()),
            ])
            .with_device_endpoints(),
        ),
    )
    .unwrap_or_else(|error| {
        panic!(
            "a memory sample below the reserve stopped the start: {}",
            error.complete_message()
        )
    });

    assert!(host.fatal_error().expect("runtime health").is_none());
    let failures = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("runtime.failed records");
    let logged = log.warnings.lock().expect("daemon log").clone();
    host.close().expect("close the started Runtime");

    let recorded = failures
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(payload))
                if payload.outcome().code().as_str() == "frame_store_budget_floor_applied" =>
            {
                Some((event.severity(), payload.outcome().clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(severity, recorded)] = recorded.as_slice() else {
        panic!("the ledger has the floor warning exactly once: {recorded:?}");
    };
    assert_eq!(*severity, EventSeverity::Warning);
    assert_floor_values(recorded, "ledger");

    let [logged] = logged.as_slice() else {
        panic!("the daemon's log has the floor warning exactly once: {logged:?}");
    };
    assert_floor_values(logged, "log");
}
