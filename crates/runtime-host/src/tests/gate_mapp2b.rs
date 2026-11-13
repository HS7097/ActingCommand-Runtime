// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-2b (register §1.345; `contracts/device-group.md`, "The switch before a
//! run"): the switch's callers, the run's clock that leaves the switch out, the member's lease, a
//! failed switch and the member's switch cooldown, through the host.
//!
//! Three fixture instances A, B and D are one device group of several (`device.one`) and C is
//! alone on its own device; each has its own application and the neutral startup package, whose
//! one step restarts the application and whose home is the fake device's first frame. Every run
//! here runs that package as its task too, so a run's own work is one application restart. The
//! load refuses every group of several (actingd's MAPP-1 gate), so the host is handed the member
//! → device key map and the group's name as the assembly builds them. A manual Runtime clock
//! moves only by the kernel's waits and the test's own steps; nothing waits on the wall clock.
//! Every expected value is the contract's or the coordinator's ruling; every comparison is within
//! one host. Neutral names only.

use super::instance_workers::phases;
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, ConfigParameter, ConfigParameterSource, EventAction,
    FactScalar, RuntimeConfigManifest, RuntimeFactRecord, RuntimeFactScope, RuntimeLifecyclePhase,
};
use actingcommand_policy::PolicyEvaluation;
use actingcommand_runtime_config::scheduling::DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS;
use actingcommand_runtime_config::{DefaultsFile, KeyRange, Settable, find_row};
use actingcommand_scheduler::{ClaimKind, DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS};
use serde_json::{Value, json};

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
/// The group's third member.
const D: &str = "fixture-instance-d";
const TASK_A: &str = "fixture.member-a";
const TASK_B: &str = "fixture.member-b";
const TASK_C: &str = "fixture.lone-c";
const GROUP: &str = "device.one";
const SEED: u64 = 29;
const APP_A: &str = "neutral.member.a";
const APP_B: &str = "neutral.member.b";
const APP_C: &str = "neutral.member.c";
const APP_D: &str = "neutral.member.d";
/// The fake device's first pixel before any input: the startup package's home.
const HOME: [u8; 3] = [255, 0, 0];
/// The startup package's `package_id`, whose samples a run of it as a task adds.
const PACKAGE_ID: &str = "neutral.test.startup";
/// The response deadline of every run (`D_run`).
const RUN_DEADLINE_MS: u64 = 600_000;
/// The response deadline of the startup package (`D_s`) in most tests.
const STARTUP_DEADLINE_MS: u64 = 60_000;
/// The fixture scheduler's heartbeat reserve (`R`, `config`'s
/// `maximum_client_heartbeat_interval_ms`).
const RESERVE_MS: u64 = 20;
/// Every task's runtime budget (`loop_budget.max_runtime_ms`).
const MAX_RUNTIME_MS: u64 = 300_000;
/// The longest response deadline, which with the reserve bounds every lease the scheduler grants.
const LONGEST_DEADLINE_MS: u64 = 1_800_000;
/// `scheduling.device_group.switch_failure_cooldown_ms`'s default.
const DEFAULT_COOLDOWN_MS: u64 = 300_000;
/// The wait a queued claim asks: the longest the fixture's scheduler takes.
const QUEUE_TIMEOUT_MS: u64 = DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS;
/// The connection of every policy run's key (the host's `POLICY_CONNECTION_VALUE`).
const POLICY_CONNECTION: u64 = u64::MAX;
const TASK_PAGE: &str = "task.page";
const MINUTE_MS: u64 = 60_000;

// ---- The fixture.

/// One task of `instance` with `priority`, due at every evaluation (a local interval clock of
/// 1 ms), from the fixture catalog's generic task, with the runtime budget `MAX_RUNTIME_MS`.
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
        "max_runtime_ms": MAX_RUNTIME_MS
    });
    spec
}

