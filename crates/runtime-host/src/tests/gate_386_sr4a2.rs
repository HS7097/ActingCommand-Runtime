// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-4a2 (SCH-M frozen model v1.0 `rd-1024/sched-design/model.md` §3.2 D-3, D-4,
//! D-6 and §6 row SR-4a2; SCHED v1.4.2 T14, T15, T16, T18; `contracts/scheduling-blackout.md`,
//! "Order of the deferrals", "Gradual quiet", "Day-boundary fit"): the policy closes over the
//! blackouts of its three sources.
//!
//! - Position 4: inside a blackout a candidate is deferred `blackout_active` and wakes at its end.
//! - Position 7: a candidate whose horizon (its response deadline, or its expected duration E
//!   under `quiet_horizon = expected`) plus `quiet_margin_ms` reaches the next blackout start is
//!   deferred `blackout_approaching` and wakes at that blackout's end; then, on an instance whose
//!   `scheduling.day_boundary.quiet` is true, one whose E does not fit its activity window is
//!   deferred `policy_day_boundary_approaching` and wakes at the next window start.
//! - Admission judges both again at its own time and refuses with the same codes.
//! - The random delay is cut at the quiet point.
//! - `ScheduleExplanation` (`actingctl schedule`, MCP `ac_schedule`) shows these reasons.
//! - Without any entry or catalog `maintenance` event nothing of this changes an evaluation.
//!
//! Neutral fixtures only: generic tasks on the fixture instances, UTC clocks, a manual Runtime
//! clock; every interval is configuration or catalog data written here. Every expected value is
//! the model's or the contract's requirement.

use super::gate_372k2::at;
use super::*;
use crate::codes::{HostCode, SchedulingRuleHostCode};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, PolicyAdmissionRecord,
    RuntimeConfigManifest, RuntimeScheduleExplanation, ScheduleExplanationInstance,
};
use actingcommand_policy::{PolicyEvaluation, SchedulingDecisionState, TaskDecision};
use actingcommand_runtime_config::instance_key;
use actingcommand_runtime_config::scheduling::{
    BLACKOUT_HOST, BLACKOUT_QUIET_HORIZON, BLACKOUT_QUIET_MARGIN_MS, DAY_BOUNDARY_QUIET,
    INSTANCE_BLACKOUT, RANDOM_DELAY_ENABLED, RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS,
};
use serde_json::{Value, json};

const SEED: u64 = 11;
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
const TASK_A: &str = "task_a";
const TASK_B: &str = "task_b";
const SHORT: &str = "task_short";
const LONG: &str = "task_long";
/// The response deadline of every scheduled policy run that is not a fact refresh read
/// (`ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS`), the default horizon of the quiet.
const DEADLINE_MS: u64 = ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS;
/// `scheduling.blackout.quiet_margin_ms` as shipped.
const MARGIN_MS: u64 = 60_000;
/// The fixture's short expected duration, and a long one.
const SHORT_MS: u64 = 60_000;
/// The activity window of the plain fixture, 06:00 to 23:30 UTC.
const DAY_WINDOW: (u16, u16) = (6 * 60, 23 * 60 + 30);
/// A bookkeeping-day window `[reset, reset - 1 min)` for a reset at 04:00 UTC.
const BOOKKEEPING_WINDOW: (u16, u16) = (4 * 60, 4 * 60 - 1);

// ---- The catalog.

/// One generic task on one instance.
struct Task {
    id: &'static str,
    instance: &'static str,
    priority: u32,
    trigger: Value,
    expected_ms: u64,
    daily_limit: u32,
}

impl Task {
    /// A task due at every evaluation (a local interval clock of 1 ms).
    fn always(id: &'static str, instance: &'static str, priority: u32, expected_ms: u64) -> Self {
        Self {
            id,
            instance,
            priority,
            trigger: json!({
                "kind": "clock",
                "schedule": {"kind": "interval", "clock_source": {"kind": "local"},
                             "every_ms": 1, "anchor_ms": 0}
            }),
            expected_ms,
            daily_limit: 24,
        }
    }

    /// A task due once a day at `minute` of the UTC day, at most once a day.
    fn daily(id: &'static str, instance: &'static str, minute: u16) -> Self {
        Self {
            id,
            instance,
            priority: 100,
            trigger: json!({
                "kind": "clock",
                "schedule": {
                    "kind": "daily",
                    "clock_source": {"kind": "server", "timezone_id": "etc/utc",
                                     "utc_offset_minutes": 0, "dst_offset_minutes": 0,
                                     "maintenance_drift_ms": 0},
                    "minutes_of_day": [minute]
                }
            }),
            expected_ms: SHORT_MS,
            daily_limit: 1,
        }
    }
}

