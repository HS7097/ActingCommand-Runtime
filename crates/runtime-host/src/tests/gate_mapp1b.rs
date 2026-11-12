// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-1b (register §1.323, MAPP-1's tests of the host's rows; §1.328, "MAPP-1b";
//! §1.330; `contracts/device-group.md`): the host's half of a device group, through the host.
//!
//! Two fixture instances A and B are one device group of several (`device.one`); the load refuses
//! every group of several until the slice "order within one device", so each host is handed the
//! member → device key map and the group's name as the assembly builds them
//! (`RuntimeHostConfig::with_device_keys`, `with_device_group_names`). One generic task per
//! member, due at every evaluation; a manual Runtime clock whose monotonic time stays put unless
//! a test moves it, so a lease taken stays unexpired. Every expected value is the contract's;
//! every comparison is within one host. Neutral names only.

use super::*;
use crate::codes::HostCode;
use crate::policy_host::held::held_signal_id;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EmulatorInstanceAction, InstanceStuckRecovery, SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_policy::{DeviceGroupWait, PolicyCode};
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
const TASK_A: &str = "fixture.member-a";
const TASK_B: &str = "fixture.member-b";
const GROUP: &str = "device.one";
const SEED: u64 = 11;
const MINUTE_MS: u64 = 60_000;
/// Bounds a broken run's end only; nothing sleeps.
const GATE_WAIT: Duration = Duration::from_secs(120);

// ---- The fixture.

/// One task of `instance` with `priority`, due at every evaluation (a local interval clock of
/// 1 ms), from the fixture catalog's generic task.
fn member_task(base: &Value, id: &str, instance: &str, priority: u32) -> Value {
    let mut spec = base.clone();
    spec["id"] = json!(id);
    spec["scope"] = json!({"kind": "instance", "instance_id": instance});
    spec["priority"] = json!(priority);
    spec["trigger"] = json!({
        "kind": "clock",
        "schedule": {"kind": "interval", "clock_source": {"kind": "local"},
                     "every_ms": 1, "anchor_ms": 0}
    });
    spec["cooldown_ms"] = json!(0);
    spec["instance_overrides"] = json!([]);
    spec["produces"] = json!([]);
    spec["on_failure"]["escalation_threshold"] = json!(1000);
    spec["loop_budget"] = json!({
        "daily_limit": 48,
        "window_iteration_limit": 48,
        "max_runtime_ms": 300_000
    });
    spec
}

/// The catalog: TASK_A on A and TASK_B on B with their priorities, under the fixture's game-wide
/// activity profile (08:00 to 22:00 UTC) with no interval between dispatches.
fn sources(priority_a: u32, priority_b: u32) -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = json!([
        member_task(&base, TASK_A, A, priority_a),
        member_task(&base, TASK_B, B, priority_b),
    ]);
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("task bytes");
    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let mut profile = activity["profiles"][1].clone();
    profile["daily_budget"] = json!(48);
    profile["max_window_iterations"] = json!(48);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    activity["profiles"] = json!([profile]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");
    sources
}

/// A host whose instances A and B are the device group `GROUP` (and C alone on its device when
/// `lone`), on a manual Runtime clock at the fixture's noon, stuck recovery off, the catalog of
/// [`sources`] active.
struct Group {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    approved: bool,
    root: TempDir,
}

