// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #394 EPOCH-CLOSE (plan additions §8.3; Alice's 09-30 choice (a) of #332 Q5): the
//! two gate tests of a crash while a run is in flight. A child process starts a host, runs a
//! direct task until its first input is committed and its next capture waits, and is then
//! killed hard. The new start on the same state root (1) comes up, with its instance usable,
//! and (2) winds the killed epoch down: the closing records of the scopes the epoch left
//! occupied (reason: released by process exit), the release of the run's lease and the run
//! settled as interrupted. Every wait returns as soon as the awaited record or line arrives;
//! `GATE_WAIT` only bounds a broken run. The takeover cool-down of the new process passes on a
//! `ManualRuntimeClock` that the test advances.

use super::instance_workers::closed_ledger_events;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    MAX_RUNTIME_SUBSCRIPTION_WAIT_MS, OwnerEpoch, PriorEpochCloseBasis, PriorEpochCloseFact,
    PriorEpochScope, RuntimePayload, RuntimeSubscriptionRequest, SubscriptionCursor,
    UnprovenReason,
};
use std::io::{BufRead, BufReader};

const GATE_WAIT: Duration = Duration::from_secs(120);
const ALIAS: &str = "node.a";
const TAKEOVER_COOLDOWN_MS: u64 = 40;
const CRASH_ROOT_ENV: &str = "ACTINGCOMMAND_EPOCH_CLOSE_CRASH_ROOT";
/// The line the child prints once its run is in flight and the run's identity is on disk.
const CRASH_POINT_LINE: &str = "epoch-close crash point: run in flight";
const INSTANCE_FILE: &str = "epoch-close-instance.json";
const RUN_FILE: &str = "epoch-close-run.json";
const PACKAGE_FILE: &str = "epoch-close-task.zip";

/// The test `config` on a state root given as a path (the child process has no `TempDir`).
fn epoch_config(root: &Path) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root, b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest())
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: TAKEOVER_COOLDOWN_MS,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

fn start_host(root: &Path, registered: InstanceId) -> RuntimeHost {
    start_host_with(epoch_config(root), registered)
}

fn start_host_with(config: RuntimeHostConfig, registered: InstanceId) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        config,
        Arc::new(FakeProvider::one(ALIAS, registered, state)),
    )
    .expect("the new process starts after the crash")
}

/// The task result and the failure code (its outcome's code) of a task terminal of `run`.
fn run_terminal<'a>(
    event: &'a PersistedEvent,
    run: &RunId,
) -> Option<(TaskOutcome, Option<&'a str>)> {
    if event.links().run_id() != Some(run) {
        return None;
    }
    match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::TerminalCommitted {
                task_result,
                outcome,
                ..
            } => Some((
                *task_result,
                outcome.as_ref().map(|outcome| outcome.code().as_str()),
            )),
            _ => None,
        },
        _ => None,
    }
}

fn prior_epoch_fact(event: &PersistedEvent) -> Option<&PriorEpochCloseFact> {
    match event.payload() {
        EventPayload::Runtime(RuntimePayload::LifecycleObserved(payload)) => {
            payload.prior_epoch_close()
        }
        _ => None,
    }
}

fn lease_releases(events: &[PersistedEvent], lease: &LeaseId) -> usize {
    events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().lease_id() == Some(lease)
        })
        .count()
}

