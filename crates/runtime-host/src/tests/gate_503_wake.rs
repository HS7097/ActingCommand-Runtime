// SPDX-License-Identifier: AGPL-3.0-only

//! #503 WAKE (0.12.101; register §1.549; `contracts/runtime-config.md`, "What wakes an idle
//! instance"; `contracts/scheduling/README.md`, "Below a pool's trigger line"), the host half.
//!
//! - h1 (the work order's host test 1): two real replays on `gate_486_asleep`'s fixture instance,
//!   one with `close_app` and one with `stop_emulator`, a judged consumer of the fixture's
//!   regenerating pool given written lines (hard 426, stop 440, trigger 450, capacity 1000): held
//!   by its hard line, the instance is put to sleep with no wake; across the steps at which one run
//!   fits above the hard line it sleeps on, its plan's wake the time the pool reaches the trigger
//!   line, and no wake is recorded; at that time less the lead one wake, `planned_time_reached`;
//!   awake, the consumer is dispatched as today, and the instance sleeps again.
//! - h2 (test 2): the idle plan keeps the release of `instance_asleep_below_trigger_line` on an
//!   instance that can sleep, also when it is a regenerating pool's next step (ruling 2).
//! - h3 (test 3): `fact_refresh_unbound`, `selection_input_unknown` and
//!   `resource_reserve_insufficient` with `amount_after` are not due, so an awake instance that can
//!   sleep takes its action and an asleep one is not woken; the reserve without `amount_after` and
//!   `heavy_scene_budget_deferred` stay due (ruling 3).
//! - h4 (ruling 6): what an asleep instance does today when the consumed pool's reading expires
//!   before the release (no reader bound): the consumer's benefit is unknown and its last reason
//!   `fact_refresh_unbound`, which ruling 3 counts held, so nothing wakes the instance at the
//!   expiry; the wake planned from the release stays, and comes at its time.
//!
//! Every evaluation runs on a manual Runtime clock; nothing reads a clock.

use super::emulator_controller::stopped_state;
use super::gate_369_s6c::{connectivity, head, send};
use super::gate_372k2::{SlotTask, at, slot_sources};
use super::gate_idle1p::{emulator_starts, stops};
use super::instance_workers::{all_events, phases};
use super::*;
use crate::application_update::IdleUpdateHold;
use crate::codes::{HostCode, IdleCode, IdleLocation, IdleWakeCause};
use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView, WakeWork};
use crate::schedule_explanation::{
    IdleGates, IdleHeldAhead, IdleHeldBack, IdleSleepers, ScheduleExplanationTable,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, InstanceStuckRecovery,
    MAX_RUNTIME_SUBSCRIPTION_WAIT_MS, RuntimeConfigManifest, RuntimeLifecyclePhase,
    RuntimeSubscriptionRequest, SubscriptionCursor,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_policy::{
    DecisionReason, EligibilityState, PolicyCode, PolicyEvaluation, SchedulingDecisionState,
    TaskDecision,
};
use actingcommand_runtime_config::{
    DefaultsFile, IdleAction, IdleConfig, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// The judged consumer, triggered once 06:05 of day 1 has passed, until a run serves it.
const TASK_W: &str = "fixture.wake";
const MINUTE_MS: u64 = 60_000;
/// `idle.close_app_wake_lead_ms` and `idle.stop_emulator_wake_lead_ms`'s defaults.
const LEAD_MS: u64 = 300_000;
const GATE_WAIT: Duration = Duration::from_secs(60);
/// The fixture's static pool (`contracts/scheduling/examples/catalog-a`): 10 read at
/// `POLICY_NOW_UNIX_MS`, one unit more every six minutes; here of capacity 1000, valued 1000 milli
/// a unit, with written lines.
const FIXTURE_POOL: &str = "fixture-pool-a";
const STEP_MS: u64 = 360_000;
/// h1's lines: one run of 10 fits above the hard line from 436 on (06:36 of day 1), and the pool
/// reaches its trigger line at 450, 440 steps after its reading: 08:00 of day 1.
const LINES: (u64, u64, u64) = (426, 440, 450);
/// h4's pool, read from `READING_KEY` (capacity 180, regenerating as the fixture's pool), with its
/// lines: hard 111, stop 130, trigger 150.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
const READ_LINES: (u64, u64, u64) = (111, 130, 150);
/// The pool the consumer produces into.
const OUT: &str = "pool.out";
/// What a run of the consumer consumes and produces.
const CONSUMED: u64 = 10;
const PRODUCED: u64 = 40;
/// The unit tests' instance and instant.
const NODE: &str = "node-a";
const AT: u64 = 1_799_971_200_000;
/// `close_app` with the production gap and leads.
const CLOSE: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: LEAD_MS,
    stop_emulator_wake_lead_ms: LEAD_MS,
};
/// What a wake after `close_app` queues.
const CLOSE_WAKE: IdleStep = IdleStep::Wake {
    action: IdleAction::CloseApp,
    work: &[WakeWork::StartupPackage],
};

// ---- The catalog

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A pool of the fixture instance read from `resource.<the id without "pool.">` through the
/// ledger, of constant `capacity`, regenerating nothing, valued 1000 milli a unit.
fn valued_pool(id: &str, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": ALIAS},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
    })
}