impl Group {
    fn start(priority_a: u32, priority_b: u32, lone: bool) -> Self {
        let root = TempDir::new().expect("tempdir");
        let a = instance_id();
        let b = instance_id();
        let device = a.min(b);
        let mut entries = vec![
            (A.to_owned(), a, Arc::new(FakeState::default())),
            (B.to_owned(), b, Arc::new(FakeState::default())),
        ];
        if lone {
            entries.push((C.to_owned(), instance_id(), Arc::new(FakeState::default())));
        }
        let mut facts = policy_facts();
        facts.outcomes.clear();
        facts.instances.push(InstanceSnapshot {
            instance_id: B.to_owned(),
            server_id: "fixture-server-b".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-b".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        });
        let resources = pending_policy_resources();
        let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
        let stuck_off = |alias: &str| {
            (
                alias.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )
        };
        let host = RuntimeHost::start(
            config(&root)
                .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), resources.clone()))
                .with_runtime_clock(clock.clone())
                .with_stuck_recovery(BTreeMap::from([stuck_off(A), stuck_off(B)]))
                .with_device_keys(BTreeMap::from([(a, device), (b, device)]))
                .with_device_group_names(BTreeMap::from([
                    (A.to_owned(), GROUP.to_owned()),
                    (B.to_owned(), GROUP.to_owned()),
                ])),
            Arc::new(FakeProvider::from_entries(entries)),
        )
        .expect("start the device group host");
        host.activate_policy_catalog(&sources(priority_a, priority_b))
            .expect("activate the catalog");
        Self {
            host,
            clock,
            facts,
            resources,
            approved: false,
            root,
        }
    }

    /// One evaluation at `unix_ms` with `trigger`, the capacity sampled there first. The Runtime
    /// clock's monotonic time does not move.
    fn cycle(&self, unix_ms: u64, trigger: PolicyTrigger) -> PolicyCycle {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &self.facts,
                &self.resources,
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                SEED,
                trigger,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }

    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.cycle(unix_ms, PolicyTrigger::Recovery)
    }

    /// The admission of `intent`, the catalog's approval recorded once before.
    fn admit(
        &mut self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        if !self.approved {
            record_policy_approval(&self.host, intent);
            self.approved = true;
        }
        self.host
            .admit_policy_dispatch(intent, chain, &policy_context(&self.host, intent))
    }

    fn close(self) {
        self.host.close().expect("close the device group host");
    }
}

/// The worker's report of a handed-off run's end.
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

/// The (task, instance) of every dispatch intent of `cycle`.
fn intents(cycle: &PolicyCycle) -> Vec<(String, String)> {
    cycle
        .evaluation
        .as_ref()
        .expect("an evaluated cycle")
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.clone(), intent.instance_id.clone()))
        .collect()
}

fn pair(task_id: &str, alias: &str) -> (String, String) {
    (task_id.to_owned(), alias.to_owned())
}

/// The intent of `task_id` in `cycle` and its reason chain.
fn intent_of(cycle: &PolicyCycle, task_id: &str) -> (DispatchIntent, DecisionReasonChain) {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("an intent of {task_id}: {evaluation:#?}"))
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    (intent, chain)
}

/// The last reason of the decision of `task_id` on `alias` in `cycle`.
fn last_reason<'a>(cycle: &'a PolicyCycle, task_id: &str, alias: &str) -> &'a Outcome {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {alias}: {evaluation:#?}"));
    &decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason of {task_id}: {decision:?}"))
        .outcome
}

/// `outcome` is `device_group_waiting` with exactly these values.
fn assert_waits(outcome: &Outcome, expected: &[(&str, Value)]) {
    assert!(
        outcome.is(PolicyCode::DeviceGroupWaiting),
        "device_group_waiting: {outcome:?}"
    );
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<serde_json::Map<String, Value>>();
    assert_eq!(outcome.values(), &expected, "{outcome:?}");
}

/// `receipt` is the denial `device_group_action_refused` of `operation` on the member `alias`.
fn assert_refused(receipt: &RuntimeReceipt, alias: &str, operation: &str) {
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    let outcome = receipt.outcome();
    assert_eq!(
        outcome.code().as_str(),
        HostCode::DeviceGroupActionRefused.as_str(),
        "{receipt:?}"
    );
    assert_eq!(outcome.value("alias"), Some(&json!(alias)), "{receipt:?}");
    assert_eq!(
        outcome.value("operation"),
        Some(&json!(operation)),
        "{receipt:?}"
    );
}

fn release(holder: &mut TestClient, token: LeaseToken) {
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
}

// ---- The tests.

