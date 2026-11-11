// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-6b2 (register §1.314 rows R6B-1, R6B-2, R6B-4 and R6B-7 with the coordinator's
// answers, §1.316; contracts/policy-suspension.md, "A stopped run"; contracts/runtime-interface.md,
// "Workbench request kinds", the row `StopInstanceRun`; coordinator work order
// rd-1111/brief-r6b2-stop-settle.md): a run the scheduler started is stopped as a client's direct
// run is, and settles as a stop. Through the host, on a manual clock and a fake device:
// - a stopped policy run: one `InputReset` on the run's lease before that lease's release, its
//   terminal the `task.failed` with `contained_task_cancelled`, one `policy.execution_recorded` of
//   the kind `stopped`, a later stop answering `settled` with `reset: done`; no failure streak,
//   backoff, retry round, pause, condition or owed rerun in the pair's decision memory (held and
//   replayed alike), no ladder, the snapshot `cancelled`, an interruption and no failure in the
//   routine summary;
// - a sensitive task's stopped run is not paused;
// - with `scheduling.rerun.condition_codes` and a `recovery_ladder.triggers` item naming
//   `contained_task_cancelled`, the stop is no condition and starts no ladder;
// - the live settlement of a stopped owed rerun ends it, and nothing promotes it again;
// - the restart settles a stopped run from its terminal as `stopped`; a run with no terminal as
//   interrupted, as before;
// - a failed reset fences only its instance and the stop starts no ladder of its own (the fence
//   is handed on as every fence is, to a `resource_fence` ladder or to a person); the other
//   instance's run goes on;
// - a stop sent while the run prepares its package reaches it; the run ends at its first check;
// - a fact refresh read is stopped and is no failure: its snapshot shows `cancelled` with no
//   failure streak.
// Every alias, id and package is neutral.

use super::gate_369s5::direct_run_package;
use super::gate_384_d2s4::{
    TerminalReadings, admit_refresh, chain_sources, intent_of, recognition_package,
};
use super::gate_386_sr2a::once_a_day_sources;
use super::gate_386_sr2b1::{admit, start as start_one};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::host::ContainedTaskCheckpointPoint;
use crate::policy_control::PolicyValueRecord;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameterSource, EventAction, InstanceRunReset, InstanceRunStop,
    PolicyControlStateRecord, RecoveryTriggerStage, RequestId, RoutineSummaryGranularity,
    RuntimeConfigManifest, RuntimeLifecyclePhase,
};
use actingcommand_policy::{FACT_REFRESH_TASK_ID, TaskTerminalState};
use actingcommand_runtime_config::scheduling::RERUN_CONDITION_CODES;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use actingcommand_scheduler::ClaimKind;
use serde_json::json;
use std::net::SocketAddr;

const X_ALIAS: &str = POLICY_INSTANCE_ALIAS;
const OTHER: &str = "r6b2.instance.other";
/// The fixture catalog's task (`policy_sources`).
const TASK: &str = "fixture.observe";
const CANCELLED: &str = "contained_task_cancelled";
const TRIGGERS: &str = "recovery_ladder.triggers";
/// The id of the task-stage trigger item `stop_naming_manifest` names the stop's code in.
const STOP_RULE: &str = "stop_named";
/// How long a test waits for something the host does on another thread.
const GATE_WAIT: Duration = Duration::from_secs(30);

// ---- The fixture.

/// The policy's facts for the fixture's two registered instances, whose aliases the host requires
/// the configured identities to be (`validate_policy_input_authority`): X as `policy_facts` names
/// it, and the other instance with X's static identity under its own alias, unavailable to the
/// policy. The catalog's task is X's alone; the other instance's run is a direct one
/// (`gate_419_iso2::scheduled` and `gate_419_iso5a::fixture` configure their Y so).
fn two_instance_facts() -> EvaluationFacts {
    let mut facts = policy_facts();
    let mut other = facts.instances[0].clone();
    other.instance_id = OTHER.to_owned();
    other.available = false;
    facts.instances.push(other);
    facts
}

/// The configured policy inputs of a host with both instances registered.
fn two_instance_inputs() -> PolicyInputSnapshot {
    PolicyInputSnapshot::new(two_instance_facts(), policy_resources())
}

/// A host on a manual clock with the policy instance X and another instance, both the policy's
/// configured identities, and X's scheduled run of the neutral package admitted under `sources`.
struct Fixture {
    root: TempDir,
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    address: SocketAddr,
    x: InstanceId,
    y: InstanceId,
    x_state: Arc<FakeState>,
    y_package: String,
    context: Box<PolicyRunContext>,
    request: ContainedTaskRequest,
}

