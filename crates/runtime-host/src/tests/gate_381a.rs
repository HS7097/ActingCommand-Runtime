// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A, test plan H-1 (R2, R3′) and the coordinator's #672 ruling on the two
//! timeouts (R5′): installation control of a held start while it prepares, through the
//! held-start checkpoints (HOST-I3). Every wait returns as soon as its message arrives;
//! `GATE_WAIT` only bounds a broken run.

use super::*;
use actingcommand_contract::{
    FactScalar, InstallHeldStartup, InstallTransitionAction, InstallTransitionPhase,
    InstallTransitionTicket, RuntimeErrorProjection, RuntimeInfo, RuntimePayload,
};
use std::sync::Mutex;

const GATE_WAIT: Duration = Duration::from_secs(120);
const SALT: &[u8] = b"runtime-host-test-salt";
const TRANSITION_ID: &str = "held-381a";
const RELEASE_TIMEOUT_MS: u64 = 60_000;
const HELD_TIMEOUT_MS: u64 = 60_000;
/// The shortest deadline the contract allows (`validate_install_timeout`): it has passed by the
/// time any later step of the start looks at it.
const EXPIRED_TIMEOUT_MS: u64 = 1;

enum AtPreparing {
    ReportHead,
    Continue,
    Latch(&'static str),
}

/// A held start on its own thread, stopped by the test hook at `Preparing` until told to go on.
struct HeldStart {
    root: PathBuf,
    held: InstallHeldStartup,
    checkpoints: mpsc::Receiver<PreparationCheckpoint>,
    commands: mpsc::Sender<AtPreparing>,
    heads: mpsc::Receiver<u64>,
    results: mpsc::Receiver<RuntimeHostResult<RuntimeHost>>,
}

impl HeldStart {
    fn spawn(root: &Path, held_timeout_ms: u64) -> Self {
        let (checkpoint_sender, checkpoints) = mpsc::channel();
        let (commands, command_receiver) = mpsc::channel::<AtPreparing>();
        let (head_sender, heads) = mpsc::channel();
        let hook_ends = Mutex::new((checkpoint_sender, command_receiver, head_sender));
        let hook: PreparationTestHook = Arc::new(
            move |checkpoint: PreparationCheckpoint,
                  probe: &PreparationProbe<'_>|
                  -> PreparationTestAction {
                let ends = hook_ends.lock().expect("hook channels");
                let (checkpoint_sender, command_receiver, head_sender) = &*ends;
                checkpoint_sender
                    .send(checkpoint)
                    .expect("report the checkpoint");
                if checkpoint == PreparationCheckpoint::Held {
                    return PreparationTestAction::Continue;
                }
                loop {
                    match command_receiver
                        .recv_timeout(GATE_WAIT)
                        .expect("a test command while preparing")
                    {
                        AtPreparing::ReportHead => head_sender
                            .send(probe.ledger_head().expect("ledger head"))
                            .expect("report the ledger head"),
                        AtPreparing::Continue => return PreparationTestAction::Continue,
                        AtPreparing::Latch(operation) => {
                            return PreparationTestAction::LatchLedgerFailure { operation };
                        }
                    }
                }
            },
        );
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let held = InstallHeldStartup {
            transition_id: TRANSITION_ID.to_owned(),
            request_id: *issuer.mint_request_id().expect("request id").transport(),
            timeout_ms: held_timeout_ms,
            previous: None,
        };
        let start_root = root.to_path_buf();
        let start_held = held.clone();
        let (result_sender, results) = mpsc::channel();
        thread::spawn(move || {
            // The test may have stopped waiting; then the result has no reader.
            let _ = result_sender.send(RuntimeHost::start(
                RuntimeHostConfig::new(start_root, SALT)
                    .with_install_held(start_held)
                    .with_preparation_test_hook(hook),
                Arc::new(FakeProvider::from_entries(Vec::new())),
            ));
        });
        Self {
            root: root.to_path_buf(),
            held,
            checkpoints,
            commands,
            heads,
            results,
        }
    }

    fn reached(&self, expected: PreparationCheckpoint) {
        assert_eq!(
            self.checkpoints
                .recv_timeout(GATE_WAIT)
                .expect("the held start reaches its next checkpoint"),
            expected
        );
    }

    fn info(&self) -> RuntimeInfo {
        serde_json::from_slice(&fs::read(self.root.join(RUNTIME_INFO_FILE)).expect("runtime info"))
            .expect("runtime info JSON")
    }

    fn ticket(&self) -> InstallTransitionTicket {
        InstallTransitionTicket {
            target: self.info().shutdown_target(),
            transition_id: TRANSITION_ID.to_owned(),
            request_id: self.held.request_id,
        }
    }

    /// The installer's release, on a connection that declared its governance identity.
    fn release(&self, timeout_ms: u64) -> TestClient {
        let mut installer = TestClient::connect_state_root(&self.root);
        installer.declare_governance_identity();
        let release = installer.request(RuntimeOperation::InstallTransition {
            target: self.info().shutdown_target(),
            action: InstallTransitionAction::Release {
                ticket: self.ticket(),
                timeout_ms,
            },
        });
        let receipt = installer.send(&release);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert!(matches!(
            receipt.result(),
            Some(RuntimeResult::InstallTransition { status })
                if status.phase == InstallTransitionPhase::Preparing
        ));
        installer
    }