/// One worker per device: A's handed-off run reaches the group's one worker,
/// `actingd-device-device.one`. While A's run is registered (its worker dropped it as it took
/// it, so it stays registered until the reconciliation settles it), B's due task waits
/// `device_group_waiting` `held` with A, A's task and the holder's kind, and no dispatch goes
/// to the device. The wait is held, due: past `scheduling.task_held.after_ms` (30 minutes by
/// default) B's pair leaves one held record with that code, counted from the round where B first
/// waited for its device. Once the run is settled its registration is forgotten.
#[test]
fn mapp1b_one_worker_per_device_and_a_member_held_behind_the_others_registered_run() {
    let mut group = Group::start(300, 200, false);
    let t0 = POLICY_NOW_UNIX_MS;
    let first = group.evaluate(t0);
    assert_eq!(intents(&first), vec![pair(TASK_A, A)]);
    let (intent, chain) = intent_of(&first, TASK_A);
    let admission = group.admit(&intent, &chain).expect("admit A's dispatch");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("A's dispatch is granted");
    };
    let token = context.lease_token().clone();
    let package_path = group.root.path().join("member-a.zip");
    fs::write(&package_path, neutral_contained_task_package(false)).expect("write package");
    let request = ContainedTaskRequest::new(package_path.to_string_lossy().into_owned())
        .expect("contained task request");
    let thread_name = Arc::new(std::sync::Mutex::new(None::<String>));
    let seen = Arc::clone(&thread_name);
    let hook: PolicyLaneHook = Arc::new(move |at: PolicyLanePoint, _: &str| {
        if at == PolicyLanePoint::Taken {
            *seen.lock().expect("the worker's thread name") =
                thread::current().name().map(str::to_owned);
            panic!("injected lane panic: the run stays registered");
        }
    });
    group
        .host
        .set_policy_lane_hook_for_test(Some(hook))
        .expect("the lane hook is set");
    let (sender, ended) = mpsc::channel();
    group
        .host
        .hand_off_scheduled_policy_run(
            context,
            request,
            Arc::new(EndSink(std::sync::Mutex::new(sender))),
        )
        .expect("the run is handed to the device's worker");
    assert_eq!(
        ended.recv_timeout(GATE_WAIT).expect("the run's end"),
        "dropped"
    );
    group
        .host
        .set_policy_lane_hook_for_test(None)
        .expect("the lane hook is cleared");
    assert_eq!(
        thread_name
            .lock()
            .expect("the worker's thread name")
            .as_deref(),
        Some("actingd-device-device.one"),
        "the group's one worker took A's run"
    );
    assert_eq!(
        group
            .host
            .policy_run_in_flight_for_test(A)
            .expect("A's lane"),
        (true, false),
        "A's run stays registered"
    );

    let held = group.evaluate(t0 + MINUTE_MS);
    assert!(intents(&held).is_empty(), "{:#?}", intents(&held));
    assert_waits(
        last_reason(&held, TASK_B, B),
        &[
            ("instance_id", json!(B)),
            ("alias", json!(A)),
            ("device_group_wait", json!(DeviceGroupWait::Held.as_str())),
            ("task_id", json!(TASK_A)),
            ("holder_kind", json!(ClaimKind::PolicyDispatch.as_str())),
        ],
    );

    let past = t0 + 30 * MINUTE_MS;
    let reported = group.evaluate(past);
    assert!(intents(&reported).is_empty(), "{:#?}", intents(&reported));
    let held_id = held_signal_id(TASK_B, B, t0);
    let signal = reported
        .task_held_signals
        .iter()
        .find(|signal| signal.signal_id == held_id)
        .unwrap_or_else(|| panic!("B's held record: {:#?}", reported.task_held_signals));
    assert_eq!(signal.instance_id, B);
    assert_eq!(
        signal.outcome.value("gate_code"),
        Some(&json!(PolicyCode::DeviceGroupWaiting.code().as_str()))
    );
    assert_eq!(signal.outcome.value("held_since_unix_ms"), Some(&json!(t0)));

    // The run's lease lapses and one reconciliation round settles it: the registration is gone.
    group.clock.advance(5_100);
    group
        .host
        .expire_lease_once_for_test(&token)
        .expect("expire A's dispatch lease");
    group.cycle(past + 5_100, PolicyTrigger::Reconciliation);
    assert_eq!(
        group
            .host
            .policy_run_in_flight_for_test(A)
            .expect("A's lane"),
        (false, false),
        "the registration is forgotten"
    );
    assert!(
        group.host.fatal_error().expect("runtime health").is_none(),
        "the Runtime keeps running"
    );
    group.close();
}

