// SPDX-License-Identifier: AGPL-3.0-only

// #382 H-1-rest (test plan v2.2 section 3.6 H-1 and section 3.10 H-1-rest; inventory-host G1,
// G3, I2 and I3): the install transition on the manual clock. Since HOST-I2 every installation
// deadline reads the host's `RuntimeClock`, so a test moves a transition past its deadline by
// moving the clock, never by waiting; the held start's checkpoints are #672's preparation hook
// (HOST-I3), reused as it is.
// - G1 (host keys 8 and 9): a held start whose `Preparing` checkpoint holds provider assembly
//   gets its Release receipt (`preparing`) while it is held there; once the clock passes the
//   release deadline the transition is Failed `release_timeout` with admission closed, and the
//   start records exactly one lifecycle failure.
// - G3 (host keys 7 and 8): the action x phase table (INSTALL.md; contracts/runtime-interface.md
//   "Refusals"; inventory-host keys 7 and 8). Each accepted action gives its new phase and
//   admission; each refused one is denied under its registered code, changes neither phase nor
//   admission and records exactly one lifecycle failure, its effect not performed (#352). Ticket
//   and owner mismatches are refused, and Abort is taken only while Draining or Drained. The
//   three deadlines run on the manual clock: `drain_timeout` aborts the drain and reopens
//   admission; `held_timeout` and `release_timeout` fail the start with admission closed.
// Every wait returns as soon as its message arrives; `GATE_WAIT` only bounds a broken run.

use super::*;
use actingcommand_contract::{
    FactScalar, InstallHeldStartup, InstallTransitionAction, InstallTransitionPhase,
    InstallTransitionStatus, InstallTransitionTicket, RuntimeInfo, RuntimePayload,
    RuntimeShutdownTarget,
};
use std::sync::Mutex;

const GATE_WAIT: Duration = Duration::from_secs(120);
const SALT: &[u8] = b"runtime-host-test-salt";
const HELD_TRANSITION: &str = "h1-held";
const HELD_TIMEOUT_MS: u64 = 60_000;
const RELEASE_TIMEOUT_MS: u64 = 60_000;
const DRAIN_TIMEOUT_MS: u64 = 60_000;
const INSTANCE: &str = "node.a";
// The installer's queries while a drain finishes (as gate_378_shutfu's): the idle Runtime drains
// at a tick, and every query ticks.
const POLL_LIMIT: usize = 100_000;

// The manual Runtime clock of the install and ladder rows (HOST-I2): its monotonic time, the one
// every installation deadline and the ladder's readiness wait read, moves only when the test
// moves it or a wait of the contained-task kernel spends it (test plan E-3, EXEC-I3: the kernel's
// timers read this clock and its waits are this clock's `pause`). Its wall time moves 1 ms at
// every reading, so records keep strictly increasing times as on a real clock (a fact observed
// at the same instant as the one before it, from another source, is refused).
pub(super) struct ManualMonotonicClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
}

impl ManualMonotonicClock {
    pub(super) fn new(unix_ms: u64) -> Self {
        Self {
            unix_ms: AtomicU64::new(unix_ms),
            monotonic_ms: AtomicU64::new(0),
        }
    }

    // Moves the monotonic time on by `duration_ms`.
    pub(super) fn advance(&self, duration_ms: u64) {
        self.monotonic_ms.fetch_add(duration_ms, Ordering::SeqCst);
    }
}

impl RuntimeClock for ManualMonotonicClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.fetch_add(1, Ordering::SeqCst) + 1,
            monotonic_ms: self.monotonic_ms.load(Ordering::SeqCst),
        })
    }

    // Test plan E-3 (§3.10): a wait of the contained-task kernel moves the monotonic time forward
    // by its duration, in whole milliseconds rounded up, and returns at once, as
    // `ManualRuntimeClock` does; a kernel wait therefore ends at its budget on this clock too.
    fn pause(&self, duration: Duration) {
        let duration_ms = duration.as_nanos().div_ceil(1_000_000);
        self.advance(u64::try_from(duration_ms).unwrap_or(u64::MAX));
    }
}

// A held start on its own thread, on `clock`: the hook reports each checkpoint, goes on at
// `Held` and holds provider assembly at `Preparing` until the test lets it go on.
struct HeldStart {
    root: PathBuf,
    held: InstallHeldStartup,
    checkpoints: mpsc::Receiver<PreparationCheckpoint>,
    proceed: mpsc::Sender<()>,
    results: mpsc::Receiver<RuntimeHostResult<RuntimeHost>>,
}