/// `pool` with the written lines `(hard, stop, trigger)`.
fn with_lines(pool: &mut Value, (hard, stop, trigger): (u64, u64, u64)) {
    pool["hard_line"] = json!(hard);
    pool["stop_line"] = json!(stop);
    pool["trigger_line"] = json!(trigger);
}

/// `gate_372k2`'s catalog (activity window 06:00-23:30 UTC) of the one task [`TASK_W`], declared
/// V2 (every timeline event valid from 0, as `gate_486_asleep`'s), judged, its trigger a UTC clock
/// `at` 06:05 of day 1 (true once it has passed, until a run serves it, naming no later time),
/// consuming [`CONSUMED`] of `consumed` and producing [`PRODUCED`] of [`OUT`] a run; `edit` gives
/// the pools beside the valued [`OUT`].
fn wake_sources(consumed: &str, edit: impl FnOnce(&mut Vec<Value>)) -> CatalogSources {
    let trigger = json!({
        "kind": "clock",
        "schedule": {
            "kind": "at",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "at_ms": at(1, 6, 5)
        }
    });
    let mut sources = slot_sources(
        1,
        &[SlotTask {
            id: TASK_W,
            priority: 100,
            trigger,
            daily_limit: 2,
        }],
        None,
    );
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value =
            serde_json::from_slice(&source.bytes).expect("a fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if let Some(tasks) = document.get_mut("tasks").and_then(Value::as_array_mut) {
            for task in tasks {
                task["benefit_judged"] = json!(true);
                task["consumes"] = json!([effect(consumed, "consume", CONSUMED)]);
                task["produces"] = json!([effect(OUT, "produce", PRODUCED)]);
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("a fixture document's bytes");
    }
    let mut pools: Value = serde_json::from_slice(&sources.pools.bytes).expect("the pools");
    let entries = pools["pools"].as_array_mut().expect("the pool list");
    edit(entries);
    entries.push(valued_pool(OUT, 1_000_000));
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("the pools' bytes");
    sources
}

/// h1: the consumer of the fixture's static pool, which may hold 1000, valued, with [`LINES`].
fn static_sources() -> CatalogSources {
    wake_sources(FIXTURE_POOL, |pools| {
        for pool in pools.iter_mut() {
            if pool["id"] == json!(FIXTURE_POOL) {
                pool["capacity"] = json!(1000);
                pool["valuation"] = json!({
                    "name": "Fixture",
                    "unit": "unit",
                    "scale": 1,
                    "base_weight_milli": 1000
                });
                with_lines(pool, LINES);
            }
        }
    })
}

/// h4: the consumer of [`POOL`], read from its key, regenerating one unit every six minutes, with
/// [`READ_LINES`].
fn read_sources() -> CatalogSources {
    wake_sources(POOL, |pools| {
        let mut pool = valued_pool(POOL, 180);
        pool["projection"] = json!({"amount": 1, "per_ms": STEP_MS});
        with_lines(&mut pool, READ_LINES);
        pools.push(pool);
    })
}

/// [`POOL`]'s reading `value` as a package publishes it, observed at `observed_at`, valid for
/// `valid_ms`, no input invalidation (`gate_486_asleep`'s `pool_reading`).
fn pool_reading(value: i64, observed_at: u64, valid_ms: u64) -> FactRecord {
    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        READING_KEY,
        ContractFactValue::Integer(value),
        "run:fixture/frame:503-wake/k2",
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at;
    record.expires_at_unix_ms = Some(observed_at + valid_ms);
    record.confidence_milli = 990;
    record.source_detector = format!(
        "{}task/k2",
        actingcommand_contract::RESOURCE_READING_DETECTOR_PREFIX
    );
    record
}

// ---- The host (`gate_486_asleep`'s replay instance)

/// The fixture instance's scripted device: emulator-controlled from the start and its opens
/// passing, so its availability is its self-check's.
fn device_state() -> Arc<FakeState> {
    let state = Arc::new(FakeState::default());
    for flag in [
        &state.emulator_control,
        &state.observed_open,
        &state.physical_task_geometry,
        &state.transition_capture_after_input,
    ] {
        flag.store(true, Ordering::Release);
    }
    state
}

/// The `idle` section's keys as the assembly renders them from the shipped defaults file, the
/// instance's own `idle.action` `action` (with `start_emulator` beside `stop_emulator`), and its
/// no-progress guard off.
fn idle_manifest(instance: InstanceId, action: IdleAction) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let section = actingcommand_runtime_config::idle::SECTION;
    let id = crate::failure_identity::identifier_text(&instance);
    let own = |member: &str, value: FactScalar| ConfigParameter {
        key: actingcommand_runtime_config::instance_key(&id, member),
        value,
        source: ConfigParameterSource::Explicit,
        settable: false,
    };
    let mut parameters =
        render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
            .expect("every key resolves")
            .into_iter()
            .filter(|row| row.key.split('.').next() == Some(section))
            .collect::<Vec<_>>();
    parameters.push(own(
        actingcommand_runtime_config::idle::ACTION,
        FactScalar::String(action.as_str().to_owned()),
    ));
    if action == IdleAction::StopEmulator {
        parameters.push(own(
            actingcommand_runtime_config::START_EMULATOR,
            FactScalar::Boolean(true),
        ));
    }
    parameters.extend(no_progress_off_manifest(&instance).parameters);
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// A standard pack root under `root` whose game of the fixture instance declares its application
/// for its server, as `close_app` reads it.
fn declared_application(root: &TempDir) -> PathBuf {
    let packs = root.path().join("packages");
    let game = packs.join("fixture-game-a");
    fs::create_dir_all(&game).expect("the game's root");
    let declared =
        json!({"servers": {"fixture-server-a": {"application_id": "neutral.application"}}});
    fs::write(
        game.join("applications.json"),
        serde_json::to_vec_pretty(&declared).expect("the declared application"),
    )
    .expect("write the declared application");
    packs
}

/// A start of the replay instance's host with the idle action `action`: its startup package, its
/// stuck recovery off; for `close_app` the standard pack root that declares its application, for
/// `stop_emulator` its configured emulator start in the start turn and internet access at once
/// (`gate_486_asleep`'s `open_closing_host` and `open_replay_host`).
fn open_host(
    root: &TempDir,
    registered: InstanceId,
    clock: &Arc<ManualRuntimeClock>,
    state: &Arc<FakeState>,
    action: IdleAction,
) -> RuntimeHost {
    // `gate_ord1c`'s fix round 3: each capture of a preparation takes the clock's millisecond.
    let moved = Arc::clone(clock);
    *state.capture_hook.lock().expect("capture hook") = Some(Box::new(move || moved.advance(1)));
    let package = root.path().join("wake-startup-package.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write the startup package");
    let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("the startup package request");
    let mut facts = policy_facts();
    facts.outcomes.clear();
    let mut host_config = config(root)
        .with_policy_inputs(PolicyInputSnapshot::new(facts, policy_resources()))
        .with_runtime_clock(clock.clone())
        .with_config_manifest(idle_manifest(registered, action))
        .with_stuck_recovery(BTreeMap::from([(
            ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled: false,
                cooldown_secs: 600,
            },
        )]))
        .with_startup_packages(BTreeMap::from([(ALIAS.to_owned(), startup)]));
    if action == IdleAction::StopEmulator {
        let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
        host_config = host_config
            .with_emulator_autostart(vec![ALIAS.to_owned()])
            .with_network_connectivity_for_test(read);
    } else {
        host_config = host_config.with_standard_packs_root(Some(declared_application(root)));
    }
    RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(ALIAS, registered, Arc::clone(state))),
    )
    .expect("start the replay's host")
}

