// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #427-A fix 1 (brief rd-1028/brief-427a-fix1.md item 4; review rd-1028/review-427a.md
// M1): a scheduled run handed to its instance's worker, whose worker panicked as it took the run,
// is not left held until a restart. The panic leaves the run's registration abandoned and its
// lease the dispatch's; a reconciliation leaves both while the lease is active. Once the lease
// lapsed (the sweep's expiry, which writes no run link), one reconciliation round settles the run
// by the start's rule: its missing run-linked release is recorded with
// `policy_settlement_release_recovered`, the run is settled exactly once (as interrupted: it has
// no terminal), its registration is forgotten and its instance is free for the next dispatch. A
// second round settles nothing more. Every expected value comes from the brief and the review, not
// from what the code does today. No sleep: the run's end is awaited on its sink's channel, and
// `GATE_WAIT` only bounds a broken run.

use super::*;

const GATE_WAIT: Duration = Duration::from_secs(120);

// The worker's report of the run's end.
struct EndSink(std::sync::Mutex<mpsc::Sender<&'static str>>);

impl PolicyRunSink for EndSink {
    fn settled(&self, settled: PolicyRunSettled) {
        let end = match settled {
            PolicyRunSettled::Completed(_) => "completed",
            PolicyRunSettled::Failed { .. } => "failed",
            PolicyRunSettled::Dropped { .. } => "dropped",
        };
        if let Ok(sender) = self.0.lock() {
            let _ = sender.send(end);
        }
    }
}

// One reconciliation round (`PolicyTrigger::Reconciliation`), as the daemon's driver runs it.
fn reconciliation_round(host: &RuntimeHost, seed: u64) {
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms: POLICY_NOW_UNIX_MS,
            monotonic_ms: POLICY_NOW_UNIX_MS,
        },
        seed,
        PolicyTrigger::Reconciliation,
    )
    .expect("a reconciliation round");
}

// The records of the run, each as its forensic projection's JSON text.
fn run_texts(client: &mut TestClient, run_id: RunId) -> Vec<(EventType, String)> {
    projected_events(
        client,
        EventQuery {
            run_id: Some(run_id),
            ..EventQuery::default()
        },
    )
    .iter()
    .map(|event| {
        (
            event.event_type,
            serde_json::to_string(event).expect("the record's JSON"),
        )
    })
    .collect()
}

fn count(records: &[(EventType, String)], event_type: EventType) -> usize {
    records
        .iter()
        .filter(|(recorded, _)| *recorded == event_type)
        .count()
}

#[test]
fn m1_an_abandoned_lane_settles_once_online_after_its_lease_lapsed_and_frees_its_instance() {
    let root = TempDir::new().expect("tempdir");
    let package = neutral_contained_task_package(false);
    let package_path = root.path().join("scheduled-task.zip");
    fs::write(&package_path, &package).expect("write package");
    let state = Arc::new(FakeState::default());
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_scheduler(SchedulerConfig {
                maximum_client_heartbeat_interval_ms: 20,
                takeover_cooldown_ms: 40,
                lease_ttl_ms: 2_000,
                ..SchedulerConfig::default()
            })
            .with_procedure_manifest(procedure_manifest_with_primary(
                &package,
                vec!["after_observation".to_owned()],
            )),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&policy_sources(1))
        .expect("activate policy catalog");
    let (_, intent, reasons) = evaluated_policy_dispatch(&host, PolicyTrigger::FactsChanged);
    record_policy_approval(&host, &intent);
    let admission = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("policy admission");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected policy run context")
    };
    let token = context.lease_token().clone();
    let run_id = context.run_id();
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("contained task request");
    let mut client = TestClient::connect(&host);

    // The worker panics as it takes the run: the run is dropped, never started.
    let hook: PolicyLaneHook = Arc::new(|at: PolicyLanePoint, _: &str| {
        if at == PolicyLanePoint::Taken {
            panic!("injected lane panic");
        }
    });
    host.set_policy_lane_hook_for_test(Some(hook))
        .expect("the lane hook is set");
    let (sender, ended) = mpsc::channel();
    host.hand_off_scheduled_policy_run(
        context,
        request,
        Arc::new(EndSink(std::sync::Mutex::new(sender))),
    )
    .expect("the run is handed to the instance's worker");
    assert_eq!(
        ended.recv_timeout(GATE_WAIT).expect("the run's end"),
        "dropped"
    );
    host.set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    assert_eq!(
        host.policy_run_in_flight_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the lane"),
        (true, false),
        "the registration is abandoned, its instance still held"
    );

    // While the dispatch's lease is active, a reconciliation leaves the run to its lease.
    reconciliation_round(&host, 8);
    let before = run_texts(&mut client, run_id);
    assert_eq!(count(&before, EventType::PolicyExecutionRecorded), 0);
    assert_eq!(count(&before, EventType::PolicyDispatchCompleted), 0);
    assert_eq!(
        host.policy_run_in_flight_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the lane"),
        (true, false)
    );
    assert!(
        !host
            .policy_instance_free_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the occupancy"),
        "the instance is held"
    );

    // The lease lapses at its TTL (the sweep's expiry), then one reconciliation round.
    clock.advance(2_100);
    host.expire_lease_once_for_test(&token)
        .expect("expire the dispatch's lease");
    reconciliation_round(&host, 9);
    let settled = run_texts(&mut client, run_id);
    assert_eq!(
        count(&settled, EventType::PolicyExecutionRecorded),
        1,
        "the run is settled once"
    );
    assert_eq!(count(&settled, EventType::PolicyDispatchCompleted), 1);
    assert_eq!(
        count(&settled, EventType::LeaseReleased),
        1,
        "the run-linked release the lease's expiry left missing is recovered once"
    );
    assert_eq!(
        settled
            .iter()
            .filter(|(_, text)| text.contains("policy_settlement_release_recovered"))
            .count(),
        1
    );
    assert!(
        settled.iter().any(|(event_type, text)| {
            *event_type == EventType::PolicyExecutionRecorded
                && text.contains("policy_settlement_interrupted")
        }),
        "a run with no terminal is settled as interrupted: {settled:#?}"
    );
    assert_eq!(
        host.policy_run_in_flight_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the lane"),
        (false, false),
        "the registration is forgotten"
    );

    // A second round settles nothing more.
    reconciliation_round(&host, 10);
    let again = run_texts(&mut client, run_id);
    assert_eq!(count(&again, EventType::PolicyExecutionRecorded), 1);
    assert_eq!(count(&again, EventType::PolicyDispatchCompleted), 1);
    assert_eq!(count(&again, EventType::LeaseReleased), 1);

    // Past the expiry's takeover cooldown, the next dispatch can go to the instance again.
    clock.advance(100);
    assert!(
        host.policy_instance_free_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the occupancy"),
        "the instance is free for the next dispatch"
    );
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "the Runtime keeps running"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close runtime host");
}