impl HeldStart {
    fn spawn(root: &Path, clock: &Arc<ManualMonotonicClock>) -> Self {
        let (checkpoint_sender, checkpoints) = mpsc::channel();
        let (proceed, proceed_receiver) = mpsc::channel::<()>();
        let hook_ends = Mutex::new((checkpoint_sender, proceed_receiver));
        let hook: PreparationTestHook = Arc::new(
            move |checkpoint: PreparationCheckpoint,
                  _probe: &PreparationProbe<'_>|
                  -> PreparationTestAction {
                let ends = hook_ends.lock().expect("hook channels");
                let (checkpoint_sender, proceed) = &*ends;
                checkpoint_sender
                    .send(checkpoint)
                    .expect("report the checkpoint");
                if checkpoint == PreparationCheckpoint::Preparing {
                    proceed
                        .recv_timeout(GATE_WAIT)
                        .expect("the test lets the held start go on");
                }
                PreparationTestAction::Continue
            },
        );
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let held = InstallHeldStartup {
            transition_id: HELD_TRANSITION.to_owned(),
            request_id: *issuer.mint_request_id().expect("request id").transport(),
            timeout_ms: HELD_TIMEOUT_MS,
            previous: None,
        };
        let start_root = root.to_path_buf();
        let start_held = held.clone();
        let start_clock = Arc::clone(clock);
        let (result_sender, results) = mpsc::channel();
        thread::spawn(move || {
            // The test may have stopped waiting; then the result has no reader.
            let _ = result_sender.send(RuntimeHost::start(
                RuntimeHostConfig::new(start_root, SALT)
                    .with_install_held(start_held)
                    .with_preparation_test_hook(hook)
                    .with_runtime_clock(start_clock),
                Arc::new(FakeProvider::from_entries(Vec::new())),
            ));
        });
        Self {
            root: root.to_path_buf(),
            held,
            checkpoints,
            proceed,
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
            transition_id: HELD_TRANSITION.to_owned(),
            request_id: self.held.request_id,
        }
    }

    // The installer's connection to the held owner, its governance identity declared.
    fn installer(&self) -> TestClient {
        let mut installer = TestClient::connect_state_root(&self.root);
        installer.declare_governance_identity();
        installer
    }

    fn go_on(&self) {
        self.proceed.send(()).expect("let the held start go on");
    }

    // The start's result; it fails the test while the start has not ended within `GATE_WAIT`.
    fn ended(&self) -> RuntimeHostResult<RuntimeHost> {
        self.results
            .recv_timeout(GATE_WAIT)
            .expect("the held start ends")
    }
}

// The installer's control connection to a running Runtime, its governance identity declared.
fn installer(host: &RuntimeHost) -> TestClient {
    let mut installer = TestClient::connect(host);
    installer.set_receipt_read_timeout();
    installer.declare_governance_identity();
    installer
}

fn send_action(
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    action: InstallTransitionAction,
) -> RuntimeReceipt {
    let request = installer.request(RuntimeOperation::InstallTransition { target, action });
    installer.send(&request)
}

// An accepted action: its receipt is completed with the transition's status.
fn completed(
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    action: InstallTransitionAction,
) -> InstallTransitionStatus {
    let shown = format!("{action:?}");
    let receipt = send_action(installer, target, action);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{shown}: {}",
        receipt.outcome().code().as_str()
    );
    let Some(RuntimeResult::InstallTransition { status }) = receipt.result() else {
        panic!("{shown}: an install transition status: {receipt:?}")
    };
    status.clone()
}

fn query(
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    transition_id: &str,
) -> InstallTransitionStatus {
    completed(
        installer,
        target,
        InstallTransitionAction::Query {
            transition_id: transition_id.to_owned(),
        },
    )
}

// Queries the drain `transition_id` until it is drained; the drain stays Draining until then.
fn query_until_drained(
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    transition_id: &str,
) -> InstallTransitionStatus {
    for _ in 0..POLL_LIMIT {
        let status = query(installer, target, transition_id);
        if status.phase == InstallTransitionPhase::Drained {
            return status;
        }
        assert_eq!(
            status.phase,
            InstallTransitionPhase::Draining,
            "{transition_id}"
        );
    }
    panic!("{transition_id}: the idle Runtime did not drain");
}