    fn head(&self) -> u64 {
        self.commands
            .send(AtPreparing::ReportHead)
            .expect("ask for the ledger head");
        self.heads
            .recv_timeout(GATE_WAIT)
            .expect("the ledger head while preparing")
    }

    fn go_on(&self, at_preparing: AtPreparing) {
        self.commands
            .send(at_preparing)
            .expect("let the held start go on");
    }

    fn finish(self) -> RuntimeHostResult<RuntimeHost> {
        self.ended().expect("the held start ends")
    }

    /// The start's result, or `None` while it has not ended within `GATE_WAIT`.
    fn ended(&self) -> Option<RuntimeHostResult<RuntimeHost>> {
        self.results.recv_timeout(GATE_WAIT).ok()
    }
}

fn host_failure_code(receipt: &RuntimeReceipt) -> Option<String> {
    receipt
        .error_projection()
        .and_then(RuntimeErrorProjection::host_code)
        .map(str::to_owned)
}

/// R2: while the new owner is `preparing` and before its deadline, a Query on a connection that
/// declared no governance identity completes and appends nothing; every other installation
/// action still requires the identity.
#[test]
fn gate_undeclared_query_while_preparing_completes_and_leaves_the_ledger_head() {
    let root = TempDir::new().expect("tempdir");
    let start = HeldStart::spawn(root.path(), HELD_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release(RELEASE_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Preparing);

    let head_before = start.head();
    let mut poller = TestClient::connect_state_root(root.path());
    let target = start.info().shutdown_target();
    let query = poller.request(RuntimeOperation::InstallTransition {
        target,
        action: InstallTransitionAction::Query {
            transition_id: TRANSITION_ID.to_owned(),
        },
    });
    let query_receipt = poller.send(&query);
    assert_eq!(
        query_receipt.state(),
        RuntimeReceiptState::Completed,
        "an undeclared Query while preparing: {:?}",
        host_failure_code(&query_receipt)
    );
    assert!(matches!(
        query_receipt.result(),
        Some(RuntimeResult::InstallTransition { status })
            if status.phase == InstallTransitionPhase::Preparing
    ));
    assert_eq!(
        start.head(),
        head_before,
        "the Query appended to the ledger"
    );

    let refusals = [
        InstallTransitionAction::BeginDrain {
            transition_id: "held-381a-next".to_owned(),
            timeout_ms: RELEASE_TIMEOUT_MS,
        },
        InstallTransitionAction::Release {
            ticket: start.ticket(),
            timeout_ms: RELEASE_TIMEOUT_MS,
        },
        InstallTransitionAction::Abort {
            ticket: start.ticket(),
        },
        InstallTransitionAction::CommitShutdown {
            ticket: start.ticket(),
        },
    ]
    .into_iter()
    .map(|action| {
        let request = poller.request(RuntimeOperation::InstallTransition {
            target,
            action: action.clone(),
        });
        let refusal = poller
            .send_result(&request)
            .map(|receipt| host_failure_code(&receipt))
            .map_err(|error| format!("{error:?}"));
        (action, refusal)
    })
    .collect::<Vec<_>>();
    start.go_on(AtPreparing::Continue);
    drop(poller);
    drop(installer);
    let released = start.finish();

    for (action, refusal) in refusals {
        assert_eq!(
            refusal,
            Ok(Some("install_identity_required".to_owned())),
            "{action:?} on an undeclared connection"
        );
    }
    released
        .unwrap_or_else(|error| panic!("the released start: {}", error.complete_message()))
        .close()
        .expect("close the released Runtime");
}

/// R3′: a Runtime failure latched while a held start prepares stops the start under its own
/// code, `install_startup_stopped` during `install_transition`, and names the latched cause as
/// `cause=<code> cause_operation=<operation>` on the FATAL line (actingd prints
/// `FATAL actingd: <complete_message>`), in the transition's status fact and in the lifecycle
/// failure's detail.
#[test]
fn gate_latched_failure_while_preparing_stops_the_start_under_its_own_code_with_the_cause() {
    const CAUSE: &str = "cause=ledger_failure cause_operation=append_runtime_event";
    let root = TempDir::new().expect("tempdir");
    let start = HeldStart::spawn(root.path(), HELD_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release(RELEASE_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Preparing);
    start.go_on(AtPreparing::Latch("append_runtime_event"));
    let stopped = start.finish().err().expect("the held start stops");
    drop(installer);

    assert_eq!(
        (stopped.code(), stopped.operation()),
        ("install_startup_stopped", "install_transition")
    );
    assert!(
        stopped.complete_message().contains(&format!(
            "install_startup_stopped during install_transition {CAUSE}"
        )),
        "FATAL line: {}",
        stopped.complete_message()
    );

    // The records of the stopped start, read through the next (cold) start of the same root.
    let host = RuntimeHost::start(
        RuntimeHostConfig::new(root.path(), SALT),
        Arc::new(FakeProvider::from_entries(Vec::new())),
    )
    .expect("a cold start after the stopped held start");
    let failures = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFailed),
            ..EventQuery::default()
        })
        .expect("lifecycle failures");
    let facts = host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::RuntimeFactRecorded),
            ..EventQuery::default()
        })
        .expect("runtime facts");
    host.close().expect("close the cold Runtime");

    let stopped_details = failures
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::Failed(payload)) => payload.lifecycle_failure(),
            _ => None,
        })
        .filter(|failure| failure.code() == "install_startup_stopped")
        .map(|failure| {
            failure
                .native_detail()
                .map(|detail| detail.text().to_owned())
        })
        .collect::<Vec<_>>();
    assert_eq!(stopped_details.len(), 1, "{stopped_details:?}");
    assert!(
        stopped_details[0]
            .as_deref()
            .is_some_and(|detail| detail.contains(CAUSE)),
        "lifecycle failure detail: {stopped_details:?}"
    );

    let failed_status = facts
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(payload))
                if payload.record().key == "host.install_transition" =>
            {
                match &payload.record().value {
                    ContractFactValue::RecordList(rows) => rows.first().cloned(),
                    _ => None,
                }
            }
            _ => None,
        })
        .filter(|row| {
            row.get("phase") == Some(&FactScalar::String("failed".to_owned()))
                && row.get("transition_id") == Some(&FactScalar::String(TRANSITION_ID.to_owned()))
        })
        .collect::<Vec<_>>();
    assert_eq!(failed_status.len(), 1, "{failed_status:?}");
    let string = |key: &str| match failed_status[0].get(key) {
        Some(FactScalar::String(value)) => Some(value.as_str()),
        _ => None,
    };
    assert_eq!(
        (
            string("failure_code"),
            string("cause"),
            string("cause_operation")
        ),
        (
            Some("install_startup_stopped"),
            Some("ledger_failure"),
            Some("append_runtime_event")
        ),
        "the failed transition's status fact: {:?}",
        failed_status[0]
    );
}