impl Fixture {
    fn start(sources: &CatalogSources, manifest: Option<RuntimeConfigManifest>) -> Self {
        let root = TempDir::new().expect("tempdir");
        let x_state = Arc::new(FakeState::default());
        let y_state = Arc::new(FakeState::default());
        // A one-tap package whose target page comes after the tap.
        let x_package = direct_run_package(&root, &x_state);
        let y_package = direct_run_package(&root, &y_state);
        let (x, y) = (instance_id(), instance_id());
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let mut host_config = config(&root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(two_instance_inputs())
            .with_scheduled_procedures(["procedure.observe".to_owned()]);
        if let Some(manifest) = manifest {
            host_config = host_config.with_config_manifest(manifest);
        }
        let host = RuntimeHost::start(
            host_config,
            Arc::new(FakeProvider::from_entries([
                (X_ALIAS.to_owned(), x, Arc::clone(&x_state)),
                (OTHER.to_owned(), y, Arc::clone(&y_state)),
            ])),
        )
        .expect("runtime host");
        host.routine_summary_set_local_offset_for_test(0)
            .expect("the host's zone");
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
        let cycle = host
            .evaluate_policy_cycle_with_test_inputs(
                &two_instance_facts(),
                &policy_resources(),
                EvaluationTime {
                    unix_ms: POLICY_NOW_UNIX_MS,
                    monotonic_ms: POLICY_NOW_UNIX_MS,
                },
                7,
                PolicyTrigger::FactsChanged,
            )
            .expect("evaluate policy dispatch");
        let evaluation = cycle.evaluation.as_ref().expect("policy evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.instance_id == X_ALIAS)
            .unwrap_or_else(|| panic!("X's dispatch intent: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("dispatch reason chain")
            .clone();
        assert_eq!(intent.task_id, TASK);
        record_policy_approval(&host, &intent);
        let request = ContainedTaskRequest::new(x_package).expect("the scheduled request");
        let PolicyDispatchAdmission::Granted { context } = host
            .admit_scheduled_policy_dispatch(
                &intent,
                &reasons,
                &policy_context(&host, &intent),
                &request,
            )
            .expect("the admission")
        else {
            panic!("expected a policy run context")
        };
        let address = host.runtime_info().socket_addr().expect("runtime address");
        Self {
            root,
            host,
            clock,
            address,
            x,
            y,
            x_state,
            y_package,
            context,
            request,
        }
    }

    /// Runs X's scheduled run with a stop of it sent at `point` (answering at once); the stop's
    /// answer, the run's request and the error the run ends with.
    fn stop_at(&self, point: ContainedTaskCheckpointPoint) -> (RuntimeReceipt, RequestId, String) {
        self.stop_at_then(point, |_| {})
    }

    /// `stop_at`, with `then` run on the device's state right after the stop was answered.
    fn stop_at_then(
        &self,
        point: ContainedTaskCheckpointPoint,
        then: impl FnOnce(&FakeState) + Send + 'static,
    ) -> (RuntimeReceipt, RequestId, String) {
        let address = self.address;
        let device = Arc::clone(&self.x_state);
        let (sent, answered) = mpsc::channel();
        let checkpoint = self
            .host
            .run_at_leased_checkpoint_for_test(
                point,
                self.x,
                self.context.lease_token().lease_id(),
                move |identity| {
                    let run = identity.request_id();
                    let answer = stop(address, X_ALIAS, run, 0);
                    then(&device);
                    sent.send((answer, run)).expect("the stop's answer");
                },
            )
            .expect("arm the run's checkpoint");
        let error = self
            .host
            .run_scheduled_contained_task(&self.context, &self.request)
            .expect_err("the stopped run does not complete");
        assert_eq!(checkpoint.consumed(), 1, "the run reached {point:?}");
        let (answer, run) = answered.recv_timeout(GATE_WAIT).expect("the stop's answer");
        (answer, run, error.code().to_owned())
    }

    /// The pair's value record as the host holds it, which must equal the one today's replay of
    /// every dispatch event rebuilds.
    fn held_record(&self, task_id: &str) -> PolicyControlStateRecord {
        held_record(&self.host, task_id)
    }

    /// X's pair's task snapshot.
    fn snapshot(&self, task_id: &str) -> actingcommand_policy::TaskRuntimeSnapshot {
        snapshot_of(&self.host, &self.clock, task_id)
    }
}

/// A configuration whose `recovery_ladder.triggers` holds one task-stage item naming the stop's
/// code and whose `scheduling.rerun.condition_codes` lists it, every other key the shipped
/// default, as actingd assembles it (`gate_387_d1c::ladder_manifest`).
fn stop_naming_manifest() -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({
            "recovery_ladder": {
                "triggers": [{"id": STOP_RULE, "stages": ["task"], "codes": [CANCELLED]}]
            },
            "scheduling": {"rerun": {"condition_codes": [CANCELLED]}}
        }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    for key in [TRIGGERS, RERUN_CONDITION_CODES] {
        assert!(
            parameters.iter().any(|parameter| parameter.key == key
                && matches!(parameter.source, ConfigParameterSource::Explicit)),
            "{key} is the file's"
        );
    }
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// `StopInstanceRun` of `run` on `alias` with `wait_ms`, on a new connection.
fn stop(address: SocketAddr, alias: &str, run: RequestId, wait_ms: u64) -> RuntimeReceipt {
    let mut client = TestClient::connect_address(address);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::StopInstanceRun {
        instance_alias: alias.to_owned(),
        request_id: run,
        wait_ms,
    });
    client.send(&request)
}

/// The stop a completed receipt answers.
fn stopped(receipt: &RuntimeReceipt) -> InstanceRunStop {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        "request_stop_instance_run_completed"
    );
    let Some(RuntimeResult::InstanceRunStopped { stopped }) = receipt.result() else {
        panic!("a stop: {receipt:?}");
    };
    *stopped
}

/// The first answer of a stop that reached the run while it ran: still stopping.
fn assert_asked(answer: &RuntimeReceipt, run: RequestId) {
    assert_eq!(
        stopped(answer),
        InstanceRunStop {
            request_id: run,
            settled: false,
            reset: None,
        }
    );
    assert!(answer.terminal().is_some(), "the stop is recorded");
}

/// The pair `task_id` of X: its value record as the host holds it, equal to the one today's
/// replay of every dispatch event rebuilds.
fn held_record(host: &RuntimeHost, task_id: &str) -> PolicyControlStateRecord {
    let (held, replayed, _) = host
        .decision_memory_for_test()
        .expect("the decision memory");
    let key = format!("decision_memory/{X_ALIAS}/{task_id}");
    let record = |records: &BTreeMap<String, PolicyValueRecord>| match records.get(&key) {
        Some(PolicyValueRecord::Control(record)) => (**record).clone(),
        other => panic!("the pair's value record: {other:?}"),
    };
    assert_eq!(record(&held), record(&replayed), "held and replayed");
    record(&held)
}

/// X's snapshot of `task_id` as the evaluation projects it now.
fn snapshot_of(
    host: &RuntimeHost,
    clock: &ManualRuntimeClock,
    task_id: &str,
) -> actingcommand_policy::TaskRuntimeSnapshot {
    let now = RuntimeClock::sample(clock).expect("the clock").unix_ms;
    host.policy_task_snapshots_for_test(now)
        .expect("the task snapshots")
        .into_iter()
        .find(|snapshot| snapshot.task_id == task_id && snapshot.instance_id == X_ALIAS)
        .unwrap_or_else(|| panic!("a snapshot of {task_id}"))
}

/// No stopped-record member of a failure: no streak, backoff, retry round, pause or owed rerun.
fn assert_no_failure_state(record: &PolicyControlStateRecord) {
    assert_eq!(record.failure_streak, None, "no streak, backoff or pause");
    assert_eq!(record.failure_settled, None, "no failure state");
    assert_eq!(record.retry_round, None, "no retry round");
    assert_eq!(record.condition_reruns, None, "no owed rerun");
}

/// The ledger sequences of the committed resets and of the releases of `lease_id`.
fn lease_resets_and_releases(host: &RuntimeHost, lease_id: LeaseId) -> (Vec<u64>, Vec<u64>) {
    let events = host
        .query_persisted_events_for_test(EventQuery {
            lease_id: Some(lease_id),
            ..EventQuery::default()
        })
        .expect("the lease's records");
    let resets = events
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Input(InputPayload::Committed(detail))
                    if detail.action() == EventAction::InputReset
            )
        })
        .map(PersistedEvent::sequence)
        .collect();
    let releases = events
        .iter()
        .filter(|event| event.event_type() == EventType::LeaseReleased)
        .map(PersistedEvent::sequence)
        .collect();
    (resets, releases)
}