/// A V2 catalog of `tasks` with one game-wide activity profile of every day's `window` and the
/// timeline events `maintenance` besides the fixture's reset.
fn catalog(tasks: &[Task], window: (u16, u16), maintenance: &[Value]) -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = tasks
        .iter()
        .map(|task| {
            let mut spec = base.clone();
            spec["id"] = json!(task.id);
            spec["scope"] = json!({"kind": "instance", "instance_id": task.instance});
            spec["priority"] = json!(task.priority);
            spec["trigger"] = task.trigger.clone();
            spec["cooldown_ms"] = json!(0);
            spec["instance_overrides"] = json!([]);
            spec["produces"] = json!([]);
            spec["expected_duration_ms"] = json!(task.expected_ms);
            spec["on_failure"]["escalation_threshold"] = json!(1000);
            spec["loop_budget"] = json!({
                "daily_limit": task.daily_limit,
                "window_iteration_limit": task.daily_limit,
                "max_runtime_ms": task.expected_ms.max(300_000)
            });
            spec
        })
        .collect::<Vec<_>>()
        .into();
    sources.tasks.bytes = serde_json::to_vec_pretty(&document).expect("task bytes");

    let mut activity: Value =
        serde_json::from_slice(&sources.activity.bytes).expect("fixture activity");
    // The game-wide profile alone, so every fixture instance runs under it.
    let mut profile = activity["profiles"][1].clone();
    profile["windows"] = json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": window.0,
        "end_minute_of_day": window.1
    }]);
    profile["daily_budget"] = json!(48);
    profile["max_window_iterations"] = json!(48);
    profile["minimum_interval_ms"] = json!(1);
    profile["maximum_interval_ms"] = json!(1);
    activity["profiles"] = json!([profile]);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("activity bytes");

    let mut timeline: Value =
        serde_json::from_slice(&sources.timeline.bytes).expect("fixture timeline");
    let events = timeline["events"].as_array_mut().expect("timeline events");
    events.extend(maintenance.iter().cloned());
    sources.timeline.bytes = serde_json::to_vec_pretty(&timeline).expect("timeline bytes");

    // V2, the version whose timeline events carry explicit validity (a maintenance event needs
    // it); the fixture's reset gets an unbounded one.
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value = serde_json::from_slice(&source.bytes).expect("fixture document");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                if event.get("validity").is_none() {
                    event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
                }
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 bytes");
    }
    sources
}

/// A catalog `maintenance` event of the server `server_id`: every day at `minute` UTC for
/// `duration_ms`, valid from the epoch on.
fn maintenance(id: &str, server_id: &str, minute: u16, duration_ms: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "server", "server_id": server_id},
        "event_kind": "maintenance",
        "schedule": {
            "kind": "daily",
            "clock_source": {"kind": "server", "timezone_id": "etc/utc",
                             "utc_offset_minutes": 0, "dst_offset_minutes": 0,
                             "maintenance_drift_ms": 0},
            "minutes_of_day": [minute]
        },
        "duration_ms": duration_ms,
        "invalidates_fact_prefixes": [],
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    })
}

/// A `once` blackout entry `[from, until)`.
fn once(id: &str, from: u64, until: u64) -> Value {
    json!({"kind": "once", "id": id, "from_unix_ms": from, "until_unix_ms": until})
}

/// A blackout list as the configuration holds it (JSON text).
fn list(entries: &[Value]) -> FactScalar {
    FactScalar::String(Value::Array(entries.to_vec()).to_string())
}

// ---- The host.

struct Fixture {
    _root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    host: RuntimeHost,
    /// Each fixture instance's registry id as the configuration keys name it, by alias.
    ids: BTreeMap<&'static str, String>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    approved: bool,
}

impl Fixture {
    /// A host of the instances `aliases` (the fixture instance, and the second fixture instance
    /// when named) whose configuration sets `settings` (given each instance's registry id, by
    /// alias), with `sources` active and its clock at `at_unix_ms`.
    fn start(
        aliases: &[&'static str],
        settings: impl FnOnce(&BTreeMap<&'static str, String>) -> Vec<(String, FactScalar)>,
        sources: &CatalogSources,
        at_unix_ms: u64,
    ) -> Self {
        let root = TempDir::new().expect("tempdir");
        let instances = aliases
            .iter()
            .map(|alias| (*alias, instance_id()))
            .collect::<Vec<_>>();
        let ids = instances
            .iter()
            .map(|(alias, id)| (*alias, crate::failure_identity::identifier_text(id)))
            .collect::<BTreeMap<_, _>>();
        let mut parameters = settings(&ids)
            .into_iter()
            .map(|(key, value)| ConfigParameter {
                key,
                value,
                source: ConfigParameterSource::Explicit,
                settable: false,
            })
            .collect::<Vec<_>>();
        actingcommand_runtime_config::annotate_settable(&mut parameters)
            .expect("every key is in the key table");
        let mut facts = policy_facts();
        facts.outcomes.clear();
        let mut resources = policy_resources();
        if aliases.contains(&B) {
            facts.instances.push(InstanceSnapshot {
                instance_id: B.to_owned(),
                server_id: "fixture-server-b".to_owned(),
                game_id: "fixture-game-a".to_owned(),
                host_id: "fixture-host-b".to_owned(),
                available: true,
                capability_operation_ids: vec!["operation.observe".to_owned()],
                preferred_task_ids: Vec::new(),
            });
            resources = pending_policy_resources();
        }
        let clock = Arc::new(ManualRuntimeClock::new(at_unix_ms, 0));
        let mut host_config = config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(facts.clone(), resources.clone()))
            .with_runtime_clock(clock.clone());
        // Without settings the host runs on the shipped defaults alone.
        if !parameters.is_empty() {
            host_config = host_config.with_config_manifest(RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters,
            });
        }
        let host = RuntimeHost::start(
            host_config,
            Arc::new(FakeProvider::from_entries(instances.iter().map(
                |(alias, id)| ((*alias).to_owned(), *id, Arc::new(FakeState::default())),
            ))),
        )
        .expect("start the blackout host");
        host.activate_policy_catalog(sources)
            .expect("activate the catalog");
        Self {
            _root: root,
            clock,
            host,
            ids,
            facts,
            resources,
            approved: false,
        }
    }

