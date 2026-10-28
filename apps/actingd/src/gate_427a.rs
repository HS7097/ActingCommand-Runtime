// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #427-A (plan r2, comment 6460013989; brief rd-1028/brief-427a-lanes.md, "Tests"): the
// policy round with the per-instance lanes on, on the APPS-I7 cycle harness of gate_382_a4_cycle
// (an in-process Runtime, fake devices, catalog-a). The round is actingd's own
// (`execute_policy_cycle_on` with the driver's lane sink, `DriverLaneSink`); the interleavings are
// forced with the host's lane hook (a run held on its worker when the worker took it, or after its
// terminal and lease release and before its settlement) and the C1 hook between an evaluation's
// sample and its validation. No sleep: every wait returns as soon as its message arrives; `WAIT`
// only bounds a broken run. Every expected value comes from the brief and the plan, not from what
// the code does today:
//
// - lanes_t1: a long run on A does not hold the round nor B's run; A's next round finds A free.
// - lanes_t2: frozen at "terminal written, lease released, not settled": no second dispatch on A,
//   a reconciliation skips A's run, the settlement is A's worker's alone and its wake arrives.
// - lanes_t3: a failed run on A leaves B's run to complete; the Runtime does not stop.
// - lanes_t4 (C1): A's settlement lands between the round's sample and its validation: the round
//   samples again, says so once, and judges the candidates again; the driver does not stop.
// - lanes_t5 (C1): past the re-sample limit the round is given up and deferred to its next
//   trigger; the next round evaluates.
// - lanes_t6: a shutdown before a handed run was taken refuses it (settled as not performed); one
//   after it was taken lets it finish and settle.
// - lanes_t7: a worker panic drops the run, leaves its instance occupied (registration abandoned,
//   for the reconciliation) and wakes the driver; the other instance's run completes.
// - lanes_t8: the switch is off by default, so the daemon's round runs a scheduled run itself; an
//   instruction turns it on and the next round hands its runs to the workers.

use super::gate_382_a4_cycle::{
    Screen, activate, dispatch_record, dispatch_records, ledger_events, run_of, run_records, start,
};
use super::*;
use actingcommand_contract::{ConfigSettingChange, EventActor, EventSource, EventType};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{PolicyLaneHook, PolicyLanePoint};
use std::sync::mpsc;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(120);
const A: &str = "fixture-instance-a";
const B: &str = "fixture-instance-b";
// The harness's instant (gate_382_a4_cycle): an occurrence of catalog-a's hourly clock trigger.
const POLICY_NOW_UNIX_MS: u64 = 1_699_963_200_000;

// The next end of `alias`'s run reported on `ended`; the ends of other instances' runs before it
// are passed over.
fn end_of(
    ended: &mpsc::Receiver<(String, Ended)>,
    alias: &str,
) -> Result<Ended, mpsc::RecvTimeoutError> {
    loop {
        let (instance, end) = ended.recv_timeout(WAIT)?;
        if instance == alias {
            return Ok(end);
        }
    }
}

// How a handed-off run ended, as its sink was told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Completed,
    Failed,
    Dropped,
}

// The daemon's own lane sink, which also tells the test each end.
struct RecordingSink {
    driver: DriverLaneSink,
    ended: Mutex<mpsc::Sender<(String, Ended)>>,
}

impl PolicyRunSink for RecordingSink {
    fn settled(&self, settled: PolicyRunSettled) {
        let (alias, ended) = match &settled {
            PolicyRunSettled::Completed(completion) => (
                completion.context.instance_alias().to_owned(),
                Ended::Completed,
            ),
            PolicyRunSettled::Failed { context, .. } => {
                (context.instance_alias().to_owned(), Ended::Failed)
            }
            PolicyRunSettled::Dropped { instance_alias, .. } => {
                (instance_alias.clone(), Ended::Dropped)
            }
        };
        self.driver.settled(settled);
        if let Ok(sender) = self.ended.lock() {
            let _ = sender.send((alias, ended));
        }
    }
}