/// The resets the fake device was asked for.
fn device_resets(state: &FakeState) -> usize {
    state
        .input_actions
        .lock()
        .expect("fake input actions")
        .iter()
        .filter(|action| matches!(action, InputAction::Reset))
        .count()
}

/// The records of `event_type` linked to the run `context`.
fn run_events(
    host: &RuntimeHost,
    event_type: EventType,
    context: &PolicyRunContext,
) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        run_id: Some(context.run_id()),
        ..EventQuery::default()
    })
    .expect("the run's records")
}

/// The run's one terminal is `task.failed` with the stop's code, as every scheduled cancel's.
fn assert_cancelled_terminal(host: &RuntimeHost, context: &PolicyRunContext) {
    let failed = run_events(host, EventType::TaskFailed, context);
    let [terminal] = failed.as_slice() else {
        panic!("one task.failed: {failed:#?}");
    };
    let EventPayload::Task(TaskPayload::Semantic(payload)) = terminal.payload() else {
        panic!("a semantic terminal: {terminal:?}");
    };
    let TaskSemanticFact::TerminalCommitted {
        task_result: TaskOutcome::Failure,
        outcome: Some(outcome),
        ..
    } = payload.fact()
    else {
        panic!("a failed terminal: {terminal:?}");
    };
    assert_eq!(outcome.code().as_str(), CANCELLED);
    assert!(run_events(host, EventType::TaskCompleted, context).is_empty());
    assert!(run_events(host, EventType::TaskCancelled, context).is_empty());
}