    fn close(self) {
        self.host.close().expect("close the host");
    }

    /// Moves the Runtime clock (both clocks) to `unix_ms` and samples the host's capacity there,
    /// as its capacity monitor does while time passes.
    fn set_time(&self, unix_ms: u64) {
        advance_manual_clock_to(&self.clock, unix_ms);
        let sample_capacity = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample_capacity().expect("capacity sample");
    }

    /// One full evaluation at `unix_ms`.
    fn evaluate(&self, unix_ms: u64) -> PolicyEvaluation {
        self.set_time(unix_ms);
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &self.facts,
                &self.resources,
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                SEED,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
            .evaluation
            .expect("a full evaluation")
    }

    /// The admission of `intent` at the Runtime clock's time, the catalog's approval recorded
    /// once before.
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

    /// Admits `intent`, completes its run and ends its lease: its admission record.
    fn run(
        &mut self,
        intent: &DispatchIntent,
        chain: &DecisionReasonChain,
    ) -> PolicyAdmissionRecord {
        let admission = self
            .admit(intent, chain)
            .unwrap_or_else(|error| panic!("admit {}: {error}", intent.task_id));
        let PolicyDispatchAdmission::Granted { context } = admission else {
            panic!("{} is granted", intent.task_id);
        };
        let record = context.admission().clone();
        self.host
            .complete_policy_dispatch(&intent.decision_id)
            .expect("complete the dispatch");
        self.host
            .release_policy_dispatch_lease_for_test(&context)
            .expect("end the dispatch's lease");
        record
    }

    /// One `ConfigureSettings` of `key`, as `actingctl config set` sends it.
    fn configure(&self, key: String, value: Value) {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::ConfigureSettings {
            expected_version: None,
            changes: vec![ConfigSettingChange {
                key,
                value: Some(value),
            }],
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
    }

    /// The ScheduleExplanation read with candidates (the read `ac_schedule` makes) of `alias`.
    fn explanation(&self, alias: &str) -> ScheduleExplanationInstance {
        let mut client = TestClient::connect(&self.host);
        let request = client.request(RuntimeOperation::ScheduleExplanation {
            instance: Some(alias.to_owned()),
            candidates: true,
        });
        let receipt = client.send(&request);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::ScheduleExplanation { explanation }) = receipt.result() else {
            panic!("a schedule explanation: {:?}", receipt.result());
        };
        let RuntimeScheduleExplanation { instances, .. } = (**explanation).clone();
        let [entry] = instances.as_slice() else {
            panic!("one entry for {alias}: {instances:#?}");
        };
        entry.clone()
    }

    /// The `policy.dispatch_rejected` record of `intent`.
    fn rejected(&self, intent: &DispatchIntent) -> PersistedEvent {
        self.host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyDispatchRejected),
                ..EventQuery::default()
            })
            .expect("rejected records")
            .into_iter()
            .find(|event| {
                matches!(
                    event.payload(),
                    EventPayload::Policy(PolicyPayload::DispatchRejected(payload))
                        if payload.decision_id() == intent.decision_id
                )
            })
            .unwrap_or_else(|| panic!("a rejected record of {}", intent.decision_id))
    }
}

// ---- What an evaluation says.

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str, alias: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {alias}: {evaluation:#?}"))
}

/// The dispatch of `task_id` on `alias` with its reason chain, if the evaluation selected it.
fn dispatched(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    alias: &str,
) -> Option<(DispatchIntent, DecisionReasonChain)> {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id && intent.instance_id == alias)?
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    Some((intent, chain))
}

fn selected(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    alias: &str,
) -> (DispatchIntent, DecisionReasonChain) {
    dispatched(evaluation, task_id, alias)
        .unwrap_or_else(|| panic!("{task_id} on {alias} is dispatched: {evaluation:#?}"))
}

/// The reason of `code` that defers `task_id` on `alias`; the decision is deferred, waits until
/// `wake` and is not dispatched.
fn deferred(
    evaluation: &PolicyEvaluation,
    task_id: &str,
    alias: &str,
    code: impl Into<Code>,
    wake: u64,
) -> Outcome {
    let code: Code = code.into();
    let decision = decision(evaluation, task_id, alias);
    let reason = decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(code))
        .unwrap_or_else(|| {
            panic!(
                "{task_id} on {alias} is deferred {}: {decision:#?}",
                code.as_str()
            )
        })
        .outcome
        .clone();
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    assert_eq!(decision.next_wake_unix_ms, Some(wake), "{decision:#?}");
    assert!(
        dispatched(evaluation, task_id, alias).is_none(),
        "a deferred pair is not dispatched: {evaluation:#?}"
    );
    reason
}