// Every lifecycle failure of `owner_epoch`'s owner (all owners: `None`) in ledger order: its
// code and its effect.
fn lifecycle_failures(
    host: &RuntimeHost,
    owner_epoch: Option<actingcommand_contract::OwnerEpoch>,
) -> Vec<(String, EffectDisposition)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFailed),
        ..EventQuery::default()
    })
    .expect("lifecycle failures")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::Failed(payload)) => payload
            .lifecycle_failure()
            .filter(|failure| owner_epoch.is_none_or(|epoch| failure.owner_epoch() == epoch))
            .map(|_| {
                (
                    payload.outcome().code().as_str().to_owned(),
                    payload.effect_disposition(),
                )
            }),
        _ => None,
    })
    .collect()
}

// The phase and admission a refused action must leave: the transition `transition_id` (none:
// no transition is live).
type Live<'a> = Option<(&'a str, InstallTransitionPhase, bool)>;

// #352: a refused action is denied under `code`, leaves the live transition's phase and
// admission as they were, and records exactly one lifecycle failure, its effect not performed.
// `host` is `None` while a held start has not returned it; its failures are read at its end.
fn assert_refused(
    host: Option<&RuntimeHost>,
    installer: &mut TestClient,
    target: RuntimeShutdownTarget,
    action: InstallTransitionAction,
    code: &str,
    live: Live<'_>,
) {
    let shown = format!("{action:?}");
    let before = host.map(|host| lifecycle_failures(host, None));
    let receipt = send_action(installer, target, action);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{shown}");
    assert_eq!(receipt.outcome().code().as_str(), code, "{shown}");
    if let (Some(host), Some(before)) = (host, before) {
        let after = lifecycle_failures(host, None);
        assert_eq!(
            after.get(before.len()..),
            Some([(code.to_owned(), EffectDisposition::NotPerformed)].as_slice()),
            "{shown}: one lifecycle failure, not performed"
        );
    }
    if let Some((transition_id, phase, admission_closed)) = live {
        let status = query(installer, target, transition_id);
        assert_eq!(
            (status.phase, status.admission_closed),
            (phase, admission_closed),
            "{shown} changed the transition"
        );
    }
}

// The transition's last status fact in the ledger `host` reads.
fn last_transition_status(host: &RuntimeHost) -> BTreeMap<String, FactScalar> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFactRecorded),
        ..EventQuery::default()
    })
    .expect("runtime facts")
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
    .next_back()
    .expect("the transition's status fact")
}

// A cold start on the same root, to read what a start that has ended recorded.
fn cold_start(root: &Path) -> RuntimeHost {
    RuntimeHost::start(
        RuntimeHostConfig::new(root, SALT),
        Arc::new(FakeProvider::from_entries(Vec::new())),
    )
    .expect("a cold start after the held start")
}

fn other_request_id() -> actingcommand_contract::RequestId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_request_id()
        .expect("request id")
        .transport()
}

// G1 (host keys 8 and 9): the Release receipt comes while the `Preparing` checkpoint holds
// provider assembly; the clock passes the release deadline and the transition is Failed
// `release_timeout` with admission closed; the start ends under `release_timeout` and its owner
// recorded exactly one lifecycle failure.
#[test]
fn gate_h1_g1_a_release_held_at_preparing_fails_closed_once_the_clock_passes_its_deadline() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualMonotonicClock::new(POLICY_NOW_UNIX_MS));
    let start = HeldStart::spawn(root.path(), &clock);
    start.reached(PreparationCheckpoint::Held);
    let target = start.info().shutdown_target();
    let owner_epoch = start.info().owner_epoch();
    let mut installer = start.installer();

    let released = completed(
        &mut installer,
        target,
        InstallTransitionAction::Release {
            ticket: start.ticket(),
            timeout_ms: RELEASE_TIMEOUT_MS,
        },
    );
    assert_eq!(
        (released.phase, released.admission_closed),
        (InstallTransitionPhase::Preparing, true)
    );
    start.reached(PreparationCheckpoint::Preparing);
    assert!(
        start.results.try_recv().is_err(),
        "the start went on past its Preparing checkpoint"
    );

    clock.advance(RELEASE_TIMEOUT_MS);
    let failed = query(&mut installer, target, HELD_TRANSITION);
    assert_eq!(
        (
            failed.phase,
            failed.admission_closed,
            failed.failure_code.as_deref()
        ),
        (
            InstallTransitionPhase::Failed,
            true,
            Some("release_timeout")
        )
    );
    start.go_on();
    let stopped = start
        .ended()
        .err()
        .expect("the timed-out start ends with an error");
    drop(installer);
    assert_eq!(
        (stopped.code(), stopped.operation()),
        ("release_timeout", "install_transition"),
        "FATAL line: {}",
        stopped.complete_message()
    );

    let host = cold_start(root.path());
    let failures = lifecycle_failures(&host, Some(owner_epoch));
    host.close().expect("close the cold Runtime");
    assert_eq!(
        failures
            .iter()
            .map(|(code, _)| code.as_str())
            .collect::<Vec<_>>(),
        ["release_timeout"],
        "the held start's lifecycle failures"
    );
}