/// The execution records of `decision_id`, in ledger order.
fn executions(host: &RuntimeHost, decision_id: &str) -> Vec<PolicyExecutionOutcome> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyExecutionRecorded),
        ..EventQuery::default()
    })
    .expect("the execution records")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
            if payload.decision_id() == decision_id =>
        {
            Some(payload.policy_result().clone())
        }
        _ => None,
    })
    .collect()
}

/// `decision_id`'s one execution record is of the kind `stopped`, with the stop's code.
fn assert_stopped_record(host: &RuntimeHost, decision_id: &str) {
    let records = executions(host, decision_id);
    let [PolicyExecutionOutcome::Stopped { outcome, .. }] = records.as_slice() else {
        panic!("one stopped record: {records:#?}");
    };
    assert_eq!(outcome.code().as_str(), CANCELLED);
}

/// No ladder started or was suppressed, and none holds or waits for `alias`.
fn assert_no_ladder(host: &RuntimeHost, alias: &str) {
    let events = all_events(host);
    assert!(
        !phases(&events).into_iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
                | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
        )),
        "no ladder record"
    );
    let (holder, queued) = host
        .instance_claims_for_test(alias)
        .expect("instance claims");
    assert_ne!(holder, Some(ClaimKind::Ladder));
    assert!(!queued.contains(&ClaimKind::Ladder), "{queued:?}");
}

/// The stopped run `context` starts no ladder of its own, whatever `recovery_ladder.triggers`
/// lists: no ladder record, started or suppressed, of the stage `task`, of the trigger item
/// `stop_named` or naming the run. The fence a failed reset leaves is handed on by the lease sweep
/// as every fence is (#403, #419), to a `resource_fence` ladder (no rule, no run) or to a person;
/// that ladder is allowed and not required, so this holds before and after the sweep's tick.
fn assert_no_ladder_of_the_stop(host: &RuntimeHost, context: &PolicyRunContext) {
    let run = context.run_id();
    let events = all_events(host);
    let own = phases(&events)
        .into_iter()
        .filter(|(event, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, .. } => {
                trigger.stage == RecoveryTriggerStage::Task
                    || trigger.rule.as_deref() == Some(STOP_RULE)
                    || trigger.run_id == Some(run)
                    || event.links().run_id() == Some(&run)
            }
            RuntimeLifecyclePhase::RecoveryLadderSuppressed { rule, .. } => {
                rule.as_deref() == Some(STOP_RULE) || event.links().run_id() == Some(&run)
            }
            _ => false,
        })
        .map(|(_, phase)| phase)
        .collect::<Vec<_>>();
    assert!(own.is_empty(), "the stop's own ladder: {own:#?}");
}

/// The status row of `alias`.
fn instance_row(address: SocketAddr, alias: &str) -> actingcommand_contract::RuntimeInstanceStatus {
    let mut reader = TestClient::connect_address(address);
    let request = reader.request(RuntimeOperation::Status);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("the status: {receipt:?}");
    };
    status
        .instances()
        .iter()
        .find(|row| row.instance_alias() == alias)
        .cloned()
        .unwrap_or_else(|| panic!("{alias} is listed"))
}