/// Whether any decision of the evaluation carries one of the three codes of this slice.
fn names_a_blackout(evaluation: &PolicyEvaluation) -> bool {
    evaluation.decisions.iter().any(|decision| {
        decision.reasons.iter().any(|reason| {
            reason.outcome.is(SchedulingRuleHostCode::BlackoutActive)
                || reason
                    .outcome
                    .is(SchedulingRuleHostCode::BlackoutApproaching)
                || reason
                    .outcome
                    .is(SchedulingRuleHostCode::PolicyDayBoundaryApproaching)
        })
    })
}

fn value<'a>(outcome: &'a Outcome, key: &str) -> &'a Value {
    outcome
        .value(key)
        .unwrap_or_else(|| panic!("{key} in {outcome:?}"))
}

/// The admission's refusal: its code, Info and not fatal, recorded at Info with the wake as its
/// `next_eligible_unix_ms`; the rejection's outcome.
fn refused(
    fixture: &mut Fixture,
    intent: &DispatchIntent,
    chain: &DecisionReasonChain,
    code: SchedulingRuleHostCode,
    wake: u64,
) -> Outcome {
    let error = fixture
        .admit(intent, chain)
        .expect_err("the admission is refused");
    assert_eq!(error.code(), code.as_str(), "{error:?}");
    assert!(!error.is_fatal(), "{error:?}");
    let rejected = fixture.rejected(intent);
    assert_eq!(rejected.severity(), EventSeverity::Info);
    let EventPayload::Policy(PolicyPayload::DispatchRejected(payload)) = rejected.payload() else {
        panic!("a rejected payload");
    };
    let rejection = payload.rejection().expect("the rejection");
    assert_eq!(rejection, &error.policy_rejection());
    assert!(rejection.outcome.is(code), "{rejection:?}");
    assert_eq!(rejection.next_eligible_unix_ms, Some(wake));
    assert!(
        fixture
            .host
            .query_persisted_events_for_test(EventQuery {
                event_type: Some(EventType::PolicyDispatchAdmitted),
                ..EventQuery::default()
            })
            .expect("admitted records")
            .iter()
            .all(|event| !matches!(
                event.payload(),
                EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                    if payload.decision_id() == intent.decision_id
            )),
        "a refused intent is not admitted"
    );
    rejection.outcome.clone()
}

// ---- T14 (SCHED §5.2, L726 「黑名单制」): a catalog maintenance event in effect defers the
// candidates in its scope with blackout_active, waking at its end; an intent evaluated before it
// that crosses into it is refused at admission with the same code; another server's event defers
// nothing here.
#[test]
fn sr4a2_t14_a_catalog_maintenance_event_defers_its_scope_and_admission_refuses_a_crossing_intent()
{
    let start = at(0, 14, 0);
    let end = at(0, 15, 0);
    let sources = catalog(
        &[Task::always(TASK_A, A, 100, SHORT_MS)],
        DAY_WINDOW,
        &[
            maintenance("neutral.maintenance", "fixture-server-a", 14 * 60, HOUR_MS),
            maintenance("neutral.other-server", "fixture-server-b", 10 * 60, HOUR_MS),
        ],
    );
    let mut fixture = Fixture::start(&[A], |_| Vec::new(), &sources, at(0, 10, 30));

    let evaluation = fixture.evaluate(at(0, 10, 30));
    selected(&evaluation, TASK_A, A);
    assert!(!names_a_blackout(&evaluation), "{evaluation:#?}");

    // The deadline horizon and the margin end exactly at the start: the last admissible instant.
    let evaluated = fixture.evaluate(start - DEADLINE_MS - MARGIN_MS);
    let (intent, chain) = selected(&evaluated, TASK_A, A);
    fixture.set_time(start + MINUTE_MS);
    let refusal = refused(
        &mut fixture,
        &intent,
        &chain,
        SchedulingRuleHostCode::BlackoutActive,
        end,
    );
    assert_eq!(value(&refusal, "instance_id"), &json!(fixture.ids[A]));
    assert_eq!(
        value(&refusal, "blackout_id"),
        &json!("neutral.maintenance")
    );
    assert_eq!(value(&refusal, "blackout_source"), &json!("catalog"));
    assert_eq!(value(&refusal, "next_eligible_unix_ms"), &json!(end));

    let inside = fixture.evaluate(start + 30 * MINUTE_MS);
    let reason = deferred(
        &inside,
        TASK_A,
        A,
        SchedulingRuleHostCode::BlackoutActive,
        end,
    );
    assert_eq!(value(&reason, "instance_id"), &json!(A));
    assert_eq!(value(&reason, "blackout_id"), &json!("neutral.maintenance"));
    assert_eq!(value(&reason, "blackout_source"), &json!("catalog"));
    assert_eq!(value(&reason, "next_eligible_unix_ms"), &json!(end));

    let after = fixture.evaluate(end);
    selected(&after, TASK_A, A);
    fixture.close();
}

