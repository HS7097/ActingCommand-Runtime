// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 MAPP-2b (register §1.353; `contracts/device-group.md`, "The switch before a
//! run"): the switch's callers, the run's clock that leaves the switch out, the member's lease, a
//! failed switch and the member's switch cooldown, through the host.
//!
//! Three fixture instances A, B and D are one device group of several (`device.one`) and C is
//! alone on its own device; each has its own application and the neutral startup package, whose
//! one step restarts the application and whose home is the fake device's first frame. Every run
//! here runs that package as its task too, and the fake device's frame shows that home throughout.
//! A run the host starts itself (the switch's startup run) begins with the package's restart,
//! before any capture (`run_host_package`, R24); a task's run (a policy run, a direct run) takes
//! its first frame first and runs that restart only when the frame shows no page of the package
//! (`ApplicationEntryRun::AfterInitialFrame`, the kernel's `contained_task.rs`), so on the home it
//! sends no application command: every restart a device logs here is a startup run's. The
//! load refuses every group of several (actingd's MAPP-1 gate), so the host is handed the member
//! → device key map and the group's name as the assembly builds them. A manual Runtime clock
//! moves only by the kernel's waits and the test's own steps; nothing waits on the wall clock but
//! a test thread's bounded wait for a step another thread takes. Every expected value is the
//! contract's or the coordinator's ruling; every comparison is within one host but the lone
//! instance's and the test-only switch's, which compare two hosts started alike by values and by
//! payloads with every id and time normalised, never by an identity. Neutral names only.
//!
//! Workflow #437 MAPP-2d (register §1.360; `contracts/device-group.md`, "The key's time"): the
//! lease across a switch, on the same fixture. A member's dispatch is granted the lease of an
//! instance alone; the switch runs inside it, its startup run taking what is left of the key;
//! once the startup run has handed the key back the run renews its lease to its own deadline
//! and the reserve, with the token the renewal returns, and every later step of the run, its
//! failed settlement's release included, uses that token. The tests named `mapp2d_*` pin it,
//! with the request deadlines a configuration gives (1 800 000 ms) where the work order asks.

use super::instance_workers::phases;
use super::*;
use crate::codes::{ApplicationSwitchStep, HostCode};
use crate::policy_control::PolicyValueRecord;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, ConfigParameter, ConfigParameterSource, EventAction,
    FactScalar, InstanceRunStop, MAX_TASK_DIAGNOSTIC_PAGE_RECORDS, PolicyConditionSettlement,
    RequestId, RuntimeConfigManifest, RuntimeFactRecord, RuntimeFactScope, RuntimeLifecyclePhase,
};
use actingcommand_ledger_forensics::{
    ForensicEventFilter, ForensicEventsRequest, ForensicOutput, ForensicReport, ForensicRequest,
    MAX_FORENSIC_EVENTS, TaskEvidenceReport, TaskRecordsRequest,
};
use actingcommand_policy::{PolicyCode, PolicyEvaluation};
use actingcommand_runtime_config::scheduling::DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS;
use actingcommand_runtime_config::{DefaultsFile, KeyRange, Settable, find_row};
use actingcommand_scheduler::{ClaimKind, DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS};
use serde_json::{Value, json};
use std::net::SocketAddr;

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
/// Workflow #437 MAPP-2d: the deadline every startup package request, configured procedure and
/// composed pack request of a configuration carries.
const LONGEST_DEADLINE_MS: u64 = 1_800_000;
/// `scheduling.device_group.switch_failure_cooldown_ms`'s default.
const DEFAULT_COOLDOWN_MS: u64 = 300_000;
/// Workflow #437 ORD-1c: `scheduling.device_group.max_queue_wait_ms`'s default, the limit of a task
/// that declares none.
const DEFAULT_LIMIT_MS: u64 = 1_800_000;
/// The wait a queued claim asks: the longest the fixture's scheduler takes.
const QUEUE_TIMEOUT_MS: u64 = DEFAULT_MAXIMUM_QUEUE_TIMEOUT_MS;
/// The connection of every policy run's key (the host's `POLICY_CONNECTION_VALUE`).
const POLICY_CONNECTION: u64 = u64::MAX;
const TASK_PAGE: &str = "task.page";
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
/// The code a run a `StopInstanceRun` stopped ends with.
const CANCELLED: &str = "contained_task_cancelled";
/// The reason of an owed rerun's candidate.
const RERUN_OWED: &str = "policy_rerun_owed";
/// How long a test thread waits for a step another thread takes.
const GATE_WAIT: Duration = Duration::from_secs(60);

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

    /// `alias`'s foreground answers `front` from now on, and the fake device counts each query of
    /// it: its log of foreground reads.
    fn count_front_reads(&self, alias: &str, front: &'static str) -> Arc<AtomicUsize> {
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        *self
            .state(alias)
            .foreground_script
            .lock()
            .expect("foreground script") = Some(Arc::new(move |_: usize, _: usize| {
            counted.fetch_add(1, Ordering::AcqRel);
            Some(front.to_owned())
        }));
        reads
    }

    /// Whether `alias`'s pair of `task_id` is owed a rerun, as the host's decision memory holds
    /// it.
    fn owes_rerun(&self, alias: &str, task_id: &str) -> bool {
        let (held, _, _) = self
            .host
            .decision_memory_for_test()
            .expect("the decision memory");
        match held.get(&format!("decision_memory/{alias}/{task_id}")) {
            Some(PolicyValueRecord::Control(record)) => record.condition_reruns.is_some(),
            other => panic!("the pair's value record: {other:?}"),
        }
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

    /// Workflow #437 MAPP-2d: `task_id`'s dispatch in `evaluation`, granted with a run of
    /// `deadline_ms`.
    fn grant_with(
        &self,
        evaluation: &PolicyEvaluation,
        task_id: &str,
        deadline_ms: u64,
    ) -> Box<PolicyRunContext> {
        match self
            .admit(evaluation, task_id, deadline_ms)
            .unwrap_or_else(|error| panic!("admit {task_id}: {error}"))
        {
            PolicyDispatchAdmission::Granted { context } => context,
            other => panic!("{task_id}'s dispatch is granted: {other:?}"),
        }
    }

    /// Workflow #437 MAPP-2d: `alias`'s first foreground read moves the Runtime clock on by
    /// `read_ms`, as a slow device would, inside the switch; the read answers A's package until
    /// `alias`'s own application took a command, and `own` after it.
    fn slow_front(&self, alias: &str, own: &'static str, read_ms: u64) {
        let clock = Arc::clone(&self.parts.clock);
        let read = Arc::new(AtomicBool::new(false));
        *self
            .state(alias)
            .foreground_script
            .lock()
            .expect("foreground script") = Some(Arc::new(move |_: usize, commands: usize| {
            if !read.swap(true, Ordering::AcqRel) {
                clock.advance(read_ms);
            }
            Some(if commands == 0 { APP_A } else { own }.to_owned())
        }));
    }

    /// Workflow #437 MAPP-2d: a client claim of A queued now on `connection` (normal priority, so
    /// it asks no preemption), behind whatever holds the device; its request id.
    fn queue_a(&self, connection: u64) -> RequestId {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let queued = self.send(
            RuntimeOperation::queue_lease(
                A,
                ids.mint_holder_id().expect("holder"),
                LeaseQueuePolicy::new(LeasePriority::Normal, QUEUE_TIMEOUT_MS)
                    .expect("queue policy"),
            ),
            connection,
        );
        let Some(RuntimeResult::LeaseQueued { status }) = queued.result() else {
            panic!("A's claim waits queued: {queued:?}");
        };
        status.request_id()
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

/// The execution records of the dispatch `decision_id`, in ledger order.
fn executions(events: &[PersistedEvent], decision_id: &str) -> Vec<PolicyExecutionOutcome> {
    events
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

/// Whether a startup package was scheduled among `events`: the switch's step 3 began.
fn startup_scheduled(events: &[PersistedEvent]) -> bool {
    phases(events)
        .iter()
        .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. }))
}

/// Whether a member is named `key`, whose value is a time or a place in the ledger: an instant on
/// either clock, a measurement on the process clock (the run's timing observations, the package's
/// load time, the bytes of the records the run wrote, which hold such times), a local day, a
/// ledger position, revision or record index, or the evaluation's `seed` (a digest of the fact
/// snapshot's id, the evaluation's time and the owner epoch). Two hosts hold these apart however
/// alike their runs are.
fn time_or_place(key: &str) -> bool {
    key.ends_with("_unix_ms")
        || key.ends_with("_monotonic_ms")
        || key.ends_with("_us")
        || key.ends_with("_at")
        || matches!(
            key,
            "task_timing"
                | "package_load_ms"
                | "returned_bytes"
                | "successful_returned_bytes"
                | "local_day"
                | "sequence"
                | "record_index"
                | "input_ledger_position"
                | "input_fact_revisions"
                | "seed"
        )
}

