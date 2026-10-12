// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #394 EPOCH-CLOSE (plan additions §8.3; Alice's 09-30 choice (a) of #332 Q5): the
//! two gate tests of a crash while a run is in flight. A child process starts a host, runs a
//! direct task until its first input is committed and its next capture waits, and is then
//! killed hard. The new start on the same state root (1) comes up, with its instance usable,
//! and (2) winds the killed epoch down: the closing records of the scopes the epoch left
//! occupied (reason: released by process exit), the release of the run's lease and the run
//! settled as interrupted. Every wait returns as soon as the awaited record or file appears;
//! `GATE_WAIT` only bounds a broken run.

use super::instance_workers::closed_ledger_events;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{
    OwnerEpoch, PriorEpochCloseBasis, PriorEpochCloseFact, PriorEpochScope, RuntimePayload,
    UnprovenReason,
};

const GATE_WAIT: Duration = Duration::from_secs(120);
const ALIAS: &str = "node.a";
const CRASH_ROOT_ENV: &str = "ACTINGCOMMAND_EPOCH_CLOSE_CRASH_ROOT";
const CRASH_MARKER_ENV: &str = "ACTINGCOMMAND_EPOCH_CLOSE_CRASH_MARKER";
const INSTANCE_FILE: &str = "epoch-close-instance.json";
const RUN_FILE: &str = "epoch-close-run.json";
const MARKER_FILE: &str = "epoch-close-crash.marker";
const PACKAGE_FILE: &str = "epoch-close-task.zip";

/// The test `config` on a state root given as a path (the child process has no `TempDir`).
fn epoch_config(root: &Path) -> RuntimeHostConfig {
    RuntimeHostConfig::new(root, b"runtime-host-test-salt")
        .with_policy_inputs(PolicyInputSnapshot::new(policy_facts(), policy_resources()))
        .with_procedure_manifest(procedure_manifest())
        .with_io_timeout(Duration::from_millis(500))
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 5_000,
            ..SchedulerConfig::default()
        })
}

fn start_host(root: &Path, registered: InstanceId) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    RuntimeHost::start(
        epoch_config(root),
        Arc::new(FakeProvider::one(ALIAS, registered, state)),
    )
    .expect("the new process starts after the crash")
}

/// The outcome and failure code of a task terminal of `run`.
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
                outcome,
                failure_code,
                ..
            } => Some((*outcome, failure_code.as_deref())),
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

/// The child process: a host on `CRASH_ROOT_ENV` runs one direct task. Its first capture is let
/// through and every later capture waits, so the run commits its first input and then stays in
/// flight with its lease held and the device in use. Once that input is recorded the child
/// notes the epoch, the run and its lease, writes the marker and waits to be killed.
#[test]
fn epoch_close_crash_child_process() {
    let Ok(root) = std::env::var(CRASH_ROOT_ENV) else {
        return;
    };
    let root = PathBuf::from(root);
    let marker = PathBuf::from(std::env::var_os(CRASH_MARKER_ENV).expect("marker path"));
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
    let deadline = Instant::now() + GATE_WAIT;
    let (run_id, lease_id) = loop {
        let events = host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events");
        let run_id = events
            .iter()
            .find(|event| {
                event.event_type() == EventType::TaskStarted
                    && event.links().request_id() == Some(&request_id)
            })
            .and_then(|event| event.links().run_id().copied());
        if let Some(run_id) = run_id {
            assert!(
                !events
                    .iter()
                    .any(|event| run_terminal(event, &run_id).is_some()),
                "the run ended before the crash point"
            );
            let lease_id = events
                .iter()
                .filter(|event| {
                    event.links().request_id() == Some(&request_id)
                        || event.links().run_id() == Some(&run_id)
                })
                .find_map(|event| event.links().lease_id().copied());
            let input_committed = events.iter().any(|event| {
                event.event_type() == EventType::InputCommitted
                    && (event.links().run_id() == Some(&run_id)
                        || event.links().request_id() == Some(&request_id))
            });
            if let (Some(lease_id), true) = (lease_id, input_committed) {
                break (run_id, lease_id);
            }
        }
        assert!(
            Instant::now() < deadline,
            "the run did not commit its first input"
        );
        thread::sleep(Duration::from_millis(20));
    };
    fs::write(
        root.join(RUN_FILE),
        serde_json::to_vec(&(host.runtime_info().owner_epoch(), run_id, lease_id))
            .expect("run identity bytes"),
    )
    .expect("run identity file");
    fs::write(marker, b"run in flight").expect("crash marker");
    loop {
        thread::sleep(Duration::from_secs(60));
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
    let marker = root.path().join(MARKER_FILE);
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::gate_epoch_close::epoch_close_crash_child_process",
            "--nocapture",
        ])
        .env(CRASH_ROOT_ENV, root.path())
        .env(CRASH_MARKER_ENV, &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the crash child");
    let deadline = Instant::now() + GATE_WAIT;
    while !marker.is_file() {
        if let Some(status) = child.try_wait().expect("poll the crash child") {
            panic!("the crash child exited before its crash point: {status}");
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill the timed-out child");
            let _ = child.wait();
            panic!("the crash child did not reach its crash point");
        }
        thread::sleep(Duration::from_millis(10));
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
/// epoch's closing records back, starts too.
#[test]
fn gate_after_a_crash_mid_run_the_new_process_starts_and_its_instance_is_usable() {
    let crashed = crash_mid_run();
    let host = start_host(crashed.root.path(), crashed.instance_id);
    assert_ne!(host.runtime_info().owner_epoch(), crashed.owner_epoch);
    assert!(host.fatal_error().expect("runtime health").is_none());
    let mut client = TestClient::connect(&host);
    let deadline = Instant::now() + GATE_WAIT;
    loop {
        let request = client.request(RuntimeOperation::acquire_lease(
            ALIAS,
            client.ids.mint_holder_id().expect("holder id"),
        ));
        let receipt = client.send(&request);
        if matches!(receipt.result(), Some(RuntimeResult::LeaseGranted { .. })) {
            break;
        }
        assert_eq!(
            receipt.error_projection().map(|error| error.code),
            Some(RuntimeErrorCode::LeaseCooldown),
            "only the takeover cool-down may hold the instance back: {:?}",
            receipt.state()
        );
        assert!(Instant::now() < deadline, "the instance stayed unusable");
        thread::sleep(Duration::from_millis(10));
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
