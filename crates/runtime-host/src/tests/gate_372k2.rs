// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #372 K2, test plan H-5 G12 (model-372-skip v3 §9 K2 tests 1–9): every due clock
//! slot that a gate stops leaves exactly one refused or deferred record (§4.2, §4.3), and a
//! selection that serves an occurrence from before its activity window leaves one owed record.
//! The records are read back from the ledger by their `signal_id` prefix and the kind
//! `goal_missed`. Every scenario runs generic tasks on one instance with the activity window
//! 06:00–23:30 UTC, UTC clocks and a manual Runtime clock.

use super::*;
use actingcommand_contract::SchedulingPauseScope;

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
/// 2023-11-15 00:00 UTC: day 0 of every scenario, after the fixture's own observations.
const DAY_ZERO_UNIX_MS: u64 = 1_700_006_400_000;
const CLOCK_SLOT_PREFIX: &str = "signal:clock_slot:";
const WINDOW_START_MINUTE: u16 = 6 * 60;
const WINDOW_END_MINUTE: u16 = 23 * 60 + 30;
pub(super) const SLOT_TASK: &str = "fixture.slot";

/// `hour:minute` UTC on scenario day `day`.
pub(super) const fn at(day: u64, hour: u64, minute: u64) -> u64 {
    DAY_ZERO_UNIX_MS + day * DAY_MS + hour * HOUR_MS + minute * MINUTE_MS
}

/// A UTC `daily` clock trigger at the given minutes of the day.
fn daily(minutes: &[u16]) -> serde_json::Value {
    serde_json::json!({
        "kind": "clock",
        "schedule": {
            "kind": "daily",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "minutes_of_day": minutes
        }
    })
}

/// One generic task on the fixture instance.
pub(super) struct SlotTask {
    id: &'static str,
    priority: u32,
    trigger: serde_json::Value,
    daily_limit: u32,
}

/// One task with a `daily` clock at `minutes` and `daily_limit` runs a day.
pub(super) fn slot_task(minutes: &[u16], daily_limit: u32) -> SlotTask {
    SlotTask {
        id: SLOT_TASK,
        priority: 100,
        trigger: daily(minutes),
        daily_limit,
    }
}

/// A catalog of `tasks` with the activity window 06:00–23:30 UTC and an activity interval of
/// 1 ms, plus a daily reset of every `task.` fact at `reset_minute` when given.
pub(super) fn slot_sources(
    version: u64,
    tasks: &[SlotTask],
    reset_minute: Option<u16>,
) -> CatalogSources {
    let mut sources = policy_sources(version);
    let mut document: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = tasks
        .iter()
        .map(|task| {
            let mut spec = base.clone();
            spec["id"] = serde_json::json!(task.id);
            spec["priority"] = serde_json::json!(task.priority);
            spec["trigger"] = task.trigger.clone();
            spec["cooldown_ms"] = serde_json::json!(0);
            spec["instance_overrides"] = serde_json::json!([]);
            spec["loop_budget"] = serde_json::json!({
                "daily_limit": task.daily_limit,
                "window_iteration_limit": task.daily_limit,
                "max_runtime_ms": 300_000
            });
            spec
        })
        .collect::<Vec<_>>()
        .into();
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("slot task bytes");

    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    let mut profile = activity["profiles"][0].clone();
    profile["windows"] = serde_json::json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": WINDOW_START_MINUTE,
        "end_minute_of_day": WINDOW_END_MINUTE
    }]);
    profile["daily_budget"] = serde_json::json!(24);
    profile["max_window_iterations"] = serde_json::json!(24);
    profile["minimum_interval_ms"] = serde_json::json!(1);
    profile["maximum_interval_ms"] = serde_json::json!(1);
    activity["profiles"] = serde_json::json!([profile]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("slot activity bytes");

    if let Some(minute) = reset_minute {
        let mut timeline: serde_json::Value =
            serde_json::from_slice(&sources.timeline.bytes).expect("fixture timeline");
        let mut reset = timeline["events"][0].clone();
        reset["id"] = serde_json::json!("fixture-task-reset");
        reset["schedule"]["minutes_of_day"] = serde_json::json!([minute]);
        reset["invalidates_fact_prefixes"] = serde_json::json!(["task."]);
        timeline["events"]
            .as_array_mut()
            .expect("timeline events")
            .push(reset);
        sources.timeline.bytes = serde_json::to_vec_pretty(&timeline).expect("slot timeline bytes");
    }
    sources
}