/// One idle record of the replay instance: its code, its `operation` and its `idle_wake_cause`.
type IdleRecord = (String, String, Option<String>);

/// The action's record (`instance_idle_action`, `run_idle_action`).
fn action_record() -> IdleRecord {
    (
        IdleCode::Taken.as_str().to_owned(),
        IdleLocation::Action.as_str().to_owned(),
        None,
    )
}

/// The record of a wake at its planned time (`instance_idle_action`, `queue_idle_wake`,
/// `planned_time_reached`).
fn wake_record() -> IdleRecord {
    (
        IdleCode::Taken.as_str().to_owned(),
        IdleLocation::Wake.as_str().to_owned(),
        Some(IdleWakeCause::PlannedTimeReached.as_str().to_owned()),
    )
}

/// The replay instance on a host of its own with its idle action and catalogue. Every idle check
/// is the host's own, on the plan its own evaluated cycle leaves, with its own planner.
struct Replay {
    host: RuntimeHost,
    clock: Arc<ManualRuntimeClock>,
    state: Arc<FakeState>,
    registered: InstanceId,
    action: IdleAction,
    approved: BTreeSet<String>,
    _root: TempDir,
}

impl Replay {
    fn start(unix_ms: u64, action: IdleAction, sources: &CatalogSources) -> Self {
        let root = TempDir::new().expect("tempdir");
        let registered = instance_id();
        let clock = Arc::new(ManualRuntimeClock::new(unix_ms, 0));
        let state = device_state();
        let host = open_host(&root, registered, &clock, &state, action);
        host.activate_policy_catalog(sources)
            .expect("activate the catalogue");
        Self {
            host,
            clock,
            state,
            registered,
            action,
            approved: BTreeSet::new(),
            _root: root,
        }
    }