// The lanes of the daemon's driver whose control is `control`, with each run's end reported on the
// returned receiver.
fn lanes(control: &Arc<PolicyDriverControl>) -> (PolicyLanes, mpsc::Receiver<(String, Ended)>) {
    let (sender, receiver) = mpsc::channel();
    let sink = RecordingSink {
        driver: DriverLaneSink {
            control: Arc::clone(control),
        },
        ended: Mutex::new(sender),
    };
    (
        PolicyLanes {
            sink: Arc::new(sink),
        },
        receiver,
    )
}

// A run of `alias` held at a lane point: `reached` reports that its worker got there, and the
// worker goes on once `release` sends (or is dropped).
struct Held {
    reached: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

fn hold_at(host: &RuntimeHost, alias: &'static str, point: PolicyLanePoint) -> Held {
    let (reached_sender, reached) = mpsc::channel();
    let (release, release_receiver) = mpsc::channel::<()>();
    let reached_sender = Mutex::new(reached_sender);
    let release_receiver = Mutex::new(release_receiver);
    let hook: PolicyLaneHook = Arc::new(move |at: PolicyLanePoint, instance: &str| {
        if at == point && instance == alias {
            if let Ok(sender) = reached_sender.lock() {
                let _ = sender.send(());
            }
            if let Ok(receiver) = release_receiver.lock() {
                let _ = receiver.recv_timeout(WAIT);
            }
        }
    });
    host.set_policy_lane_hook_for_test(Some(hook))
        .expect("the lane hook is set");
    Held { reached, release }
}

// One round with the lanes on, at trigger `trigger`, and its daemon lines.
fn round(
    host: &RuntimeHost,
    policy: &PolicyBootstrap,
    lanes: &PolicyLanes,
    trigger: PolicyTrigger,
) -> (PolicyCycleExecution, String) {
    let mut out: Vec<u8> = Vec::new();
    let execution = execute_policy_cycle_on(
        host,
        policy,
        trigger,
        &|| Duration::ZERO,
        &mut out,
        Some(lanes),
    )
    .expect("the round runs and does not stop the driver");
    (execution, String::from_utf8(out).expect("stdout text"))
}

fn aliases(execution: &PolicyCycleExecution) -> Vec<&str> {
    execution
        .cycle
        .pending_dispatch_intents
        .iter()
        .map(|intent| intent.instance_id.as_str())
        .collect()
}

fn decision_of<'a>(execution: &'a PolicyCycleExecution, alias: &str) -> &'a str {
    execution
        .cycle
        .pending_dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == alias)
        .map(|intent| intent.decision_id.as_str())
        .unwrap_or_else(|| panic!("{alias}'s intent in the round"))
}

fn assert_running(host: &RuntimeHost) {
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime latched: {:?}",
        host.fatal_error()
    );
}

// The completions recorded for `decision_id`.
fn completions(root: &Path, decision_id: &str) -> usize {
    dispatch_records(&ledger_events(root), decision_id)
        .into_iter()
        .filter(|event| event.event_type() == EventType::PolicyDispatchCompleted)
        .count()
}

// The driver's pending wake, taken: its trigger and its recompute wakes.
fn driver_wake(control: &PolicyDriverControl) -> Option<(PolicyTrigger, usize)> {
    match control.wait(Duration::ZERO).expect("the driver's wake") {
        PolicyDriverWake::Trigger(trigger, recomputes) => Some((trigger, recomputes.len())),
        PolicyDriverWake::TimedOut => None,
        PolicyDriverWake::Shutdown => panic!("the driver was shut down"),
    }
}