/// One clock-slot record as the ledger holds it.
#[derive(Debug, Clone)]
struct SlotRow {
    /// `refused`, `deferred` or `owed`: the `signal_id` segment after the prefix.
    kind: String,
    /// The code at the head of the `fact_code` token.
    code: String,
    task_id: String,
    observed_at_unix_ms: u64,
    /// The token's `key=value` pairs.
    values: BTreeMap<String, String>,
}

impl SlotRow {
    fn time(&self, key: &str) -> Option<u64> {
        self.values.get(key).map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("{key} is a decimal time: {self:?}"))
        })
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
}

/// Every clock-slot record in the ledger, in ledger order.
fn slot_rows(host: &RuntimeHost) -> Vec<SlotRow> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyPlanningSignalObserved),
        ..EventQuery::default()
    })
    .expect("planning signals")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload))
            if payload.kind() == PolicyPlanningSignalKind::GoalMissed =>
        {
            let identity = payload.signal_id().strip_prefix(CLOCK_SLOT_PREFIX)?;
            let mut tokens = payload.fact_code().split(':');
            let code = tokens.next().unwrap_or_default().to_owned();
            let values = tokens
                .map(|token| {
                    let (key, value) = token
                        .split_once('=')
                        .unwrap_or_else(|| panic!("a key=value token: {token}"));
                    (key.to_owned(), value.to_owned())
                })
                .collect();
            Some(SlotRow {
                kind: identity.split(':').next().unwrap_or_default().to_owned(),
                code,
                task_id: payload.task_id().unwrap_or_default().to_owned(),
                observed_at_unix_ms: payload.observed_at_unix_ms(),
                values,
            })
        }
        _ => None,
    })
    .collect()
}

/// The (kind, gate code) of every record of `occurrence`, in ledger order.
fn classes(rows: &[SlotRow], occurrence: u64) -> Vec<(&str, &str)> {
    rows.iter()
        .filter(|row| row.time("occurrence_unix_ms") == Some(occurrence))
        .map(|row| (row.kind.as_str(), row.text("gate_code").unwrap_or_default()))
        .collect()
}

/// How the fixture instance is backed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// A device the host self-checks, as an operator's instance pause requires; its
    /// availability is its preparation's.
    Device,
    /// A fixture whose availability follows `Slots::available` at every evaluation.
    Fixture,
}

fn open_slot_host(
    root: &TempDir,
    registered: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
    backend: Backend,
) -> RuntimeHost {
    let mut facts = policy_facts();
    facts.outcomes.clear();
    let provider = FakeProvider::one(
        POLICY_INSTANCE_ALIAS,
        registered,
        Arc::new(FakeState::default()),
    );
    let provider = match backend {
        Backend::Device => provider,
        Backend::Fixture => provider.fixture_simulation(),
    };
    RuntimeHost::start(
        config(root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
            .with_runtime_clock(clock.clone()),
        Arc::new(provider),
    )
    .expect("start the clock-slot host")
}

/// One host on a manual Runtime clock, and the records read from it so far.
pub(super) struct Slots {
    pub(super) host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    root: TempDir,
    registered: InstanceId,
    backend: Backend,
    available: bool,
    approved: BTreeSet<String>,
    read: usize,
}

impl Slots {
    pub(super) fn start(sources: &CatalogSources, unix_ms: u64) -> Self {
        Self::start_on(Backend::Device, sources, unix_ms)
    }

    fn start_on(backend: Backend, sources: &CatalogSources, unix_ms: u64) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let host = open_slot_host(&root, registered, &clock, backend);
        host.activate_policy_catalog(sources)
            .expect("activate the clock-slot catalog");
        Self {
            host,
            clock,
            root,
            registered,
            backend,
            available: true,
            approved: BTreeSet::new(),
            read: 0,
        }
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time, as its
    /// capacity monitor does while time passes.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    fn activate(&self, sources: &CatalogSources, unix_ms: u64) {
        self.set_time(unix_ms);
        self.host
            .activate_policy_catalog(sources)
            .expect("activate the next clock-slot catalog");
    }

    pub(super) fn evaluate(&mut self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let mut facts = policy_facts();
        facts.outcomes.clear();
        facts.instances[0].available = self.available;
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &facts,
                &policy_resources(),
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                7,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
    }

    /// Evaluates at `unix_ms`, then admits the dispatch the cycle selected for `task_id`,
    /// completes it and ends its lease, as a run of the task does.
    pub(super) fn dispatch(&mut self, unix_ms: u64, task_id: &str) {
        let cycle = self.evaluate(unix_ms);
        let evaluation = cycle.evaluation.as_ref().expect("evaluation");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == task_id)
            .unwrap_or_else(|| panic!("{task_id} is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("dispatch reason chain")
            .clone();
        if self.approved.insert(intent.catalog_hash.clone()) {
            record_policy_approval(&self.host, &intent);
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&self.host, &intent))
            .unwrap_or_else(|error| panic!("admit {task_id} at {unix_ms}: {error}"))
        else {
            panic!("{task_id} is granted at {unix_ms}");
        };
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    fn scheduling(&self, operation: RuntimeOperation) {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(operation);
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// An operator's instance pause, as `actingctl pause` sends it.
    pub(super) fn pause(&self) {
        self.scheduling(RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
            },
            pause_reason: actingcommand_contract::SchedulingPauseReason::new("slot_gate_pause")
                .expect("pause reason"),
            drain_timeout_ms: 1_000,
        });
    }

    pub(super) fn resume(&self) {
        self.scheduling(RuntimeOperation::ResumeScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
            },
            expected: None,
        });
    }

    /// A client lease that holds the instance until it is released.
    fn hold(&self) -> (TestClient, LeaseToken) {
        let mut holder = TestClient::connect(&self.host);
        let (_, token) = holder.acquire(POLICY_INSTANCE_ALIAS);
        (holder, token)
    }

    fn release((mut holder, token): (TestClient, LeaseToken)) {
        let release = holder.request(RuntimeOperation::ReleaseLease { token });
        assert_eq!(
            holder.send(&release).state(),
            RuntimeReceiptState::Completed
        );
    }

    /// The clock-slot records written since the last call, in ledger order.
    fn new_rows(&mut self) -> Vec<SlotRow> {
        let rows = slot_rows(&self.host);
        let new = rows[self.read..].to_vec();
        self.read = rows.len();
        new
    }

    /// A clean stop and a new start on the same state root.
    fn restart(self, unix_ms: u64) -> Self {
        let Self {
            host,
            root,
            registered,
            backend,
            available,
            approved,
            read,
            ..
        } = self;
        host.close().expect("close the clock-slot host");
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let host = open_slot_host(&root, registered, &clock, backend);
        Self {
            host,
            clock,
            root,
            registered,
            backend,
            available,
            approved,
            read,
        }
    }

    fn close(self) {
        self.host.close().expect("close the clock-slot host");
    }

    /// A clean stop that keeps the state root for a read after it.
    pub(super) fn close_keeping_root(self) -> TempDir {
        let Self { host, root, .. } = self;
        host.close().expect("close the clock-slot host");
        root
    }
}