/// The catalog: TASK_A on A (priority 200) and TASK_B on B (300), so B's task wins the device,
/// and TASK_C on C, alone on its device.
fn sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = json!([
        member_task(&base, TASK_A, A, 200),
        member_task(&base, TASK_B, B, 300),
        member_task(&base, TASK_C, C, 100),
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

/// The neutral startup package (an application entry): from any screen its one step restarts
/// the instance's application, to `home`, which its recognition pack knows by pixel 0.
fn startup_package(root: &TempDir) -> PathBuf {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": PACKAGE_ID,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": space.clone(),
        "entry_task_id": "task",
        "capture_interval_ms": 10,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 2
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space.clone(),
        "entry_page": "any",
        "target_page": "home",
        "operations": [
            {"id": "start", "from": "any", "to": "home", "application": {"action": "restart"}}
        ]
    });
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [{"type": "color", "id": "page/home",
                     "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME}]
    });
    let pages = json!({"schema_version": "0.3", "pages": [
        {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []}
    ]});
    let files: [(&str, Value); 6] = [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
        (
            "resources/navigation/neutral.test.navigation.json",
            json!({"navigation": []}),
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, value) in files {
        zip.start_file(path, options).expect("zip entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("zip content");
    }
    let path = root.path().join("startup.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner())
        .expect("write the startup package");
    path
}

/// The configuration in effect with `scheduling.device_group.switch_failure_cooldown_ms` set to
/// `cooldown_ms` by the file.
fn cooldown_manifest(cooldown_ms: u64) -> RuntimeConfigManifest {
    let mut parameters = vec![ConfigParameter {
        key: DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS.to_owned(),
        value: FactScalar::DurationMs(cooldown_ms),
        source: ConfigParameterSource::Explicit,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// What a host of the fixture is started with, kept so a restart starts the same one.
struct Setup {
    startup_deadline_ms: u64,
    cooldown_ms: Option<u64>,
}

/// Everything a host of the fixture is started from, kept so a restart starts the same one.
struct Parts {
    clock: Arc<ManualRuntimeClock>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    package: PathBuf,
    ids: BTreeMap<&'static str, InstanceId>,
    states: BTreeMap<&'static str, Arc<FakeState>>,
    setup: Setup,
    root: TempDir,
}

impl Parts {
    /// A host whose instances A, B and D are the device group `GROUP` and C is alone, each with
    /// its own application and the startup package, on the manual Runtime clock, the catalog of
    /// [`sources`] active.
    fn start_host(&self) -> RuntimeHost {
        let ids = &self.ids;
        let device = ids[A].min(ids[B]).min(ids[D]);
        let entries = [A, B, C, D]
            .into_iter()
            .map(|alias| {
                (
                    alias.to_owned(),
                    ids[alias],
                    Arc::clone(&self.states[alias]),
                )
            })
            .collect::<Vec<_>>();
        let startup = ContainedTaskRequest::new(self.package.to_string_lossy().into_owned())
            .and_then(|request| request.with_response_deadline_ms(self.setup.startup_deadline_ms))
            .expect("startup package request");
        let mut host_config = config(&self.root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                self.facts.clone(),
                self.resources.clone(),
            ))
            .with_runtime_clock(self.clock.clone())
            .with_startup_packages(
                [A, B, C, D]
                    .into_iter()
                    .map(|alias| (alias.to_owned(), startup.clone()))
                    .collect(),
            )
            .with_device_keys(BTreeMap::from([
                (ids[A], device),
                (ids[B], device),
                (ids[D], device),
            ]))
            .with_device_group_names(BTreeMap::from([
                (A.to_owned(), GROUP.to_owned()),
                (B.to_owned(), GROUP.to_owned()),
                (D.to_owned(), GROUP.to_owned()),
            ]));
        if let Some(cooldown_ms) = self.setup.cooldown_ms {
            host_config = host_config.with_config_manifest(cooldown_manifest(cooldown_ms));
        }
        let host = RuntimeHost::start(host_config, Arc::new(FakeProvider::from_entries(entries)))
            .expect("start the device group host");
        host.activate_policy_catalog(&sources())
            .expect("activate the catalog");
        host
    }
}

/// A host of the fixture ([`Parts::start_host`]) at the fixture's noon, and its parts.
struct Bench {
    host: RuntimeHost,
    parts: Parts,
}

impl Bench {
    fn start(setup: Setup) -> Self {
        let root = TempDir::new().expect("tempdir");
        let package = startup_package(&root);
        let members = [(A, APP_A), (B, APP_B), (C, APP_C), (D, APP_D)];
        let ids = members
            .iter()
            .map(|(alias, _)| (*alias, instance_id()))
            .collect::<BTreeMap<_, _>>();
        let states = members
            .iter()
            .map(|(alias, application)| {
                let state = Arc::new(FakeState::default());
                state.observed_open.store(true, Ordering::Release);
                state.physical_task_geometry.store(true, Ordering::Release);
                *state.application_id.lock().expect("fake application id") =
                    Some((*application).to_owned());
                (*alias, state)
            })
            .collect::<BTreeMap<_, _>>();
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
        facts.instances.push(InstanceSnapshot {
            instance_id: C.to_owned(),
            server_id: "fixture-server-c".to_owned(),
            game_id: "fixture-game-a".to_owned(),
            host_id: "fixture-host-c".to_owned(),
            available: true,
            capability_operation_ids: vec!["operation.observe".to_owned()],
            preferred_task_ids: Vec::new(),
        });
        // The policy's configured identities are the registered aliases, all four
        // (`validate_policy_input_authority`); D takes A's static identity and is not available
        // to the policy, which has no task of its.
        let mut identity = facts.instances[0].clone();
        identity.instance_id = D.to_owned();
        identity.available = false;
        facts.instances.push(identity);
        let mut resources = pending_policy_resources();
        let mut host_c = resources.hosts[0].clone();
        host_c.host_id = "fixture-host-c".to_owned();
        resources.hosts.push(host_c);
        let parts = Parts {
            clock: Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)),
            facts,
            resources,
            package,
            ids,
            states,
            setup,
            root,
        };
        Self {
            host: parts.start_host(),
            parts,
        }
    }

    /// The fixture with the startup package's response deadline `STARTUP_DEADLINE_MS` and the
    /// cooldown key at its default.
    fn standard() -> Self {
        Self::start(Setup {
            startup_deadline_ms: STARTUP_DEADLINE_MS,
            cooldown_ms: None,
        })
    }

    /// The same host closed and started again on the same root, devices and clock.
    fn restart(self) -> Self {
        let Self { host, parts } = self;
        host.close().expect("close the device group host");
        Self {
            host: parts.start_host(),
            parts,
        }
    }

    /// The fixture's manual Runtime clock.
    fn clock(&self) -> &ManualRuntimeClock {
        &self.parts.clock
    }

    /// One evaluation at `unix_ms`, the capacity sampled first.
    fn evaluate(&self, unix_ms: u64) -> PolicyEvaluation {
        self.clock().set_unix_ms(unix_ms);
        self.sample_capacity();
        self.host
            .evaluate_policy_cycle_with_test_inputs(
                &self.parts.facts,
                &self.parts.resources,
                EvaluationTime {
                    unix_ms,
                    monotonic_ms: unix_ms,
                },
                SEED,
                PolicyTrigger::Recovery,
            )
            .unwrap_or_else(|error| panic!("evaluate at {unix_ms}: {error}"))
            .evaluation
            .expect("an evaluated cycle")
    }

    fn sample_capacity(&self) {
        let sample = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample().expect("capacity sample");
    }

    /// The run request of every run: the startup package with the response deadline `deadline_ms`.
    fn request(&self, deadline_ms: u64) -> ContainedTaskRequest {
        ContainedTaskRequest::new(self.parts.package.to_string_lossy().into_owned())
            .and_then(|request| request.with_response_deadline_ms(deadline_ms))
            .expect("the run's request")
    }

    /// The admission of `task_id`'s intent in `evaluation` with a run of `deadline_ms`.
    fn admit(
        &self,
        evaluation: &PolicyEvaluation,
        task_id: &str,
        deadline_ms: u64,
    ) -> RuntimeHostResult<PolicyDispatchAdmission> {
        let (intent, chain) = intent_of(evaluation, task_id);
        record_policy_approval(&self.host, &intent);
        self.sample_capacity();
        self.host.admit_scheduled_policy_dispatch(
            &intent,
            &chain,
            &policy_context(&self.host, &intent),
            &self.request(deadline_ms),
        )
    }

    /// `task_id`'s dispatch in `evaluation`, granted with a run of `RUN_DEADLINE_MS`.
    fn grant(&self, evaluation: &PolicyEvaluation, task_id: &str) -> Box<PolicyRunContext> {
        match self
            .admit(evaluation, task_id, RUN_DEADLINE_MS)
            .unwrap_or_else(|error| panic!("admit {task_id}: {error}"))
        {
            PolicyDispatchAdmission::Granted { context } => context,
            other => panic!("{task_id}'s dispatch is granted: {other:?}"),
        }
    }

    /// The granted run of `context`, on the policy path.
    fn run(&self, context: &PolicyRunContext) -> RuntimeHostResult<RuntimeReceipt> {
        self.host
            .run_scheduled_contained_task(context, &self.request(RUN_DEADLINE_MS))
    }

    /// The run of `context` completed and settled: its own time, as its execution record has it.
    fn run_and_settle(&self, context: &PolicyRunContext) -> u64 {
        let receipt = self
            .run(context)
            .unwrap_or_else(|error| panic!("the run succeeds: {error:?}"));
        let (execution, _) = self
            .host
            .complete_scheduled_policy_run(context, &receipt)
            .expect("the run settles");
        match execution.outcome {
            PolicyExecutionOutcome::Succeeded { runtime_ms } => runtime_ms,
            other => panic!("a success on its own time: {other:?}"),
        }
    }

    fn id(&self, alias: &str) -> InstanceId {
        self.parts.ids[alias]
    }

    fn state(&self, alias: &str) -> &FakeState {
        &self.parts.states[alias]
    }

    /// Every application command that reached `alias`'s device.
    fn actions(&self, alias: &str) -> Vec<ApplicationLifecycleAction> {
        self.state(alias)
            .application_actions
            .lock()
            .expect("fake application actions")
            .clone()
    }

    /// `alias`'s foreground answers `front` from now on (`None`: no resumed activity).
    fn front(&self, alias: &str, front: Option<&'static str>) {
        *self
            .state(alias)
            .foreground_script
            .lock()
            .expect("foreground script") =
            Some(Arc::new(move |_: usize, _: usize| front.map(str::to_owned)));
    }

    /// `alias`'s foreground answers `before` until its own application took a command, then its
    /// own application.
    fn front_until_started(&self, alias: &str, before: &'static str, own: &'static str) {
        *self
            .state(alias)
            .foreground_script
            .lock()
            .expect("foreground script") = Some(Arc::new(move |_: usize, commands: usize| {
            Some(if commands == 0 { before } else { own }.to_owned())
        }));
    }

    /// `alias`'s program fact `key` recorded with `value` now.
    fn record_fact(&self, alias: &str, key: &str, value: &str) {
        self.host
            .record_runtime_fact_for_test(RuntimeFactRecord {
                scope: RuntimeFactScope::Instance {
                    instance_id: self.id(alias),
                },
                key: key.to_owned(),
                value: ContractFactValue::String(value.to_owned()),
                observed_at_unix_ms: POLICY_NOW_UNIX_MS,
                source: OriginModule::Runtime,
                ttl_ms: None,
            })
            .expect("record the fact");
    }

    /// `alias`'s program fact `key`, when it holds one.
    fn fact(&self, alias: &str, key: &str) -> Option<RuntimeFactRecord> {
        self.host
            .runtime_fact_for_test(
                &RuntimeFactScope::Instance {
                    instance_id: self.id(alias),
                },
                key,
            )
            .expect("read the fact")
    }

    /// The package's samples on `alias`, newest first.
    fn samples(&self, alias: &str) -> Vec<u64> {
        self.host
            .run_durations_for_test(alias)
            .expect("the run-duration record")
            .samples(PACKAGE_ID)
            .to_vec()
    }

    /// Every record after `after`, in order.
    fn events_after(&self, after: u64) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events")
            .into_iter()
            .filter(|event| event.sequence() > after)
            .collect()
    }

    /// The ledger's last sequence.
    fn head(&self) -> u64 {
        self.events_after(0)
            .last()
            .map_or(0, PersistedEvent::sequence)
    }

    /// The records of the run of `context`, in order.
    fn run_records(&self, context: &PolicyRunContext) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery {
                run_id: Some(context.run_id()),
                ..EventQuery::default()
            })
            .expect("the run's records")
    }

    /// A request on `connection` processed as a client's.
    fn send(&self, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        self.host
            .process_request_for_test(
                &runtime_request(&ids, operation),
                ConnectionId::new(connection).expect("connection"),
            )
            .expect("the request's receipt")
    }

    /// The time `token` has left as the device's key on the policy connection.
    fn remaining(&self, token: &LeaseToken) -> Option<u64> {
        self.host
            .key_remaining_ms_for_test(token, policy_connection())
            .expect("the key")
    }

    fn close(self) {
        self.host.close().expect("close the device group host");
    }
}