// G3, the running Runtime's rows (host keys 7 and 8): the drain's actions by phase, and the
// drain deadline on the manual clock.
#[test]
fn gate_h1_g3_a_drain_answers_each_action_by_its_phase_and_times_out_on_the_manual_clock() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualMonotonicClock::new(POLICY_NOW_UNIX_MS));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            INSTANCE,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host on the manual clock");
    let target = host.runtime_info().shutdown_target();
    let mut installer = installer(&host);
    let mut owner = TestClient::connect(&host);
    let release = |owner: &mut TestClient, token: LeaseToken| {
        let request = owner.request(RuntimeOperation::ReleaseLease { token });
        let receipt = owner.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    };

    // No live transition: a Query or an Abort names nothing; another owner's target is refused.
    assert_refused(
        Some(&host),
        &mut installer,
        target,
        InstallTransitionAction::Query {
            transition_id: "h1-g3-none".to_owned(),
        },
        "install_transition_unknown",
        None,
    );
    assert_refused(
        Some(&host),
        &mut installer,
        target,
        InstallTransitionAction::Abort {
            ticket: InstallTransitionTicket {
                target,
                transition_id: "h1-g3-none".to_owned(),
                request_id: other_request_id(),
            },
        },
        "install_transition_unknown",
        None,
    );
    let other_owner = RuntimeShutdownTarget {
        owner_epoch: *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport(),
        ..target
    };
    assert_refused(
        Some(&host),
        &mut installer,
        other_owner,
        InstallTransitionAction::BeginDrain {
            transition_id: "h1-g3-a".to_owned(),
            timeout_ms: DRAIN_TIMEOUT_MS,
        },
        "install_owner_mismatch",
        None,
    );

    // Draining: a lease is held, so the drain stays open, admission closed.
    let (_, token) = owner.acquire(INSTANCE);
    let begun = completed(
        &mut installer,
        target,
        InstallTransitionAction::BeginDrain {
            transition_id: "h1-g3-a".to_owned(),
            timeout_ms: DRAIN_TIMEOUT_MS,
        },
    );
    assert_eq!(
        (begun.phase, begun.admission_closed),
        (InstallTransitionPhase::Draining, true)
    );
    let ticket = begun.ticket;
    let draining: Live<'_> = Some(("h1-g3-a", InstallTransitionPhase::Draining, true));
    for (action, code) in [
        (
            InstallTransitionAction::BeginDrain {
                transition_id: "h1-g3-b".to_owned(),
                timeout_ms: DRAIN_TIMEOUT_MS,
            },
            "install_transition_conflict",
        ),
        (
            InstallTransitionAction::BeginDrain {
                transition_id: "h1-g3-a".to_owned(),
                timeout_ms: DRAIN_TIMEOUT_MS,
            },
            "install_transition_conflict",
        ),
        (
            InstallTransitionAction::CommitShutdown {
                ticket: ticket.clone(),
            },
            "install_not_drained",
        ),
        (
            InstallTransitionAction::Release {
                ticket: ticket.clone(),
                timeout_ms: RELEASE_TIMEOUT_MS,
            },
            "install_release_state_invalid",
        ),
        (
            InstallTransitionAction::Abort {
                ticket: InstallTransitionTicket {
                    request_id: other_request_id(),
                    ..ticket.clone()
                },
            },
            "install_ticket_mismatch",
        ),
    ] {
        assert_refused(Some(&host), &mut installer, target, action, code, draining);
    }

    // Drained once idle; Abort from Drained reopens admission, and a second Abort is refused.
    release(&mut owner, token);
    let drained = query_until_drained(&mut installer, target, "h1-g3-a");
    assert!(drained.admission_closed);
    let aborted = completed(
        &mut installer,
        target,
        InstallTransitionAction::Abort {
            ticket: ticket.clone(),
        },
    );
    assert_eq!(
        (aborted.phase, aborted.admission_closed),
        (InstallTransitionPhase::Aborted, false)
    );
    assert_refused(
        Some(&host),
        &mut installer,
        target,
        InstallTransitionAction::Abort { ticket },
        "install_abort_state_invalid",
        Some(("h1-g3-a", InstallTransitionPhase::Aborted, false)),
    );

    // Abort from Draining reopens admission.
    let (_, token) = owner.acquire(INSTANCE);
    let begun = completed(
        &mut installer,
        target,
        InstallTransitionAction::BeginDrain {
            transition_id: "h1-g3-c".to_owned(),
            timeout_ms: DRAIN_TIMEOUT_MS,
        },
    );
    assert_eq!(begun.phase, InstallTransitionPhase::Draining);
    let aborted = completed(
        &mut installer,
        target,
        InstallTransitionAction::Abort {
            ticket: begun.ticket,
        },
    );
    assert_eq!(
        (aborted.phase, aborted.admission_closed),
        (InstallTransitionPhase::Aborted, false)
    );
    release(&mut owner, token);

    // drain_timeout: the clock passes the drain's deadline; the drain is Aborted under
    // `drain_timeout`, admission reopens, and one lifecycle failure records the timeout.
    completed(
        &mut installer,
        target,
        InstallTransitionAction::BeginDrain {
            transition_id: "h1-g3-d".to_owned(),
            timeout_ms: DRAIN_TIMEOUT_MS,
        },
    );
    query_until_drained(&mut installer, target, "h1-g3-d");
    clock.advance(DRAIN_TIMEOUT_MS);
    let timed_out = query(&mut installer, target, "h1-g3-d");
    assert_eq!(
        (
            timed_out.phase,
            timed_out.admission_closed,
            timed_out.failure_code.as_deref()
        ),
        (
            InstallTransitionPhase::Aborted,
            false,
            Some("drain_timeout")
        )
    );
    assert_eq!(
        lifecycle_failures(&host, None)
            .iter()
            .filter(|(code, _)| code == "drain_timeout")
            .count(),
        1,
        "one lifecycle failure records the drain timeout"
    );

    // CommitShutdown from Drained ends the drain with the owner's shutdown.
    completed(
        &mut installer,
        target,
        InstallTransitionAction::BeginDrain {
            transition_id: "h1-g3-e".to_owned(),
            timeout_ms: DRAIN_TIMEOUT_MS,
        },
    );
    let drained = query_until_drained(&mut installer, target, "h1-g3-e");
    let commit = send_action(
        &mut installer,
        target,
        InstallTransitionAction::CommitShutdown {
            ticket: drained.ticket,
        },
    );
    assert_eq!(
        commit.state(),
        RuntimeReceiptState::Admitted,
        "{}",
        commit.outcome().code().as_str()
    );
    assert!(
        matches!(
            commit.result(),
            Some(RuntimeResult::ShutdownAccepted { target: accepted }) if *accepted == target
        ),
        "{:?}",
        commit.result()
    );
    drop(owner);
    drop(installer);
    host.close().expect("close the Runtime");
}