/// Waits until the ledger records an input commit. The subscription returns as soon as one is
/// appended (or at once when one is already there); a round that ends empty after the
/// contract's longest subscription wait is repeated, so the rounds together last `GATE_WAIT`.
fn await_an_input_commit(host: &RuntimeHost) {
    let mut client = TestClient::connect(host);
    client
        .stream
        .set_read_timeout(Some(GATE_WAIT))
        .expect("subscription read timeout");
    let rounds = GATE_WAIT
        .as_millis()
        .div_ceil(u128::from(MAX_RUNTIME_SUBSCRIPTION_WAIT_MS));
    let mut cursor = SubscriptionCursor::default();
    for _ in 0..rounds {
        let request = client.request(RuntimeOperation::SubscribeEvents {
            request: RuntimeSubscriptionRequest::new(
                EventQuery {
                    event_type: Some(EventType::InputCommitted),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
                cursor,
                MAX_RUNTIME_SUBSCRIPTION_WAIT_MS,
                1,
            )
            .expect("subscription request"),
        });
        let receipt = client.send(&request);
        let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
            panic!("expected an event batch: {:?}", receipt.state());
        };
        if !batch.events().is_empty() {
            return;
        }
        cursor = batch.next_cursor();
    }
    panic!("the run did not commit its first input");
}

/// The child process: a host on `CRASH_ROOT_ENV` runs one direct task. Its first capture is let
/// through and every later capture waits, so the run commits its first input and then stays in
/// flight with its lease held and the device in use. Once that input is recorded the child
/// notes the epoch, the run and its lease, prints `CRASH_POINT_LINE` and waits to be killed.
// Test plan v2.2 §4 #15: a child-process entry, not a test; its parent runs it with
// `--ignored --exact`.
#[test]
#[ignore]
fn epoch_close_crash_child_process() {
    let Ok(root) = std::env::var(CRASH_ROOT_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let registered: InstanceId =
        serde_json::from_slice(&fs::read(root.join(INSTANCE_FILE)).expect("instance bytes"))
            .expect("instance identifier");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        epoch_config(&root),
        Arc::new(FakeProvider::one(ALIAS, registered, Arc::clone(&state))),
    )
    .expect("child runtime host");
    state.capture_hold_after.store(
        state.capture_count.load(Ordering::Acquire) + 1,
        Ordering::Release,
    );
    let package = neutral_contained_task_package(true);
    let package_sha256 = format!("{:x}", Sha256::digest(&package));
    let package_path = root.join(PACKAGE_FILE);
    fs::write(&package_path, package).expect("write the task package");
    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::run_contained_task(
        ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(package_path.display().to_string(), package_sha256)
            .expect("task request"),
    ));
    let request_id = request.request_id();
    // The request is sent and its receipt never read: the run is still in flight at the kill.
    write_frame(
        &mut client.stream,
        &request,
        DEFAULT_RUNTIME_MAX_FRAME_BYTES,
    )
    .expect("send the run request");
    await_an_input_commit(&host);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let run_id = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::TaskStarted
                && event.links().request_id() == Some(&request_id)
        })
        .and_then(|event| event.links().run_id().copied())
        .expect("the run started");
    assert!(
        !events
            .iter()
            .any(|event| run_terminal(event, &run_id).is_some()),
        "the run ended before the crash point"
    );
    assert!(
        events.iter().any(|event| {
            event.event_type() == EventType::InputCommitted
                && (event.links().run_id() == Some(&run_id)
                    || event.links().request_id() == Some(&request_id))
        }),
        "the committed input is the run's"
    );
    let lease_id = events
        .iter()
        .filter(|event| {
            event.links().request_id() == Some(&request_id)
                || event.links().run_id() == Some(&run_id)
        })
        .find_map(|event| event.links().lease_id().copied())
        .expect("the run's lease");
    fs::write(
        root.join(RUN_FILE),
        serde_json::to_vec(&(host.runtime_info().owner_epoch(), run_id, lease_id))
            .expect("run identity bytes"),
    )
    .expect("run identity file");
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{CRASH_POINT_LINE}").expect("announce the crash point");
    stdout.flush().expect("flush the crash point");
    drop(stdout);
    loop {
        thread::park();
    }
}

/// A state root whose Runtime was killed hard while a direct run was in flight.
struct CrashedRun {
    root: TempDir,
    instance_id: InstanceId,
    /// The killed process's owner epoch.
    owner_epoch: OwnerEpoch,
    run_id: RunId,
    lease_id: LeaseId,
    /// The ledger as the killed process left it.
    prefix: Vec<PersistedEvent>,
}