/// The day's routine summary of `alias`: its successes, failures and interruptions, summed.
fn routine_totals(address: SocketAddr, alias: &str) -> (u32, usize, u32) {
    let mut reader = TestClient::connect_address(address);
    let request = reader.request(RuntimeOperation::RoutineSummary {
        instance: Some(alias.to_owned()),
        granularity: RoutineSummaryGranularity::Day,
        since_revision: None,
    });
    let receipt = reader.send(&request);
    let Some(RuntimeResult::RoutineSummary { summary }) = receipt.result() else {
        panic!("a routine summary: {receipt:?}");
    };
    let tasks = summary
        .instances
        .iter()
        .flat_map(|instance| instance.buckets.iter())
        .flat_map(|bucket| bucket.tasks.iter())
        .collect::<Vec<_>>();
    (
        tasks.iter().map(|task| task.successes).sum(),
        tasks.iter().map(|task| task.failures.len()).sum(),
        tasks.iter().map(|task| task.interruptions).sum(),
    )
}

/// Closes a host whose Runtime latched on the injected ledger failure.
fn close_latched(host: RuntimeHost) {
    if let Err(error) = host.close() {
        assert_eq!(error.code(), "ledger_failure", "{error:?}");
    }
}

// ---- Tests.

// R6B-1, R6B-2, R6B-4: X's scheduled run is stopped at its checkpoint. The stop answers at once
// that it is still stopping; the run ends `contained_task_cancelled`, the Runtime resets the device
// once with the run's lease before that lease's release, the run's terminal is the `task.failed`
// every scheduled cancel writes, and it settles as one `stopped` record. A later stop answers
// settled with the reset done and no cause. Nothing of a failure is kept: no streak, backoff, retry
// round, pause, condition or owed rerun, held and replayed alike; no ladder; the snapshot shows
// `cancelled`; the routine summary counts an interruption and no failure; X is neither fenced nor
// busy, and the Runtime is not halted.
#[test]
fn r6b2_a_stopped_policy_run_is_reset_on_its_lease_and_settles_as_a_stop() {
    let fixture = Fixture::start(&policy_sources(1), None);
    let (first, run, error) = fixture.stop_at(ContainedTaskCheckpointPoint::PackageAdmitted);
    assert_asked(&first, run);
    assert_eq!(error, CANCELLED);

    let lease_id = fixture.context.lease_token().lease_id();
    let (resets, releases) = lease_resets_and_releases(&fixture.host, lease_id);
    assert_eq!(resets.len(), 1, "one reset: {resets:?}");
    assert_eq!(releases.len(), 1, "one release: {releases:?}");
    assert!(resets[0] < releases[0], "{resets:?} {releases:?}");
    assert_eq!(device_resets(&fixture.x_state), 1);
    assert_cancelled_terminal(&fixture.host, &fixture.context);
    assert_stopped_record(&fixture.host, fixture.context.decision_id());

    let again = stop(fixture.address, X_ALIAS, run, 0);
    assert_eq!(
        stopped(&again),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Done),
        }
    );
    assert!(again.outcome().causes().is_empty(), "{again:?}");

    assert_no_failure_state(&fixture.held_record(TASK));
    assert_no_ladder(&fixture.host, X_ALIAS);
    let snapshot = fixture.snapshot(TASK);
    assert_eq!(snapshot.terminal_state, Some(TaskTerminalState::Cancelled));
    assert_eq!(
        snapshot.last_dispatched_unix_ms,
        Some(fixture.context.admission().activity.admitted_at_unix_ms)
    );
    assert_eq!(snapshot.failure_streak, None, "the pair has no settlement");
    assert_eq!(routine_totals(fixture.address, X_ALIAS), (0, 0, 1));
    let row = instance_row(fixture.address, X_ALIAS);
    assert!(row.activity().is_none(), "{row:?}");
    assert!(row.resource_fence().is_none(), "{row:?}");
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none()
    );
    fixture.host.close().expect("close host");
}

// A sensitive task's stopped run is not paused: its record is `stopped` and its pair keeps no
// failure state at all.
#[test]
fn r6b2_a_sensitive_tasks_stopped_run_is_not_paused() {
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("task fixture");
    tasks["tasks"][0]["sensitive"] = json!(true);
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("task bytes");
    let fixture = Fixture::start(&sources, None);
    let (first, run, error) = fixture.stop_at(ContainedTaskCheckpointPoint::PackageAdmitted);
    assert_asked(&first, run);
    assert_eq!(error, CANCELLED);
    assert_stopped_record(&fixture.host, fixture.context.decision_id());
    assert_no_failure_state(&fixture.held_record(TASK));
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none()
    );
    fixture.host.close().expect("close host");
}

