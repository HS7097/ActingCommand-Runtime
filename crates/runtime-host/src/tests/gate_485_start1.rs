// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #485 START-1 (0.12.75; register §1.489; brief `rd-1119/brief-485-start-settle-bounded.md`
//! and the coordinator's ruling B): the start's settlement of the direct runs earlier epochs left
//! unfinished. Each epoch is written by a host on a manual Runtime clock and closed; the next start
//! on the same state root settles it. The records each start writes for the epoch's runs are
//! compared with what the start of 0.12.74 writes for the same ledger, record for record and in
//! ledger order (pinned on that code first, then kept):
//!
//! - a run the process left during its entry check, before any package admission (its records the
//!   capture's `task.evidence_indexed` facts alone), gets its `task.terminal_intent` and its
//!   `task.cancelled` (`contained_task_recovered_after_restart`), and no lease record, its lease
//!   having expired;
//! - a run whose lease expired before the process ended, its package admitted, gets the same two
//!   records and no lease record;
//! - two such runs and a finished run in one epoch: the two settled in the order of their first
//!   records, the finished run nothing.
//!
//! The run whose granted lease is still open (the release written as well) is gate_epoch_close's
//! gate 2 and #411 A65's gates, which this slice leaves unchanged.

use super::*;
use crate::codes::HostCode;
use actingcommand_contract::{IssuedRunId, IssuedTaskId};

const ALIAS: &str = "neutral.instance";

/// A run's task and run identities, minted for records a test appends.
#[derive(Clone, Copy)]
struct RunIds {
    task: IssuedTaskId,
    run: IssuedRunId,
}

impl RunIds {
    fn mint(ids: &IdentifierIssuer) -> Self {
        Self {
            task: ids.mint_task_id().expect("task id"),
            run: ids.mint_run_id().expect("run id"),
        }
    }

    fn run_id(&self) -> RunId {
        *self.run.transport()
    }
}

/// What one epoch holds.
#[derive(Clone, Copy, Default)]
struct Spec {
    /// A finished run: its package admitted, `filler` records of its own, its terminal; its lease
    /// released.
    finished: bool,
    /// The finished run's own records, and as many client actions, records of no run.
    filler: usize,
    /// A run left during its entry check: two evidence facts, no package admitted.
    entry_check: bool,
    /// A run left after its package admission and its start.
    lease_expired: bool,
}

/// The runs an epoch wrote, and the lease of its unfinished runs (expired before the end).
#[derive(Default)]
struct Runs {
    finished: Option<RunId>,
    entry_check: Option<RunId>,
    lease_expired: Option<RunId>,
    lease: Option<LeaseId>,
}

/// A state root whose host wrote `Spec` and was closed, as the next start finds it.
struct Epoch {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    instance_id: InstanceId,
    runs: Runs,
}

fn epoch_config(root: &TempDir, clock: &Arc<ManualRuntimeClock>) -> RuntimeHostConfig {
    config(root)
        .with_runtime_clock(clock.clone())
        .with_scheduler(SchedulerConfig {
            maximum_client_heartbeat_interval_ms: 20,
            takeover_cooldown_ms: 40,
            lease_ttl_ms: 200,
            ..SchedulerConfig::default()
        })
}