/// `run` (lowercase hexadecimal digits) as [`without_identities`] writes it: a run of 32 or more
/// is an identity and is replaced.
fn push_hex_run(text: &mut String, run: &mut String) {
    if run.len() >= 32 {
        text.push_str("<hex>");
    } else {
        text.push_str(run);
    }
    run.clear();
}

/// `text` with each run of 32 or more lowercase hexadecimal digits replaced: a minted id (a
/// request, run, lease, instance or causation id), a decision or snapshot id, a digest.
fn without_identities(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    let mut run = String::new();
    for character in text.chars() {
        if matches!(character, '0'..='9' | 'a'..='f') {
            run.push(character);
        } else {
            push_hex_run(&mut written, &mut run);
            written.push(character);
        }
    }
    push_hex_run(&mut written, &mut run);
    written
}

/// `value` with its ids and times normalised: every member [`time_or_place`] names is replaced
/// whole, and every string loses its identities ([`without_identities`]). Everything else stays.
fn normalised(value: &mut Value) {
    match value {
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                if time_or_place(key) {
                    *member = json!("<normalised>");
                } else {
                    normalised(member);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalised),
        Value::String(text) => *text = without_identities(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// The records of a run as the lone instance's comparison reads them: each one's type, severity
/// and whole payload, normalised ([`normalised`]).
fn comparable(events: &[PersistedEvent]) -> Vec<(EventType, EventSeverity, Value)> {
    events
        .iter()
        .map(|event| {
            let mut payload =
                serde_json::to_value(event.payload()).expect("a record's payload encodes");
            normalised(&mut payload);
            (event.event_type(), event.severity(), payload)
        })
        .collect()
}

/// `StopInstanceRun` of `run` on `alias`, answering at once (`wait_ms` 0), on a new connection.
fn stop_run(address: SocketAddr, alias: &str, run: RequestId) -> RuntimeReceipt {
    let mut client = TestClient::connect_address(address);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::StopInstanceRun {
        instance_alias: alias.to_owned(),
        request_id: run,
        wait_ms: 0,
    });
    client.send(&request)
}

/// Workflow #437 MAPP-2d: what the first `PackageAdmitted` record among `events` holds: the run's
/// deadline and the expiry of the key it ran on.
fn admitted<'a>(events: impl IntoIterator<Item = &'a PersistedEvent>) -> (u64, u64) {
    events
        .into_iter()
        .find_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                TaskSemanticFact::PackageAdmitted {
                    response_deadline_monotonic_ms: Some(deadline),
                    ..
                } => Some((*deadline, payload.lease_expires_at_monotonic_ms()?)),
                _ => None,
            },
            _ => None,
        })
        .expect("a run's package admission with its deadline and its key's expiry")
}

/// Workflow #437 MAPP-2d: the records among `events` under the causation id of the switch that
/// stopped `stopped`'s application: that stop's and the switch's startup run's.
fn switch_records(events: &[PersistedEvent], stopped: InstanceId) -> Vec<&PersistedEvent> {
    let causation = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::CommandReceived
                && event.links().instance_id() == Some(&stopped)
        })
        .and_then(|event| event.links().causation_id())
        .expect("the stop and its causation");
    events
        .iter()
        .filter(|event| event.links().causation_id() == Some(causation))
        .collect()
}

/// Workflow #437 MAPP-2d: the effect of each `lease.released` record of the lease `lease_id` among
/// `events`.
fn release_effects(events: &[PersistedEvent], lease_id: LeaseId) -> Vec<Option<EffectDisposition>> {
    events
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().lease_id() == Some(&lease_id)
        })
        .map(|event| event.payload().effect_disposition())
        .collect()
}

/// Workflow #437 MAPP-2d: the sequence of the first record among `events` that `matches` takes.
fn first_sequence(events: &[PersistedEvent], matches: impl Fn(&PersistedEvent) -> bool) -> u64 {
    events
        .iter()
        .find(|&event| matches(event))
        .map(PersistedEvent::sequence)
        .expect("the record")
}

/// Workflow #437 MAPP-2d fix round 1: the ledger under `root` after `after_sequence` as the
/// forensic side reads a run (`actingledger`'s task evidence, the reader FL-2's gate reads a run's
/// records with), every task diagnostic stream of the window read with it; a refusal of the whole
/// read fails here.
fn task_evidence(root: &Path, after_sequence: u64) -> TaskEvidenceReport {
    let request = ForensicRequest::task_evidence(
        root,
        ForensicEventsRequest::new(
            ForensicEventFilter::default(),
            after_sequence,
            None,
            MAX_FORENSIC_EVENTS,
        )
        .expect("a task evidence window"),
    )
    .with_task_records(TaskRecordsRequest {
        cursor: None,
        limit: MAX_TASK_DIAGNOSTIC_PAGE_RECORDS,
        include_private: true,
    })
    .expect("a task records page");
    match actingcommand_ledger_forensics::run(request) {
        Ok(ForensicOutput::Machine(ForensicReport::TaskEvidence(report))) => *report,
        other => panic!("the forensic reader reads the window: {other:?}"),
    }
}

// ---- The tests.