// Question (i): `scheduling.rerun.condition_codes` and a task-stage item of
// `recovery_ladder.triggers` both name `contained_task_cancelled`. The stop takes precedence over
// both: the record is `stopped` (no condition), nothing is owed, and no ladder starts.
#[test]
fn r6b2_the_stop_takes_precedence_over_condition_codes_and_ladder_triggers_naming_its_code() {
    let fixture = Fixture::start(&policy_sources(1), Some(stop_naming_manifest()));
    let (first, run, error) = fixture.stop_at(ContainedTaskCheckpointPoint::PackageAdmitted);
    assert_asked(&first, run);
    assert_eq!(error, CANCELLED);
    assert_cancelled_terminal(&fixture.host, &fixture.context);
    assert_stopped_record(&fixture.host, fixture.context.decision_id());
    assert_no_failure_state(&fixture.held_record(TASK));
    assert_no_ladder(&fixture.host, X_ALIAS);
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none()
    );
    fixture.host.close().expect("close host");
}

// Question 3, live: an operator pause ended a run by a condition (the shipped table lists
// `contained_task_paused`), so its pair is owed a rerun; the owed rerun is dispatched, admitted and
// stopped, and its live settlement (`PolicyExecutionInput::Stopped`) ends the owed rerun: the
// record is `stopped`, nothing is owed any more, held and replayed alike, and the next evaluation
// promotes nothing.
#[test]
fn r6b2_the_live_settlement_of_a_stopped_owed_rerun_ends_it() {
    const BETWEEN_RUNS_MS: u64 = 600_000;
    let root = TempDir::new().expect("tempdir");
    let (host, clock) = start_one(&root, instance_id(), POLICY_NOW_UNIX_MS, None);
    host.activate_policy_catalog(&once_a_day_sources())
        .expect("activate the catalog");
    let (first, _) = admit(&host, POLICY_NOW_UNIX_MS, 100, true);
    clock.advance(75_000);
    host.record_policy_dispatch_outcome(
        &first.decision_id,
        &PolicyExecutionInput::Failed {
            outcome: Outcome::new(RuntimeErrorCode::ContainedTaskPaused.code()),
            failure_identity: None,
            class: PolicyFailureClass::Recoverable,
        },
    )
    .expect("the condition settlement");
    assert!(
        held_record(&host, TASK).condition_reruns.is_some(),
        "the condition owes a rerun"
    );

    let at = POLICY_NOW_UNIX_MS + BETWEEN_RUNS_MS;
    clock.set_unix_ms(at);
    let (_, rerun, chain) = evaluated_policy_dispatch_at(&host, PolicyTrigger::Recovery, at, 101);
    assert_eq!(rerun.task_id, TASK);
    assert!(
        chain
            .reasons
            .iter()
            .any(|reason| reason.outcome.code().as_str() == "policy_rerun_owed"),
        "the owed rerun is dispatched: {chain:#?}"
    );
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&rerun, &chain, &policy_context(&host, &rerun))
        .expect("the rerun's admission")
    else {
        panic!("expected a granted admission")
    };
    drop(context);
    clock.advance(75_000);
    let settled = host
        .record_policy_dispatch_outcome(
            &rerun.decision_id,
            &PolicyExecutionInput::Stopped {
                outcome: Outcome::new(RuntimeErrorCode::ContainedTaskCancelled),
            },
        )
        .expect("the live settlement of the stop");
    assert!(
        matches!(&settled.outcome, PolicyExecutionOutcome::Stopped { outcome, .. }
            if outcome.code().as_str() == CANCELLED),
        "{settled:?}"
    );
    let record = held_record(&host, TASK);
    assert_eq!(record.condition_reruns, None, "the owed rerun ended");
    assert_eq!(record.failure_streak, None, "no streak");

    let later = at + BETWEEN_RUNS_MS;
    clock.set_unix_ms(later);
    let evaluation = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: later,
                monotonic_ms: later,
            },
            102,
            PolicyTrigger::Recovery,
        )
        .expect("the evaluation")
        .evaluation
        .expect("an evaluation");
    assert!(
        evaluation
            .decisions
            .iter()
            .all(|decision| decision
                .reasons
                .iter()
                .all(|reason| reason.outcome.code().as_str() != "policy_rerun_owed")),
        "nothing promotes the stopped rerun: {evaluation:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// R6B-7 (the runtime-host half), question 2: the stopped run's terminal is written but its live
// settlement is not (it fails at its execution append, as a daemon dying there leaves it); the
// restart's reconciliation settles it from its terminal as `stopped`, and the pair keeps no failure
// state. A run with no terminal (its lease expired before it ran) is settled as interrupted, as
// before: the ledger cannot tell that a stop was asked of it.
#[test]
fn r6b2_the_restart_settles_a_stopped_run_from_its_terminal_and_one_without_as_interrupted() {
    let fixture = Fixture::start(&policy_sources(1), None);
    fixture
        .host
        .fail_next_policy_execution_append_for_test()
        .expect("arm the append failure");
    let (first, run, error) = fixture.stop_at(ContainedTaskCheckpointPoint::PackageAdmitted);
    assert_asked(&first, run);
    assert_eq!(
        error, "ledger_failure",
        "the live settlement did not append"
    );
    assert_cancelled_terminal(&fixture.host, &fixture.context);
    let decision_id = fixture.context.decision_id().to_owned();
    assert!(executions(&fixture.host, &decision_id).is_empty());
    let Fixture {
        root,
        host,
        x,
        y,
        context,
        ..
    } = fixture;
    drop(context);
    close_latched(host);

    let clock = Arc::new(ManualRuntimeClock::new(POLICY_RESTART_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_policy_inputs(two_instance_inputs()),
        Arc::new(FakeProvider::from_entries([
            (X_ALIAS.to_owned(), x, Arc::new(FakeState::default())),
            (OTHER.to_owned(), y, Arc::new(FakeState::default())),
        ])),
    )
    .expect("the restart settles the run");
    assert_stopped_record(&host, &decision_id);
    assert_no_failure_state(&held_record(&host, TASK));
    assert_eq!(
        snapshot_of(&host, &clock, TASK).terminal_state,
        Some(TaskTerminalState::Cancelled)
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");

    // A run with no terminal: its lease expired before it ran.
    let root = TempDir::new().expect("tempdir");
    let registered = instance_id();
    let package = neutral_contained_task_package(false);
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write the package");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 200,
                ..SchedulerConfig::default()
            })
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(
            FakeProvider::one(
                POLICY_INSTANCE_ALIAS,
                registered,
                Arc::new(FakeState::default()),
            )
            .fixture_simulation(),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { context } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("expected a granted admission")
    };
    clock.advance(250);
    host.expire_lease_once_for_test(context.lease_token())
        .expect("expire the run's lease");
    host.fail_next_policy_execution_append_for_test()
        .expect("arm the append failure");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("the scheduled request");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the settlement's append fails");
    assert_eq!(error.code(), "ledger_failure");
    drop(context);
    close_latched(host);

    let interrupted =
        actingcommand_contract::codes::SharedCode::PolicySettlementInterrupted.as_str();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_RESTART_UNIX_MS, 0))),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the restart settles the run");
    let records = executions(&host, &intent.decision_id);
    let [PolicyExecutionOutcome::Failed { failure }] = records.as_slice() else {
        panic!("one failed record: {records:#?}");
    };
    assert_eq!(failure.outcome.code().as_str(), interrupted);
    assert_eq!(failure.original_class, PolicyFailureClass::Severe);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the restarted host");
}