// G3, the held start's rows (host keys 7 and 8): Held takes only its Release; Preparing takes
// nothing more; the transition is Released, with admission open, once preparation finished.
#[test]
fn gate_h1_g3_a_held_start_answers_each_action_by_its_phase_and_opens_when_released() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualMonotonicClock::new(POLICY_NOW_UNIX_MS));
    let start = HeldStart::spawn(root.path(), &clock);
    start.reached(PreparationCheckpoint::Held);
    let target = start.info().shutdown_target();
    let owner_epoch = start.info().owner_epoch();
    let ticket = start.ticket();
    let mut installer = start.installer();

    let held: Live<'_> = Some((HELD_TRANSITION, InstallTransitionPhase::Held, true));
    for (action, code) in [
        (
            InstallTransitionAction::Abort {
                ticket: ticket.clone(),
            },
            "install_abort_state_invalid",
        ),
        (
            InstallTransitionAction::CommitShutdown {
                ticket: ticket.clone(),
            },
            "install_not_drained",
        ),
        (
            InstallTransitionAction::BeginDrain {
                transition_id: "h1-g3-held-drain".to_owned(),
                timeout_ms: DRAIN_TIMEOUT_MS,
            },
            "install_transition_conflict",
        ),
        (
            InstallTransitionAction::Release {
                ticket: InstallTransitionTicket {
                    request_id: other_request_id(),
                    ..ticket.clone()
                },
                timeout_ms: RELEASE_TIMEOUT_MS,
            },
            "install_ticket_mismatch",
        ),
    ] {
        assert_refused(None, &mut installer, target, action, code, held);
    }

    let preparing = completed(
        &mut installer,
        target,
        InstallTransitionAction::Release {
            ticket: ticket.clone(),
            timeout_ms: RELEASE_TIMEOUT_MS,
        },
    );
    assert_eq!(
        (preparing.phase, preparing.admission_closed),
        (InstallTransitionPhase::Preparing, true)
    );
    start.reached(PreparationCheckpoint::Preparing);
    let still_preparing: Live<'_> =
        Some((HELD_TRANSITION, InstallTransitionPhase::Preparing, true));
    for (action, code) in [
        (
            InstallTransitionAction::Release {
                ticket: ticket.clone(),
                timeout_ms: RELEASE_TIMEOUT_MS,
            },
            "install_release_state_invalid",
        ),
        (
            InstallTransitionAction::Abort { ticket },
            "install_abort_state_invalid",
        ),
    ] {
        assert_refused(None, &mut installer, target, action, code, still_preparing);
    }

    start.go_on();
    let host = start
        .ended()
        .unwrap_or_else(|error| panic!("the released start: {}", error.complete_message()));
    let released = query(&mut installer, target, HELD_TRANSITION);
    assert_eq!(
        (released.phase, released.admission_closed),
        (InstallTransitionPhase::Released, false)
    );
    let failures = lifecycle_failures(&host, Some(owner_epoch));
    drop(installer);
    host.close().expect("close the released Runtime");

    // Each refusal recorded exactly one lifecycle failure, its effect not performed.
    let mut refused = failures
        .iter()
        .map(|(code, effect)| (code.as_str(), *effect))
        .collect::<Vec<_>>();
    refused.sort_unstable_by_key(|(code, _)| *code);
    let not_performed = EffectDisposition::NotPerformed;
    assert_eq!(
        refused,
        [
            ("install_abort_state_invalid", not_performed),
            ("install_abort_state_invalid", not_performed),
            ("install_not_drained", not_performed),
            ("install_release_state_invalid", not_performed),
            ("install_ticket_mismatch", not_performed),
            ("install_transition_conflict", not_performed),
        ],
        "the held owner's lifecycle failures"
    );
}