/// The register's MAPP-2's tests, "a switch kills the first application, starts the second and
/// reaches its home before the task, under one causation id, the key never granted to another
/// claim in between", with the work order's run of B with A in front: the device sees A's stop,
/// then B's startup run's restart, and nothing of B's task, whose run finds B's home on its first
/// frame and so sends no application command; one application is alive after each step; A's `task.page` and `application.foreground` are gone; A's stop records are A's, B's
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
        [ApplicationLifecycleAction::Restart],
        "B's startup run's restart only: B's task, on B's home, sends no command"
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
        ],
        "A's stop, then B's startup run's restart; B's task writes no application command"
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
/// before B's startup run (its restart, the only command B's device sees: B's task finds B's home
/// on its first frame) and B's task; with B's own package in front nothing reaches any device (no
/// stop, no startup run, and B's task, on its home, sends no command), the switch is reached once
/// and schedules no startup package.
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
        [ApplicationLifecycleAction::Restart],
        "B's startup run's restart only: B's task, on B's home, sends no command"
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
    assert!(
        bench.actions(B).is_empty(),
        "no startup run, and B's task, on B's home, sends no command"
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
/// ended B's candidate carries no cooldown and B's task is dispatched again. A second failed
/// switch starts the cooldown again, and a restart drops it: B's candidates carry no switch
/// cooldown after it. The key: default `300000`, range `1..=3600000`, settable.
///
/// Adapted by Workflow #437 ORD-1c (register §1.363), which passes the order within one device:
/// A's run put A in front, so after the cooldown the device stays with A while A's task ends by
/// B's limit, and B's task is dispatched in the round its limit (the default
/// `scheduling.device_group.max_queue_wait_ms`, 1 800 000, from the first round after the
/// cooldown) comes; MAPP-1's pick gave it the device in the first round after the cooldown.
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
    let (reasons, _) = decision_of(&again, TASK_B, B);
    assert!(switch_cooldown_reason(reasons).is_none());
    // Workflow #437 ORD-1c (register §1.363): B competes again, but A's run put A in front, so
    // the device stays with A while A's task ends by B's limit; B's wait began at t2, and at its
    // limit nothing of A ends by it and B's task is dispatched (`ord1c_t7`).
    let t3 = t2 + DEFAULT_LIMIT_MS;
    let due = bench.evaluate(t3);
    assert!(
        dispatches(&due, TASK_B),
        "B is tried again once its limit came: {due:#?}"
    );
    let (reasons, _) = decision_of(&due, TASK_B, B);
    assert!(switch_cooldown_reason(reasons).is_none());

    let context = bench.grant(&due, TASK_B);
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
    let restarted = bench.evaluate(t3 + MINUTE_MS);
    let (reasons, _) = decision_of(&restarted, TASK_B, B);
    assert!(
        switch_cooldown_reason(reasons).is_none(),
        "no switch cooldown after the restart: {reasons:#?}"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's lone instance, as the review asked it shown. The lone path is unchanged
/// because the run path's existing gates (the ED-2 gate among them) pass unchanged and because
/// the slice's only entry is the group-of-several check (`HostShared::switch_applies`); this test
/// pins that the entry is never taken and that the records are equal.
///
/// - The entry is the only way in: B, a member, with A in front, run on two hosts started alike,
///   one with the test-only switch off (`switch_off_for_test`, compiled into this crate's test
///   build only) and one with it on. Off: the fake device logs no foreground read of B, no stop
///   of A and no startup run, and no application command at all (B's task finds B's home on its
///   first frame and sends none), the switch's entry is never reached and B's lease is
///   `D_run + R`. On: the switch's three steps (one foreground read, A's stop, B's startup run's
///   restart, then B's task, which sends no command), its entry reached once, the lease
///   `D_run + R` as well (Workflow #437 MAPP-2d: a member is granted an instance alone's lease;
///   MAPP-2b granted `D_s + D_run + R` here).
/// - C, alone on its device, with the slice reachable: the fake device logs no foreground read
///   of C and C's application log is empty (no startup run, and C's task finds C's home on its
///   first frame and sends no command), A, B and D see nothing, no startup package is scheduled; C's lease is exactly `D_run + R`; its dispatch clock holds a switch time of 0
///   for the settlement to subtract; no switch cooldown entry exists afterwards and the switch's
///   entry was never reached.
/// - C's run on that host and on a second host started alike with the switch off from the start:
///   the run's records are equal record by record, type, severity and whole payload, with ids and
///   times normalised ([`comparable`]); the receipt state, the own time, the budget verdict (a
///   success on its own time), the sample and `last_duration_ms` are equal too. Two hosts, not
///   one: a second dispatch of the same pair on one host carries the pair's history (its budget
///   counts and runtime reserved, its expected duration from the first run's sample, its
///   ranking), so only two hosts started alike leave nothing but ids and times to tell the runs
///   apart; nothing is compared between them but normalised payloads and values.
#[test]
fn mapp2b_the_group_check_is_the_only_entry_and_a_lone_instance_never_takes_it() {
    // B: the test-only switch where it makes a difference, on two hosts started alike (a second
    // run of B's task on one host would meet the task's runtime budget, which its first run's
    // sample fills).
    for off in [true, false] {
        let bench = Bench::standard();
        bench
            .host
            .switch_off_for_test(off)
            .expect("the slice's branch");
        let reads = bench.count_front_reads(B, APP_A);
        let mark = bench.head();
        let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
        let context = bench.grant(&evaluation, TASK_B);
        let lease = bench.remaining(context.lease_token());
        bench.run_and_settle(&context);
        let scheduled = startup_scheduled(&bench.events_after(mark));
        let entries = bench.host.switch_entries_for_test().expect("count");
        if off {
            assert_eq!(
                lease,
                Some(RUN_DEADLINE_MS + RESERVE_MS),
                "off: the lease of the run alone"
            );
            assert_eq!(reads.load(Ordering::Acquire), 0, "off: no foreground read");
            assert!(bench.actions(A).is_empty(), "off: A is not stopped");
            assert!(
                bench.actions(B).is_empty(),
                "off: no startup run, and B's task, on B's home, sends no command"
            );
            assert!(!scheduled, "off: no startup package is scheduled");
            assert_eq!(entries, 0, "off: the switch's entry is never reached");
        } else {
            assert_eq!(
                lease,
                Some(RUN_DEADLINE_MS + RESERVE_MS),
                "on: the lease of the run alone, as off (MAPP-2d)"
            );
            assert_eq!(reads.load(Ordering::Acquire), 1, "on: the switch's read");
            assert_eq!(
                bench.actions(A),
                [ApplicationLifecycleAction::Stop],
                "on: A, in front, is stopped"
            );
            assert_eq!(
                bench.actions(B),
                [ApplicationLifecycleAction::Restart],
                "on: B's startup run's restart only: B's task, on B's home, sends no command"
            );
            assert!(scheduled, "on: B's startup package is scheduled");
            assert_eq!(entries, 1, "on: the switch's entry is reached once");
        }
        assert!(bench.actions(D).is_empty(), "D was not in front");
        assert!(bench.host.fatal_error().expect("health").is_none());
        bench.close();
    }

    // C: the slice reachable on the first host, switched off from the start on the second.
    let mut runs = Vec::new();
    for off in [false, true] {
        let bench = Bench::standard();
        bench
            .host
            .switch_off_for_test(off)
            .expect("the slice's branch");
        let reads = bench.count_front_reads(C, APP_C);
        let mark = bench.head();
        let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
        let context = bench.grant(&evaluation, TASK_C);
        let lease = bench.remaining(context.lease_token());
        let receipt = bench.run(&context).expect("C's run succeeds");
        let switch_ms = bench
            .host
            .policy_switch_ms_for_test(context.decision_id())
            .expect("the dispatch clock");
        let (execution, _) = bench
            .host
            .complete_scheduled_policy_run(&context, &receipt)
            .expect("C's run settles");
        let runtime_ms = match execution.outcome {
            PolicyExecutionOutcome::Succeeded { runtime_ms } => runtime_ms,
            other => panic!("a success on its own time: {other:?}"),
        };
        if !off {
            assert_eq!(reads.load(Ordering::Acquire), 0, "no foreground read of C");
            assert!(
                bench.actions(C).is_empty(),
                "C's log is empty: no startup run, and C's task, on C's home, sends no command"
            );
            for alias in [A, B, D] {
                assert!(bench.actions(alias).is_empty(), "no stop of {alias}");
            }
            assert!(!startup_scheduled(&bench.events_after(mark)));
            assert_eq!(
                lease,
                Some(RUN_DEADLINE_MS + RESERVE_MS),
                "C's lease is D_run + R"
            );
            assert_eq!(
                switch_ms,
                Some(0),
                "the settlement subtracts a switch time of 0"
            );
            assert_eq!(
                bench
                    .host
                    .switch_cooldown_entries_for_test()
                    .expect("the cooldowns"),
                0,
                "no switch cooldown entry"
            );
            assert_eq!(bench.host.switch_entries_for_test().expect("count"), 0);
        }
        let sample = bench.samples(C).first().copied();
        assert_eq!(sample, Some(runtime_ms), "the sample is the run's own time");
        let last = bench
            .fact(C, &format!("task.{TASK_C}.last_duration_ms"))
            .map(|record| record.value);
        runs.push((
            comparable(&bench.run_records(&context)),
            receipt.state(),
            runtime_ms,
            sample,
            last,
        ));
        assert!(bench.host.fatal_error().expect("health").is_none());
        bench.close();
    }
    let [reachable, switched_off] = runs.as_slice() else {
        panic!("two runs: {runs:#?}");
    };
    assert!(!reachable.0.is_empty(), "the run has records");
    assert_eq!(
        reachable.0.len(),
        switched_off.0.len(),
        "as many records: {reachable:#?} {switched_off:#?}"
    );
    // Every differing record at once: a comparison that stops at the first one hides the rest.
    let differing: Vec<_> = reachable
        .0
        .iter()
        .zip(&switched_off.0)
        .enumerate()
        .filter(|(_, (with, without))| with != without)
        .collect();
    assert!(
        differing.is_empty(),
        "the records of C's run are the same on both hosts; these differ: {differing:#?}"
    );
    assert_eq!(
        reachable, switched_off,
        "the same receipt state, own time, sample and last_duration_ms"
    );
}

/// The work order's "no interruption", for the switch's caller: while B's run holds the device's
/// key with its own package in front, A's queued claim waits; the run reaches the switch, which
/// stops nothing and starts nothing, so A's application is neither started nor stopped and no
/// command reaches B's device at all (B's task finds B's home on its first frame and sends none);
/// A's claim gets the key only after B's run ended.
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
    assert!(
        bench.actions(B).is_empty(),
        "B is never stopped, and B's task, on B's home, sends no command"
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
/// startup package (its restart, the only command B's device sees) and then B's task, which finds
/// B's home on its first frame and sends no command, and completes; B's launch with A in front stops A and then
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
        [ApplicationLifecycleAction::Restart],
        "B's startup run's restart only: B's task, on B's home, sends no command"
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

/// The work order's "the cooldown also holds a rerun the member is owed": B's switch fails at
/// `startup_package` and its run is settled by a condition (the test's hook, standing in for a
/// code `scheduling.rerun.condition_codes` lists), so B's pair is owed a rerun while B's switch
/// cooldown runs (the key set to 120 s by the file). During the cooldown B's candidate is not
/// dispatched: it waits `dispatch_instance_cooldown` with `failure_code`
/// `application_switch_failed`, its wake the cooldown's end, the rerun stays owed, and A's task is
/// dispatched. After the cooldown B's task is dispatched again as the owed rerun
/// (`policy_rerun_owed`).
///
/// Adapted by Workflow #437 ORD-1c (register §1.363), which passes the order within one device:
/// A's dispatch now runs (A in front, its task's wait over), and after the cooldown B's owed rerun
/// waits for A until its limit (the default `scheduling.device_group.max_queue_wait_ms`,
/// 1 800 000, from the first round after the cooldown) and is dispatched in the round it comes;
/// before ORD-1c A's dispatch was left unadmitted and MAPP-1's pick gave B the device in the first
/// round after the cooldown, which the order would give A's waiting task instead.
#[test]
fn mapp2b_the_switch_cooldown_also_holds_the_rerun_the_member_is_owed() {
    const COOLDOWN_MS: u64 = 120_000;
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
    bench
        .host
        .settle_next_policy_run_by_condition_for_test(PolicyConditionSettlement {
            refunded: true,
            count: 1,
            expires_at_unix_ms: t0 + 3 * HOUR_MS,
        })
        .expect("the next settlement's condition");
    let failed = bench.run(&context).expect_err("the switch fails");
    assert_switch_failed(&failed, B, "startup_package", None);
    assert!(bench.owes_rerun(B, TASK_B), "B's pair is owed a rerun");
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
    assert!(
        switch_cooldown_reason(reasons).is_some(),
        "B's owed rerun waits for its switch cooldown: {reasons:#?}"
    );
    assert_eq!(
        wake,
        Some(t1 + COOLDOWN_MS),
        "its wake is the cooldown's end"
    );
    assert!(
        dispatches(&held, TASK_A),
        "A's task is dispatched meanwhile"
    );
    assert!(bench.owes_rerun(B, TASK_B), "the rerun is still owed");
    // Workflow #437 ORD-1c (register §1.363): A's dispatch runs, so A is in front and its task's
    // wait ends; after the cooldown B's owed rerun waits for A by its limit (it gains no
    // precedence over the member in front) and goes in the round that limit comes.
    let a = bench.grant(&held, TASK_A);
    bench.run_and_settle(&a);

    bench.clock().advance(COOLDOWN_MS);
    let t2 = t1 + COOLDOWN_MS + MINUTE_MS;
    let after = bench.evaluate(t2);
    let (reasons, _) = decision_of(&after, TASK_B, B);
    assert!(
        switch_cooldown_reason(reasons).is_none(),
        "the cooldown is over: {reasons:#?}"
    );
    assert!(bench.owes_rerun(B, TASK_B), "the rerun is still owed");
    let again = bench.evaluate(t2 + DEFAULT_LIMIT_MS);
    let (_, chain) = intent_of(&again, TASK_B);
    let codes = chain
        .reasons
        .iter()
        .map(|reason| reason.outcome.code().as_str().to_owned())
        .collect::<Vec<_>>();
    assert!(
        codes.iter().any(|code| code == RERUN_OWED),
        "B's task is dispatched as the owed rerun: {codes:?}"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's "while one member is in its cooldown, a task of the device's other member is
/// dispatched and runs to its end with its own records": B's switch fails at its foreground read
/// (nothing stopped) and B's switch cooldown runs. At the next round B is not dispatched and A's
/// task is; it is granted, runs and settles a success on its own time: no application command
/// reached A, B or D (A's own package is in front, so the switch does nothing, and A's task finds
/// A's home on its first frame and sends none), the run's records that name an instance name A only (its
/// request and its terminal among them), its run-duration sample and `last_duration_ms` are its
/// own time, and B's cooldown still runs.
#[test]
fn mapp2b_while_one_member_cools_down_the_other_members_task_runs_to_its_end_with_its_own_records()
{
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    *bench
        .state(B)
        .foreground_error_once
        .lock()
        .expect("one-shot foreground error") =
        Some(DeviceError::transient("fixture foreground query failed"));
    let failed = bench.run(&context).expect_err("B's switch fails");
    assert_switch_failed(
        &failed,
        B,
        "foreground_read",
        Some(HostCode::ApplicationForegroundUnknown.as_str()),
    );

    let held = bench.evaluate(POLICY_NOW_UNIX_MS + MINUTE_MS);
    assert!(!dispatches(&held, TASK_B), "{:#?}", held.dispatch_intents);
    let a = bench.grant(&held, TASK_A);
    let runtime_ms = bench.run_and_settle(&a);
    assert!(
        bench.actions(A).is_empty(),
        "A's own package in front: no switch step, and A's task, on A's home, sends no command"
    );
    assert!(bench.actions(B).is_empty(), "nothing of B");
    assert!(bench.actions(D).is_empty(), "nothing of D");
    let records = bench.run_records(&a);
    for event_type in [EventType::TaskRequested, EventType::TaskCompleted] {
        let record = records
            .iter()
            .find(|event| event.event_type() == event_type)
            .unwrap_or_else(|| panic!("A's run's {event_type:?}"));
        assert_eq!(record.links().instance_id(), Some(&bench.id(A)));
    }
    let named = records
        .iter()
        .filter_map(|event| event.links().instance_id().copied())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        named,
        BTreeSet::from([bench.id(A)]),
        "every record of A's run that names an instance names A"
    );
    assert_eq!(
        bench.samples(A),
        vec![runtime_ms],
        "A's sample is its own time"
    );
    let last = bench
        .fact(A, &format!("task.{TASK_A}.last_duration_ms"))
        .expect("A's last duration");
    assert_eq!(
        last.value,
        ContractFactValue::Integer(i64::try_from(runtime_ms).expect("a duration"))
    );
    assert!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown")
            .is_some(),
        "B's switch cooldown still runs"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's stop request in mid-switch (`contracts/device-group.md`, "A stop or a shutdown
/// in mid-switch"): B's run switches with A in front, on a thread of its own; once A has been
/// stopped, the capture of B's startup run waits while a `StopInstanceRun` of B's run is sent,
/// which answers at once that the run is still stopping. The switch goes on to its end (B's
/// startup run reaches its home), and B's run then stops at its first check: it ends
/// `contained_task_cancelled`, the Runtime resets the device once on the run's key before the
/// key's one release, the run settles as one `stopped` record, A stays stopped, no switch cooldown
/// starts, no ladder starts and the key is free.
#[test]
fn mapp2b_a_stop_request_in_mid_switch_lets_the_switch_end_and_stops_the_run_at_its_first_check() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let decision_id = context.decision_id().to_owned();
    let lease_id = context.lease_token().lease_id();
    bench.front_until_started(B, APP_A, APP_B);
    let a_state = Arc::clone(&bench.parts.states[A]);
    let (entered_tx, entered) = mpsc::channel::<()>();
    let (proceed_tx, proceed) = mpsc::channel::<()>();
    let pause = std::sync::Mutex::new(Some((entered_tx, proceed)));
    *bench.state(B).pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, _: usize| {
            let a_stopped = a_state
                .application_actions
                .lock()
                .expect("A's application commands")
                .contains(&ApplicationLifecycleAction::Stop);
            let waiting = if a_stopped {
                pause.lock().expect("the pause").take()
            } else {
                None
            };
            if let Some((entered, proceed)) = waiting {
                let _ = entered.send(());
                let _ = proceed.recv_timeout(GATE_WAIT);
            }
            Vec::<(usize, [u8; 3])>::new()
        }));
    let address = bench
        .host
        .runtime_info()
        .socket_addr()
        .expect("runtime address");
    let mark = bench.head();
    let host = &bench.host;
    let request = bench.request(RUN_DEADLINE_MS);
    let (run, answer, ended) = thread::scope(|scope| {
        let runner = scope.spawn(move || host.run_scheduled_contained_task(&context, &request));
        entered
            .recv_timeout(GATE_WAIT)
            .expect("B's startup run captures after A's stop");
        let events = bench.events_after(mark);
        let (scheduled, _) = phases(&events)
            .into_iter()
            .find(|(_, phase)| {
                matches!(phase, RuntimeLifecyclePhase::StartupPackageScheduled { .. })
            })
            .expect("B's startup package is scheduled under B's run's request");
        let run = scheduled
            .links()
            .request_id()
            .copied()
            .expect("B's run's request");
        let answer = stop_run(address, B, run);
        proceed_tx.send(()).expect("the startup run goes on");
        (run, answer, runner.join().expect("B's run's thread"))
    });

    assert_eq!(answer.state(), RuntimeReceiptState::Completed, "{answer:?}");
    let Some(RuntimeResult::InstanceRunStopped { stopped }) = answer.result() else {
        panic!("a stop: {answer:?}");
    };
    assert_eq!(
        *stopped,
        InstanceRunStop {
            request_id: run,
            settled: false,
            reset: None,
        },
        "the stop reached the run, still stopping"
    );
    let error = ended.expect_err("the stopped run does not complete");
    assert_eq!(error.code(), CANCELLED, "{error:?}");
    assert_eq!(
        bench.actions(A),
        [ApplicationLifecycleAction::Stop],
        "A stays stopped"
    );
    assert_eq!(
        bench.actions(B).first(),
        Some(&ApplicationLifecycleAction::Restart),
        "B's startup run went on"
    );
    let events = bench.events_after(mark);
    let causation = events
        .iter()
        .find(|event| {
            event.event_type() == EventType::CommandReceived
                && event.links().instance_id() == Some(&bench.id(A))
        })
        .and_then(|event| event.links().causation_id().copied())
        .expect("A's stop and its causation");
    assert!(
        events.iter().any(|event| {
            event.event_type() == EventType::TaskCompleted
                && event.links().causation_id() == Some(&causation)
        }),
        "B's startup run reached its home"
    );
    let on_key = bench
        .host
        .query_persisted_events_for_test(EventQuery {
            lease_id: Some(lease_id),
            ..EventQuery::default()
        })
        .expect("the key's records");
    let resets = on_key
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Input(InputPayload::Committed(detail))
                    if detail.action() == EventAction::InputReset
            )
        })
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    let releases = on_key
        .iter()
        .filter(|event| event.event_type() == EventType::LeaseReleased)
        .map(PersistedEvent::sequence)
        .collect::<Vec<_>>();
    assert_eq!(resets.len(), 1, "one reset: {resets:?}");
    assert_eq!(releases.len(), 1, "one release: {releases:?}");
    assert!(resets[0] < releases[0], "{resets:?} {releases:?}");
    let settled = executions(&events, &decision_id);
    let [PolicyExecutionOutcome::Stopped { outcome, .. }] = settled.as_slice() else {
        panic!("one stopped record: {settled:#?}");
    };
    assert_eq!(outcome.code().as_str(), CANCELLED);
    assert_eq!(
        bench.host.instance_claims_for_test(B).expect("claims"),
        (None, Vec::new()),
        "the key is free"
    );
    assert_eq!(
        bench
            .host
            .switch_cooldown_entries_for_test()
            .expect("the cooldowns"),
        0,
        "no switch cooldown: the switch did not fail"
    );
    assert!(!ladder_started(&events), "no recovery ladder");
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