// R6B-2, #419: the stop's reset fails on X's device. The failure fences X only and is the stop's
// cause (`instance_resources_fenced`, `reset: failed`); with a ladder trigger naming the stop's
// code, the stopped run's terminal starts no ladder of its own; the fence is handed on as every
// fence is, to a `resource_fence` ladder of X or to a person, which the test neither requires nor
// waits for; the run still settles as a stop; the other instance is not fenced and its run
// completes; the Runtime is not halted.
#[test]
fn r6b2_a_failed_reset_fences_only_its_instance_and_its_stop_starts_no_ladder() {
    let fixture = Fixture::start(&policy_sources(1), Some(stop_naming_manifest()));
    let (first, run, error) =
        fixture.stop_at_then(ContainedTaskCheckpointPoint::PackageAdmitted, |device| {
            // The run's next input, the Runtime's reset, fails on the device.
            device.fail_input.store(true, Ordering::Release);
        });
    assert_asked(&first, run);
    assert_eq!(error, CANCELLED);
    let again = stop(fixture.address, X_ALIAS, run, 0);
    assert_eq!(
        stopped(&again),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Failed),
        }
    );
    let [cause] = again.outcome().causes() else {
        panic!("the reset's failure is the cause: {again:?}");
    };
    assert_eq!(
        cause.outcome().code().as_str(),
        "instance_resources_fenced",
        "{again:?}"
    );
    assert!(
        instance_row(fixture.address, X_ALIAS)
            .resource_fence()
            .is_some(),
        "X is fenced"
    );
    assert!(
        instance_row(fixture.address, OTHER)
            .resource_fence()
            .is_none(),
        "the other instance is untouched"
    );
    assert_no_ladder_of_the_stop(&fixture.host, &fixture.context);
    assert_stopped_record(&fixture.host, fixture.context.decision_id());

    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let other_run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            OTHER,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(fixture.y_package.clone()).expect("task request"),
        ),
    );
    let receipt = fixture
        .host
        .process_request_for_test(&other_run, ConnectionId::new(1_621).expect("connection"))
        .expect("the other instance's run");
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                ..
            })
        ),
        "{receipt:?}"
    );
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none()
    );
    fixture.host.close().expect("close host");
}

