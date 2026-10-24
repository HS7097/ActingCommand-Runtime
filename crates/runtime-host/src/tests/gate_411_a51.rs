// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A51 (0.12.9; the 10-21 incident: one Lab-view read ran on the ledger writer for
//! minutes, the reads waiting behind it timed out, the host mapped those timeouts to its fatal
//! state and the Runtime began to stop, silently, while its close waited for the writer).
//! Requirements of the slice (brief A51, frozen by the coordinator 10-21 19:3x), the host's part:
//!
//! - T3 (R3): a ledger read the writer does not answer in time latches nothing. While the
//!   writer is held in a read, a `Status` and the performance loop's runtime-fact seal both read
//!   the ledger's head behind it; both answer the registered non-fatal `writer_response_timeout`,
//!   the fatal state stays empty and no stop is requested; once the read ends, `Status` answers
//!   `Completed` again;
//! - T5 (R4): the first fatal condition the host latches reaches the daemon's log at once
//!   (the daemon reporter), before any close, and only the first.
//!
//! The writer is held by the ledger's read hold gate, which the test releases; the reply limit
//! is set short, so a held read's callers time out within the test. Existing tests missed this:
//! AO-1's "a read timeout is not fatal" was tested for the frame cleaner only, and every other
//! read site mapped any read failure to `ledger_failure`.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RuntimeConfigManifest,
};
use actingcommand_ledger::{LedgerCode, WriterReadHoldGate};
use std::sync::Mutex;

/// T3: how long a caller waits for one writer reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// The daemon's log, as actingd's reporter would write it.
#[derive(Default)]
struct DaemonLog {
    fatal: Mutex<Vec<Outcome>>,
}

impl DaemonLog {
    fn fatal(&self) -> Vec<Outcome> {
        self.fatal.lock().expect("daemon log").clone()
    }
}

impl RuntimeDaemonReporter for DaemonLog {
    fn fatal_latched(&self, outcome: &Outcome) {
        self.fatal.lock().expect("daemon log").push(outcome.clone());
    }

    fn step_skipped(&self, _outcome: &Outcome) {}

    fn warning_recorded(&self, _outcome: &Outcome) {}
}

/// A start as actingd makes one, with its configuration manifest: its `config.*` runtime facts
/// leave the runtime-fact store to seal.
fn manifest() -> RuntimeConfigManifest {
    let mut parameters = vec![ConfigParameter {
        key: "bind_port".to_owned(),
        value: FactScalar::Integer(0),
        source: ConfigParameterSource::Default,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn status(host: &RuntimeHost, ids: &IdentifierIssuer, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(
        &runtime_request(ids, RuntimeOperation::Status),
        ConnectionId::new(connection).expect("connection"),
    )
    .expect("a status receipt")
}

#[test]
fn a51_t3_reads_the_writer_does_not_answer_in_time_latch_nothing() {
    let root = TempDir::new().expect("tempdir");
    // No performance loop runs: the test runs the seal of its tick itself.
    let host = RuntimeHost::start(
        config(&root)
            .with_config_manifest(manifest())
            .with_frame_retention_enabled(false)
            .with_ledger_reply_timeout(REPLY_TIMEOUT),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let hold = WriterReadHoldGate::arm(root.path()).expect("read hold");

    // The writer is held in the first read; the status's head read and the seal's wait for it.
    let held = status(&host, &ids, 1);
    let tick = host.append_runtime_fact_snapshot_for_test();

    assert!(
        host.fatal_error().expect("health").is_none(),
        "nothing latched: {held:?} {tick:?}"
    );
    assert!(
        !host.is_shutdown_requested().expect("shutdown flag"),
        "no stop requested"
    );
    assert!(
        hold.wait_held(Duration::ZERO),
        "the writer was held in a read"
    );
    assert_eq!(held.state(), RuntimeReceiptState::Failed, "{held:?}");
    assert_eq!(
        held.outcome().code().as_str(),
        LedgerCode::WriterResponseTimeout.as_str(),
        "{held:?}"
    );
    let tick = tick.expect_err("the seal's head read was not answered");
    assert_eq!(
        tick.code(),
        LedgerCode::WriterResponseTimeout.as_str(),
        "{tick:?}"
    );
    assert!(!tick.is_fatal(), "{tick:?}");

    hold.release();
    let answered = status(&host, &ids, 2);
    assert_eq!(
        answered.state(),
        RuntimeReceiptState::Completed,
        "{answered:?}"
    );
    host.close().expect("close host");
}

#[test]
fn a51_t5_the_first_latched_fatal_reaches_the_daemon_log_at_once() {
    let root = TempDir::new().expect("tempdir");
    let log = Arc::new(DaemonLog::default());
    let host = RuntimeHost::start(
        config(&root).with_daemon_reporter(log.clone()),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    assert!(log.fatal().is_empty(), "nothing latched at the start");
    let condition = RuntimeHostError::fatal(
        RuntimeErrorCode::LedgerFailure,
        HostLocation::RuntimeStateSource,
        RuntimeErrorCode::LedgerFailure,
    );

    host.latch_fatal_for_test(condition.clone())
        .expect("the condition latched");

    // In the log at once, before any close, as its daemon line carries it.
    assert_eq!(log.fatal(), vec![condition.daemon_outcome()]);
    // A later condition is not the first one.
    host.latch_fatal_for_test(RuntimeHostError::fatal(
        RuntimeErrorCode::LedgerFailure,
        HostLocation::ReadRuntimeFactPosition,
        RuntimeErrorCode::LedgerFailure,
    ))
    .expect("a second condition latched");
    assert_eq!(log.fatal().len(), 1, "only the first condition");
    assert!(host.close().is_err(), "the close ends with the condition");
    assert_eq!(log.fatal().len(), 1, "the close reports nothing more");
}