fn start_epoch_host(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    instance_id: InstanceId,
) -> RuntimeHost {
    RuntimeHost::start(
        epoch_config(root, clock),
        Arc::new(FakeProvider::one(
            ALIAS,
            instance_id,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

fn package_admitted() -> TaskSemanticFact {
    TaskSemanticFact::PackageAdmitted {
        package_label: "neutral.semantic.task".to_owned(),
        task_label: "task".to_owned(),
        response_deadline_monotonic_ms: None,
        content_hash: None,
        variables: None,
    }
}

/// The fact a run's capture writes, the first an entry check records.
fn evidence_indexed() -> TaskSemanticFact {
    TaskSemanticFact::EvidenceIndexed {
        frame_width: 16,
        frame_height: 9,
    }
}

fn append(
    host: &RuntimeHost,
    request: &RuntimeRequest,
    token: &LeaseToken,
    run: RunIds,
    fact: TaskSemanticFact,
) {
    host.append_contained_task_semantic_for_test(request, token, run.task, run.run, fact)
        .expect("append a run's record");
}

fn record_client_action(client: &mut TestClient) {
    let action = client.request(RuntimeOperation::RecordClientAction {
        action: ClientActionRecord::new("settings", "save", ClientActionKind::Button, None, None)
            .expect("client action"),
    });
    assert!(
        matches!(
            client.send(&action).result(),
            Some(RuntimeResult::ClientActionRecorded)
        ),
        "the client action is recorded"
    );
}

fn write_epoch(spec: Spec) -> Epoch {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let instance_id = instance_id();
    let host = start_epoch_host(&root, &clock, instance_id);
    let mut client = TestClient::connect(&host);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let mut runs = Runs::default();
    if spec.finished {
        let (_, token) = client.acquire(ALIAS);
        let request = client.request(RuntimeOperation::Health);
        let run = RunIds::mint(&ids);
        append(&host, &request, &token, run, package_admitted());
        for _ in 0..spec.filler {
            append(&host, &request, &token, run, evidence_indexed());
        }
        host.append_contained_task_terminal_for_test(
            &request,
            &token,
            run.task,
            run.run,
            TaskOutcome::Success,
            false,
            Some("neutral/terminal".to_owned()),
            1,
            None,
        )
        .expect("the finished run's terminal");
        let release = client.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            client.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        runs.finished = Some(run.run_id());
    }
    for _ in 0..spec.filler {
        record_client_action(&mut client);
    }
    if spec.entry_check || spec.lease_expired {
        let (_, token) = client.acquire(ALIAS);
        if spec.entry_check {
            let request = client.request(RuntimeOperation::Health);
            let run = RunIds::mint(&ids);
            append(&host, &request, &token, run, evidence_indexed());
            append(&host, &request, &token, run, evidence_indexed());
            runs.entry_check = Some(run.run_id());
        }
        if spec.lease_expired {
            let request = client.request(RuntimeOperation::Health);
            let run = RunIds::mint(&ids);
            append(&host, &request, &token, run, package_admitted());
            append(&host, &request, &token, run, TaskSemanticFact::RunStarted);
            append(&host, &request, &token, run, evidence_indexed());
            runs.lease_expired = Some(run.run_id());
        }
        clock.advance(250);
        host.expire_lease_once_for_test(&token)
            .expect("the runs' lease expires before the process ends");
        runs.lease = Some(token.lease_id());
    }
    drop(client);
    host.close().expect("close the epoch's host");
    Epoch {
        root,
        clock,
        instance_id,
        runs,
    }
}

/// The next start on the epoch's state root.
fn restart(epoch: &Epoch) -> RuntimeHost {
    let host = start_epoch_host(&epoch.root, &epoch.clock, epoch.instance_id);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host
}

/// A record as the comparison reads it: its run, its type, and a terminal's outcome code.
type Written = (RunId, EventType, Option<String>);

fn terminal_code(event: &PersistedEvent) -> Option<String> {
    match event.payload() {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::TerminalCommitted { outcome, .. } => outcome
                .as_ref()
                .map(|outcome| outcome.code().as_str().to_owned()),
            _ => None,
        },
        _ => None,
    }
}

/// What `host`'s start wrote after its own start event: the records of the epoch's runs, in ledger
/// order, and how many lease records of the unfinished runs' lease (a grant, a release, an
/// expiry and the like; a run's terminal records carry its lease link too, and are the first).
fn written_by_the_start(host: &RuntimeHost, epoch: &Epoch) -> (Vec<Written>, usize) {
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("the ledger's records");
    let start = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::RuntimeStarted | EventType::RuntimeTakeover
            )
        })
        .map(PersistedEvent::sequence)
        .max()
        .expect("the start's own start event");
    let runs = [
        epoch.runs.finished,
        epoch.runs.entry_check,
        epoch.runs.lease_expired,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let after = events
        .iter()
        .filter(|event| event.sequence() > start)
        .collect::<Vec<_>>();
    let written = after
        .iter()
        .filter_map(|event| {
            let run = *event.links().run_id()?;
            runs.contains(&run)
                .then(|| (run, event.event_type(), terminal_code(event)))
        })
        .collect();
    let lease_records = after
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::LeaseGranted
                    | EventType::LeaseTransferred
                    | EventType::LeaseRenewed
                    | EventType::LeaseReleased
                    | EventType::LeaseExpired
                    | EventType::LeaseTransitionIntent
                    | EventType::LeaseTransitionFailed
            ) && epoch
                .runs
                .lease
                .is_some_and(|lease| event.links().lease_id() == Some(&lease))
        })
        .count();
    (written, lease_records)
}

/// The two records the start writes for a run it settles: its terminal intent, then its
/// cancellation as recovered after the restart.
fn settled(run: RunId) -> [Written; 2] {
    [
        (run, EventType::TaskTerminalIntent, None),
        (
            run,
            EventType::TaskCancelled,
            Some(
                HostCode::ContainedTaskRecoveredAfterRestart
                    .as_str()
                    .to_owned(),
            ),
        ),
    ]
}

/// Point 2 of the ruling: a run the process left during its entry check, before any package
/// admission, is settled as 0.12.74 settles it.
#[test]
fn start1_a_run_left_during_its_entry_check_is_settled_as_before() {
    let epoch = write_epoch(Spec {
        entry_check: true,
        ..Spec::default()
    });
    let run = epoch.runs.entry_check.expect("the entry check's run");
    let host = restart(&epoch);
    assert_eq!(
        written_by_the_start(&host, &epoch),
        (settled(run).to_vec(), 0)
    );
    host.close().expect("close the start");
}

/// Point 2 of the ruling: a run whose lease expired before the process ended is settled as
/// 0.12.74 settles it, its terminal and no lease record.
#[test]
fn start1_a_run_whose_lease_expired_before_the_end_is_settled_as_before() {
    let epoch = write_epoch(Spec {
        lease_expired: true,
        ..Spec::default()
    });
    let run = epoch
        .runs
        .lease_expired
        .expect("the run of the expired lease");
    let host = restart(&epoch);
    assert_eq!(
        written_by_the_start(&host, &epoch),
        (settled(run).to_vec(), 0)
    );
    host.close().expect("close the start");
}

/// Test 2: the unfinished runs of one epoch are settled in the order of their first records, the
/// same records as 0.12.74 writes; the finished run gets nothing.
#[test]
fn start1_the_runs_of_one_epoch_are_settled_as_before_in_their_order() {
    let epoch = write_epoch(Spec {
        finished: true,
        filler: 0,
        entry_check: true,
        lease_expired: true,
    });
    let entry = epoch.runs.entry_check.expect("the entry check's run");
    let expired = epoch
        .runs
        .lease_expired
        .expect("the run of the expired lease");
    let host = restart(&epoch);
    let expected = settled(entry)
        .into_iter()
        .chain(settled(expired))
        .collect::<Vec<_>>();
    assert_eq!(written_by_the_start(&host, &epoch), (expected, 0));
    host.close().expect("close the start");
}