    fn close(self) {
        assert!(
            self.host
                .fatal_error()
                .expect("the Runtime's health")
                .is_none()
        );
        self.host.close().expect("close the replay's host");
    }

    /// Moves the Runtime clock and samples the host's capacity at the new time.
    fn set_time(&self, unix_ms: u64) {
        self.clock.set_unix_ms(unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// How many times the idle action was taken on the device: the emulator's stops or the
    /// application's stops.
    fn actions_taken(&self) -> usize {
        if self.action == IdleAction::StopEmulator {
            stops(&self.state)
        } else {
            self.state
                .application_actions
                .lock()
                .expect("application actions")
                .iter()
                .filter(|action| **action == ApplicationLifecycleAction::Stop)
                .count()
        }
    }

    /// One evaluation at `unix_ms` on the test inputs, with no idle check.
    fn evaluate(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let mut facts = policy_facts();
        facts.outcomes.clear();
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

    /// One cycle at `unix_ms` as actingd's driver asks for it: the evaluation, then the host's own
    /// idle check on the plan it leaves.
    fn cycle(&self, unix_ms: u64) -> PolicyCycle {
        self.set_time(unix_ms);
        let cycle = self
            .host
            .evaluate_policy_cycle(PolicyTrigger::Recovery)
            .unwrap_or_else(|error| panic!("the cycle at {unix_ms}: {error}"));
        assert!(
            cycle.evaluation.is_some(),
            "the cycle at {unix_ms} is evaluated: {:?}",
            cycle.directive
        );
        cycle
    }

    /// Evaluates at `unix_ms`, admits the dispatch the cycle selected for [`TASK_W`], completes it
    /// and ends its lease, as a run of the task does.
    fn dispatch(&mut self, unix_ms: u64) {
        let cycle = self.evaluate(unix_ms);
        let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == TASK_W)
            .unwrap_or_else(|| panic!("the task is selected at {unix_ms}: {evaluation:#?}"))
            .clone();
        let reasons = evaluation
            .reason_chains
            .iter()
            .find(|chain| chain.id == intent.reason_chain_id)
            .expect("the dispatch's reason chain")
            .clone();
        if self.approved.insert(intent.catalog_hash.clone()) {
            record_policy_approval(&self.host, &intent);
        }
        let PolicyDispatchAdmission::Granted { context } = self
            .host
            .admit_policy_dispatch(&intent, &reasons, &policy_context(&self.host, &intent))
            .unwrap_or_else(|error| panic!("admit the task at {unix_ms}: {error}"))
        else {
            panic!("the task is granted at {unix_ms}");
        };
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
    }

    /// The instance's idle records after `after`, in order: `instance_idle_action` and
    /// `instance_idle_stop_withheld`.
    fn records(&self, after: u64) -> Vec<IdleRecord> {
        let events = all_events(&self.host);
        phases(&events)
            .into_iter()
            .filter(|(event, _)| {
                event.sequence() > after && event.links().instance_id() == Some(&self.registered)
            })
            .filter_map(|(_, phase)| {
                let RuntimeLifecyclePhase::DeviceStateObserved { outcome } = phase else {
                    return None;
                };
                let code = outcome.code().as_str();
                (code == IdleCode::Taken.as_str() || code == IdleCode::StopWithheld.as_str()).then(
                    || {
                        let text = |key: &str| {
                            outcome
                                .value(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        };
                        (
                            code.to_owned(),
                            text("operation").unwrap_or_default(),
                            text("idle_wake_cause"),
                        )
                    },
                )
            })
            .collect()
    }

    /// Whether nothing holds or waits for the instance's key.
    fn claims_free(&self) -> bool {
        self.host
            .instance_claims_for_test(ALIAS)
            .expect("the instance's claims")
            == (None, Vec::new())
    }

    /// Waits until the configured start the wake queued has ended and returns the code of the
    /// report line that ended it.
    fn await_start_end(&self) -> String {
        let ends = [
            HostCode::EmulatorAutostartStarted,
            HostCode::EmulatorAutostartPreparationFailed,
            HostCode::EmulatorAutostartFailed,
            HostCode::EmulatorAutostartSkipped,
        ];
        let end = |line: &Outcome| ends.iter().any(|code| line.is(*code));
        let lines = self
            .host
            .emulator_autostart_reports()
            .await_lines_for_test(|lines| lines.iter().any(end), GATE_WAIT)
            .expect("the wake's configured start ended");
        lines
            .iter()
            .find(|&line| end(line))
            .map(|line| line.code().as_str().to_owned())
            .expect("the line that ended it")
    }

    /// Waits until nothing holds or waits for the instance's key: the wake's claims have ended,
    /// its startup package's run among them (`gate_486_asleep`'s `await_free`).
    fn await_free(&self, mut after: u64) {
        let empty_answers = GATE_WAIT
            .as_millis()
            .div_ceil(u128::from(MAX_RUNTIME_SUBSCRIPTION_WAIT_MS));
        let mut empty = 0;
        while !self.claims_free() {
            let query = EventQuery {
                event_type: Some(EventType::LeaseReleased),
                instance_id: Some(self.registered),
                from_sequence: Some(after + 1),
                ..EventQuery::default()
            };
            let subscription = RuntimeSubscriptionRequest::new(
                query.clone(),
                ProjectionProfile::Forensic,
                SubscriptionCursor::default(),
                MAX_RUNTIME_SUBSCRIPTION_WAIT_MS,
                1,
            )
            .expect("subscription request");
            let receipt = send(
                &self.host,
                RuntimeOperation::SubscribeEvents {
                    request: subscription,
                },
                5_031,
            );
            let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
                panic!("an event batch: {receipt:?}");
            };
            if batch.timed_out() {
                empty += 1;
                assert!(empty < empty_answers, "the instance's key is released");
                continue;
            }
            empty = 0;
            after = self
                .host
                .query_persisted_events_for_test(query)
                .expect("the instance's key releases")
                .first()
                .map(PersistedEvent::sequence)
                .expect("the instance's next key release");
        }
        let admission = self
            .host
            .instance_admission_for_test(ALIAS)
            .expect("the instance's admission guard");
        drop(admission.lock().expect("the instance's admission guard"));
    }

    /// The instance's latest plan, as the idle check reads it.
    fn plan(&self) -> IdlePlan {
        self.host
            .idle_plan_and_runs_counted_for_test(ALIAS)
            .expect("the idle plan")
            .0
            .expect("an evaluated plan")
    }
}

/// The consumer's decision is eligible and deferred, its last reason of `code`: its wake.
fn deferred_by(cycle: &PolicyCycle, code: &str) -> Option<u64> {
    let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == TASK_W && decision.instance_id.as_deref() == Some(ALIAS)
        })
        .unwrap_or_else(|| panic!("the consumer's decision: {evaluation:#?}"));
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    assert_eq!(
        decision.eligibility,
        EligibilityState::True,
        "{decision:#?}"
    );
    assert_eq!(
        decision
            .reasons
            .last()
            .map(|reason| reason.outcome.code().as_str()),
        Some(code),
        "{decision:#?}"
    );
    assert!(
        evaluation.dispatch_intents.is_empty(),
        "nothing is dispatched: {evaluation:#?}"
    );
    decision.next_wake_unix_ms
}