// ---- Workflow #437 MAPP-2d: the lease across a switch.

/// The work order's "with real deadlines": B's startup package and B's run both carry the
/// deadline a configuration gives (1 800 000 ms), and B's run has A in front. B's dispatch is
/// granted (MAPP-2b's bound refused it) with `D_run + R`, as C's, alone, with the same run. The
/// switch's front read takes 1 000 000 ms, as a slow device would, so the startup run takes what
/// is left of the key: its deadline is the granted key's expiry less the reserve. B's run then
/// holds a renewed lease that outlasts the granted one, and its deadline is its grant plus its
/// own response deadline moved on by the switch's time and by nothing else: what an instance
/// alone's computation gives (C's run on the same host, its grant plus its response deadline)
/// plus the switch. The key never expires (no `lease.expired`, each run's deadline inside the key
/// it ran on), and B's run settles a success.
#[test]
fn mapp2d_with_real_deadlines_a_member_switches_and_runs_on_its_renewed_lease() {
    const READ_MS: u64 = 1_000_000;
    let bench = Bench::start(Setup {
        startup_deadline_ms: LONGEST_DEADLINE_MS,
        cooldown_ms: None,
    });
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let b = bench.grant_with(&evaluation, TASK_B, LONGEST_DEADLINE_MS);
    let c = bench.grant_with(&evaluation, TASK_C, LONGEST_DEADLINE_MS);
    assert_eq!(
        bench.remaining(b.lease_token()),
        Some(LONGEST_DEADLINE_MS + RESERVE_MS),
        "B's lease is its run's own"
    );
    assert_eq!(
        bench.remaining(c.lease_token()),
        Some(LONGEST_DEADLINE_MS + RESERVE_MS),
        "C's, alone, the same"
    );
    // C, alone, first: today's computation, its grant plus its response deadline.
    let c_receipt = bench
        .host
        .run_scheduled_contained_task(&c, &bench.request(LONGEST_DEADLINE_MS))
        .expect("C's run succeeds");
    bench
        .host
        .complete_scheduled_policy_run(&c, &c_receipt)
        .expect("C's run settles");
    let (c_deadline, c_key) = admitted(&bench.run_records(&c));
    assert_eq!(
        c_key,
        c.lease_token().expires_at_monotonic_ms(),
        "C, alone, renews nothing"
    );

    let granted_expiry = b.lease_token().expires_at_monotonic_ms();
    bench.slow_front(B, APP_B, READ_MS);
    let mark = bench.head();
    let receipt = bench
        .host
        .run_scheduled_contained_task(&b, &bench.request(LONGEST_DEADLINE_MS))
        .expect("B's run succeeds");
    let switch_ms = bench
        .host
        .policy_switch_ms_for_test(b.decision_id())
        .expect("the dispatch clock")
        .expect("B's switch time");
    let (execution, _) = bench
        .host
        .complete_scheduled_policy_run(&b, &receipt)
        .expect("B's run settles");
    assert!(
        matches!(execution.outcome, PolicyExecutionOutcome::Succeeded { .. }),
        "{:?}",
        execution.outcome
    );

    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(bench.actions(B), [ApplicationLifecycleAction::Restart]);
    assert!(
        switch_ms >= READ_MS,
        "the switch holds its read: {switch_ms}"
    );
    let events = bench.events_after(mark);
    let (startup_deadline, startup_key) = admitted(switch_records(&events, bench.id(A)));
    assert_eq!(
        startup_key, granted_expiry,
        "the startup run ran on the granted key"
    );
    assert_eq!(
        startup_deadline,
        granted_expiry - RESERVE_MS,
        "the startup run takes what is left of the key"
    );
    let (run_deadline, run_key) = admitted(&bench.run_records(&b));
    assert!(
        run_key > granted_expiry,
        "B's run holds a renewed lease that outlasts the granted one: {run_key} <= {granted_expiry}"
    );
    assert_eq!(
        run_deadline,
        granted_expiry - RESERVE_MS + switch_ms,
        "B's deadline: its grant and its own response deadline, moved on by the switch only"
    );
    assert!(
        run_deadline + RESERVE_MS <= run_key,
        "B's deadline lies inside its renewed key: {run_deadline} {run_key}"
    );
    assert_eq!(
        granted_expiry - (run_deadline - switch_ms),
        c_key - c_deadline,
        "B's deadline less the switch stands to B's grant as an instance alone's to its grant"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type() == EventType::LeaseExpired),
        "no key expired"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's "the lease granted to a member that may switch equals an instance alone's,
/// for the same run", and the ruling's "when no switch is needed nothing is renewed": B's dispatch
/// and C's, both with a run of `D_run`, hold the same time on their keys, `D_run + R`. With B's
/// own package in front, B's switch reads the front and does nothing else; no lease is renewed,
/// and B's run, like C's, holds its granted token throughout (its package admission names the
/// granted key's expiry). No application command reaches any device.
#[test]
fn mapp2d_a_member_is_granted_an_instance_alones_lease_and_renews_nothing_without_a_startup_run() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let b = bench.grant(&evaluation, TASK_B);
    let c = bench.grant(&evaluation, TASK_C);
    let b_lease = bench.remaining(b.lease_token());
    assert_eq!(
        b_lease,
        bench.remaining(c.lease_token()),
        "a member's lease equals an instance alone's"
    );
    assert_eq!(b_lease, Some(RUN_DEADLINE_MS + RESERVE_MS));
    let mark = bench.head();
    for context in [&b, &c] {
        bench.run_and_settle(context);
        let (_, key) = admitted(&bench.run_records(context));
        assert_eq!(
            key,
            context.lease_token().expires_at_monotonic_ms(),
            "{}: the granted token throughout",
            context.instance_alias()
        );
    }
    assert!(
        !bench
            .events_after(mark)
            .iter()
            .any(|event| event.event_type() == EventType::LeaseRenewed),
        "nothing is renewed"
    );
    assert_eq!(
        bench.host.switch_entries_for_test().expect("count"),
        1,
        "B's switch is reached, C's never"
    );
    for alias in [A, B, C, D] {
        assert!(bench.actions(alias).is_empty(), "no command on {alias}");
    }
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's startup run that takes its whole remaining time, and the one for which too
/// little is left. B's lease is `D_run + R` (600 020 ms), its startup package's deadline `D_s`
/// (60 000 ms). The switch's front read takes 570 000 ms: what is left of the key is less than
/// the startup run's own deadline, so its deadline is the key's expiry less the reserve; it
/// reaches its home, B's lease is renewed after it, and B's run, requested after the renewal,
/// runs on the renewed lease and settles a success. The read takes 600 000 ms: the key has no
/// more than the reserve left, too little for the startup run to start at all (its deadline would
/// not lie after now), so the switch fails at `startup_package`, caused by
/// `contained_task_deadline_unavailable`, before any stop; the run is settled failed with
/// `application_switch_failed`, the key is released with no claim left, B's switch cooldown runs,
/// nothing is renewed and no ladder starts.
#[test]
fn mapp2d_the_startup_run_takes_what_is_left_and_too_little_fails_the_switch_at_its_step() {
    const LEFT_MS: u64 = 30_000;
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let lease_id = context.lease_token().lease_id();
    let granted_expiry = context.lease_token().expires_at_monotonic_ms();
    bench.slow_front(B, APP_B, RUN_DEADLINE_MS - LEFT_MS);
    let mark = bench.head();
    bench.run_and_settle(&context);
    let events = bench.events_after(mark);
    let startup = switch_records(&events, bench.id(A));
    let (startup_deadline, startup_key) = admitted(startup.iter().copied());
    assert_eq!(startup_key, granted_expiry);
    assert_eq!(
        startup_deadline,
        granted_expiry - RESERVE_MS,
        "the startup run takes the key's whole remaining time"
    );
    let startup_completed = startup
        .iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .map(|event| event.sequence())
        .expect("B's startup run reaches its home");
    let renewed = first_sequence(&events, |event| {
        event.event_type() == EventType::LeaseRenewed && event.links().lease_id() == Some(&lease_id)
    });
    let run = bench.run_records(&context);
    let requested = first_sequence(&run, |event| event.event_type() == EventType::TaskRequested);
    assert!(
        startup_completed < renewed && renewed < requested,
        "renewed after the startup run, before the run: {startup_completed} {renewed} {requested}"
    );
    let (run_deadline, run_key) = admitted(&run);
    assert!(run_key > granted_expiry, "B's run holds its renewed lease");
    assert!(run_deadline + RESERVE_MS <= run_key);
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();

    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    bench.slow_front(B, APP_B, RUN_DEADLINE_MS);
    let mark = bench.head();
    let failed = bench.run(&context).expect_err("the switch fails");
    assert_switch_failed(
        &failed,
        B,
        ApplicationSwitchStep::StartupPackage.as_str(),
        Some(HostCode::ContainedTaskDeadlineUnavailable.as_str()),
    );
    let events = bench.events_after(mark);
    assert_eq!(
        failed_settlement(&events, context.decision_id()).as_deref(),
        Some(HostCode::ApplicationSwitchFailed.as_str())
    );
    assert!(bench.actions(A).is_empty(), "refused before any stop");
    assert!(bench.actions(B).is_empty(), "no startup run");
    assert_eq!(
        bench.host.instance_claims_for_test(B).expect("claims"),
        (None, Vec::new()),
        "the key is released and no claim is left"
    );
    assert!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown")
            .is_some(),
        "B's switch cooldown runs"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type() == EventType::LeaseRenewed),
        "nothing is renewed"
    );
    assert!(!ladder_started(&events), "no recovery ladder");
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's renewal that fails: B's run with A in front, and A's client claim queued
/// behind it. The renewal after B's startup run is asked with a token the scheduler does not
/// hold (the test-only hook), so the scheduler refuses it, `lease_mismatch`. The switch fails at
/// `lease_renewal` with that cause and B's own package; the run is settled failed with
/// `application_switch_failed` and never requested; the device is as the contract says (A
/// stopped, B started, no task run); B's key is released, its release performed, and A's queued
/// claim is granted the device afterwards; B's switch cooldown runs and no ladder starts.
#[test]
fn mapp2d_a_failed_renewal_fails_the_switch_at_lease_renewal_and_hands_the_device_on() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let lease_id = context.lease_token().lease_id();
    bench.front_until_started(B, APP_A, APP_B);
    let queued_request = bench.queue_a(74);
    bench
        .host
        .refuse_next_switch_renewal_for_test()
        .expect("the renewal is refused");
    let mark = bench.head();

    let failed = bench.run(&context).expect_err("the switch fails");

    assert_switch_failed(
        &failed,
        B,
        ApplicationSwitchStep::LeaseRenewal.as_str(),
        Some(RuntimeErrorCode::LeaseMismatch.as_str()),
    );
    assert_eq!(
        failed.outcome().value("application_id"),
        Some(&json!(APP_B)),
        "the member's own package"
    );
    let events = bench.events_after(mark);
    assert_eq!(
        failed_settlement(&events, context.decision_id()).as_deref(),
        Some(HostCode::ApplicationSwitchFailed.as_str())
    );
    assert!(
        !bench
            .run_records(&context)
            .iter()
            .any(|event| event.event_type() == EventType::TaskRequested),
        "B's run never starts"
    );
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(
        bench.actions(B),
        [ApplicationLifecycleAction::Restart],
        "B started by its startup run, no task run"
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type() == EventType::LeaseRenewed),
        "nothing is renewed"
    );
    let effects = release_effects(&events, lease_id);
    assert!(!effects.is_empty(), "B's key is released");
    assert!(
        effects
            .iter()
            .all(|effect| *effect != Some(EffectDisposition::NotPerformed)),
        "its release is performed: {effects:?}"
    );
    assert_eq!(
        bench.host.instance_claims_for_test(A).expect("claims"),
        (Some(ClaimKind::ClientLease), Vec::new()),
        "A's queued claim holds the device"
    );
    assert!(
        events.iter().any(|event| {
            event.links().request_id() == Some(&queued_request)
                && matches!(
                    event.event_type(),
                    EventType::LeaseGranted | EventType::LeaseTransferred
                )
        }),
        "A's queued claim is granted"
    );
    assert!(
        bench
            .host
            .switch_cooldown_remaining_ms_for_test(B)
            .expect("the cooldown")
            .is_some(),
        "B's switch cooldown runs"
    );
    assert!(!ladder_started(&events), "no recovery ladder");
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}