// ---- T16 (SCHED §5.3 item 1, L726 「渐进静默」): a candidate whose response deadline plus
// quiet_margin_ms passes the next blackout start is deferred blackout_approaching and wakes at the
// blackout's end; admission refuses an intent that no longer fits; the margin in effect decides,
// and a change of it changes the result; ScheduleExplanation shows the reason.
#[test]
fn sr4a2_t16_the_quiet_defers_a_deadline_that_reaches_the_start_and_follows_the_margin() {
    let start = at(0, 14, 0);
    let end = at(0, 15, 0);
    let last = start - DEADLINE_MS - MARGIN_MS;
    let sources = catalog(&[Task::always(TASK_A, A, 100, SHORT_MS)], DAY_WINDOW, &[]);
    let mut fixture = Fixture::start(
        &[A],
        |_| {
            vec![
                (
                    BLACKOUT_HOST.to_owned(),
                    list(&[once("quiet-window", start, end)]),
                ),
                (
                    BLACKOUT_QUIET_MARGIN_MS.to_owned(),
                    FactScalar::DurationMs(MARGIN_MS),
                ),
            ]
        },
        &sources,
        last,
    );

    let evaluated = fixture.evaluate(last);
    let (intent, chain) = selected(&evaluated, TASK_A, A);
    assert!(!names_a_blackout(&evaluated), "{evaluated:#?}");
    fixture.set_time(last + 1);
    let refusal = refused(
        &mut fixture,
        &intent,
        &chain,
        SchedulingRuleHostCode::BlackoutApproaching,
        end,
    );
    assert_eq!(value(&refusal, "instance_id"), &json!(fixture.ids[A]));
    assert_eq!(value(&refusal, "blackout_id"), &json!("quiet-window"));
    assert_eq!(value(&refusal, "blackout_source"), &json!("host"));
    assert_eq!(value(&refusal, "blackout_start_unix_ms"), &json!(start));
    assert_eq!(value(&refusal, "required_ms"), &json!(DEADLINE_MS));

    let quiet = fixture.evaluate(last + 1);
    let reason = deferred(
        &quiet,
        TASK_A,
        A,
        SchedulingRuleHostCode::BlackoutApproaching,
        end,
    );
    assert_eq!(value(&reason, "instance_id"), &json!(A));
    assert_eq!(value(&reason, "blackout_id"), &json!("quiet-window"));
    assert_eq!(value(&reason, "blackout_source"), &json!("host"));
    assert_eq!(value(&reason, "blackout_start_unix_ms"), &json!(start));
    assert_eq!(value(&reason, "required_ms"), &json!(DEADLINE_MS));
    let entry = fixture.explanation(A);
    assert_eq!(entry.next_wake_unix_ms, Some(end));
    assert_eq!(entry.not_running.task_id.as_deref(), Some(TASK_A));
    assert!(
        entry
            .not_running
            .why
            .is(SchedulingRuleHostCode::BlackoutApproaching),
        "{entry:#?}"
    );
    assert_eq!(
        value(&entry.not_running.why, "blackout_start_unix_ms"),
        &json!(start)
    );

    // Without a margin the same instant plus one millisecond fits again, and the quiet starts at
    // the deadline alone.
    fixture.configure(BLACKOUT_QUIET_MARGIN_MS.to_owned(), json!(0));
    selected(&fixture.evaluate(last + 2), TASK_A, A);
    let quiet = fixture.evaluate(start - DEADLINE_MS + 1);
    deferred(
        &quiet,
        TASK_A,
        A,
        SchedulingRuleHostCode::BlackoutApproaching,
        end,
    );

    selected(&fixture.evaluate(end), TASK_A, A);
    fixture.close();
}

// ---- T16 (SCHED §5.3 item 2: 「越接近开始，只有越短的任务能进」): under quiet_horizon expected the
// horizon is each candidate's expected duration, so near the start the long candidate waits while
// the short one still runs, until its own quiet point.
#[test]
fn sr4a2_t16_shorter_candidates_still_run_up_to_their_own_quiet_point() {
    const LONG_MS: u64 = 20 * MINUTE_MS;
    let start = at(0, 14, 0);
    let end = at(0, 15, 0);
    let sources = catalog(
        &[
            Task::always(SHORT, A, 100, SHORT_MS),
            Task::always(LONG, A, 300, LONG_MS),
        ],
        DAY_WINDOW,
        &[],
    );
    let fixture = Fixture::start(
        &[A],
        |_| {
            vec![
                (
                    BLACKOUT_HOST.to_owned(),
                    list(&[once("quiet-window", start, end)]),
                ),
                (
                    BLACKOUT_QUIET_HORIZON.to_owned(),
                    FactScalar::String("expected".to_owned()),
                ),
            ]
        },
        &sources,
        at(0, 13, 0),
    );

    // Far from the start the higher-ranked long candidate runs.
    selected(&fixture.evaluate(at(0, 13, 0)), LONG, A);

    let near = fixture.evaluate(at(0, 13, 45));
    let reason = deferred(
        &near,
        LONG,
        A,
        SchedulingRuleHostCode::BlackoutApproaching,
        end,
    );
    assert_eq!(value(&reason, "required_ms"), &json!(LONG_MS));
    selected(&near, SHORT, A);

    let short_last = start - SHORT_MS - MARGIN_MS;
    selected(&fixture.evaluate(short_last), SHORT, A);
    let quiet = fixture.evaluate(short_last + 1);
    let reason = deferred(
        &quiet,
        SHORT,
        A,
        SchedulingRuleHostCode::BlackoutApproaching,
        end,
    );
    assert_eq!(value(&reason, "required_ms"), &json!(SHORT_MS));
    deferred(
        &quiet,
        LONG,
        A,
        SchedulingRuleHostCode::BlackoutApproaching,
        end,
    );
    fixture.close();
}