// ---- h1

#[test]
fn wake_h1_an_asleep_instance_sleeps_below_the_trigger_line_and_is_woken_once_at_it() {
    let below = PolicyCode::InstanceAsleepBelowTriggerLine.as_str();
    let reached = at(1, 8, 0);
    assert_eq!(
        reached,
        POLICY_NOW_UNIX_MS + (LINES.2 - 10) * STEP_MS,
        "the pool reaches its trigger line at 08:00"
    );
    for action in [IdleAction::CloseApp, IdleAction::StopEmulator] {
        let mut replay = Replay::start(at(1, 6, 0), action, &static_sources());
        let before = head(&replay.host);

        // 06:30, read 435: one run would leave 425, below the hard line. The line holds the
        // consumer, which names no wake on this instance, and the host's own check takes the
        // action with no wake.
        assert_eq!(
            deferred_by(
                &replay.cycle(at(1, 6, 30)),
                PolicyCode::PoolLineHeld.as_str()
            ),
            Some(at(1, 6, 36)),
            "{action:?}: the pool's next step"
        );
        assert_eq!(
            replay.host.await_idle_action_ends_for_test(1, GATE_WAIT),
            Some(1),
            "{action:?}: the action ended"
        );
        assert_eq!(replay.actions_taken(), 1, "{action:?}: the action");

        // From 06:36 one run fits above the hard line. Asleep, the consumer waits for the line:
        // its wake, and the plan's, is 08:00; no wake is recorded across the steps.
        for (moment, reading) in [
            (at(1, 6, 37), 436),
            (at(1, 6, 43), 437),
            (at(1, 7, 1), 440),
            (at(1, 7, 31), 445),
        ] {
            let cycle = replay.cycle(moment);
            assert_eq!(
                deferred_by(&cycle, below),
                Some(reached),
                "{action:?} at {moment}"
            );
            let evaluation = cycle.evaluation.as_ref().expect("an evaluated cycle");
            let waits = evaluation
                .decisions
                .iter()
                .find(|decision| decision.task_id == TASK_W)
                .and_then(|decision| decision.reasons.last())
                .expect("the consumer's reason");
            for (key, expected) in [
                ("instance_id", json!(ALIAS)),
                ("pool_id", json!(FIXTURE_POOL)),
                ("pool_reading", json!(reading)),
                ("trigger_line", json!(LINES.2)),
                ("next_eligible_unix_ms", json!(reached)),
                ("lines_source", json!("catalog")),
            ] {
                assert_eq!(
                    waits.outcome.value(key),
                    Some(&expected),
                    "{action:?} at {moment}: {key}"
                );
            }
            let plan = replay.plan();
            assert!(!plan.due_now, "{action:?} at {moment}: {plan:?}");
            assert_eq!(
                plan.next_wake_unix_ms,
                Some(reached),
                "{action:?} at {moment}: {plan:?}"
            );
        }
        assert_eq!(
            replay.records(before),
            [action_record()],
            "{action:?}: one action, no wake across the steps"
        );

        // 07:55, the line less the lead: one wake at its planned time.
        if action == IdleAction::StopEmulator {
            replay.state.emulator.script_read(stopped_state());
        }
        let woken_at = head(&replay.host);
        assert_eq!(
            deferred_by(&replay.cycle(reached - LEAD_MS), below),
            Some(reached),
            "{action:?}: asleep until the wake"
        );
        assert_eq!(
            replay.records(before),
            [action_record(), wake_record()],
            "{action:?}: the wake at its planned time"
        );
        if action == IdleAction::StopEmulator {
            assert_eq!(
                replay.await_start_end(),
                HostCode::EmulatorAutostartStarted.as_str(),
                "the wake's claim started the emulator"
            );
            assert_eq!(emulator_starts(&replay.state), 1, "one Start");
        }
        replay.await_free(woken_at);

        // Awake, the consumer is dispatched as today; its trigger served, the instance sleeps
        // again.
        replay.dispatch(reached - 2 * MINUTE_MS);
        replay.cycle(reached + 5 * MINUTE_MS);
        assert_eq!(
            replay.host.await_idle_action_ends_for_test(2, GATE_WAIT),
            Some(2),
            "{action:?}: the second action ended"
        );
        assert_eq!(replay.actions_taken(), 2, "{action:?}: asleep again");
        assert_eq!(
            replay.records(before),
            [action_record(), wake_record(), action_record()],
            "{action:?}: woken once"
        );
        replay.close();
    }
}