// G3's held deadline (host key 8): the clock passes the held deadline while the installer never
// releases; the start ends under `held_timeout`, its transition Failed with admission closed,
// and its owner recorded exactly one lifecycle failure.
#[test]
fn gate_h1_g3_a_held_start_fails_closed_once_the_clock_passes_its_held_deadline() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualMonotonicClock::new(POLICY_NOW_UNIX_MS));
    let start = HeldStart::spawn(root.path(), &clock);
    start.reached(PreparationCheckpoint::Held);
    let owner_epoch = start.info().owner_epoch();

    clock.advance(HELD_TIMEOUT_MS);
    let stopped = start
        .ended()
        .err()
        .expect("the timed-out held start ends with an error");
    assert_eq!(
        (stopped.code(), stopped.operation()),
        ("held_timeout", "install_transition"),
        "FATAL line: {}",
        stopped.complete_message()
    );

    let host = cold_start(root.path());
    let failures = lifecycle_failures(&host, Some(owner_epoch));
    let status = last_transition_status(&host);
    host.close().expect("close the cold Runtime");
    assert_eq!(
        failures
            .iter()
            .map(|(code, _)| code.as_str())
            .collect::<Vec<_>>(),
        ["held_timeout"],
        "the held start's lifecycle failures"
    );
    assert_eq!(
        (
            status.get("transition_id"),
            status.get("phase"),
            status.get("admission_closed"),
            status.get("failure_code"),
        ),
        (
            Some(&FactScalar::String(HELD_TRANSITION.to_owned())),
            Some(&FactScalar::String("failed".to_owned())),
            Some(&FactScalar::Boolean(true)),
            Some(&FactScalar::String("held_timeout".to_owned())),
        ),
        "the held transition's last status: {status:?}"
    );
}