/// The work order's "while the switch and the run hold the device, another member's claim stays
/// queued through the renewal": A's client claim is queued behind B's run, with A in front. The
/// switch stops A and runs B's startup run, which hands the key back; B's lease is then renewed,
/// once, under B's run's request and the same lease id, not refused; B's run is requested and
/// completes; only then is A's claim granted the device. In order: the startup run's completion,
/// the renewal, B's run's request, its completion, A's grant.
///
/// Fix round 1 (the review's open point): the renewal's records are written under B's run's own
/// request before that request's lifecycle begins, where MAPP-2b already writes the startup
/// package's `startup_package_scheduled` (the stop's records are A's own stop request's and the
/// startup run's its own request's, under the switch's causation id). Before the lifecycle that
/// request holds exactly those four records; the forensic side's reader of a run, the task
/// evidence FL-2's gate reads with, reads the closed ledger after the mark without refusing
/// anything and reads every record of that request in the host's order.
#[test]
fn mapp2d_another_members_claim_stays_queued_through_the_renewal() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let lease_id = context.lease_token().lease_id();
    bench.front_until_started(B, APP_A, APP_B);
    let queued_request = bench.queue_a(75);
    assert_eq!(
        bench.host.instance_claims_for_test(A).expect("claims"),
        (
            Some(ClaimKind::PolicyDispatch),
            vec![ClaimKind::ClientLease]
        )
    );
    let mark = bench.head();

    bench.run_and_settle(&context);

    let events = bench.events_after(mark);
    let startup_completed = switch_records(&events, bench.id(A))
        .into_iter()
        .find(|event| event.event_type() == EventType::TaskCompleted)
        .map(PersistedEvent::sequence)
        .expect("B's startup run reaches its home");
    let renewals = events
        .iter()
        .filter(|event| event.event_type() == EventType::LeaseRenewed)
        .collect::<Vec<_>>();
    let [renewal] = renewals.as_slice() else {
        panic!("one renewal: {}", renewals.len());
    };
    assert_eq!(renewal.links().lease_id(), Some(&lease_id), "B's own lease");
    let run = bench.run_records(&context);
    let run_requested = run
        .iter()
        .find(|event| event.event_type() == EventType::TaskRequested)
        .expect("B's run is requested");
    assert_eq!(
        renewal.links().request_id(),
        run_requested.links().request_id(),
        "under B's run's request"
    );
    let run_completed =
        first_sequence(&run, |event| event.event_type() == EventType::TaskCompleted);
    let granted = first_sequence(&events, |event| {
        event.links().request_id() == Some(&queued_request)
            && matches!(
                event.event_type(),
                EventType::LeaseGranted | EventType::LeaseTransferred
            )
    });
    let order = [
        startup_completed,
        renewal.sequence(),
        run_requested.sequence(),
        run_completed,
        granted,
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "startup run, renewal, run, A's grant: {order:?}"
    );
    assert_eq!(
        bench.host.instance_claims_for_test(A).expect("claims"),
        (Some(ClaimKind::ClientLease), Vec::new())
    );
    assert!(bench.host.fatal_error().expect("health").is_none());

    // Fix round 1: the records of B's run's request before its request lifecycle (its first
    // record linked to its run): MAPP-2b's `startup_package_scheduled`, then the renewal's three.
    let run_request = *run_requested
        .links()
        .request_id()
        .expect("B's run's request");
    let of_run_request = |events: &[PersistedEvent]| {
        events
            .iter()
            .filter(|event| event.links().request_id() == Some(&run_request))
            .map(|event| (event.sequence(), event.event_type()))
            .collect::<Vec<_>>()
    };
    let written = of_run_request(&events);
    let lifecycle = first_sequence(&events, |event| {
        event.links().request_id() == Some(&run_request)
            && event.links().run_id() == Some(&context.run_id())
    });
    let before_lifecycle = written
        .iter()
        .filter(|(sequence, _)| *sequence < lifecycle)
        .map(|(_, kind)| *kind)
        .collect::<Vec<_>>();
    assert_eq!(
        before_lifecycle,
        [
            EventType::RuntimeLifecycleObserved,
            EventType::SchedulerAdmitted,
            EventType::LeaseTransitionIntent,
            EventType::LeaseRenewed,
        ],
        "before B's run's lifecycle: its startup_package_scheduled, then the renewal"
    );
    // The forensic side's reader of a run (the task evidence FL-2's gate reads with) reads the
    // closed ledger after `mark`: it refuses nothing, and it reads every record of B's run's
    // request, those before its lifecycle included, in the host's order.
    let Bench { host, parts } = bench;
    host.close().expect("close the device group host");
    let report = task_evidence(parts.root.path(), mark);
    assert!(
        report.failures.is_empty(),
        "the reader refuses nothing: {:?}",
        report.failures
    );
    assert!(report.corrupt_tail.is_none(), "{:?}", report.corrupt_tail);
    assert_eq!(
        of_run_request(&report.page.events),
        written,
        "the reader reads every record of B's run's request, in the host's order"
    );
}