// ---- h4 (ruling 6)

#[test]
fn wake_h4_a_reading_that_expires_before_the_release_wakes_nothing_and_the_planned_wake_stays() {
    let below = PolicyCode::InstanceAsleepBelowTriggerLine.as_str();
    let replay = Replay::start(at(1, 6, 0), IdleAction::CloseApp, &read_sources());
    let before = head(&replay.host);
    // The pool read at 120 at 06:29, valid for twenty minutes.
    let read_at = at(1, 6, 29);
    replay.set_time(read_at);
    replay
        .host
        .publish_fact(pool_reading(120, read_at, 20 * MINUTE_MS))
        .expect("the pool's reading is published");

    // 06:30: one run would leave 110, below the hard line; the application is closed.
    assert_eq!(
        deferred_by(
            &replay.cycle(at(1, 6, 30)),
            PolicyCode::PoolLineHeld.as_str()
        ),
        Some(at(1, 6, 35)),
        "the pool's next step"
    );
    assert_eq!(
        replay.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "the close ended"
    );

    // 06:36, 121: the consumer waits for the line, 150 at 09:29.
    let release = read_at + (READ_LINES.2 - 120) * STEP_MS;
    assert_eq!(release, at(1, 9, 29));
    assert_eq!(
        deferred_by(&replay.cycle(at(1, 6, 36)), below),
        Some(release)
    );
    let plan = replay.plan();
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, Some(release), "{plan:?}");

    // 06:50, past the reading's expiry: what this build does. The consumer's benefit is unknown
    // (its reading expired) and no reader is bound, so its last reason is `fact_refresh_unbound`,
    // which ruling 3 counts held: not due and no wake of its own. The instance is not woken.
    let expired = replay.cycle(at(1, 6, 50));
    assert_eq!(
        deferred_by(&expired, PolicyCode::FactRefreshUnbound.as_str()),
        None,
        "no wake of its own"
    );
    let unknown = expired
        .evaluation
        .as_ref()
        .expect("an evaluated cycle")
        .decisions
        .iter()
        .find(|decision| decision.task_id == TASK_W)
        .expect("the consumer's decision")
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(PolicyCode::BenefitUnknown))
        .expect("its benefit is unknown")
        .outcome
        .value("benefit_gap")
        .cloned();
    assert_eq!(unknown, Some(json!("reading_unknown")));
    let plan = replay.plan();
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, None, "{plan:?}");
    assert_eq!(
        replay.records(before),
        [action_record()],
        "nothing wakes the instance at the expiry"
    );

    // The wake planned from the release less the lead stays, and comes at its time.
    let woken_at = head(&replay.host);
    replay.cycle(release - LEAD_MS);
    assert_eq!(
        replay.records(before),
        [action_record(), wake_record()],
        "the planned wake"
    );
    replay.await_free(woken_at);
    replay.close();
}