fn policy_connection() -> ConnectionId {
    ConnectionId::new(POLICY_CONNECTION).expect("the policy connection")
}

/// The intent of `task_id` in `evaluation` and its reason chain.
fn intent_of(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> (DispatchIntent, DecisionReasonChain) {
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

/// Whether `evaluation` dispatches `task_id`.
fn dispatches(evaluation: &PolicyEvaluation, task_id: &str) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == task_id)
}

/// The reasons of `task_id`'s decision on `alias` in `evaluation`, and its wake.
fn decision_of<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    alias: &str,
) -> (&'a [actingcommand_policy::DecisionReason], Option<u64>) {
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {alias}: {evaluation:#?}"));
    (&decision.reasons, decision.next_wake_unix_ms)
}

/// Whether `reasons` hold the member's switch cooldown: `dispatch_instance_cooldown` with
/// `failure_code` `application_switch_failed`.
fn switch_cooldown_reason(
    reasons: &[actingcommand_policy::DecisionReason],
) -> Option<&actingcommand_contract::outcome::Outcome> {
    reasons
        .iter()
        .map(|reason| &reason.outcome)
        .find(|outcome| {
            outcome.code().as_str() == HostCode::DispatchInstanceCooldown.as_str()
                && outcome.value("failure_code")
                    == Some(&json!(HostCode::ApplicationSwitchFailed.as_str()))
        })
}

/// `error` is `application_switch_failed` before `alias`'s run at `step`, its `failure_code` the
/// code of its cause, which is `failure_code` when given.
fn assert_switch_failed(
    error: &RuntimeHostError,
    alias: &str,
    step: &str,
    failure_code: Option<&str>,
) {
    assert_eq!(
        error.code(),
        HostCode::ApplicationSwitchFailed.as_str(),
        "{error:?}"
    );
    let outcome = error.outcome();
    assert_eq!(outcome.value("alias"), Some(&json!(alias)), "{outcome:?}");
    assert_eq!(
        outcome.value("application_switch_step"),
        Some(&json!(step)),
        "{outcome:?}"
    );
    let cause = outcome
        .codes()
        .get(1)
        .map(|code| code.as_str().to_owned())
        .expect("the step's failure is the cause");
    assert_eq!(
        outcome.value("failure_code"),
        Some(&json!(cause)),
        "{outcome:?}"
    );
    if let Some(failure_code) = failure_code {
        assert_eq!(cause, failure_code, "{outcome:?}");
    }
}

/// The application commands the ledger recorded as completed, in order: the instance each was
/// recorded under and its action.
fn application_steps(events: &[PersistedEvent]) -> Vec<(InstanceId, EventAction)> {
    events
        .iter()
        .filter(|event| event.event_type() == EventType::ApplicationCompleted)
        .map(|event| {
            (
                *event
                    .links()
                    .instance_id()
                    .expect("an application record names its instance"),
                event.payload().action(),
            )
        })
        .collect()
}

/// How many applications are alive on the device after each of `steps`, from `alive` before
/// them: a stop ends its instance's application, a launch or a restart starts it.
fn alive_after_each(steps: &[(InstanceId, EventAction)], alive: &[InstanceId]) -> Vec<usize> {
    let mut alive = alive.iter().copied().collect::<BTreeSet<_>>();
    steps
        .iter()
        .map(|(instance, action)| {
            if *action == ApplicationLifecycleAction::Stop.event_action() {
                alive.remove(instance);
            } else {
                alive.insert(*instance);
            }
            alive.len()
        })
        .collect()
}

/// Whether any recovery ladder started among `events`.
fn ladder_started(events: &[PersistedEvent]) -> bool {
    phases(events)
        .iter()
        .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. }))
}