/// Test 1, an activation overnight: `daily [07:00, 19:00]`, limit 2, no history. The
/// evaluation at 23:40 the evening before writes nothing; 06:09 writes one owed record with
/// `first_run`; 19:00 writes one refused `policy_budget_exhausted`/`task_daily` record with N the
/// next 06:00 and X the next 07:00; 19:01, 23:45 and a restart add nothing. With the slots
/// moved to `[06:00, 19:00]` after the 07:00 run, N = X.
#[test]
fn gate_clock_slot_first_run_is_owed_and_a_spent_slot_is_refused_once() {
    for moved_to_window_start in [false, true] {
        let mut slots = Slots::start(
            &slot_sources(1, &[slot_task(&[420, 1140], 2)], None),
            at(0, 23, 40),
        );
        slots.evaluate(at(0, 23, 40));
        assert!(
            slots.new_rows().is_empty(),
            "a pair with no dispatch is not traced while its window is closed"
        );

        slots.dispatch(at(1, 6, 9), SLOT_TASK);
        let rows = slots.new_rows();
        assert_eq!(rows.len(), 1, "one owed record at 06:09: {rows:#?}");
        let owed = &rows[0];
        assert_eq!(
            (owed.kind.as_str(), owed.code.as_str()),
            ("owed", "clock_slot_owed")
        );
        assert_eq!(owed.task_id, SLOT_TASK);
        assert_eq!(owed.observed_at_unix_ms, at(1, 6, 9));
        assert_eq!(owed.time("occurrence_unix_ms"), Some(at(0, 19, 0)));
        assert_eq!(owed.time("window_start_unix_ms"), Some(at(1, 6, 0)));
        assert_eq!(owed.time("next_occurrence_unix_ms"), Some(at(1, 7, 0)));
        assert_eq!(owed.text("first_run"), Some("true"));

        slots.dispatch(at(1, 7, 0), SLOT_TASK);
        assert!(
            slots.new_rows().is_empty(),
            "an occurrence inside the window is not owed"
        );
        if moved_to_window_start {
            slots.activate(
                &slot_sources(2, &[slot_task(&[360, 1140], 2)], None),
                at(1, 8, 6),
            );
        }

        slots.evaluate(at(1, 19, 0));
        let rows = slots.new_rows();
        assert_eq!(rows.len(), 1, "one refused record at 19:00: {rows:#?}");
        let refused = &rows[0];
        assert_eq!(
            (refused.kind.as_str(), refused.code.as_str()),
            ("refused", "clock_slot_refused")
        );
        assert_eq!(refused.text("gate_code"), Some("policy_budget_exhausted"));
        assert_eq!(refused.text("budget_dimension"), Some("task_daily"));
        assert_eq!(refused.time("occurrence_unix_ms"), Some(at(1, 19, 0)));
        assert_eq!(refused.time("window_end_unix_ms"), Some(at(1, 23, 30)));
        assert_eq!(refused.time("next_eligible_unix_ms"), Some(at(2, 6, 0)));
        let next_occurrence = if moved_to_window_start {
            at(2, 6, 0)
        } else {
            at(2, 7, 0)
        };
        assert_eq!(
            refused.time("next_occurrence_unix_ms"),
            Some(next_occurrence)
        );
        if moved_to_window_start {
            assert_eq!(
                refused.time("next_eligible_unix_ms"),
                refused.time("next_occurrence_unix_ms"),
                "N = X"
            );
        }

        slots.evaluate(at(1, 19, 1));
        slots.evaluate(at(1, 23, 45));
        let mut slots = slots.restart(at(1, 23, 46));
        slots.evaluate(at(1, 23, 46));
        assert!(
            slots.new_rows().is_empty(),
            "19:01, 23:45 and a restart add nothing"
        );
        slots.close();
    }
}