// ---- h2, h3: the idle plan

fn reason_of(outcome: Outcome) -> DecisionReason {
    let detail = format!("fixture: {}", outcome.code().as_str());
    DecisionReason { outcome, detail }
}

/// An eligible deferred decision of the instance with `last` as its one reason and its own wake.
fn deferred(last: Outcome, next_wake_unix_ms: Option<u64>) -> TaskDecision {
    TaskDecision {
        task_id: TASK_W.to_owned(),
        instance_id: Some(NODE.to_owned()),
        eligibility: EligibilityState::True,
        state: SchedulingDecisionState::Deferred,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: vec![reason_of(last)],
        next_wake_unix_ms,
    }
}

/// A task of the instance whose trigger is false, naming no time.
fn idle_task() -> TaskDecision {
    TaskDecision {
        task_id: "task.later".to_owned(),
        eligibility: EligibilityState::False,
        state: SchedulingDecisionState::Blocked,
        ..deferred(Outcome::new(PolicyCode::TriggerFalse), None)
    }
}

/// The idle plan of the instance from an evaluation of `decisions` at `at`; with `pool_steps` the
/// instance is one whose idle action can put it to sleep and those are the next projection steps
/// of the regenerating pools the evaluation read.
fn plan_of(decisions: Vec<TaskDecision>, at: u64, pool_steps: Option<&BTreeSet<u64>>) -> IdlePlan {
    let none = BTreeSet::new();
    let instances = if pool_steps.is_some() {
        BTreeSet::from([NODE.to_owned()])
    } else {
        BTreeSet::new()
    };
    let mut table = ScheduleExplanationTable::default();
    table.record_stopping(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:503-wake",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
        IdleGates {
            held_back: &IdleHeldBack::new(),
            held_ahead: &IdleHeldAhead::new(),
            update_hold: IdleUpdateHold::default(),
        },
        IdleSleepers {
            instances: &instances,
            pool_steps: pool_steps.unwrap_or(&none),
        },
    );
    table.idle_plan(NODE).expect("an evaluated plan")
}