// ---- T18 (SR:650, L726 「恢复后立即执行」, SCHED §5.4): a clock that falls due inside a blackout is
// dispatched once at the first evaluation after it, counted in the same bookkeeping day.
#[test]
fn sr4a2_t18_a_clock_due_inside_a_blackout_runs_once_after_it_in_the_same_bookkeeping_day() {
    let sources = catalog(&[Task::daily(TASK_A, A, 12 * 60)], DAY_WINDOW, &[]);
    let mut fixture = Fixture::start(
        &[A],
        |_| {
            vec![(
                BLACKOUT_HOST.to_owned(),
                list(&[once("midday", at(1, 11, 30), at(1, 13, 0))]),
            )]
        },
        &sources,
        at(0, 12, 0),
    );
    // The previous day's occurrence is served, so the next one is the day's own.
    let first = fixture.evaluate(at(0, 12, 0));
    let (intent, chain) = selected(&first, TASK_A, A);
    fixture.run(&intent, &chain);

    let due = fixture.evaluate(at(1, 12, 0));
    deferred(
        &due,
        TASK_A,
        A,
        SchedulingRuleHostCode::BlackoutActive,
        at(1, 13, 0),
    );

    let after = fixture.evaluate(at(1, 13, 0));
    let (intent, chain) = selected(&after, TASK_A, A);
    let admission = fixture.run(&intent, &chain);
    let day = i64::try_from(at(1, 0, 0) / DAY_MS).expect("a day number");
    assert_eq!(admission.activity.local_day, day);
    assert_eq!(admission.budget.task_daily_used, Some(1));
    assert!(
        dispatched(&fixture.evaluate(at(1, 13, 10)), TASK_A, A).is_none(),
        "the occurrence is served once"
    );
    fixture.close();
}

// ---- T15 (L726; SCHED §5.1 c): with the activity profile a bookkeeping day and no blackout, a
// short task is admitted at any time inside it, and its local day turns at the reset (04:00 UTC).
#[test]
fn sr4a2_t15_without_blackouts_a_bookkeeping_day_admits_at_any_time_and_turns_at_the_reset() {
    let sources = catalog(
        &[Task::always(SHORT, A, 100, SHORT_MS)],
        BOOKKEEPING_WINDOW,
        &[],
    );
    let mut fixture = Fixture::start(&[A], |_| Vec::new(), &sources, at(1, 1, 0));
    let previous_day = i64::try_from(at(0, 0, 0) / DAY_MS).expect("a day number");
    for (unix_ms, local_day) in [
        (at(1, 1, 0), previous_day),
        // A 1 min run ending exactly at the window's end (03:59) still fits.
        (at(1, 3, 58), previous_day),
        (at(1, 4, 0), previous_day + 1),
        (at(1, 12, 0), previous_day + 1),
        (at(1, 23, 59), previous_day + 1),
    ] {
        let evaluation = fixture.evaluate(unix_ms);
        assert!(!names_a_blackout(&evaluation), "{evaluation:#?}");
        let (intent, chain) = selected(&evaluation, SHORT, A);
        let admission = fixture.run(&intent, &chain);
        assert_eq!(admission.activity.local_day, local_day, "at {unix_ms}");
    }
    fixture.close();
}

// ---- SR-4a2 row: an instance's own entry defers only that instance; another instance runs.
#[test]
fn sr4a2_an_instance_entry_defers_only_its_instance() {
    let from = at(0, 12, 0);
    let until = at(0, 13, 0);
    let sources = catalog(
        &[
            Task::always(TASK_A, A, 100, SHORT_MS),
            Task::always(TASK_B, B, 100, SHORT_MS),
        ],
        DAY_WINDOW,
        &[],
    );
    let fixture = Fixture::start(
        &[A, B],
        |ids| {
            vec![(
                instance_key(&ids[A], INSTANCE_BLACKOUT),
                list(&[once("absence", from, until)]),
            )]
        },
        &sources,
        at(0, 12, 30),
    );
    let evaluation = fixture.evaluate(at(0, 12, 30));
    let reason = deferred(
        &evaluation,
        TASK_A,
        A,
        SchedulingRuleHostCode::BlackoutActive,
        until,
    );
    assert_eq!(value(&reason, "blackout_id"), &json!("absence"));
    assert_eq!(value(&reason, "blackout_source"), &json!("instance"));
    selected(&evaluation, TASK_B, B);
    let names_b = evaluation.decisions.iter().any(|decision| {
        decision.instance_id.as_deref() == Some(B)
            && decision
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(SchedulingRuleHostCode::BlackoutActive))
    });
    assert!(
        !names_b,
        "the other instance is not blacked out: {evaluation:#?}"
    );
    selected(&fixture.evaluate(until), TASK_A, A);
    fixture.close();
}