/// Two members never hold the device at once: B won the device's pick, then a client lease of A
/// took the device before B's dispatch was admitted. The admission refuses it with B's wait,
/// `device_group_waiting` `held` (A, the client's claim kind), at info; the next round defers B's
/// task the same way and A's own task keeps its own holder's code. Once A's lease is released B's
/// task is dispatched, and A's waits behind it.
#[test]
fn mapp1b_a_member_waits_held_behind_the_others_holder_and_runs_after_it() {
    let mut group = Group::start(200, 300, false);
    let t0 = POLICY_NOW_UNIX_MS;
    let first = group.evaluate(t0);
    assert_eq!(intents(&first), vec![pair(TASK_B, B)]);
    let (intent, chain) = intent_of(&first, TASK_B);
    let mut holder = TestClient::connect(&group.host);
    let (_, token) = holder.acquire(A);

    let refused = group
        .admit(&intent, &chain)
        .expect_err("A's lease holds the device for B");
    assert!(!refused.is_fatal(), "{refused:?}");
    assert_eq!(
        refused.code(),
        PolicyCode::DeviceGroupWaiting.code().as_str()
    );
    let rejection = refused.policy_rejection().outcome;
    assert_eq!(rejection.value("instance_id"), Some(&json!(B)));
    assert_eq!(rejection.value("alias"), Some(&json!(A)));
    assert_eq!(
        rejection.value("device_group_wait"),
        Some(&json!(DeviceGroupWait::Held.as_str()))
    );
    assert_eq!(
        rejection.value("holder_kind"),
        Some(&json!(ClaimKind::ClientLease.as_str()))
    );
    assert_eq!(rejection.value("task_id"), None, "no policy run holds it");

    let held = group.evaluate(t0 + MINUTE_MS);
    assert!(intents(&held).is_empty(), "{:#?}", intents(&held));
    assert_waits(
        last_reason(&held, TASK_B, B),
        &[
            ("instance_id", json!(B)),
            ("alias", json!(A)),
            ("device_group_wait", json!(DeviceGroupWait::Held.as_str())),
            ("holder_kind", json!(ClaimKind::ClientLease.as_str())),
        ],
    );
    let own = last_reason(&held, TASK_A, A);
    assert!(own.is(HostCode::DispatchInstanceHeld), "{own:?}");

    release(&mut holder, token);
    let after = group.evaluate(t0 + 2 * MINUTE_MS);
    assert_eq!(intents(&after), vec![pair(TASK_B, B)]);
    assert_waits(
        last_reason(&after, TASK_A, A),
        &[
            ("instance_id", json!(A)),
            ("alias", json!(B)),
            (
                "device_group_wait",
                json!(DeviceGroupWait::Selected.as_str()),
            ),
            ("task_id", json!(TASK_B)),
        ],
    );
    drop(holder);
    group.close();
}

/// A scheduling pause of B completes while A's lease holds the device: B's pause waits only for
/// B's own work. B's task then waits with its own pause's code, A's with its own holder's code
/// (A is not paused), and once A's lease is released A's task is dispatched while B stays paused.
#[test]
fn mapp1b_a_pause_of_one_member_waits_only_for_its_own_work_and_leaves_the_other_scheduled() {
    let group = Group::start(300, 200, false);
    let mut holder = TestClient::connect(&group.host);
    let (_, token) = holder.acquire(A);
    let mut client = TestClient::connect(&group.host);
    let pause = client.request(RuntimeOperation::PauseScheduling {
        scope: SchedulingPauseScope::Instance {
            instance_alias: B.to_owned(),
        },
        pause_reason: SchedulingPauseReason::new("mapp1b_member_pause").expect("pause reason"),
        drain_timeout_ms: 1_000,
    });
    let receipt = client.send(&pause);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );

    let t0 = POLICY_NOW_UNIX_MS;
    let paused = group.evaluate(t0);
    assert!(intents(&paused).is_empty(), "{:#?}", intents(&paused));
    let own_pause = last_reason(&paused, TASK_B, B);
    assert!(
        own_pause.is(HostCode::DispatchPausedInstance),
        "{own_pause:?}"
    );
    let own_holder = last_reason(&paused, TASK_A, A);
    assert!(
        own_holder.is(HostCode::DispatchInstanceHeld),
        "{own_holder:?}"
    );

    release(&mut holder, token);
    let after = group.evaluate(t0 + MINUTE_MS);
    assert_eq!(intents(&after), vec![pair(TASK_A, A)]);
    let still = last_reason(&after, TASK_B, B);
    assert!(still.is(HostCode::DispatchPausedInstance), "{still:?}");
    drop(holder);
    drop(client);
    group.close();
}