/// Test 2, a daily displacement: `daily [06:30]`, limit 1, last dispatch the day before at
/// 06:07. The 06:07 dispatch writes an owed record without `first_run` and with X today 06:30;
/// 06:30 writes one refused record with E 23:30. A follower `all(clock [06:30],
/// dependency_completed(head, succeeded))` that ran owed at 06:08, with a reset of `task.` at
/// 06:29, gets no refused record at 06:30 and one owed record the next morning.
#[test]
fn gate_clock_slot_daily_displacement_is_owed_then_refused_and_a_follower_only_owed() {
    const HEAD: &str = "fixture.head";
    const FOLLOWER: &str = "fixture.follower";
    let head = SlotTask {
        id: HEAD,
        priority: 200,
        trigger: daily(&[390]),
        daily_limit: 1,
    };
    let follower = SlotTask {
        id: FOLLOWER,
        priority: 100,
        trigger: serde_json::json!({
            "kind": "all",
            "predicates": [
                daily(&[390]),
                {"kind": "dependency_completed", "task_id": HEAD, "terminal_states": ["succeeded"]}
            ]
        }),
        daily_limit: 1,
    };
    let mut slots = Slots::start(&slot_sources(1, &[head, follower], Some(389)), at(0, 6, 0));
    // Day 0 is the history: the first runs, then the head's own slot.
    slots.dispatch(at(0, 6, 7), HEAD);
    slots.dispatch(at(0, 6, 8), FOLLOWER);
    slots.evaluate(at(0, 6, 30));
    let mut follower_rows = slots
        .new_rows()
        .into_iter()
        .filter(|row| row.task_id == FOLLOWER)
        .collect::<Vec<_>>();

    slots.dispatch(at(1, 6, 7), HEAD);
    let rows = slots.new_rows();
    assert_eq!(rows.len(), 1, "one owed record at 06:07: {rows:#?}");
    let owed = &rows[0];
    assert_eq!((owed.kind.as_str(), owed.task_id.as_str()), ("owed", HEAD));
    assert_eq!(owed.time("occurrence_unix_ms"), Some(at(0, 6, 30)));
    assert_eq!(owed.time("window_start_unix_ms"), Some(at(1, 6, 0)));
    assert_eq!(owed.time("next_occurrence_unix_ms"), Some(at(1, 6, 30)));
    assert_eq!(owed.text("first_run"), None, "the pair ran the day before");

    slots.dispatch(at(1, 6, 8), FOLLOWER);
    slots.evaluate(at(1, 6, 30));
    let rows = slots.new_rows();
    follower_rows.extend(rows.iter().filter(|row| row.task_id == FOLLOWER).cloned());
    let refused = rows
        .iter()
        .filter(|row| row.kind == "refused")
        .collect::<Vec<_>>();
    assert_eq!(refused.len(), 1, "one refused record at 06:30: {rows:#?}");
    assert_eq!(refused[0].task_id, HEAD);
    assert_eq!(refused[0].observed_at_unix_ms, at(1, 6, 30));
    assert_eq!(
        refused[0].text("gate_code"),
        Some("policy_budget_exhausted")
    );
    assert_eq!(refused[0].text("budget_dimension"), Some("task_daily"));
    assert_eq!(refused[0].time("occurrence_unix_ms"), Some(at(1, 6, 30)));
    assert_eq!(refused[0].time("window_end_unix_ms"), Some(at(1, 23, 30)));

    slots.dispatch(at(2, 6, 7), HEAD);
    slots.dispatch(at(2, 6, 8), FOLLOWER);
    let next_morning = slots
        .new_rows()
        .into_iter()
        .filter(|row| row.task_id == FOLLOWER)
        .collect::<Vec<_>>();
    assert_eq!(
        next_morning
            .iter()
            .map(|row| (row.kind.as_str(), row.time("occurrence_unix_ms")))
            .collect::<Vec<_>>(),
        [("owed", Some(at(1, 6, 30)))],
        "one owed follower record the next morning"
    );
    follower_rows.extend(next_morning);
    assert!(
        follower_rows.iter().all(|row| row.kind == "owed"),
        "a follower after the reset is not due at its slot: {follower_rows:#?}"
    );
    slots.close();
}