/// The failed execution record of the dispatch `decision_id`: its failure's code.
fn failed_settlement(events: &[PersistedEvent], decision_id: &str) -> Option<String> {
    events.iter().find_map(|event| match event.payload() {
        EventPayload::Policy(PolicyPayload::ExecutionRecorded(payload))
            if payload.decision_id() == decision_id =>
        {
            match payload.policy_result() {
                PolicyExecutionOutcome::Failed { failure } => {
                    Some(failure.outcome.code().as_str().to_owned())
                }
                _ => None,
            }
        }
        _ => None,
    })
}

// ---- The tests.

/// The register's MAPP-2's tests, "a switch kills the first application, starts the second and
/// reaches its home before the task, under one causation id, the key never granted to another
/// claim in between", with the work order's run of B with A in front: the device sees A's stop,
/// then B's startup restart, then B's task's own restart; one application is alive after each
/// step; A's `task.page` and `application.foreground` are gone; A's stop records are A's, B's
/// startup run and B's run are B's, the stops and the startup run share one causation id and the
/// startup run's scheduling record is under B's run's request; A's queued claim is never granted
/// or handed the key before B's run ends.
#[test]
fn mapp2b_a_run_of_b_with_a_in_front_stops_a_starts_b_and_then_runs_b() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    for alias in [A, B] {
        bench.record_fact(alias, TASK_PAGE, "neutral/home");
        bench.record_fact(alias, APPLICATION_FOREGROUND_FACT_KEY, APP_A);
    }
    bench.front_until_started(B, APP_A, APP_B);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let queued = bench.send(
        RuntimeOperation::queue_lease(
            A,
            ids.mint_holder_id().expect("holder"),
            LeaseQueuePolicy::new(LeasePriority::Normal, QUEUE_TIMEOUT_MS).expect("queue policy"),
        ),
        72,
    );
    let Some(RuntimeResult::LeaseQueued { status }) = queued.result() else {
        panic!("A's claim waits queued: {queued:?}");
    };
    let queued_request = status.request_id();
    let mark = bench.head();

    bench.run_and_settle(&context);

    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(
        bench.actions(B),
        [
            ApplicationLifecycleAction::Restart,
            ApplicationLifecycleAction::Restart
        ],
        "B's startup restart, then its task's own"
    );
    assert!(bench.actions(D).is_empty(), "D was not in front");
    assert!(bench.actions(C).is_empty(), "C is not in the group");
    let events = bench.events_after(mark);
    let steps = application_steps(&events);
    assert_eq!(
        steps,
        vec![
            (bench.id(A), ApplicationLifecycleAction::Stop.event_action()),
            (
                bench.id(B),
                ApplicationLifecycleAction::Restart.event_action()
            ),
            (
                bench.id(B),
                ApplicationLifecycleAction::Restart.event_action()
            ),
        ]
    );
    assert!(
        alive_after_each(&steps, &[bench.id(A)])
            .into_iter()
            .all(|alive| alive <= 1),
        "one application alive after each step: {steps:?}"
    );
    assert!(bench.fact(A, TASK_PAGE).is_none(), "A's page is forgotten");
    assert!(bench.fact(A, APPLICATION_FOREGROUND_FACT_KEY).is_none());

    let causation = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::CommandReceived
                && event.links().instance_id() == Some(&bench.id(A))
        })
        .and_then(|event| event.links().causation_id().copied())
        .expect("A's stop and its causation");
    let run = bench.run_records(&context);
    let run_requested = run
        .iter()
        .find(|event| event.event_type() == EventType::TaskRequested)
        .expect("B's run is requested");
    assert_eq!(run_requested.links().instance_id(), Some(&bench.id(B)));
    let (scheduled, _) = phases(&events)
        .into_iter()
        .find(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. }))
        .expect("B's startup package is scheduled");
    assert_eq!(scheduled.links().causation_id(), Some(&causation));
    assert_eq!(scheduled.links().instance_id(), Some(&bench.id(B)));
    assert_eq!(
        scheduled.links().request_id(),
        run_requested.links().request_id(),
        "under B's run's request"
    );
    let startup_requested = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::TaskRequested
                && event.links().causation_id() == Some(&causation)
        })
        .expect("B's startup run");
    assert_eq!(startup_requested.links().instance_id(), Some(&bench.id(B)));
    let startup_ended = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::TaskCompleted
                && event.links().causation_id() == Some(&causation)
        })
        .expect("B's startup run reaches its home");
    assert!(
        startup_ended.sequence() < run_requested.sequence(),
        "B's home before its task"
    );
    let run_ended = run
        .iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .expect("B's run completes");
    assert!(
        !events.iter().any(|event| {
            event.sequence() < run_ended.sequence()
                && event.links().request_id() == Some(&queued_request)
                && matches!(
                    event.event_type(),
                    EventType::LeaseGranted | EventType::LeaseTransferred
                )
        }),
        "A's queued claim never gets the key before B's run ends"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's "the front unknown: every other member is stopped first. The front already
/// B: no stop, no startup run": with no resumed activity reported, A and D are both stopped
/// before B's startup run and B's task; with B's own package in front the run only runs (its
/// task's own restart), the switch is reached once and schedules no startup package.
#[test]
fn mapp2b_the_front_unknown_stops_every_other_member_and_b_in_front_switches_nothing() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    bench.front(B, None);
    let mark = bench.head();
    bench.run_and_settle(&context);
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(bench.actions(D), [ApplicationLifecycleAction::Stop]);
    assert_eq!(
        bench.actions(B),
        [
            ApplicationLifecycleAction::Restart,
            ApplicationLifecycleAction::Restart
        ]
    );
    assert!(bench.actions(C).is_empty(), "C is not in the group");
    let steps = application_steps(&bench.events_after(mark));
    assert!(
        alive_after_each(&steps, &[bench.id(A), bench.id(D)])
            .into_iter()
            .skip(1)
            .all(|alive| alive <= 1),
        "one application alive once the stops began: {steps:?}"
    );
    bench.close();

    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let mark = bench.head();
    bench.run_and_settle(&context);
    assert!(bench.actions(A).is_empty(), "no stop");
    assert!(bench.actions(D).is_empty(), "no stop");
    assert_eq!(
        bench.actions(B),
        [ApplicationLifecycleAction::Restart],
        "only the task's own restart: no startup run"
    );
    assert_eq!(bench.host.switch_entries_for_test().expect("count"), 1);
    assert!(
        !phases(&bench.events_after(mark))
            .iter()
            .any(|(_, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::StartupPackageScheduled { .. }
            )),
        "no startup package is scheduled"
    );
    bench.close();
}