/// The #672 ruling (a), R5′: a release whose deadline passes while the start prepares ends the
/// start under `release_timeout` (operation `install_transition`), a top code the watchdog
/// restarts, never under `install_preparation_not_authorized`. The installer's polls tick the
/// deadline while the start waits at `Preparing`, so the timeout precedes the start's next check.
#[test]
fn gate_a_release_that_times_out_ends_the_start_under_release_timeout() {
    const POLL_LIMIT: usize = 100_000;
    let root = TempDir::new().expect("tempdir");
    let start = HeldStart::spawn(root.path(), HELD_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Held);
    let mut installer = start.release(EXPIRED_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Preparing);
    let target = start.info().shutdown_target();
    let mut failed = None;
    for _ in 0..POLL_LIMIT {
        let query = installer.request(RuntimeOperation::InstallTransition {
            target,
            action: InstallTransitionAction::Query {
                transition_id: TRANSITION_ID.to_owned(),
            },
        });
        if let Some(RuntimeResult::InstallTransition { status }) = installer.send(&query).result()
            && status.phase == InstallTransitionPhase::Failed
        {
            failed = Some(status.clone());
            break;
        }
    }
    start.go_on(AtPreparing::Continue);
    let ended = start.finish();
    drop(installer);

    let failed = failed.expect("the release deadline passes while the start prepares");
    assert_eq!(failed.failure_code.as_deref(), Some("release_timeout"));
    let stopped = ended.err().expect("the timed-out start ends with an error");
    assert_eq!(
        (stopped.code(), stopped.operation()),
        ("release_timeout", "install_transition"),
        "FATAL line: {}",
        stopped.complete_message()
    );
    assert!(
        stopped
            .complete_message()
            .starts_with("runtime host error release_timeout during install_transition"),
        "FATAL line: {}",
        stopped.complete_message()
    );
}

/// The #672 ruling (b), R5′: a held start whose held deadline passes while the installer never
/// releases, aborts or polls it ends under `held_timeout` (operation `install_transition`), a top
/// code the watchdog restarts, instead of staying held until something shuts it down.
#[test]
fn gate_a_held_start_whose_deadline_passes_ends_under_held_timeout() {
    let root = TempDir::new().expect("tempdir");
    let start = HeldStart::spawn(root.path(), EXPIRED_TIMEOUT_MS);
    start.reached(PreparationCheckpoint::Held);
    let Some(ended) = start.ended() else {
        panic!("the held start stayed held past its held deadline");
    };
    let stopped = ended
        .err()
        .expect("the timed-out held start ends with an error");
    assert_eq!(
        (stopped.code(), stopped.operation()),
        ("held_timeout", "install_transition"),
        "FATAL line: {}",
        stopped.complete_message()
    );
    assert!(
        stopped
            .complete_message()
            .starts_with("runtime host error held_timeout during install_transition"),
        "FATAL line: {}",
        stopped.complete_message()
    );
}