/// Test 3: an instance pause at the slot writes one deferred `dispatch_paused_instance`
/// record; after the resume the pair is dispatched and nothing more is written. An instance
/// unavailable for one cycle inside the window writes nothing.
#[test]
fn gate_clock_slot_operator_pause_is_deferred_once_and_a_short_unavailability_is_silent() {
    let mut slots = Slots::start(&slot_sources(1, &[slot_task(&[600], 1)], None), at(0, 9, 0));
    slots.pause();
    slots.evaluate(at(0, 10, 0));
    let rows = slots.new_rows();
    assert_eq!(rows.len(), 1, "one deferred record at the slot: {rows:#?}");
    let deferred = &rows[0];
    assert_eq!(
        (deferred.kind.as_str(), deferred.code.as_str()),
        ("deferred", "clock_slot_deferred")
    );
    assert_eq!(deferred.text("gate_code"), Some("dispatch_paused_instance"));
    assert_eq!(deferred.time("occurrence_unix_ms"), Some(at(0, 10, 0)));
    assert_eq!(deferred.time("window_end_unix_ms"), Some(at(0, 23, 30)));
    assert_eq!(deferred.time("next_eligible_unix_ms"), None);
    assert_eq!(deferred.time("next_occurrence_unix_ms"), Some(at(1, 10, 0)));

    slots.evaluate(at(0, 10, 10));
    slots.resume();
    slots.dispatch(at(0, 10, 30), SLOT_TASK);
    assert!(
        slots.new_rows().is_empty(),
        "a pause and its resume leave one record"
    );
    slots.close();

    // The instance availability of a fixture follows the evaluation inputs.
    let mut slots = Slots::start_on(
        Backend::Fixture,
        &slot_sources(1, &[slot_task(&[600], 1)], None),
        at(0, 9, 0),
    );
    slots.dispatch(at(0, 10, 0), SLOT_TASK);
    slots.available = false;
    let cycle = slots.evaluate(at(1, 10, 0));
    assert!(
        cycle.pending_dispatch_intents.is_empty(),
        "an unavailable instance is not dispatched"
    );
    slots.available = true;
    slots.dispatch(at(1, 10, 1), SLOT_TASK);
    assert!(
        slots.new_rows().is_empty(),
        "one unavailable cycle inside the window writes nothing"
    );
    slots.close();
}