/// The work order's clock: a switch that takes 400 s (the front read moves the Runtime clock on,
/// as a slow device would) before a short task leaves the run's own time unchanged. The run is
/// settled as a success on its own time, not rewritten as over its 300 s budget; its run-duration
/// sample and its `last_duration_ms` are that time; the wall time of the dispatch exceeds the
/// budget and holds the switch and the run.
#[test]
fn mapp2b_a_switched_runs_sample_last_duration_and_budget_verdict_are_its_own_time() {
    const SWITCH_MS: u64 = 400_000;
    let bench = Bench::start(Setup {
        startup_deadline_ms: 600_000,
        cooldown_ms: None,
    });
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let clock = Arc::clone(&bench.parts.clock);
    let read = Arc::new(AtomicBool::new(false));
    *bench
        .state(B)
        .foreground_script
        .lock()
        .expect("foreground script") = Some(Arc::new(move |_: usize, commands: usize| {
        if !read.swap(true, Ordering::AcqRel) {
            clock.advance(SWITCH_MS);
        }
        Some(if commands == 0 { APP_A } else { APP_B }.to_owned())
    }));
    let started = bench.clock().monotonic_ms();

    let runtime_ms = bench.run_and_settle(&context);

    let ended = bench.clock().monotonic_ms();
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert!(
        runtime_ms < MAX_RUNTIME_MS,
        "the run's own time is within its budget: {runtime_ms}"
    );
    assert!(
        ended - started >= SWITCH_MS + runtime_ms,
        "the dispatch's wall time holds the switch and the run: {} < {SWITCH_MS} + {runtime_ms}",
        ended - started
    );
    assert!(
        ended - started > MAX_RUNTIME_MS,
        "the wall time is over the budget"
    );
    assert_eq!(
        bench.samples(B),
        vec![runtime_ms],
        "the sample is the run's own time"
    );
    let last = bench
        .fact(B, &format!("task.{TASK_B}.last_duration_ms"))
        .expect("the settlement's last duration");
    assert_eq!(
        last.value,
        ContractFactValue::Integer(i64::try_from(runtime_ms).expect("a duration"))
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's lease: a dispatch of B, which may switch, is granted `D_s + D_run + R`, and a
/// dispatch of C, alone on its device, `D_run + R` as before. The borrowed-key startup run's
/// time check passes on B's key with the run's whole response deadline still to come, and fails
/// on a key granted without the startup run's share (the slice's branch off at the admission);
/// the run on that key fails its switch at `startup_package` before any stop. A member whose
/// `D_s + D_run` exceeds the longest response deadline, so whose lease the scheduler could not
/// grant, is refused before any lease and any device step, `contained_task_deadline_unavailable`,
/// on its dispatch and on its direct run, the same way each time; C's dispatch with that run is
/// granted its lease as before.
#[test]
fn mapp2b_a_member_dispatch_is_granted_the_startup_and_the_run_and_one_that_cannot_fit_is_refused()
{
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let b = bench.grant(&evaluation, TASK_B);
    let c = bench.grant(&evaluation, TASK_C);
    assert_eq!(
        bench.remaining(b.lease_token()),
        Some(STARTUP_DEADLINE_MS + RUN_DEADLINE_MS + RESERVE_MS)
    );
    assert_eq!(
        bench
            .host
            .key_remaining_ms_for_test(c.lease_token(), policy_connection())
            .expect("C's key"),
        Some(RUN_DEADLINE_MS + RESERVE_MS),
        "a lone instance's lease as before"
    );
    bench
        .host
        .run_borrowed_startup_for_test(
            b.request(),
            b.lease_token(),
            policy_connection(),
            RUN_DEADLINE_MS,
        )
        .expect("no Runtime failure")
        .expect("the key covers the startup run and the run's response deadline");
    bench.close();

    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    bench.host.switch_off_for_test(true).expect("the slice off");
    let b = bench.grant(&evaluation, TASK_B);
    bench.host.switch_off_for_test(false).expect("the slice on");
    assert_eq!(
        bench.remaining(b.lease_token()),
        Some(RUN_DEADLINE_MS + RESERVE_MS)
    );
    let refused = bench
        .host
        .run_borrowed_startup_for_test(
            b.request(),
            b.lease_token(),
            policy_connection(),
            RUN_DEADLINE_MS,
        )
        .expect("no Runtime failure")
        .expect_err("a key without the startup run's share");
    assert_eq!(
        refused.code(),
        HostCode::ContainedTaskDeadlineUnavailable.as_str()
    );
    bench.front(B, Some(APP_A));
    let failed = bench.run(&b).expect_err("the switch fails");
    assert_switch_failed(
        &failed,
        B,
        "startup_package",
        Some(HostCode::ContainedTaskDeadlineUnavailable.as_str()),
    );
    assert!(bench.actions(A).is_empty(), "refused before any stop");
    assert!(bench.actions(B).is_empty());
    bench.close();

    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let long_ms = LONGEST_DEADLINE_MS;
    let refused = bench
        .admit(&evaluation, TASK_B, long_ms)
        .expect_err("B's startup run and run cannot both fit");
    assert!(!refused.is_fatal(), "{refused:?}");
    assert_eq!(
        refused.code(),
        HostCode::ContainedTaskDeadlineUnavailable.as_str()
    );
    assert_eq!(
        bench.host.instance_claims_for_test(B).expect("claims"),
        (None, Vec::new()),
        "no lease"
    );
    let holder = IdentifierIssuer::new().expect("identifier issuer");
    for connection in [91, 92] {
        let direct = bench.send(
            RuntimeOperation::run_contained_task(
                B,
                holder.mint_holder_id().expect("holder"),
                bench.request(long_ms),
            ),
            connection,
        );
        assert_eq!(direct.state(), RuntimeReceiptState::Denied, "{direct:?}");
        assert_eq!(
            direct.outcome().code().as_str(),
            HostCode::ContainedTaskDeadlineUnavailable.as_str(),
            "{direct:?}"
        );
    }
    for alias in [A, B, D] {
        assert!(bench.actions(alias).is_empty(), "no device step: {alias}");
    }
    assert_eq!(bench.host.switch_entries_for_test().expect("count"), 0);
    let c = match bench
        .admit(&evaluation, TASK_C, long_ms)
        .expect("C's dispatch is admitted")
    {
        PolicyDispatchAdmission::Granted { context } => context,
        other => panic!("C's dispatch is granted: {other:?}"),
    };
    assert_eq!(
        bench
            .host
            .key_remaining_ms_for_test(c.lease_token(), policy_connection())
            .expect("C's key"),
        Some(long_ms + RESERVE_MS)
    );
    bench.close();
}

/// The work order's failure at each of the three steps, and the register's "a failed switch fails
/// that task only": the run ends `application_switch_failed` with that step and its cause; it is
/// settled failed with that code through the failed settlement; B's key is released and no claim
/// of B is left, no recovery ladder starts, and B's switch cooldown runs. `foreground_read`: the
/// read's ADB failure, nothing stopped. `stop`: A's stop refused at its device, A's page kept, no
/// startup run. `startup_package`: A stopped, B's startup run reaching no home, B's run never
/// requested.
#[test]
fn mapp2b_a_failure_at_each_step_settles_the_run_failed_releases_the_key_and_starts_no_ladder() {
    let cases: [(&str, Option<&str>); 3] = [
        (
            "foreground_read",
            Some(HostCode::ApplicationForegroundUnknown.as_str()),
        ),
        (
            "stop",
            Some(SharedCode::ApplicationBackendOperationFailed.as_str()),
        ),
        ("startup_package", None),
    ];
    for (step, failure_code) in cases {
        let bench = Bench::standard();
        let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
        let context = bench.grant(&evaluation, TASK_B);
        bench.record_fact(A, TASK_PAGE, "neutral/home");
        match step {
            "foreground_read" => {
                *bench
                    .state(B)
                    .foreground_error_once
                    .lock()
                    .expect("one-shot foreground error") =
                    Some(DeviceError::transient("fixture foreground query failed"));
            }
            "stop" => {
                bench.front(B, Some(APP_A));
                *bench
                    .state(A)
                    .application_error
                    .lock()
                    .expect("application error") =
                    Some(DeviceError::transient("fixture stop refused"));
            }
            _ => {
                bench.front(B, Some(APP_A));
                bench
                    .state(B)
                    .unknown_capture
                    .store(true, Ordering::Release);
            }
        }
        let mark = bench.head();

        let failed = bench.run(&context).expect_err("the switch fails");

        assert_switch_failed(&failed, B, step, failure_code);
        let events = bench.events_after(mark);
        assert_eq!(
            failed_settlement(&events, context.decision_id()).as_deref(),
            Some(HostCode::ApplicationSwitchFailed.as_str()),
            "{step}: settled failed with the code"
        );
        assert!(
            !bench
                .run_records(&context)
                .iter()
                .any(|event| event.event_type() == EventType::TaskRequested),
            "{step}: B's run never starts"
        );
        assert_eq!(
            bench.host.instance_claims_for_test(B).expect("claims"),
            (None, Vec::new()),
            "{step}: the key is released and no ladder holds or waits for it"
        );
        assert!(!ladder_started(&events), "{step}: no recovery ladder");
        assert!(
            bench
                .host
                .switch_cooldown_remaining_ms_for_test(B)
                .expect("the cooldown")
                .is_some(),
            "{step}: B's switch cooldown runs"
        );
        match step {
            "foreground_read" => {
                for alias in [A, B, D] {
                    assert!(bench.actions(alias).is_empty(), "{step}: {alias}");
                }
            }
            "stop" => {
                assert!(
                    bench.fact(A, TASK_PAGE).is_some(),
                    "a failed stop forgets no page"
                );
                assert!(bench.actions(B).is_empty(), "{step}: no startup run");
            }
            _ => {
                assert_eq!(
                    bench.actions(A),
                    [ApplicationLifecycleAction::Stop],
                    "{step}: the other member stopped before the startup run"
                );
            }
        }
        assert!(bench.host.fatal_error().expect("health").is_none());
        bench.close();
    }
}

/// The register's "a failed switch starts the member's switch cooldown, on a fake clock", with
/// the key set to 120 s by the file: B's next candidate is deferred `dispatch_instance_cooldown`
/// with `failure_code` `application_switch_failed`, `next_eligible_unix_ms` the cooldown's end and
/// its wake there, while A's task is dispatched and runs on the device; once the cooldown has
/// ended B's task is dispatched again. A second failed switch starts the cooldown again, and a
/// restart drops it: B's candidates carry no switch cooldown after it. The key: default `300000`,
/// range `1..=3600000`, settable.
#[test]
fn mapp2b_a_failed_switch_holds_the_member_for_the_cooldown_while_the_other_member_runs() {
    const COOLDOWN_MS: u64 = 120_000;
    let (row, _) = find_row(DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS).expect("the key's row");
    assert_eq!(
        row.range,
        KeyRange::Integer {
            min: 1,
            max: 3_600_000
        }
    );
    assert_eq!(row.settable, Settable::Yes);
    let shipped = DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1;
    assert_eq!(
        shipped["scheduling"]["device_group"]["switch_failure_cooldown_ms"],
        json!(DEFAULT_COOLDOWN_MS)
    );

    let bench = Bench::start(Setup {
        startup_deadline_ms: STARTUP_DEADLINE_MS,
        cooldown_ms: Some(COOLDOWN_MS),
    });
    let t0 = POLICY_NOW_UNIX_MS;
    let evaluation = bench.evaluate(t0);
    let context = bench.grant(&evaluation, TASK_B);
    bench.front(B, Some(APP_A));
    bench
        .state(B)
        .unknown_capture
        .store(true, Ordering::Release);
    let failed = bench.run(&context).expect_err("the switch fails");
    assert_switch_failed(&failed, B, "startup_package", None);
    assert_eq!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown"),
        Some(COOLDOWN_MS)
    );

    let t1 = t0 + MINUTE_MS;
    let held = bench.evaluate(t1);
    assert!(!dispatches(&held, TASK_B), "{:#?}", held.dispatch_intents);
    let (reasons, wake) = decision_of(&held, TASK_B, B);
    let cooldown = switch_cooldown_reason(reasons)
        .unwrap_or_else(|| panic!("B waits for its switch cooldown: {reasons:#?}"));
    assert_eq!(cooldown.value("instance_id"), Some(&json!(B)));
    assert_eq!(
        cooldown.value("next_eligible_unix_ms"),
        Some(&json!(t1 + COOLDOWN_MS))
    );
    assert_eq!(
        wake,
        Some(t1 + COOLDOWN_MS),
        "its wake is the cooldown's end"
    );
    let a = bench.grant(&held, TASK_A);
    bench.run_and_settle(&a);

    bench.clock().advance(COOLDOWN_MS);
    assert_eq!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown"),
        None
    );
    let t2 = t1 + COOLDOWN_MS + MINUTE_MS;
    let again = bench.evaluate(t2);
    assert!(dispatches(&again, TASK_B), "B is tried again: {again:#?}");
    let (reasons, _) = decision_of(&again, TASK_B, B);
    assert!(switch_cooldown_reason(reasons).is_none());

    let context = bench.grant(&again, TASK_B);
    bench.run(&context).expect_err("the switch fails again");
    assert_eq!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown"),
        Some(COOLDOWN_MS),
        "a second failure starts the cooldown again"
    );
    let bench = bench.restart();
    assert_eq!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown"),
        None,
        "a restart drops the cooldown"
    );
    let restarted = bench.evaluate(t2 + MINUTE_MS);
    let (reasons, _) = decision_of(&restarted, TASK_B, B);
    assert!(
        switch_cooldown_reason(reasons).is_none(),
        "no switch cooldown after the restart: {reasons:#?}"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's lone instance, compared within one host with and without the slice's branch
/// reachable: C's dispatch, run and settlement with the slice on, then again with it off
/// (`switch_off_for_test`), give the same lease, the same records of the run (types and
/// severities, in order), the same receipt state, the same own time as the execution's runtime
/// (so the same budget verdict, a success), the same sample and the same `last_duration_ms`; the
/// switch is never reached.
#[test]
fn mapp2b_a_lone_instance_writes_the_same_with_and_without_the_slice_reachable() {
    let bench = Bench::standard();
    let mut readings = Vec::new();
    for (round, off) in [(0_u64, false), (1, true)] {
        bench
            .host
            .switch_off_for_test(off)
            .expect("the slice's branch");
        let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS + round * 3_600_000);
        let context = bench.grant(&evaluation, TASK_C);
        let lease = bench
            .host
            .key_remaining_ms_for_test(context.lease_token(), policy_connection())
            .expect("C's key");
        let receipt = bench.run(&context).expect("C's run succeeds");
        let state = receipt.state();
        let (execution, _) = bench
            .host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("C's run settles");
        let runtime_ms = match execution.outcome {
            PolicyExecutionOutcome::Succeeded { runtime_ms } => runtime_ms,
            other => panic!("a success: {other:?}"),
        };
        let records = bench
            .run_records(&context)
            .iter()
            .map(|event| (event.event_type(), event.severity()))
            .collect::<Vec<_>>();
        let sample = bench.samples(C).first().copied();
        let last = bench
            .fact(C, &format!("task.{TASK_C}.last_duration_ms"))
            .map(|record| record.value);
        readings.push((lease, records, state, runtime_ms, sample, last));
    }
    let [on, off] = readings.as_slice() else {
        panic!("two runs: {readings:#?}");
    };
    assert_eq!(
        on, off,
        "the same with and without the slice's branch reachable"
    );
    assert_eq!(
        on.0,
        Some(RUN_DEADLINE_MS + RESERVE_MS),
        "the lease as before"
    );
    assert_eq!(on.4, Some(on.3), "the sample is the run's time");
    assert_eq!(bench.host.switch_entries_for_test().expect("count"), 0);
    assert!(bench.actions(A).is_empty() && bench.actions(B).is_empty());
    bench.close();
}