// Answer 8: the run's control is registered before its package's preparation. A stop sent there
// reaches the run (answered at once, still stopping), and the run ends at its first check: the
// Runtime's reset on its lease, its cancelled terminal and its `stopped` record, the reset done.
#[test]
fn r6b2_a_stop_during_the_preparation_reaches_the_run() {
    let fixture = Fixture::start(&policy_sources(1), None);
    let (first, run, error) = fixture.stop_at(ContainedTaskCheckpointPoint::ScheduledPreparation);
    assert_asked(&first, run);
    assert_eq!(error, CANCELLED);
    let (resets, releases) =
        lease_resets_and_releases(&fixture.host, fixture.context.lease_token().lease_id());
    assert_eq!(resets.len(), 1, "one reset: {resets:?}");
    assert_eq!(releases.len(), 1, "one release: {releases:?}");
    assert!(resets[0] < releases[0], "{resets:?} {releases:?}");
    assert_cancelled_terminal(&fixture.host, &fixture.context);
    assert_stopped_record(&fixture.host, fixture.context.decision_id());
    assert_eq!(
        stopped(&stop(fixture.address, X_ALIAS, run, 0)),
        InstanceRunStop {
            request_id: run,
            settled: true,
            reset: Some(InstanceRunReset::Done),
        }
    );
    assert!(
        fixture
            .host
            .fatal_error()
            .expect("runtime health")
            .is_none()
    );
    fixture.host.close().expect("close host");
}

// R6B-1, a fact refresh read: the instance's recognition maintenance package is dispatched to read
// an unknown key and stopped at its checkpoint. The stop reaches it, the read ends
// `contained_task_cancelled` and settles as a stop: its snapshot shows `cancelled` with no failure
// streak, so no failure backoff and no failure in a row follow (the fact refresh's gates read only
// a failed run as one: the `policy` crate's
// `gate_d2_s4::r6b2_a_stopped_reader_is_no_failure_in_a_row`).
#[test]
fn r6b2_a_fact_refresh_read_is_stopped_and_is_no_failure() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("recognition.zip");
    fs::write(&package, recognition_package(false)).expect("write the recognition package");
    let state = Arc::new(FakeState::default());
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_recognition_packages(BTreeMap::from([(
                POLICY_INSTANCE_ALIAS.to_owned(),
                package.clone(),
            )]))
            .with_config_manifest(day_boundary_off_manifest(&instance)),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance, Arc::clone(&state))
                .fixture_simulation()
                .with_vision_provider(Arc::new(TerminalReadings)),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&chain_sources())
        .expect("activate the chain catalog");
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let evaluation = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: POLICY_NOW_UNIX_MS,
                monotonic_ms: POLICY_NOW_UNIX_MS,
            },
            31,
            PolicyTrigger::Recovery,
        )
        .expect("policy cycle")
        .evaluation
        .expect("a full evaluation");
    let (refresh, chain) = intent_of(&evaluation, FACT_REFRESH_TASK_ID);
    record_policy_approval(&host, &refresh);
    let (context, request) = admit_refresh(&host, &refresh, &chain);
    let (sent, answered) = mpsc::channel();
    let checkpoint = host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::PackageAdmitted,
            instance,
            context.lease_token().lease_id(),
            move |identity| {
                let run = identity.request_id();
                sent.send((stop(address, POLICY_INSTANCE_ALIAS, run, 0), run))
                    .expect("the stop's answer");
            },
        )
        .expect("arm the read's checkpoint");
    let error = host
        .run_scheduled_contained_task(&context, &request)
        .expect_err("the stopped read does not complete");
    assert_eq!(checkpoint.consumed(), 1, "the read was admitted");
    let (first, run) = answered.recv_timeout(GATE_WAIT).expect("the stop's answer");
    assert_asked(&first, run);
    assert_eq!(error.code(), CANCELLED, "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");
    assert_cancelled_terminal(&host, &context);
    assert_stopped_record(&host, context.decision_id());
    let snapshot = snapshot_of(&host, &clock, FACT_REFRESH_TASK_ID);
    assert_eq!(snapshot.terminal_state, Some(TaskTerminalState::Cancelled));
    assert_eq!(snapshot.failure_streak, None, "no failure in a row");
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(context);
    host.close().expect("close the host");
}