/// Test 4, against the held branch: held at the slot, then refused by the budget once the
/// hold ends, gives exactly one refused record and no deferred one. Held until 23:40 gives
/// exactly one deferred `dispatch_instance_held` record, written at the first cycle after
/// 23:30, and nothing more when the hold ends.
#[test]
fn gate_clock_slot_a_hold_is_traced_only_once_it_outlasts_the_window() {
    let sources = || slot_sources(1, &[slot_task(&[600], 1)], None);

    let mut slots = Slots::start(&sources(), at(0, 6, 0));
    // The day's run serves the previous day's slot and spends the budget.
    slots.dispatch(at(0, 6, 5), SLOT_TASK);
    let holder = slots.hold();
    slots.evaluate(at(0, 10, 0));
    slots.evaluate(at(0, 10, 20));
    Slots::release(holder);
    slots.evaluate(at(0, 10, 30));
    slots.evaluate(at(0, 23, 31));
    let rows = slots.new_rows();
    assert_eq!(
        classes(&rows, at(0, 10, 0)),
        [("refused", "policy_budget_exhausted")],
        "{rows:#?}"
    );
    let refused = rows
        .iter()
        .find(|row| row.time("occurrence_unix_ms") == Some(at(0, 10, 0)))
        .expect("the refused record");
    assert_eq!(refused.observed_at_unix_ms, at(0, 10, 30));
    slots.close();

    let mut slots = Slots::start(&sources(), at(0, 9, 0));
    // The previous day's run: the pair has a dispatch.
    slots.dispatch(at(0, 10, 0), SLOT_TASK);
    let holder = slots.hold();
    slots.evaluate(at(1, 10, 0));
    slots.evaluate(at(1, 23, 0));
    assert!(
        slots.new_rows().is_empty(),
        "a hold inside the window writes nothing"
    );
    slots.evaluate(at(1, 23, 31));
    let rows = slots.new_rows();
    assert_eq!(
        classes(&rows, at(1, 10, 0)),
        [("deferred", "dispatch_instance_held")],
        "{rows:#?}"
    );
    let deferred = &rows[0];
    assert_eq!(deferred.observed_at_unix_ms, at(1, 23, 31));
    assert_eq!(deferred.time("window_end_unix_ms"), None);
    assert_eq!(deferred.time("next_eligible_unix_ms"), None);
    assert_eq!(deferred.time("next_occurrence_unix_ms"), Some(at(2, 10, 0)));
    slots.evaluate(at(1, 23, 35));
    Slots::release(holder);
    slots.evaluate(at(1, 23, 41));
    assert!(
        slots.new_rows().is_empty(),
        "nothing more once the hold ends"
    );
    slots.close();
}

/// Test 5, the order of classes for one occurrence: a pause, then the budget after the resume,
/// gives one deferred and one refused record; the budget, then a pause, gives one refused
/// record only; a pause until 23:40 gives one deferred record and nothing at the resume.
#[test]
fn gate_clock_slot_classes_keep_their_order_per_occurrence() {
    let sources = || slot_sources(1, &[slot_task(&[600], 1)], None);
    let slot = at(1, 10, 0);

    let mut slots = Slots::start(&sources(), at(1, 6, 0));
    slots.dispatch(at(1, 6, 5), SLOT_TASK);
    slots.pause();
    slots.evaluate(at(1, 10, 0));
    slots.resume();
    slots.evaluate(at(1, 10, 30));
    slots.evaluate(at(1, 23, 31));
    let rows = slots.new_rows();
    assert_eq!(
        classes(&rows, slot),
        [
            ("deferred", "dispatch_paused_instance"),
            ("refused", "policy_budget_exhausted")
        ],
        "pause, then budget: {rows:#?}"
    );
    slots.close();

    let mut slots = Slots::start(&sources(), at(1, 6, 0));
    slots.dispatch(at(1, 6, 5), SLOT_TASK);
    slots.evaluate(at(1, 10, 0));
    slots.pause();
    slots.evaluate(at(1, 10, 30));
    slots.evaluate(at(1, 23, 31));
    let rows = slots.new_rows();
    assert_eq!(
        classes(&rows, slot),
        [("refused", "policy_budget_exhausted")],
        "budget, then pause: {rows:#?}"
    );
    slots.close();

    let mut slots = Slots::start(&sources(), at(0, 9, 0));
    slots.dispatch(at(0, 10, 0), SLOT_TASK);
    slots.pause();
    slots.evaluate(at(1, 10, 0));
    slots.evaluate(at(1, 23, 31));
    slots.resume();
    slots.evaluate(at(1, 23, 41));
    let rows = slots.new_rows();
    assert_eq!(
        classes(&rows, slot),
        [("deferred", "dispatch_paused_instance")],
        "pause until 23:40: {rows:#?}"
    );
    slots.close();
}

/// Test 6, two due pairs on one instance: the pair the round does not select gets no record;
/// the selected one, owed, gets its own.
#[test]
fn gate_clock_slot_a_pair_waiting_in_the_round_leaves_no_record() {
    let first = SlotTask {
        id: "fixture.first",
        priority: 200,
        trigger: daily(&[300]),
        daily_limit: 1,
    };
    let second = SlotTask {
        id: "fixture.second",
        priority: 100,
        trigger: daily(&[300]),
        daily_limit: 1,
    };
    let mut slots = Slots::start(&slot_sources(1, &[first, second], None), at(0, 6, 0));
    let cycle = slots.evaluate(at(0, 6, 5));
    let selected = cycle
        .pending_dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        selected.len(),
        1,
        "one dispatch per instance and round: {selected:?}"
    );
    let waiting = if selected[0] == "fixture.first" {
        "fixture.second"
    } else {
        "fixture.first"
    };
    let rows = slots.new_rows();
    assert_eq!(
        rows.iter()
            .map(|row| (row.kind.as_str(), row.task_id.as_str()))
            .collect::<Vec<_>>(),
        [("owed", selected[0])],
        "{rows:#?}"
    );
    assert!(rows.iter().all(|row| row.task_id != waiting));
    slots.close();
}