/// The coordinator's ruling on the stale token (11-14): after a run whose lease was renewed, the
/// device's lease is really gone, whether the run succeeded or failed at a step after its
/// renewal, on both run paths. Fix round 1: every switch here takes time. B's first front read
/// moves the Runtime clock on by `SWITCH_READ_MS` (5 000 ms, [`Bench::slow_front`]), after the
/// grant and before the renewal, so the renewal (`D_run + R` from its own instant, as the grant
/// is from its) replaces the token with one that expires at least that much later.
///
/// - A policy run of B with A in front and A's client claim queued: once completed and settled
///   (its package admission names a key that expires at least `SWITCH_READ_MS` after the granted
///   one), and once failed right after its switch (the test-only hook, `backend_operation_failed`,
///   before the run's request lifecycle, so its failed settlement releases its key). Each time
///   B's lease was renewed, every `lease.released` record of it is performed (none written as not
///   performed while the lease lived on), the scheduler holds no lease of B's (the device's lease
///   is A's client claim) and A's queued claim is granted at once.
/// - The defect's shape: the same failed policy run, with its failed settlement's release made to
///   take the granted token (the test-only hook
///   `release_failed_policy_run_with_granted_token_for_test`). Its one `lease.released` is written
///   not performed, B's renewed lease still holds the device with A's claim queued behind it, and
///   A's claim is not granted.
/// - A direct run of B with A in front: once completed (its package admission names a key that
///   expires at least `SWITCH_READ_MS` after the one its startup run ran on, the granted one), and
///   once failed right after its switch (the same hook, its receipt failed with the code). Each
///   time its lease was renewed, its releases are performed, the scheduler holds no lease at all
///   on the device, and A's claim asked then is granted at once.
///
/// Why a build that releases with the granted token fails here: once the renewal has replaced it,
/// the granted token no longer validates (the scheduler compares a token whole, and now the two
/// differ in their expiry), so a release with it meets `lease_mismatch`, which the lease end reads
/// as a lease already removed (`cleanup_token_guarded`, `host/lease.rs`): its `lease.released` is
/// written not performed and the renewed lease lives on to its expiry. The first assertion to fail
/// is "every release performed" (line 2753 on the policy path, 2909 on the direct path); "no lease
/// of B's, A's queued claim holds the device" (2759), "A's queued claim is granted" (2764), "no
/// lease on the device" (2915) and "A's claim is granted at once" (2924) fail after it. The block
/// "the defect's shape" (lines 2787 to 2849) shows it on the policy path: there these assertions'
/// opposites hold (2825, 2830, 2838). Without the switch's time the clock did not move between the
/// grant and the renewal, the two tokens were equal in all five members, and such a build passed.
#[test]
fn mapp2d_a_renewed_lease_is_gone_after_its_run_and_the_next_claim_is_granted_at_once() {
    const SWITCH_READ_MS: u64 = 5_000;
    for fail_after_switch in [false, true] {
        let bench = Bench::standard();
        let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
        let context = bench.grant(&evaluation, TASK_B);
        let lease_id = context.lease_token().lease_id();
        let granted_expiry = context.lease_token().expires_at_monotonic_ms();
        bench.slow_front(B, APP_B, SWITCH_READ_MS);
        let queued_request = bench.queue_a(76);
        if fail_after_switch {
            bench
                .host
                .fail_next_run_after_switch_for_test()
                .expect("the run fails after its switch");
        }
        let mark = bench.head();
        if fail_after_switch {
            let failed = bench
                .run(&context)
                .expect_err("the run fails after its switch");
            assert_eq!(
                failed.code(),
                RuntimeErrorCode::BackendOperationFailed.as_str(),
                "{failed:?}"
            );
            assert_eq!(
                failed_settlement(&bench.events_after(mark), context.decision_id()).as_deref(),
                Some(RuntimeErrorCode::BackendOperationFailed.as_str()),
                "the run settled failed"
            );
        } else {
            bench.run_and_settle(&context);
            let (_, run_key) = admitted(&bench.run_records(&context));
            assert!(
                run_key >= granted_expiry + SWITCH_READ_MS,
                "policy: the renewed token expires later than the granted one: {run_key} < \
                 {granted_expiry} + {SWITCH_READ_MS}"
            );
        }
        let events = bench.events_after(mark);
        assert!(
            events.iter().any(|event| {
                event.event_type() == EventType::LeaseRenewed
                    && event.links().lease_id() == Some(&lease_id)
            }),
            "policy, failed {fail_after_switch}: B's lease was renewed"
        );
        let effects = release_effects(&events, lease_id);
        assert!(
            !effects.is_empty(),
            "policy, failed {fail_after_switch}: B's lease is released"
        );
        assert!(
            effects
                .iter()
                .all(|effect| *effect != Some(EffectDisposition::NotPerformed)),
            "policy, failed {fail_after_switch}: every release performed: {effects:?}"
        );
        assert_eq!(
            bench.host.instance_claims_for_test(A).expect("claims"),
            (Some(ClaimKind::ClientLease), Vec::new()),
            "policy, failed {fail_after_switch}: no lease of B's, A's queued claim holds the device"
        );
        assert!(
            events.iter().any(|event| {
                event.links().request_id() == Some(&queued_request)
                    && matches!(
                        event.event_type(),
                        EventType::LeaseGranted | EventType::LeaseTransferred
                    )
            }),
            "policy, failed {fail_after_switch}: A's queued claim is granted"
        );
        assert_eq!(
            bench
                .host
                .switch_cooldown_entries_for_test()
                .expect("the cooldowns"),
            0,
            "policy, failed {fail_after_switch}: the switch did not fail"
        );
        assert!(!ladder_started(&events), "no recovery ladder");
        assert!(bench.host.fatal_error().expect("health").is_none());
        bench.close();
    }

    // The defect's shape: the same failed policy run, its failed settlement's release made to take
    // the granted token (the test-only hook), as a build that released with it would.
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    let context = bench.grant(&evaluation, TASK_B);
    let lease_id = context.lease_token().lease_id();
    bench.slow_front(B, APP_B, SWITCH_READ_MS);
    let queued_request = bench.queue_a(77);
    bench
        .host
        .fail_next_run_after_switch_for_test()
        .expect("the run fails after its switch");
    bench
        .host
        .release_failed_policy_run_with_granted_token_for_test()
        .expect("the release takes the granted token");
    let mark = bench.head();
    let failed = bench
        .run(&context)
        .expect_err("the run fails after its switch");
    assert_eq!(
        failed.code(),
        RuntimeErrorCode::BackendOperationFailed.as_str(),
        "{failed:?}"
    );
    let events = bench.events_after(mark);
    assert_eq!(
        failed_settlement(&events, context.decision_id()).as_deref(),
        Some(RuntimeErrorCode::BackendOperationFailed.as_str()),
        "granted token: the run settled failed"
    );
    assert!(
        events.iter().any(|event| {
            event.event_type() == EventType::LeaseRenewed
                && event.links().lease_id() == Some(&lease_id)
        }),
        "granted token: B's lease was renewed"
    );
    assert_eq!(
        release_effects(&events, lease_id),
        vec![Some(EffectDisposition::NotPerformed)],
        "granted token: its one release is written as not performed"
    );
    assert_eq!(
        bench.host.instance_claims_for_test(A).expect("claims"),
        (
            Some(ClaimKind::PolicyDispatch),
            vec![ClaimKind::ClientLease]
        ),
        "granted token: B's renewed lease still holds the device, A's claim waits behind it"
    );
    assert!(
        !events.iter().any(|event| {
            event.links().request_id() == Some(&queued_request)
                && matches!(
                    event.event_type(),
                    EventType::LeaseGranted | EventType::LeaseTransferred
                )
        }),
        "granted token: A's queued claim is not granted"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();

    for fail_after_switch in [false, true] {
        let bench = Bench::standard();
        bench.slow_front(B, APP_B, SWITCH_READ_MS);
        if fail_after_switch {
            bench
                .host
                .fail_next_run_after_switch_for_test()
                .expect("the run fails after its switch");
        }
        let holder = IdentifierIssuer::new().expect("identifier issuer");
        let mark = bench.head();
        let direct = bench.send(
            RuntimeOperation::run_contained_task(
                B,
                holder.mint_holder_id().expect("holder"),
                bench.request(RUN_DEADLINE_MS),
            ),
            92,
        );
        if fail_after_switch {
            assert_eq!(direct.state(), RuntimeReceiptState::Failed, "{direct:?}");
            assert_eq!(
                direct.outcome().code().as_str(),
                RuntimeErrorCode::BackendOperationFailed.as_str(),
                "{direct:?}"
            );
        } else {
            assert_eq!(direct.state(), RuntimeReceiptState::Completed, "{direct:?}");
        }
        let events = bench.events_after(mark);
        let renewed = events
            .iter()
            .find(|event| event.event_type() == EventType::LeaseRenewed)
            .unwrap_or_else(|| panic!("direct, failed {fail_after_switch}: the lease renewed"));
        let lease_id = renewed
            .links()
            .lease_id()
            .copied()
            .expect("the renewed lease");
        if !fail_after_switch {
            // The startup run ran on the granted key, the run on the renewed one.
            let (_, granted_key) = admitted(switch_records(&events, bench.id(A)));
            let (_, run_key) = admitted(
                events
                    .iter()
                    .filter(|event| event.sequence() > renewed.sequence()),
            );
            assert!(
                run_key >= granted_key + SWITCH_READ_MS,
                "direct: the renewed token expires later than the granted one: {run_key} < \
                 {granted_key} + {SWITCH_READ_MS}"
            );
        }
        let effects = release_effects(&events, lease_id);
        assert!(
            !effects.is_empty(),
            "direct, failed {fail_after_switch}: the run's lease is released"
        );
        assert!(
            effects
                .iter()
                .all(|effect| *effect != Some(EffectDisposition::NotPerformed)),
            "direct, failed {fail_after_switch}: every release performed: {effects:?}"
        );
        assert_eq!(
            bench.host.instance_claims_for_test(B).expect("claims"),
            (None, Vec::new()),
            "direct, failed {fail_after_switch}: no lease on the device"
        );
        let acquired = bench.send(
            RuntimeOperation::acquire_lease(A, holder.mint_holder_id().expect("holder")),
            93,
        );
        assert!(
            matches!(acquired.result(), Some(RuntimeResult::LeaseGranted { .. })),
            "direct, failed {fail_after_switch}: A's claim is granted at once: {acquired:?}"
        );
        assert!(bench.host.fatal_error().expect("health").is_none());
        bench.close();
    }
}

/// Replaces MAPP-2b's `mapp2b_known_gap_a_member_that_cannot_fit_is_selected_and_refused_every_round`
/// (Workflow #437 MAPP-2d): the same member with the same deadlines (B's startup package 60 000
/// ms and its run 1 800 000 ms, whose sum MAPP-2b's bound refused in every round) is selected and
/// wins the device's one pick while A waits `device_group_waiting`; its admission is granted, with
/// no dispatch refusal, a lease of `D_run + R`; its run switches (A stopped, B's startup run) and
/// completes, and the dispatch settles a success. B's direct run with the same deadline, which
/// MAPP-2b refused before its claim, switches and completes too.
#[test]
fn mapp2d_the_member_that_could_not_fit_is_dispatched_and_runs() {
    let bench = Bench::standard();
    let evaluation = bench.evaluate(POLICY_NOW_UNIX_MS);
    assert!(
        dispatches(&evaluation, TASK_B),
        "B's task is selected: {evaluation:#?}"
    );
    assert!(!dispatches(&evaluation, TASK_A), "A's task is not");
    let (reasons, _) = decision_of(&evaluation, TASK_A, A);
    assert!(
        reasons.iter().any(|reason| reason.outcome.code().as_str()
            == PolicyCode::DeviceGroupWaiting.code().as_str()),
        "A waits for the device: {reasons:#?}"
    );
    let mark = bench.head();
    let context = bench.grant_with(&evaluation, TASK_B, LONGEST_DEADLINE_MS);
    assert_eq!(
        bench.remaining(context.lease_token()),
        Some(LONGEST_DEADLINE_MS + RESERVE_MS)
    );
    bench.front_until_started(B, APP_A, APP_B);
    let receipt = bench
        .host
        .run_scheduled_contained_task(&context, &bench.request(LONGEST_DEADLINE_MS))
        .expect("B's run succeeds");
    let (execution, _) = bench
        .host
        .complete_scheduled_policy_run(&context, &receipt)
        .expect("B's run settles");
    assert!(
        matches!(execution.outcome, PolicyExecutionOutcome::Succeeded { .. }),
        "{:?}",
        execution.outcome
    );
    assert!(
        !bench.events_after(mark).iter().any(|event| matches!(
            event.payload(),
            EventPayload::Policy(PolicyPayload::DispatchRejected(_))
        )),
        "no dispatch refusal"
    );
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(bench.actions(B), [ApplicationLifecycleAction::Restart]);
    assert!(
        bench
            .run_records(&context)
            .iter()
            .any(|event| event.event_type() == EventType::TaskCompleted),
        "B's run completes"
    );
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();

    let bench = Bench::standard();
    bench.front_until_started(B, APP_A, APP_B);
    let holder = IdentifierIssuer::new().expect("identifier issuer");
    let direct = bench.send(
        RuntimeOperation::run_contained_task(
            B,
            holder.mint_holder_id().expect("holder"),
            bench.request(LONGEST_DEADLINE_MS),
        ),
        94,
    );
    assert_eq!(direct.state(), RuntimeReceiptState::Completed, "{direct:?}");
    assert_eq!(bench.actions(A), [ApplicationLifecycleAction::Stop]);
    assert_eq!(bench.actions(B), [ApplicationLifecycleAction::Restart]);
    assert!(bench.host.fatal_error().expect("health").is_none());
    bench.close();
}