/// `instance_asleep_below_trigger_line` released at `release`.
fn below_line(release: u64) -> Outcome {
    Outcome::new(PolicyCode::InstanceAsleepBelowTriggerLine)
        .instance_id(NODE)
        .pool_id("pool.neutral")
        .pool_reading(121_i64)
        .trigger_line(150_u64)
        .next_eligible_unix_ms(release)
}

/// The instance free, its emulator running, both actions able to bring it back.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

#[test]
fn wake_h2_the_release_stays_the_plans_wake_also_when_it_is_a_pool_step() {
    let release = AT + 6 * MINUTE_MS;
    let steps = BTreeSet::from([release]);
    // On an instance that can sleep: not due, its wake the release, though it is a pool's step.
    let plan = plan_of(
        vec![deferred(below_line(release), Some(release))],
        AT,
        Some(&steps),
    );
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, Some(release), "{plan:?}");
    // On any other instance as well.
    let plan = plan_of(vec![deferred(below_line(release), Some(release))], AT, None);
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, Some(release), "{plan:?}");
    // Ruling 7 is unchanged for every other decision: a wake that is a pool's step names none there.
    let other = deferred(Outcome::new(PolicyCode::ScoreDeferred), Some(release));
    let plan = plan_of(vec![other], AT, Some(&steps));
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, None, "{plan:?}");
    // Not due, whatever its wake.
    let plan = plan_of(vec![deferred(below_line(release), None)], AT, Some(&steps));
    assert!(!plan.due_now, "{plan:?}");
}

#[test]
fn wake_h3_three_answers_only_a_change_lets_through_are_held_and_the_others_stay_due() {
    let steps = BTreeSet::new();
    let later = AT + 10 * MINUTE_MS;
    let held = [
        Outcome::new(PolicyCode::FactRefreshUnbound)
            .instance_id(NODE)
            .fact_key("resource.neutral"),
        Outcome::new(PolicyCode::SelectionInputUnknown).instance_id(NODE),
        Outcome::new(PolicyCode::ResourceReserveInsufficient)
            .pool_id("pool.neutral")
            .reserve_at_least(10_i64)
            .amount_after(5_i64),
    ];
    for last in held {
        let code = last.code().as_str().to_owned();
        let decisions = || vec![deferred(last.clone(), None), idle_task()];
        let plan = plan_of(decisions(), AT, Some(&steps));
        assert!(!plan.due_now, "{code}: {plan:?}");
        assert_eq!(plan.next_wake_unix_ms, None, "{code}: {plan:?}");
        // Awake, the instance takes its action; asleep, it is not woken.
        let mut planner = IdlePlanner::default();
        assert_eq!(
            planner.check(NODE, &CLOSE, free(), Some(plan), AT),
            Some(IdleStep::Act {
                action: IdleAction::CloseApp,
                next_due_unix_ms: None,
                wake_at_unix_ms: None,
            }),
            "{code}: the action"
        );
        planner.finished(NODE, IdleEnd::Done);
        let plan = plan_of(decisions(), later, Some(&steps));
        assert_eq!(
            planner.check(NODE, &CLOSE, free(), Some(plan), later),
            None,
            "{code}: not woken"
        );
    }

    let due = [
        Outcome::new(PolicyCode::ResourceReserveInsufficient)
            .pool_id("pool.neutral")
            .reserve_at_least(10_i64),
        Outcome::new(PolicyCode::HeavySceneBudgetDeferred),
    ];
    for last in due {
        let code = last.code().as_str().to_owned();
        let plan = plan_of(vec![deferred(last.clone(), None)], AT, Some(&steps));
        assert!(plan.due_now, "{code}: {plan:?}");
        // Awake, the instance is not closed; closed, it is woken at once.
        let mut open = IdlePlanner::default();
        assert_eq!(
            open.check(NODE, &CLOSE, free(), Some(plan), AT),
            None,
            "{code}: kept open"
        );
        let mut planner = IdlePlanner::default();
        let quiet = plan_of(vec![idle_task()], AT, Some(&steps));
        assert!(
            matches!(
                planner.check(NODE, &CLOSE, free(), Some(quiet), AT),
                Some(IdleStep::Act { .. })
            ),
            "{code}: closed first"
        );
        planner.finished(NODE, IdleEnd::Done);
        let plan = plan_of(vec![deferred(last, None)], later, Some(&steps));
        assert_eq!(
            planner.check(NODE, &CLOSE, free(), Some(plan), later),
            Some(CLOSE_WAKE),
            "{code}: woken"
        );
    }
}