// ---- SR-4a2 row: a once entry starts at its from and expires at its until; ScheduleExplanation
// (`ac_schedule`) shows the blackout as the instance's reason and its end as the wake.
#[test]
fn sr4a2_a_once_entry_starts_and_expires() {
    let from = at(0, 14, 0);
    let until = at(0, 14, 30);
    let sources = catalog(&[Task::always(TASK_A, A, 100, SHORT_MS)], DAY_WINDOW, &[]);
    let fixture = Fixture::start(
        &[A],
        |_| {
            vec![(
                BLACKOUT_HOST.to_owned(),
                list(&[once("deploy", from, until)]),
            )]
        },
        &sources,
        at(0, 13, 0),
    );
    selected(&fixture.evaluate(at(0, 13, 0)), TASK_A, A);
    for inside in [from, until - 1] {
        let evaluation = fixture.evaluate(inside);
        let reason = deferred(
            &evaluation,
            TASK_A,
            A,
            SchedulingRuleHostCode::BlackoutActive,
            until,
        );
        assert_eq!(value(&reason, "blackout_id"), &json!("deploy"));
        assert_eq!(value(&reason, "blackout_source"), &json!("host"));
        assert_eq!(value(&reason, "next_eligible_unix_ms"), &json!(until));
    }
    let entry = fixture.explanation(A);
    assert_eq!(entry.next_wake_unix_ms, Some(until));
    assert_eq!(entry.not_running.task_id.as_deref(), Some(TASK_A));
    assert!(
        entry
            .not_running
            .why
            .is(SchedulingRuleHostCode::BlackoutActive),
        "{entry:#?}"
    );
    assert_eq!(
        value(&entry.not_running.why, "blackout_id"),
        &json!("deploy")
    );
    assert_eq!(
        value(&entry.not_running.why, "next_eligible_unix_ms"),
        &json!(until)
    );
    let expired = fixture.evaluate(until);
    selected(&expired, TASK_A, A);
    assert!(!names_a_blackout(&expired), "{expired:#?}");
    fixture.close();
}

// ---- SR-4a2 row (SCH-M D-6, Alice's ruling 50): on a bookkeeping day, a run whose expected
// duration would cross the reset waits for the next window while a short one runs up to it;
// admission refuses an intent that no longer fits; ScheduleExplanation shows the reason; with the
// instance's own switch off the long run is admitted.
#[test]
fn sr4a2_the_day_boundary_defers_a_long_run_and_admits_a_short_one() {
    const LONG_MS: u64 = HOUR_MS;
    let window_end = at(1, 3, 59);
    let next_window = at(1, 4, 0);
    let sources = catalog(
        &[
            Task::always(SHORT, A, 100, SHORT_MS),
            Task::always(LONG, A, 300, LONG_MS),
        ],
        BOOKKEEPING_WINDOW,
        &[],
    );
    let mut fixture = Fixture::start(
        &[A],
        |ids| {
            vec![(
                instance_key(&ids[A], DAY_BOUNDARY_QUIET),
                FactScalar::Boolean(true),
            )]
        },
        &sources,
        at(1, 1, 0),
    );

    let early = fixture.evaluate(at(1, 1, 0));
    let (intent, chain) = selected(&early, LONG, A);
    fixture.set_time(at(1, 3, 0));
    let refusal = refused(
        &mut fixture,
        &intent,
        &chain,
        SchedulingRuleHostCode::PolicyDayBoundaryApproaching,
        next_window,
    );
    assert_eq!(value(&refusal, "instance_id"), &json!(fixture.ids[A]));
    assert_eq!(value(&refusal, "window_end_unix_ms"), &json!(window_end));
    assert_eq!(value(&refusal, "required_ms"), &json!(LONG_MS));
    assert_eq!(
        value(&refusal, "next_eligible_unix_ms"),
        &json!(next_window)
    );

    let late = fixture.evaluate(at(1, 3, 0));
    let reason = deferred(
        &late,
        LONG,
        A,
        SchedulingRuleHostCode::PolicyDayBoundaryApproaching,
        next_window,
    );
    assert_eq!(value(&reason, "instance_id"), &json!(A));
    assert_eq!(value(&reason, "window_end_unix_ms"), &json!(window_end));
    assert_eq!(value(&reason, "required_ms"), &json!(LONG_MS));
    selected(&late, SHORT, A);
    let entry = fixture.explanation(A);
    assert_eq!(entry.not_running.task_id.as_deref(), Some(LONG));
    assert!(
        entry
            .not_running
            .why
            .is(SchedulingRuleHostCode::PolicyDayBoundaryApproaching),
        "{entry:#?}"
    );

    fixture.configure(
        instance_key(&fixture.ids[A], DAY_BOUNDARY_QUIET),
        json!(false),
    );
    let switched_off = fixture.evaluate(at(1, 3, 0) + 1);
    selected(&switched_off, LONG, A);
    assert!(!names_a_blackout(&switched_off), "{switched_off:#?}");
    fixture.close();
}

