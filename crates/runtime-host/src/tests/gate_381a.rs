// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 A, test plan H-1 (R2, R3′): installation control of a held start while it
//! prepares, through the held-start checkpoints (HOST-I3). Every wait returns as soon as its
//! message arrives; `GATE_WAIT` only bounds a broken run.

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
    start: thread::JoinHandle<RuntimeHostResult<RuntimeHost>>,
}

impl HeldStart {
    fn spawn(root: &Path) -> Self {
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
            timeout_ms: RELEASE_TIMEOUT_MS,
            previous: None,
        };
        let start_root = root.to_path_buf();
        let start_held = held.clone();
        let start = thread::spawn(move || {
            RuntimeHost::start(
                RuntimeHostConfig::new(start_root, SALT)
                    .with_install_held(start_held)
                    .with_preparation_test_hook(hook),
                Arc::new(FakeProvider::from_entries(Vec::new())),
            )
        });
        Self {
            root: root.to_path_buf(),
            held,
            checkpoints,
            commands,
            heads,
            start,
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
    fn release(&self) -> TestClient {
        let mut installer = TestClient::connect_state_root(&self.root);
        installer.declare_governance_identity();
        let release = installer.request(RuntimeOperation::InstallTransition {
            target: self.info().shutdown_target(),
            action: InstallTransitionAction::Release {
                ticket: self.ticket(),
                timeout_ms: RELEASE_TIMEOUT_MS,
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
        self.start.join().expect("held start thread")
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
    let start = HeldStart::spawn(root.path());
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release();
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
    let head_after = start.head();
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
        (action, host_failure_code(&poller.send(&request)))
    })
    .collect::<Vec<_>>();
    start.go_on(AtPreparing::Continue);
    drop(poller);
    drop(installer);
    start
        .finish()
        .expect("the released start finishes")
        .close()
        .expect("close the released Runtime");

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
    assert_eq!(head_after, head_before, "the Query appended to the ledger");
    for (action, code) in refusals {
        assert_eq!(
            code.as_deref(),
            Some("install_identity_required"),
            "{action:?} on an undeclared connection"
        );
    }
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
    let start = HeldStart::spawn(root.path());
    start.reached(PreparationCheckpoint::Held);
    let installer = start.release();
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