fn crash_mid_run() -> CrashedRun {
    let root = TempDir::new().expect("tempdir");
    let instance_id = instance_id();
    fs::write(
        root.path().join(INSTANCE_FILE),
        serde_json::to_vec(&instance_id).expect("instance bytes"),
    )
    .expect("instance file");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--ignored",
            "--exact",
            "tests::gate_epoch_close::epoch_close_crash_child_process",
            "--nocapture",
        ])
        .env(CRASH_ROOT_ENV, root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the crash child");
    // The child's output is read to its end; the crash point is signalled the moment its line
    // arrives, and the end of the output before it means the child exited early.
    let output = child.stdout.take().expect("the crash child's output");
    let (crash_point_sender, crash_point) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else {
                break;
            };
            if line.contains(CRASH_POINT_LINE) {
                let _ = crash_point_sender.send(());
            }
        }
    });
    match crash_point.recv_timeout(GATE_WAIT) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let status = child.wait().expect("the crash child's exit status");
            panic!("the crash child exited before its crash point: {status}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            child.kill().expect("kill the timed-out child");
            let _ = child.wait();
            panic!("the crash child did not reach its crash point");
        }
    }
    // The hard kill: the process ends with its run in flight and its owner record in use.
    child.kill().expect("hard-kill the Runtime");
    let _ = child.wait();
    let (owner_epoch, run_id, lease_id): (OwnerEpoch, RunId, LeaseId) =
        serde_json::from_slice(&fs::read(root.path().join(RUN_FILE)).expect("run identity bytes"))
            .expect("run identity");
    let prefix = closed_ledger_events(root.path());
    CrashedRun {
        root,
        instance_id,
        owner_epoch,
        run_id,
        lease_id,
        prefix,
    }
}

impl CrashedRun {
    /// The time of the killed epoch's last record, where the new process's clock starts.
    fn last_record_unix_ms(&self) -> u64 {
        self.prefix
            .iter()
            .map(PersistedEvent::timestamp_unix_ms)
            .max()
            .expect("the killed epoch's records")
    }

    /// The closure scopes the killed epoch left occupied: those of its records of the run or
    /// of the run's lease (`contracts/ledger-store.md`: the old owner, instance, run and lease,
    /// and the request and correlation only when there is no run). Retention facts open no
    /// scope.
    fn occupied_scopes(&self) -> BTreeSet<PriorEpochScope> {
        self.prefix
            .iter()
            .filter(|event| {
                event.payload().artifact_retention().is_none()
                    && (event.links().run_id() == Some(&self.run_id)
                        || event.links().lease_id() == Some(&self.lease_id))
            })
            .filter_map(|event| {
                let links = event.links();
                let run = links.run_id().copied();
                Some(PriorEpochScope {
                    owner: self.owner_epoch,
                    instance: *links.instance_id()?,
                    run,
                    lease: links.lease_id().copied(),
                    request: match run {
                        Some(_) => None,
                        None => Some(*links.request_id()?),
                    },
                    correlation: match run {
                        Some(_) => None,
                        None => Some(*links.correlation_id()?),
                    },
                })
            })
            .collect()
    }
}