/// Test 7, the token bound: every gate code of the gate table, with every `budget_dimension`
/// token and 13-digit times, gives a refused and a deferred `fact_code` that the contract's
/// planning-signal validation accepts; so does the longest owed token.
#[test]
fn gate_clock_slot_every_token_passes_the_planning_signal_validation() {
    use crate::policy_host::clock_slot::{SlotRecord, SlotValues, clock_slot_signal};
    use actingcommand_contract::{
        AuditInput, EventDraft, EventLinksDraft, EventOrigin, PolicyBudgetDimension,
        PolicyPayloadDraft,
    };
    use actingcommand_ledger::Sha256SecretFingerprinter;

    /// Every gate code of the K2 gate table that leaves a record.
    const GATE_CODES: [&str; 13] = [
        "policy_budget_exhausted",
        "policy_activity_window_closed",
        "policy_task_paused",
        "instance_override_disabled",
        "capability_missing",
        "activity_profile_missing",
        "dispatch_paused_global",
        "dispatch_paused_instance",
        "dispatch_instance_held",
        "policy_retry_backoff_active",
        "policy_activity_interval_active",
        "dispatch_instance_cooldown",
        "instance_unavailable",
    ];
    const TIME: u64 = 1_700_042_400_000;
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let fingerprinter =
        Sha256SecretFingerprinter::new(b"clock-slot-token-bound").expect("fingerprinter");
    // The contract's own path: the event draft of a planning signal, sanitized.
    let validate = |signal: PolicyPlanningSignalEventData| -> String {
        let fact_code = signal.fact_code.clone();
        let links = EventLinksDraft::default()
            .with_request_id(issuer.mint_request_id().expect("request id"))
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
            .with_action_id(issuer.mint_action_id().expect("action id"));
        EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            TIME,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Scheduler,
                OriginModule::Policy,
                EventActor::Scheduler,
            ),
            links,
            PolicyPayloadDraft::planning_signal_observed(signal, AuditInput::new()).into(),
        )
        .sanitize(&fingerprinter)
        .unwrap_or_else(|error| panic!("{fact_code} passes the validation: {error:?}"));
        fact_code
    };
    let dimensions = [
        None,
        Some(PolicyBudgetDimension::TaskDaily),
        Some(PolicyBudgetDimension::TaskWindow),
        Some(PolicyBudgetDimension::TaskRuntime),
        Some(PolicyBudgetDimension::ActivityDaily),
        Some(PolicyBudgetDimension::ActivityWindow),
        Some(PolicyBudgetDimension::ActivityRuntime),
    ];
    for gate_code in GATE_CODES {
        for dimension in dimensions {
            for kind in [SlotRecord::Refused, SlotRecord::Deferred] {
                let values = SlotValues {
                    gate_code: Some(gate_code.to_owned()),
                    budget_dimension: dimension,
                    occurrence_unix_ms: TIME,
                    window_end_unix_ms: Some(TIME),
                    next_eligible_unix_ms: Some(TIME),
                    next_occurrence_unix_ms: Some(TIME),
                    ..SlotValues::default()
                };
                let fact_code = validate(clock_slot_signal(
                    kind,
                    SLOT_TASK,
                    POLICY_INSTANCE_ALIAS,
                    &values,
                    TIME,
                ));
                if let Some(dimension) = dimension {
                    let token = serde_json::to_value(dimension).expect("dimension token");
                    let token = token.as_str().expect("a vocabulary token");
                    assert!(
                        fact_code.contains(&format!(":budget_dimension={token}:")),
                        "{fact_code}"
                    );
                }
            }
        }
    }
    let owed = SlotValues {
        occurrence_unix_ms: TIME,
        window_start_unix_ms: Some(TIME),
        next_occurrence_unix_ms: Some(TIME),
        first_run: true,
        ..SlotValues::default()
    };
    validate(clock_slot_signal(
        SlotRecord::Owed,
        SLOT_TASK,
        POLICY_INSTANCE_ALIAS,
        &owed,
        TIME,
    ));
}