/// The work order's "no interruption", for the switch's caller: while B's run holds the device's
/// key with its own package in front, A's queued claim waits; the run reaches the switch, which
/// stops nothing and starts nothing, so A's application is neither started nor stopped and B's
/// is never stopped; A's claim gets the key only after B's run ended.
#[test]
fn mapp2b_no_switch_interrupts_the_run_that_holds_the_device() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let queued = bench.send(
        RuntimeOperation::queue_lease(
            A,
            ids.mint_holder_id().expect("holder"),
            LeaseQueuePolicy::new(LeasePriority::Normal, QUEUE_TIMEOUT_MS).expect("queue policy"),
        ),
        73,
    );
    let Some(RuntimeResult::LeaseQueued { status }) = queued.result() else {
        panic!("A's claim waits queued: {queued:?}");
    };
    let queued_request = status.request_id();
    assert_eq!(
        bench.host.instance_claims_for_test(A).expect("claims"),
        (
            Some(ClaimKind::PolicyDispatch),
            vec![ClaimKind::ClientLease]
        )
    );
    let mark = bench.head();

    bench.run_and_settle(&context);

    assert_eq!(bench.host.switch_entries_for_test().expect("count"), 1);
    assert!(
        bench.actions(A).is_empty(),
        "A is neither started nor stopped"
    );
    assert_eq!(
        bench.actions(B),
        [ApplicationLifecycleAction::Restart],
        "B's run's own restart only: B is never stopped"
    );
    let events = bench.events_after(mark);
    let run_ended = bench
        .run_records(&context)
        .into_iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .expect("B's run completes");
    assert!(
        !events.iter().any(|event| {
            event.sequence() < run_ended.sequence()
                && event.links().request_id() == Some(&queued_request)
                && matches!(
                    event.event_type(),
                    EventType::LeaseGranted | EventType::LeaseTransferred
                )
        }),
        "A's queued claim waits until B's run ended"
    );
    bench.close();
}