// Brief "Tests": a long and a short task on two instances overlap. A's run is held on A's worker;
// the round has returned already, B's run is run and settled meanwhile, and A's run settles once
// released. Brief: the next task for the same instance: A's registration ended with its
// settlement, so the next round does not find A held by a policy dispatch.
#[test]
fn lanes_t1_a_long_run_on_one_instance_holds_neither_the_round_nor_the_other_instance() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, _clock) = start(root.path(), &[Screen::Opens, Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    let (lanes, ended) = lanes(&control);
    let held = hold_at(&host, A, PolicyLanePoint::Taken);

    let (execution, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    assert_eq!(aliases(&execution), [A, B], "one intent per instance");
    assert!(!execution.failed_runs);
    let (a, b) = (decision_of(&execution, A), decision_of(&execution, B));
    held.reached
        .recv_timeout(WAIT)
        .expect("A's worker took A's run after the round returned");
    assert_eq!(
        end_of(&ended, B).expect("B's run ends"),
        Ended::Completed,
        "B's run runs while A's is held"
    );
    assert_eq!(completions(root.path(), b), 1, "B is settled");
    assert_eq!(completions(root.path(), a), 0, "A is not settled yet");
    assert_eq!(
        host.policy_run_in_flight_for_test(A).expect("A's lane"),
        (true, true),
        "A's run is in flight on its worker"
    );
    assert_eq!(
        host.policy_run_in_flight_for_test(B).expect("B's lane"),
        (false, false),
        "B's registration ended with its settlement"
    );

    held.release.send(()).expect("release A's run");
    assert_eq!(end_of(&ended, A).expect("A's run ends"), Ended::Completed);
    assert_eq!(completions(root.path(), a), 1, "A is settled once");
    assert_eq!(
        host.policy_run_in_flight_for_test(A).expect("A's lane"),
        (false, false)
    );
    host.set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    let (next, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    let evaluation = next.cycle.evaluation.as_ref().expect("an evaluation");
    assert!(
        evaluation
            .decisions
            .iter()
            .filter(|decision| decision.instance_id.as_deref() == Some(A))
            .flat_map(|decision| decision.reasons.iter())
            .all(
                |reason| reason.outcome.code().as_str() != "dispatch_instance_held"
                    || reason.outcome.value("holder_kind")
                        != Some(&serde_json::json!("policy_dispatch"))
            ),
        "A is not held by a settled run: {evaluation:#?}"
    );
    assert_running(&host);
    drop(held);
    host.close().expect("the Runtime closes");
}

// Brief "Tests": frozen at "terminal record written, lease released, not settled". A round and a
// reconciliation meet A's run there: no second dispatch on A, the reconciliation leaves A's run to
// its worker, the settlement is recorded once (the worker's own success), and the wake after it
// reaches the driver.
#[test]
fn lanes_t2_frozen_after_the_terminal_no_second_dispatch_one_settlement_and_the_wake() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, _clock) = start(root.path(), &[Screen::Opens, Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    let (lanes, ended) = lanes(&control);
    let held = hold_at(&host, A, PolicyLanePoint::Ran);

    let (execution, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    let a = decision_of(&execution, A).to_owned();
    held.reached
        .recv_timeout(WAIT)
        .expect("A's run returned its receipt and is not settled");
    assert_eq!(end_of(&ended, B).expect("B's run ends"), Ended::Completed);
    let _ = driver_wake(&control);
    let events = ledger_events(root.path());
    let a_run = run_of(&events, &a);
    assert!(
        run_records(&events, a_run)
            .iter()
            .any(|event| event.event_type() == EventType::TaskCompleted),
        "A's terminal is written"
    );
    assert!(
        run_records(&events, a_run)
            .iter()
            .any(|event| event.event_type() == EventType::LeaseReleased),
        "A's lease is released"
    );
    assert_eq!(completions(root.path(), &a), 0, "A is not settled");

    let (reconciled, _) = round(&host, &policy, &lanes, PolicyTrigger::Reconciliation);
    assert!(
        !aliases(&reconciled).contains(&A),
        "no dispatch on A while its run is unsettled"
    );
    let (again, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    assert!(
        !aliases(&again).contains(&A),
        "no second dispatch on A while its run is unsettled: {:#?}",
        again.cycle.pending_dispatch_intents
    );
    assert_eq!(completions(root.path(), &a), 0, "nothing settled A early");
    assert_running(&host);

    held.release.send(()).expect("release A's run");
    assert_eq!(
        end_of(&ended, A).expect("A's run ends"),
        Ended::Completed,
        "A's worker settles A's success"
    );
    assert_eq!(completions(root.path(), &a), 1, "A is settled once");
    assert_eq!(
        driver_wake(&control),
        Some((PolicyTrigger::FactsChanged, 0)),
        "the wake after A's settlement reaches the driver"
    );
    assert!(
        dispatch_record(
            &ledger_events(root.path()),
            &a,
            EventType::PolicyDispatchRejected
        )
        .is_none(),
        "A's dispatch was settled, never refused"
    );
    assert_running(&host);
    host.set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    drop(held);
    host.close().expect("the Runtime closes");
}

// Brief "Tests": a failure on one instance leaves the others running (#419). A's run fails; B's
// run completes; the Runtime does not stop; A's failure wakes the driver, as `failed_runs` does.
#[test]
fn lanes_t3_a_failed_run_on_one_instance_leaves_the_other_running() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, _clock) = start(root.path(), &[Screen::Stuck, Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    let (lanes, ended) = lanes(&control);

    let (execution, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    assert_eq!(aliases(&execution), [A, B]);
    let mut ends = BTreeMap::new();
    for _ in 0..2 {
        let (alias, end) = ended.recv_timeout(WAIT).expect("a run ends");
        ends.insert(alias, end);
    }
    assert_eq!(ends.get(A), Some(&Ended::Failed), "A's run fails");
    assert_eq!(ends.get(B), Some(&Ended::Completed), "B's run completes");
    assert_running(&host);
    assert_eq!(
        completions(root.path(), decision_of(&execution, A)),
        1,
        "A's failure is settled"
    );
    assert_eq!(completions(root.path(), decision_of(&execution, B)), 1);
    assert_eq!(
        driver_wake(&control).map(|(trigger, _)| trigger),
        Some(PolicyTrigger::FactsChanged),
        "the failure wakes the driver"
    );
    host.close().expect("the Runtime closes");
}

// Brief "Tests" (C1): A's completion lands between the round's sample and its validation. The
// hook releases A's run, held after its terminal, and waits for A's settlement; the harness
// catalog references no outcome of A's task, so the validation's verdict that the sample changed
// is forced once, as a catalog that references it gives. The round samples again (one daemon
// line), judges the candidates again on the new sample and returns: the driver does not stop.
#[test]
fn lanes_t4_a_settlement_between_sample_and_validation_is_judged_again() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, _clock) = start(root.path(), &[Screen::Opens, Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    let (lanes, ended) = lanes(&control);
    let held = hold_at(&host, A, PolicyLanePoint::Ran);
    let (execution, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    let a = decision_of(&execution, A).to_owned();
    held.reached.recv_timeout(WAIT).expect("A's run is held");
    assert_eq!(end_of(&ended, B).expect("B's run ends"), Ended::Completed);

    let (back, returned) = mpsc::channel();
    let release = held.release.clone();
    host.run_between_policy_sample_and_validation_for_test(Box::new(move || {
        let _ = release.send(());
        let settled = end_of(&ended, A);
        let _ = back.send((ended, settled));
    }))
    .expect("the sample hook is set");
    host.change_policy_outcome_snapshots_for_test(1)
        .expect("one changed sample");
    let (again, out) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    let (_ended, settled) = returned.recv_timeout(WAIT).expect("the hook ran");
    assert_eq!(
        settled.expect("A settles inside the round"),
        Ended::Completed
    );
    let lines = out
        .lines()
        .filter(|line| line.starts_with("actingd policy_cycle_"))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "one re-sample line: {out:?}");
    assert!(
        lines[0].starts_with("actingd policy_cycle_resampled")
            && lines[0].contains("code=policy_outcome_snapshot_changed")
            && lines[0].contains("attempt=1"),
        "{out:?}"
    );
    assert!(
        again.cycle.evaluation.is_some(),
        "the candidates are judged again"
    );
    assert_eq!(completions(root.path(), &a), 1);
    assert_running(&host);
    host.set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    drop(held);
    host.close().expect("the Runtime closes");
}

// Brief "Tests" (C1): past the re-sample limit the round ends as given up: one line per
// re-sample and one for the give-up, no evaluation, a deferred directive the driver keeps as its
// next trigger. The driver goes on, and the next round evaluates.
#[test]
fn lanes_t5_past_the_resample_limit_the_round_is_given_up_and_the_driver_goes_on() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, clock) = start(root.path(), &[Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    let (lanes, _ended) = lanes(&control);
    let limit = u64::try_from(MAX_POLICY_RESAMPLES).expect("the limit");
    host.change_policy_outcome_snapshots_for_test(limit + 1)
        .expect("changed samples past the limit");

    let (given_up, out) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    let resampled = out
        .lines()
        .filter(|line| line.starts_with("actingd policy_cycle_resampled"))
        .count();
    assert_eq!(resampled, MAX_POLICY_RESAMPLES, "{out:?}");
    assert_eq!(
        out.lines()
            .filter(|line| line.starts_with("actingd policy_cycle_given_up"))
            .count(),
        1,
        "{out:?}"
    );
    assert!(given_up.cycle.evaluation.is_none());
    assert!(given_up.cycle.pending_dispatch_intents.is_empty());
    assert_eq!(given_up.cycle.directive.kind, PolicyRecomputeKind::Deferred);
    let now = clock.now_unix_ms();
    let mut schedule = PolicyDriverSchedule::new(policy.cadence.clone(), &given_up.cycle, now)
        .expect("the driver's schedule");
    apply_policy_cycle_result(
        &control,
        &mut schedule,
        PolicyTrigger::Recovery,
        Vec::new(),
        now,
        Ok(given_up),
    )
    .expect("the driver keeps the given-up round");
    assert!(
        schedule.deferred.is_some(),
        "the round waits for its next trigger"
    );

    let (next, out) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    assert!(next.cycle.evaluation.is_some(), "the next round evaluates");
    assert!(!out.contains("policy_cycle_resampled"), "{out:?}");
    assert_running(&host);
    host.close().expect("the Runtime closes");
}

// Brief "Tests": a shutdown before and after the hand-off was taken. Before: A's run is handed and
// not yet taken when the host's stop is requested; its worker refuses it (`runtime_stopping`),
// settled as not performed, and nothing ran. After: A's run was taken and runs past its terminal
// when the stop is requested; it finishes and is settled. The close then joins the workers.
#[test]
fn lanes_t6_a_shutdown_refuses_a_run_not_taken_and_finishes_a_run_taken() {
    for (point, taken) in [
        (PolicyLanePoint::Taken, false),
        (PolicyLanePoint::Ran, true),
    ] {
        let root = TempDir::new().expect("tempdir");
        let (host, policy, _clock) = start(root.path(), &[Screen::Opens], None, false);
        activate(&host, &policy);
        let control = Arc::new(PolicyDriverControl::default());
        let (lanes, ended) = lanes(&control);
        let held = hold_at(&host, A, point);
        let (execution, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
        let a = decision_of(&execution, A).to_owned();
        held.reached.recv_timeout(WAIT).expect("A's run is held");

        // The close's first step, before it joins the workers.
        host.request_shutdown_for_test()
            .expect("the stop is requested");
        held.release.send(()).expect("release A's run");
        let end = end_of(&ended, A).expect("A's run ends");
        if taken {
            assert_eq!(end, Ended::Completed, "a run taken finishes");
        } else {
            assert_eq!(end, Ended::Failed, "a run not taken is refused");
        }
        host.set_policy_lane_hook_for_test(None)
            .expect("the lane hook is cleared");
        drop(held);
        host.close().expect("the Runtime closes");
        let events = ledger_events(root.path());
        assert!(
            dispatch_record(&events, &a, EventType::PolicyDispatchCompleted).is_some(),
            "A's dispatch is settled"
        );
        let ran = run_records(&events, run_of(&events, &a))
            .iter()
            .any(|event| event.event_type() == EventType::TaskCompleted);
        assert_eq!(ran, taken, "a refused run never ran; a taken one ran");
    }
}

// Brief "Tests": a worker panic. The panic drops A's run (not retried); A's registration stays as
// abandoned, so A is still occupied and its settlement is left to the reconciliation; the driver
// is woken; B's run completes and the Runtime does not stop.
#[test]
fn lanes_t7_a_worker_panic_drops_the_run_and_leaves_its_instance_occupied() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, _clock) = start(root.path(), &[Screen::Opens, Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    let (lanes, ended) = lanes(&control);
    let hook: PolicyLaneHook = Arc::new(|at: PolicyLanePoint, instance: &str| {
        if at == PolicyLanePoint::Taken && instance == A {
            panic!("injected lane panic");
        }
    });
    host.set_policy_lane_hook_for_test(Some(hook))
        .expect("the lane hook is set");

    let (execution, _) = round(&host, &policy, &lanes, PolicyTrigger::Recovery);
    assert_eq!(aliases(&execution), [A, B]);
    let mut ends = BTreeMap::new();
    for _ in 0..2 {
        let (alias, end) = ended.recv_timeout(WAIT).expect("a run ends");
        ends.insert(alias, end);
    }
    host.set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    assert_eq!(ends.get(A), Some(&Ended::Dropped), "A's run is dropped");
    assert_eq!(ends.get(B), Some(&Ended::Completed), "B's run completes");
    assert_eq!(
        host.policy_run_in_flight_for_test(A).expect("A's lane"),
        (true, false),
        "A stays occupied, its run no worker's"
    );
    assert_eq!(
        completions(root.path(), decision_of(&execution, A)),
        0,
        "the dropped run is not replayed nor settled by the worker"
    );
    assert_eq!(
        driver_wake(&control).map(|(trigger, _)| trigger),
        Some(PolicyTrigger::FactsChanged)
    );
    assert_running(&host);
    host.close().expect("the Runtime closes");
}

// Brief "Tests" and point 6: the switch. Off by default: the daemon's round runs A's scheduled run
// itself, so A is settled when the round returns. An instruction turns it on, and the next round
// (an hour on, A's task due again) hands A's run to A's worker: held there, the round has
// returned with A unsettled, and A's end wakes the driver.
#[test]
fn lanes_t8_the_switch_is_off_by_default_and_an_instruction_turns_it_on_at_the_next_round() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy, clock) = start(root.path(), &[Screen::Opens], None, false);
    activate(&host, &policy);
    let control = Arc::new(PolicyDriverControl::default());
    assert!(
        !host.scheduling_parallel_enabled().expect("the switch"),
        "off by default"
    );
    let off = execute_policy_cycle_in(&host, &policy, PolicyTrigger::Recovery, Some(&control))
        .expect("the round runs");
    let first = decision_of(&off, A).to_owned();
    assert_eq!(
        completions(root.path(), &first),
        1,
        "with the switch off the round ran and settled A itself"
    );

    let client = RuntimeClient::connect(
        RuntimeClientConfig::new(
            root.path(),
            RuntimeChannel::Cli,
            EventActor::Agent,
            EventSource::Adapter,
        )
        .with_io_timeout(Duration::from_secs(5)),
    )
    .expect("connect to the Runtime");
    client
        .configure_settings(vec![ConfigSettingChange {
            key: "scheduling.parallel.enabled".to_owned(),
            value: Some(serde_json::json!(true)),
        }])
        .expect("the instruction is taken");
    assert!(host.scheduling_parallel_enabled().expect("the switch"));
    drop(client);

    // catalog-a's task is due every hour (its clock trigger): the next occurrence.
    clock.advance_to(POLICY_NOW_UNIX_MS + 3_600_000);
    let held = hold_at(&host, A, PolicyLanePoint::Taken);
    let on = execute_policy_cycle_in(&host, &policy, PolicyTrigger::Recovery, Some(&control))
        .expect("the round runs");
    let second = decision_of(&on, A).to_owned();
    held.reached.recv_timeout(WAIT).expect("A's worker took it");
    assert_eq!(
        completions(root.path(), &second),
        0,
        "the round returned before A's run, handed to A's worker"
    );
    held.release.send(()).expect("release A's run");
    match control.wait(WAIT).expect("the driver's wake") {
        PolicyDriverWake::Trigger(trigger, _) => assert_eq!(trigger, PolicyTrigger::FactsChanged),
        PolicyDriverWake::TimedOut | PolicyDriverWake::Shutdown => {
            panic!("A's end wakes the driver")
        }
    }
    assert_eq!(
        completions(root.path(), &second),
        1,
        "A's worker settled it"
    );
    host.set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    drop(held);
    assert_running(&host);
    host.close().expect("the Runtime closes");
}