/// Test 8, the golden id: one fixed (task, alias, occurrence) gives one fixed `signal_id`. The
/// digest was computed apart from the code under test, with
/// `py -3 -c "import hashlib; print(hashlib.sha256(b'\x00'.join([b'fixture.slot', b'fixture-instance-a', b'1700042400000'])).hexdigest())"`.
#[test]
fn gate_clock_slot_signal_id_matches_its_golden_digest() {
    use crate::policy_host::clock_slot::{SlotRecord, signal_id};

    const DIGEST: &str = "71e71662307a84a87c8c3106c9825315b88842d26ebdaa8bae127ab06c2facc2";
    let occurrence = at(0, 10, 0);
    assert_eq!(occurrence, 1_700_042_400_000);
    for (kind, word) in [
        (SlotRecord::Refused, "refused"),
        (SlotRecord::Deferred, "deferred"),
        (SlotRecord::Owed, "owed"),
    ] {
        assert_eq!(
            signal_id(kind, SLOT_TASK, POLICY_INSTANCE_ALIAS, occurrence),
            format!("signal:clock_slot:{word}:{DIGEST}")
        );
    }
}

/// Test 9, E1: a record of each code serializes with exactly the planning-signal key set,
/// deserializes and serializes again to the same bytes, and its projection row decodes to it.
#[test]
fn gate_clock_slot_records_round_trip_through_ledger_and_projection() {
    use crate::policy_host::clock_slot::{SlotRecord, SlotValues, clock_slot_signal};
    use actingcommand_contract::{PolicyBudgetDimension, PolicyPlanningSignalPayload};

    let owed = SlotValues {
        occurrence_unix_ms: at(0, 19, 0),
        window_start_unix_ms: Some(at(1, 6, 0)),
        next_occurrence_unix_ms: Some(at(1, 7, 0)),
        first_run: true,
        ..SlotValues::default()
    };
    let deferred = SlotValues {
        gate_code: Some("dispatch_paused_instance".to_owned()),
        occurrence_unix_ms: at(1, 7, 0),
        window_end_unix_ms: Some(at(1, 23, 30)),
        next_occurrence_unix_ms: Some(at(1, 19, 0)),
        ..SlotValues::default()
    };
    let refused = SlotValues {
        gate_code: Some("policy_budget_exhausted".to_owned()),
        budget_dimension: Some(PolicyBudgetDimension::TaskDaily),
        occurrence_unix_ms: at(1, 19, 0),
        window_end_unix_ms: Some(at(1, 23, 30)),
        next_eligible_unix_ms: Some(at(2, 6, 0)),
        next_occurrence_unix_ms: Some(at(2, 7, 0)),
        ..SlotValues::default()
    };
    let signals = [
        (SlotRecord::Owed, &owed, at(1, 6, 9)),
        (SlotRecord::Deferred, &deferred, at(1, 7, 0)),
        (SlotRecord::Refused, &refused, at(1, 19, 0)),
    ]
    .map(|(kind, values, observed_at_unix_ms)| {
        clock_slot_signal(
            kind,
            SLOT_TASK,
            POLICY_INSTANCE_ALIAS,
            values,
            observed_at_unix_ms,
        )
    });
    let slots = Slots::start(
        &slot_sources(1, &[slot_task(&[420, 1140], 2)], None),
        at(1, 6, 0),
    );
    for signal in &signals {
        slots
            .host
            .record_policy_planning_signal(signal.clone())
            .expect("record the clock-slot signal");
    }
    let events = slots
        .host
        .query_persisted_events_for_test(EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        })
        .expect("planning signals");
    let payloads = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::PlanningSignalObserved(payload)) => Some(payload),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), signals.len());
    for (payload, signal) in payloads.iter().zip(&signals) {
        assert_eq!(payload.signal_id(), signal.signal_id);
        assert_eq!(payload.fact_code(), signal.fact_code);
        let bytes = serde_json::to_vec(payload).expect("encode the record");
        let record: serde_json::Value = serde_json::from_slice(&bytes).expect("record JSON");
        let keys = record
            .as_object()
            .expect("a record object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "action",
                "signal_id",
                "instance_id",
                "task_id",
                "kind",
                "fact_code",
                "observed_at_unix_ms",
                "audit",
            ])
        );
        let decoded: PolicyPlanningSignalPayload =
            serde_json::from_slice(&bytes).expect("decode the record");
        assert_eq!(
            serde_json::to_vec(&decoded).expect("encode the record again"),
            bytes
        );
    }
    let root = slots.close_keeping_root();
    let state =
        RuntimeStateStore::open(root.path(), b"runtime-host-test-salt").expect("open the state");
    for signal in &signals {
        let (_, stored) = state
            .load_planning_signal(&signal.signal_id)
            .expect("read the projection")
            .expect("a projection row");
        assert_eq!(&stored, signal);
    }
}