/// The register's "a direct run (the Lab) goes through the same switch" and "`ApplicationLifecycle`
/// `launch` stops the member in front first": B's direct run with A in front stops A, runs B's
/// startup package and then B's task, and completes; B's launch with A in front stops A and then
/// launches B, with no startup run; B's launch with B in front stops nothing; C's launch, alone on
/// its device, never reaches the switch.
#[test]
fn mapp2b_a_direct_run_and_a_launch_of_a_member_stop_the_member_in_front_first() {
    let bench = Bench::standard();
    bench.front_until_started(B, APP_A, APP_B);
    let holder = IdentifierIssuer::new().expect("identifier issuer");
    let direct = bench.send(
        RuntimeOperation::run_contained_task(
            B,
            holder.mint_holder_id().expect("holder"),
            bench.request(RUN_DEADLINE_MS),
        ),
        84,
    );
    assert_eq!(direct.state(), RuntimeReceiptState::Completed, "{direct:?}");
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(
        bench.actions(B),
        [
            ApplicationLifecycleAction::Restart,
            ApplicationLifecycleAction::Restart
        ]
    );
    assert_eq!(bench.host.switch_entries_for_test().expect("count"), 1);
    bench.close();

    let bench = Bench::standard();
    bench.front(B, Some(APP_A));
    let mark = bench.head();
    let launched = bench.send(
        RuntimeOperation::ApplicationLifecycle {
            instance_alias: B.to_owned(),
            holder_id: *holder.mint_holder_id().expect("holder").transport(),
            action: ApplicationLifecycleAction::Launch,
        },
        85,
    );
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(bench.actions(B), [ApplicationLifecycleAction::Launch]);
    assert_eq!(
        application_steps(&bench.events_after(mark)),
        vec![
            (bench.id(A), ApplicationLifecycleAction::Stop.event_action()),
            (
                bench.id(B),
                ApplicationLifecycleAction::Launch.event_action()
            ),
        ],
        "A stopped before B's launch"
    );
    bench.close();

    let bench = Bench::standard();
    for (alias, connection) in [(B, 86), (C, 87)] {
        let launched = bench.send(
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: alias.to_owned(),
                holder_id: *holder.mint_holder_id().expect("holder").transport(),
                action: ApplicationLifecycleAction::Launch,
            },
            connection,
        );
        assert_eq!(
            launched.state(),
            RuntimeReceiptState::Completed,
            "{launched:?}"
        );
    }
    assert!(bench.actions(A).is_empty(), "B in front: no stop");
    assert!(bench.actions(D).is_empty());
    assert_eq!(bench.actions(B), [ApplicationLifecycleAction::Launch]);
    assert_eq!(bench.actions(C), [ApplicationLifecycleAction::Launch]);
    assert_eq!(
        bench.host.switch_entries_for_test().expect("count"),
        1,
        "B's launch reached the switch, C's did not"
    );
    bench.close();
}