/// A person's hold of A holds the device for B (`device_group_waiting` `player_hold`, A) while A
/// keeps its own gate's code; once the hold ends the device goes to the round's pick again. A
/// fence of A holds B (`fenced`, A).
#[test]
fn mapp1b_a_hold_or_a_fence_of_one_member_holds_the_other_and_the_holds_end_frees_it() {
    let t0 = POLICY_NOW_UNIX_MS;
    let group = Group::start(300, 200, false);
    group
        .host
        .hold_player_for_test(A, true)
        .expect("a person holds A");
    let held = group.evaluate(t0);
    assert!(intents(&held).is_empty(), "{:#?}", intents(&held));
    assert_waits(
        last_reason(&held, TASK_B, B),
        &[
            ("instance_id", json!(B)),
            ("alias", json!(A)),
            (
                "device_group_wait",
                json!(DeviceGroupWait::PlayerHold.as_str()),
            ),
        ],
    );
    let own = last_reason(&held, TASK_A, A);
    assert!(own.is(HostCode::DispatchPlayerHeld), "{own:?}");
    group
        .host
        .hold_player_for_test(A, false)
        .expect("the hold ends");
    let freed = group.evaluate(t0 + MINUTE_MS);
    assert_eq!(intents(&freed), vec![pair(TASK_A, A)]);
    assert_waits(
        last_reason(&freed, TASK_B, B),
        &[
            ("instance_id", json!(B)),
            ("alias", json!(A)),
            (
                "device_group_wait",
                json!(DeviceGroupWait::Selected.as_str()),
            ),
            ("task_id", json!(TASK_A)),
        ],
    );
    group.close();

    let group = Group::start(300, 200, false);
    group.host.fence_instance_for_test(A).expect("fence A");
    let fenced = group.evaluate(t0);
    assert!(
        !intents(&fenced).contains(&pair(TASK_B, B)),
        "{:#?}",
        intents(&fenced)
    );
    assert_waits(
        last_reason(&fenced, TASK_B, B),
        &[
            ("instance_id", json!(B)),
            ("alias", json!(A)),
            ("device_group_wait", json!(DeviceGroupWait::Fenced.as_str())),
        ],
    );
    group.close();
}

/// A member of a group of several keeps no device session across lease ends, whatever its
/// backends, while an instance alone on its device keeps it as before. An operator's emulator
/// control of a member is refused before its intent, and a keyless observation of a member while
/// another member holds the device is refused, each `device_group_action_refused` with the member
/// and the action.
#[test]
fn mapp1b_a_member_keeps_no_session_and_its_device_level_actions_are_refused() {
    let group = Group::start(300, 200, true);
    for (alias, keeps) in [(A, false), (B, false), (C, true)] {
        assert_eq!(
            group
                .host
                .keeps_device_session_for_test(alias)
                .expect("the session gate"),
            keeps,
            "{alias}"
        );
    }

    let mut client = TestClient::connect(&group.host);
    let control = client.request(RuntimeOperation::ControlEmulatorInstance {
        instance_alias: A.to_owned(),
        action: EmulatorInstanceAction::Start,
    });
    let receipt = client.send(&control);
    assert_refused(&receipt, A, "control_emulator_instance");
    assert!(
        receipt.terminal().is_none(),
        "refused before its intent: {receipt:?}"
    );

    let mut holder = TestClient::connect(&group.host);
    let (_, token) = holder.acquire(A);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: B.to_owned(),
    });
    let receipt = client.send(&observe);
    assert_refused(&receipt, B, "observe_readonly");
    release(&mut holder, token);
    drop(holder);
    drop(client);
    group.close();
}