// ---- SR-4a2 row (`scheduling-blackout.md`, position 9: the cut gains the next blackout start
// minus the horizon and quiet_margin_ms): a random delay drawn shortly before the quiet point is
// cut to it, and the pair is dispatched there, still before the quiet.
#[test]
fn sr4a2_the_random_delay_never_passes_the_quiet_point() {
    let start = at(0, 14, 0);
    let end = at(0, 15, 0);
    let quiet_point = start - DEADLINE_MS - MARGIN_MS;
    let due_at = quiet_point - 30_000;
    let sources = catalog(&[Task::daily(TASK_A, A, 13 * 60 + 28)], DAY_WINDOW, &[]);
    let fixture = Fixture::start(
        &[A],
        |ids| {
            let own = |member: &str, value: FactScalar| (instance_key(&ids[A], member), value);
            vec![
                (
                    BLACKOUT_HOST.to_owned(),
                    list(&[once("quiet-window", start, end)]),
                ),
                own(RANDOM_DELAY_ENABLED, FactScalar::Boolean(true)),
                own(RANDOM_DELAY_MIN_MS, FactScalar::DurationMs(MINUTE_MS)),
                own(RANDOM_DELAY_MAX_MS, FactScalar::DurationMs(HOUR_MS)),
            ]
        },
        &sources,
        due_at,
    );
    // Every draw is at least a minute, so the 30 s left to the quiet point cut it.
    let drawn = fixture.evaluate(due_at);
    let waiting = deferred(
        &drawn,
        TASK_A,
        A,
        HostCode::DispatchRandomDelayWaiting,
        quiet_point,
    );
    assert_eq!(value(&waiting, "due_at_unix_ms"), &json!(due_at));
    assert_eq!(value(&waiting, "random_delay_ms"), &json!(30_000));
    assert_eq!(
        value(&waiting, "next_eligible_unix_ms"),
        &json!(quiet_point)
    );

    let planned = fixture.evaluate(quiet_point);
    selected(&planned, TASK_A, A);
    assert!(!names_a_blackout(&planned), "{planned:#?}");
    fixture.close();
}

// ---- SR-4a2 row: with no blackout entry in either list and no maintenance event, nothing of this
// slice changes an evaluation: the instants another configuration would black out or quiet run
// as before, no decision names a blackout, and a random delay is not cut.
#[test]
fn sr4a2_empty_lists_leave_the_evaluation_unchanged() {
    const DELAY_MS: u64 = 10 * MINUTE_MS;
    let sources = catalog(&[Task::always(TASK_A, A, 100, SHORT_MS)], DAY_WINDOW, &[]);
    let mut fixture = Fixture::start(&[A], |_| Vec::new(), &sources, at(0, 13, 0));
    for unix_ms in [
        at(0, 13, 0),
        at(0, 14, 0) - DEADLINE_MS - MARGIN_MS + 1,
        at(0, 14, 0),
        at(0, 14, 30),
    ] {
        let evaluation = fixture.evaluate(unix_ms);
        assert!(!names_a_blackout(&evaluation), "{evaluation:#?}");
        let (intent, chain) = selected(&evaluation, TASK_A, A);
        fixture.run(&intent, &chain);
    }
    fixture.close();

    // A fixed delay of 10 min is drawn whole: nothing cuts it before the window's end.
    let sources = catalog(&[Task::daily(TASK_A, A, 13 * 60 + 28)], DAY_WINDOW, &[]);
    let due_at = at(0, 13, 28) + 30_000;
    let fixture = Fixture::start(
        &[A],
        |ids| {
            let own = |member: &str, value: FactScalar| (instance_key(&ids[A], member), value);
            vec![
                own(RANDOM_DELAY_ENABLED, FactScalar::Boolean(true)),
                own(RANDOM_DELAY_MIN_MS, FactScalar::DurationMs(DELAY_MS)),
                own(RANDOM_DELAY_MAX_MS, FactScalar::DurationMs(DELAY_MS)),
            ]
        },
        &sources,
        due_at,
    );
    let drawn = fixture.evaluate(due_at);
    let waiting = deferred(
        &drawn,
        TASK_A,
        A,
        HostCode::DispatchRandomDelayWaiting,
        due_at + DELAY_MS,
    );
    assert_eq!(value(&waiting, "random_delay_ms"), &json!(DELAY_MS));
    assert!(!names_a_blackout(&drawn), "{drawn:#?}");
    selected(&fixture.evaluate(due_at + DELAY_MS), TASK_A, A);
    fixture.close();
}
