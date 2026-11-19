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
//!
//! What the settlement reads, through the host's count of the payload records its reads return
//! (`actingd contained_settlement`): the same with and without a page's worth of records of the
//! finished run and as many records of no run (test 1); an epoch with no unfinished run reads its
//! finished run's terminal and package admission and nothing else (test 3). The start log's stages
//! (`actingd start_stages`), on a scripted clock: every member named, the members adding up to
//! `total_ms` exactly (test 4); and a host's start that reached readiness ended every stage but the
//! daemon's, which a host started without the process's instant does not measure.

use super::*;
use crate::codes::HostCode;
use crate::recovery_timing::{StartStage, StartStages};
use actingcommand_contract::{IssuedRunId, IssuedTaskId};

/// Tests 1 and 3: the finished run's records of its own and the records of no run, one startup
/// read's page each.
const FILLER: usize = crate::recovery_read::RECOVERY_PAGE_EVENTS;

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

/// Test 1: the payload records the settlement reads do not grow with the records of a finished
/// run or with records of no run: one finished run and one run whose lease expired, then the same
/// with `FILLER` records more of the finished run's own and `FILLER` client actions.
#[test]
fn start1_the_records_read_do_not_grow_with_records_of_finished_runs_or_of_none() {
    let measure = |filler| {
        let epoch = write_epoch(Spec {
            finished: true,
            filler,
            entry_check: false,
            lease_expired: true,
        });
        let expired = epoch
            .runs
            .lease_expired
            .expect("the run of the expired lease");
        let host = restart(&epoch);
        assert_eq!(
            written_by_the_start(&host, &epoch),
            (settled(expired).to_vec(), 0)
        );
        let reads = host
            .contained_settlement_reads_for_test()
            .expect("what the settlement read");
        host.close().expect("close the start");
        reads
    };
    let small = measure(0);
    let large = measure(FILLER);
    assert_eq!((small.finished, small.read), (1, 1), "{small:?}");
    assert_eq!(large, small);
}

/// Test 3: an epoch with no unfinished run reads its finished run's terminal and package
/// admission, by type, and nothing else, whatever that run's own records.
#[test]
fn start1_an_epoch_without_an_unfinished_run_reads_next_to_nothing() {
    let epoch = write_epoch(Spec {
        finished: true,
        filler: FILLER,
        ..Spec::default()
    });
    let host = restart(&epoch);
    assert_eq!(written_by_the_start(&host, &epoch), (Vec::new(), 0));
    let reads = host
        .contained_settlement_reads_for_test()
        .expect("what the settlement read");
    assert_eq!(
        (reads.runs, reads.finished, reads.read, reads.records),
        (1, 1, 0, 2),
        "{reads:?}"
    );
    host.close().expect("close the start");
}

/// A reading of the stages' clock from `script`, one value each time it is read.
fn scripted(script: Vec<u64>) -> Box<dyn Fn() -> u64> {
    let readings = std::cell::RefCell::new(script.into_iter());
    Box::new(move || {
        readings
            .borrow_mut()
            .next()
            .expect("a scripted reading of the stages' clock")
    })
}

/// Test 4: on a scripted clock, the line `actingd start_stages` names every stage, and the
/// members of the stages that ended add up to `total_ms` exactly; a stage that did not end prints
/// `-`, and without the process's instant the first stage the host ends counts from the origin.
#[test]
fn start1_the_start_log_names_every_stage_and_the_stages_add_up_to_the_whole() {
    let mut stages = StartStages::with_reading(scripted(vec![
        3, 10, 10, 25, 26, 40, 41, 41, 60, 1_060, 1_061, 1_200, 1_210, 1_300, 1_340, 1_400, 1_450,
        1_500,
    ]));
    for stage in StartStage::ALL {
        stages.end(stage);
    }
    let times = stages.times();
    let line = times.to_string();
    let mut sum = 0;
    for stage in StartStage::ALL {
        let member = times.member_ms(stage).expect("every stage ended");
        assert!(
            line.contains(&format!("{}={member} ", stage.member())),
            "{line}"
        );
        sum += member;
    }
    assert_eq!(sum, 1_500);
    assert_eq!(times.total_ms(), Some(1_500));
    assert!(
        line.starts_with("daemon_ms=3 owner_ms=7 storage_ms=0 "),
        "{line}"
    );
    assert!(line.contains(" provider_ms=1000 "), "{line}");
    assert!(line.contains(" settle_ms=40 "), "{line}");
    assert!(line.ends_with(" publish_ms=50 total_ms=1500"), "{line}");

    let mut stages = StartStages::with_reading(scripted(vec![5, 9, 30]));
    stages.end(StartStage::Owner);
    stages.end(StartStage::Storage);
    stages.end(StartStage::LedgerOpen);
    let times = stages.times();
    assert_eq!(times.member_ms(StartStage::Daemon), None);
    assert_eq!(times.member_ms(StartStage::Owner), Some(5));
    assert_eq!(times.member_ms(StartStage::LedgerOpen), Some(21));
    assert_eq!(times.member_ms(StartStage::Settlement), None);
    assert_eq!(times.total_ms(), Some(30));
    let line = times.to_string();
    assert!(
        line.starts_with(
            "daemon_ms=- owner_ms=5 storage_ms=4 ledger_open_ms=21 frame_retention_ms=- "
        ),
        "{line}"
    );
    assert!(line.contains(" settle_ms=- "), "{line}");
    assert!(line.ends_with(" publish_ms=- total_ms=30"), "{line}");
}

/// Test 4, on a host: a start that reached readiness ended every stage but the daemon's (a host
/// started without the process's instant), and its members add up to its `total_ms`.
#[test]
fn start1_a_start_that_reached_readiness_ended_every_stage_after_the_daemons() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let times = host.start_stages_for_test();
    assert_eq!(times.member_ms(StartStage::Daemon), None);
    let sum = StartStage::ALL
        .into_iter()
        .skip(1)
        .map(|stage| {
            times
                .member_ms(stage)
                .expect("every stage of a start that reached readiness ended")
        })
        .sum::<u64>();
    assert_eq!(times.total_ms(), Some(sum));
    host.close().expect("close the host");
}