/// Gate 1, "the new process starts": after the hard kill the new process starts to ready on the
/// same state root, is not fatal, and its instance is usable once the takeover cool-down has
/// passed (`contracts/actingd-unlock-owner.md`); the start after it, which reads the killed
/// epoch's closing records back, starts too. The new process runs on a manual clock that starts
/// where the killed epoch's records end; the test moves it past the cool-down.
#[test]
fn gate_after_a_crash_mid_run_the_new_process_starts_and_its_instance_is_usable() {
    let crashed = crash_mid_run();
    let clock = Arc::new(ManualRuntimeClock::new(crashed.last_record_unix_ms(), 0));
    let host = start_host_with(
        epoch_config(crashed.root.path()).with_runtime_clock(clock.clone()),
        crashed.instance_id,
    );
    assert_ne!(host.runtime_info().owner_epoch(), crashed.owner_epoch);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let mut client = TestClient::connect(&host);
    let acquire = |client: &mut TestClient| {
        let request = client.request(RuntimeOperation::acquire_lease(
            ALIAS,
            client.ids.mint_holder_id().expect("holder id"),
        ));
        client.send(&request)
    };
    let granted = |receipt: &RuntimeReceipt| {
        matches!(receipt.result(), Some(RuntimeResult::LeaseGranted { .. }))
    };
    let during_cooldown = acquire(&mut client);
    if !granted(&during_cooldown) {
        assert_eq!(
            during_cooldown.error_projection().map(|error| error.code),
            Some(RuntimeErrorCode::LeaseCooldown),
            "only the takeover cool-down may hold the instance back: {:?}",
            during_cooldown.state()
        );
        clock.advance(TAKEOVER_COOLDOWN_MS);
        let after_cooldown = acquire(&mut client);
        assert!(
            granted(&after_cooldown),
            "the instance stayed unusable after the takeover cool-down: {:?} {:?}",
            after_cooldown.state(),
            after_cooldown.error_projection().map(|error| error.code)
        );
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close the new process");
    let host = start_host(crashed.root.path(), crashed.instance_id);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the start after it");
}

/// Gate 2, "the old state is wound down": the new start writes the killed epoch's closing
/// records, one import of the epoch and one scope close for every scope it left occupied, all
/// with the reason released by process exit; the run's lease is released once; and the run,
/// which has no terminal of its own, is settled once as interrupted.
#[test]
fn gate_after_a_crash_mid_run_the_new_start_winds_the_old_epoch_down() {
    let crashed = crash_mid_run();
    let (old, run, lease) = (crashed.owner_epoch, crashed.run_id, crashed.lease_id);
    assert!(
        !crashed
            .prefix
            .iter()
            .any(|event| run_terminal(event, &run).is_some()),
        "the killed run has no terminal"
    );
    assert_eq!(lease_releases(&crashed.prefix, &lease), 0, "nor a release");
    let occupied = crashed.occupied_scopes();
    assert!(
        occupied.iter().any(|scope| scope.run == Some(run)),
        "{occupied:?}"
    );
    let host = start_host(crashed.root.path(), crashed.instance_id);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let released_by_exit = PriorEpochCloseBasis::Unproven {
        reason: UnprovenReason::ProcessExitOnly,
    };

    // The closing records.
    let imports = events
        .iter()
        .filter_map(|event| match prior_epoch_fact(event) {
            Some(PriorEpochCloseFact::OwnerImported(record)) if record.evidence.subject == old => {
                Some((event, record))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(import_event, import)] = imports.as_slice() else {
        panic!("one import of the killed epoch: {}", imports.len());
    };
    assert_eq!(import.basis, released_by_exit);
    let import_source = TerminalEvent {
        sequence: import_event.sequence(),
        event_id: *import_event.event_id(),
    };
    let closes = events
        .iter()
        .filter_map(|event| match prior_epoch_fact(event) {
            Some(PriorEpochCloseFact::ScopeClosed(record)) if record.subject.owner == old => {
                Some(record)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        closes
            .iter()
            .all(|close| close.basis == released_by_exit && close.proof == import_source),
        "{closes:?}"
    );
    for scope in &occupied {
        assert_eq!(
            closes
                .iter()
                .filter(|close| close.subject == *scope)
                .count(),
            1,
            "one close of {scope:?}"
        );
    }

    // The lease release.
    assert_eq!(lease_releases(&events, &lease), 1);

    // The run settled as interrupted.
    let terminals = events
        .iter()
        .filter_map(|event| run_terminal(event, &run))
        .collect::<Vec<_>>();
    assert_eq!(
        terminals,
        [(
            TaskOutcome::Cancelled,
            Some(HostCode::ContainedTaskRecoveredAfterRestart.as_str())
        )]
    );
    host.close().expect("close the new process");
}
