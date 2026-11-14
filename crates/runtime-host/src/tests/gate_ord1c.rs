// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 ORD-1c (register §1.373; `contracts/scheduling/README.md`, "Order within one
//! device"; `contracts/device-group.md`, "The member in front"; the coordinator's rulings 1 to 4
//! on board #437, not the owner's words): the host passes the order within one device to the
//! evaluator: the member it holds in front of each device of several, when each waiting pair's wait
//! began, and `scheduling.device_group.max_queue_wait_ms`.
//!
//! Fixture instances A and B are one device group of several (`device.one`) and C is alone on its
//! own device, each with its own application and the neutral startup package, whose one step
//! restarts the application and whose home is the fake device's first frame; every run here runs
//! that package as its task too, and the fake device's foreground answers each member's own
//! application, so a member's switch finds its own package in front and its steps end there. The
//! load refuses every group of several, so the host is handed the member → device key map and the
//! group's name as the assembly builds them. The catalog is the fixture catalog's generic task,
//! declared V2 (so a task may declare `max_queue_wait_ms`), one or two tasks per member, each due
//! at every evaluation and declaring an expected duration of one minute; the configuration turns
//! the expected runtime off, so the duration a dispatch reserves (`E`) is that minute throughout.
//! A manual Runtime clock moves only by the kernel's waits and the test's own steps; nothing
//! waits on the wall clock. Every expected value is the contract's or the coordinator's ruling;
//! every comparison is within one host but t8's, which compares two hosts started alike with the
//! same instance ids by normalised payloads, never by an identity. Neutral names only.
//!
//! Fix round 1 (the review on board #437): t9 places a switch between an evaluation's read of the
//! members in front and its staging, through the evaluation's own test hook and two channels (no
//! sleep); t10 to t14 pin each place outside the switch's own result that forgets the member in
//! front (a Runtime-level failure of the switch, a stop or a failed start of a member, a ladder
//! rung that fails after it stopped the others, an emulator restart by a rung, the idle action
//! closing the application). The ladder's and the idle action's tests wait for their worker's end
//! as their own gates do.
//!
//! Fix round 2 (the review of fix round 1 and CI): t15 pins that a task step whose restart command
//! fails forgets the member in front; t14's fixture carries the `idle` section's keys and registers
//! A with its emulator running; t8 replaces the machine's samples inside the performance summary
//! records only.
//!
//! Fix round 3 (CI on the head of fix round 2): t14's start moves the manual clock by one
//! millisecond inside A's first capture, its start preparation's, as a real open takes time (A,
//! bound to its emulator, is a device self-checked policy instance: the start withdraws its
//! availability and its passing self-check republishes the seed, which the fact store refused at
//! the withdrawal's instant). t15's runs take a first frame that shows no page of the package, so
//! the package's restart step runs.

use super::gate_373_nav3a::StandardRoot;
use super::instance_workers::wait_until;
use super::*;
use crate::codes::HostCode;
use crate::idle_action::IdlePlan;
use crate::policy_host::QUEUE_ORDER_OFF_FOR_TEST;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, EmulatorInstanceAction,
    FactScalar, RecoveryDrillMode, RecoveryRung, RecoveryRungOutcome, RuntimeConfigManifest,
    RuntimeFactScope, RuntimeLifecyclePhase, RuntimePayload, SchedulingPauseReason,
    SchedulingPauseScope, config_parameters_fact_key,
};
use actingcommand_policy::{DeviceGroupWait, PolicyCode, PolicyEvaluation};
use actingcommand_runtime_config::scheduling::{
    DEVICE_GROUP_MAX_QUEUE_WAIT_MS, DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS,
    EXPECTED_RUNTIME_ENABLED,
};
use actingcommand_runtime_config::{
    ConfigCode, DefaultSource, DefaultsFile, IdleAction, KeyRange, Settable, find_row,
    instance_key, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

const A: &str = POLICY_INSTANCE_ALIAS;
const B: &str = POLICY_INSTANCE_ALIAS_B;
/// An instance alone on its own device, beside the group.
const C: &str = "fixture-instance-c";
const TASK_A: &str = "fixture.member-a";
const TASK_B: &str = "fixture.member-b";
/// B's second task, after `TASK_B` in B's own order.
const TASK_B2: &str = "fixture.member-b2";
const TASK_C: &str = "fixture.lone-c";
const GROUP: &str = "device.one";
const SEED: u64 = 37;
const APP_A: &str = "neutral.member.a";
const APP_B: &str = "neutral.member.b";
const APP_C: &str = "neutral.member.c";
/// The fake device's first pixel before any input: the startup package's home.
const HOME: [u8; 3] = [255, 0, 0];
/// A first pixel that shows no page of the startup package (the fake device's unknown frame).
const NO_PAGE: [u8; 3] = [1, 2, 3];
const PACKAGE_ID: &str = "neutral.test.startup";
/// The response deadline of every run.
const RUN_DEADLINE_MS: u64 = 600_000;
/// The response deadline of the startup package.
const STARTUP_DEADLINE_MS: u64 = 60_000;
const MINUTE_MS: u64 = 60_000;
/// Every task's declared expected duration: with the expected runtime off, the time each of its
/// dispatches reserves (`E`).
const E_MS: u64 = MINUTE_MS;
/// `scheduling.device_group.max_queue_wait_ms`'s default.
const DEFAULT_LIMIT_MS: u64 = 1_800_000;
/// `scheduling.device_group.switch_failure_cooldown_ms` as the fixture's file sets it.
const COOLDOWN_MS: u64 = MINUTE_MS;
/// The first evaluation of every test: one minute after the clock's start, so it comes after the
/// records of a request the test sends before it.
const T0: u64 = POLICY_NOW_UNIX_MS + MINUTE_MS;
/// How long a test thread waits for a step another thread takes; only a broken run waits it out.
const GATE_WAIT: Duration = Duration::from_secs(60);
/// The game and server of the policy identity t14 gives A, and the standard-pack root's.
const IDLE_GAME: &str = "neutral";
const IDLE_SERVER: &str = "test";
/// A's next work in t14: four hours away, past `idle.min_gap_ms`'s default.
const IDLE_GAP_MS: u64 = 4 * 60 * MINUTE_MS;
/// The value key of the device group's wait word, and the order's values.
const DEVICE_GROUP_WAIT: &str = "device_group_wait";
const LIMIT_MS: &str = "limit_ms";
const REMAINING_MS: &str = "remaining_ms";
const REQUIRED_MS: &str = "required_ms";

// ---- The fixture.

/// One task `id` of `instance` with `priority`, due at every evaluation (a local interval clock of
/// 1 ms), from the fixture catalog's generic task, declaring `E_MS` and, when given, its longest
/// wait in its device's queue.
fn member_task(
    base: &Value,
    id: &str,
    instance: &str,
    priority: u32,
    limit_ms: Option<u64>,
) -> Value {
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
    spec["expected_duration_ms"] = json!(E_MS);
    spec["instance_overrides"] = json!([]);
    spec["produces"] = json!([]);
    spec["on_failure"]["escalation_threshold"] = json!(1000);
    spec["loop_budget"] = json!({
        "daily_limit": 48,
        "window_iteration_limit": 48,
        "max_runtime_ms": 1_800_000
    });
    if let Some(limit_ms) = limit_ms {
        spec["max_queue_wait_ms"] = json!(limit_ms);
    }
    spec
}

/// TASK_A on A (priority 300, so it comes first in the allocation order), TASK_B on B (200).
fn one_task_each(base: &Value) -> Vec<Value> {
    vec![
        member_task(base, TASK_A, A, 300, None),
        member_task(base, TASK_B, B, 200, None),
    ]
}

/// TASK_A on A (300), TASK_B (200) and TASK_B2 (150) on B, none declaring a limit.
fn two_tasks_of_b(base: &Value) -> Vec<Value> {
    vec![
        member_task(base, TASK_A, A, 300, None),
        member_task(base, TASK_B, B, 200, None),
        member_task(base, TASK_B2, B, 150, None),
    ]
}

/// TASK_A on A (300), TASK_B on B (200) declaring a limit of two minutes, and TASK_B2 on B (150)
/// declaring none.
fn one_declared_limit(base: &Value) -> Vec<Value> {
    vec![
        member_task(base, TASK_A, A, 300, None),
        member_task(base, TASK_B, B, 200, Some(2 * MINUTE_MS)),
        member_task(base, TASK_B2, B, 150, None),
    ]
}

/// TASK_A on A (300) declaring a limit of two minutes, TASK_B on B (200) declaring none.
fn a_declares_two_minutes(base: &Value) -> Vec<Value> {
    vec![
        member_task(base, TASK_A, A, 300, Some(2 * MINUTE_MS)),
        member_task(base, TASK_B, B, 200, None),
    ]
}

/// TASK_A on A and TASK_C on C, for the instances alone on their devices.
fn lone_tasks(base: &Value) -> Vec<Value> {
    vec![
        member_task(base, TASK_A, A, 300, None),
        member_task(base, TASK_C, C, 100, None),
    ]
}

/// The fixture catalog declared V2 (each timeline event with its explicit unbounded validity) with
/// `tasks` from its generic task, under its game-wide activity profile (08:00 to 22:00 UTC) with no
/// interval between dispatches.
fn sources(tasks: fn(&Value) -> Vec<Value>) -> CatalogSources {
    let mut sources = policy_sources(1);
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
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("V2 fixture bytes");
    }
    let mut document: Value = serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks");
    let base = document["tasks"][0].clone();
    document["tasks"] = Value::Array(tasks(&base));
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

/// One parameter of the fixture's configuration file.
fn parameter(key: &str, value: FactScalar, source: ConfigParameterSource) -> ConfigParameter {
    ConfigParameter {
        key: key.to_owned(),
        value,
        source,
        settable: false,
    }
}

/// The configuration in effect: the expected runtime off, the switch cooldown `COOLDOWN_MS`, and
/// `scheduling.device_group.max_queue_wait_ms` at the spec's limit by the file, else at its
/// default; with the spec's `idle_close_a`, A's own `idle.action` `close_app` (`a`, A's id) and the
/// `idle` section's keys as the assembly renders them from the shipped defaults file. Fix round 2:
/// the start's check of A's own idle keys reads each member A does not name from the section's key
/// in the configuration, not from the defaults file, so without those rows the start refused
/// `idle.min_gap_ms` (`config_value_invalid`, `wrong_type`: no value; CI's first run of t14 showed
/// the code at `record_runtime_config`, the key is the check's first read).
fn manifest(spec: Spec, a: InstanceId) -> RuntimeConfigManifest {
    let limit = match spec.limit_ms {
        Some(limit_ms) => parameter(
            DEVICE_GROUP_MAX_QUEUE_WAIT_MS,
            FactScalar::DurationMs(limit_ms),
            ConfigParameterSource::Explicit,
        ),
        None => parameter(
            DEVICE_GROUP_MAX_QUEUE_WAIT_MS,
            FactScalar::DurationMs(DEFAULT_LIMIT_MS),
            ConfigParameterSource::Default,
        ),
    };
    let mut parameters = vec![
        parameter(
            EXPECTED_RUNTIME_ENABLED,
            FactScalar::Boolean(false),
            ConfigParameterSource::Explicit,
        ),
        parameter(
            DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS,
            FactScalar::DurationMs(COOLDOWN_MS),
            ConfigParameterSource::Explicit,
        ),
        limit,
    ];
    if spec.idle_close_a {
        let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
        let section = actingcommand_runtime_config::idle::SECTION;
        parameters.extend(
            render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
                .expect("every key resolves")
                .into_iter()
                .filter(|row| row.key.split('.').next() == Some(section)),
        );
        parameters.push(parameter(
            &instance_key(
                &crate::failure_identity::identifier_text(&a),
                actingcommand_runtime_config::idle::ACTION,
            ),
            FactScalar::String(IdleAction::CloseApp.as_str().to_owned()),
            ConfigParameterSource::Explicit,
        ));
    }
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// What a host of the fixture is started with.
#[derive(Clone, Copy)]
struct Spec {
    /// The catalog's tasks, from the fixture catalog's generic task.
    tasks: fn(&Value) -> Vec<Value>,
    /// Whether A and B are one device group of several (C is alone in any case).
    grouped: bool,
    /// `scheduling.device_group.max_queue_wait_ms` as the file sets it; its default when `None`.
    limit_ms: Option<u64>,
    /// Whether B has the startup package (A and C have it in any case).
    startup_b: bool,
    /// Whether A's own `idle.action` is `close_app`, with a standard-pack root whose game declares
    /// A's application for the server `IDLE_SERVER` (no instance names an idle action otherwise).
    idle_close_a: bool,
}

/// The fixture's usual host: A and B one device group of several with one task each, the default
/// limit, every instance with the startup package, no idle action.
const GROUPED: Spec = Spec {
    tasks: one_task_each,
    grouped: true,
    limit_ms: None,
    startup_b: true,
    idle_close_a: false,
};

/// A standard-pack root under `root` whose game `IDLE_GAME` lists no package and declares A's
/// application for the server `IDLE_SERVER` (`applications.json`), as the idle action reads it.
fn idle_packs(root: &TempDir) -> PathBuf {
    let packs = StandardRoot::new(root.path());
    packs.write_bundle();
    let declared = json!({"servers": {IDLE_SERVER: {"application_id": APP_A}}});
    fs::write(
        packs.root.join(IDLE_GAME).join("applications.json"),
        serde_json::to_vec_pretty(&declared).expect("applications JSON"),
    )
    .expect("write the declared applications");
    packs.root
}

/// Everything a host of the fixture is started from, kept so a restart starts the same one.
struct Parts {
    clock: Arc<ManualRuntimeClock>,
    facts: EvaluationFacts,
    resources: EvaluationResources,
    package: PathBuf,
    ids: BTreeMap<&'static str, InstanceId>,
    states: BTreeMap<&'static str, Arc<FakeState>>,
    spec: Spec,
    root: TempDir,
}

impl Parts {
    /// A host of A, B and C, A and B one device group of several when the spec says so, each with
    /// its own application and the startup package (B's as the spec says), on the manual Runtime
    /// clock, the catalog of [`sources`] active.
    fn start_host(&self) -> RuntimeHost {
        let ids = &self.ids;
        let entries = [A, B, C]
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
            .and_then(|request| request.with_response_deadline_ms(STARTUP_DEADLINE_MS))
            .expect("startup package request");
        let mut host_config = config(&self.root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                self.facts.clone(),
                self.resources.clone(),
            ))
            .with_runtime_clock(self.clock.clone())
            .with_startup_packages(
                [A, B, C]
                    .into_iter()
                    .filter(|alias| self.spec.startup_b || *alias != B)
                    .map(|alias| (alias.to_owned(), startup.clone()))
                    .collect(),
            )
            .with_config_manifest(manifest(self.spec, ids[A]));
        if self.spec.idle_close_a {
            host_config = host_config.with_standard_packs_root(Some(idle_packs(&self.root)));
            // Fix round 3: A, bound to its emulator, is a device self-checked policy instance. The
            // start withdraws its availability (`backend_selfcheck:unchecked`) at the clock, then
            // prepares it; the preparation's open takes A's first frame, and once both opens are
            // recorded the passing self-check republishes the seed at the clock. On a clock that
            // did not move between the two the fact store refused the seed as not newer than the
            // withdrawal, and the start failed. A's first frame takes the clock's millisecond, as a
            // real open takes time.
            let clock = Arc::clone(&self.clock);
            *self.states[A].capture_hook.lock().expect("capture hook") =
                Some(Box::new(move || clock.advance(1)));
        }
        if self.spec.grouped {
            let device = ids[A].min(ids[B]);
            host_config = host_config
                .with_device_keys(BTreeMap::from([(ids[A], device), (ids[B], device)]))
                .with_device_group_names(BTreeMap::from([
                    (A.to_owned(), GROUP.to_owned()),
                    (B.to_owned(), GROUP.to_owned()),
                ]));
        }
        let host = RuntimeHost::start(host_config, Arc::new(FakeProvider::from_entries(entries)))
            .expect("start the fixture host");
        host.activate_policy_catalog(&sources(self.spec.tasks))
            .expect("activate the catalog");
        host
    }
}

/// A fresh instance id for each of A, B and C.
fn fresh_ids() -> BTreeMap<&'static str, InstanceId> {
    [A, B, C]
        .into_iter()
        .map(|alias| (alias, instance_id()))
        .collect()
}

/// A host of the fixture at the fixture's noon, and its parts.
struct Bench {
    host: RuntimeHost,
    parts: Parts,
}

impl Bench {
    fn start(spec: Spec) -> Self {
        Self::start_with(fresh_ids(), spec)
    }

    /// A host of the fixture whose instances have `ids`.
    fn start_with(ids: BTreeMap<&'static str, InstanceId>, spec: Spec) -> Self {
        let root = TempDir::new().expect("tempdir");
        let package = startup_package(&root);
        let states = [(A, APP_A), (B, APP_B), (C, APP_C)]
            .into_iter()
            .map(|(alias, application)| {
                let state = Arc::new(FakeState::default());
                state.observed_open.store(true, Ordering::Release);
                state.physical_task_geometry.store(true, Ordering::Release);
                *state.application_id.lock().expect("fake application id") =
                    Some(application.to_owned());
                // Fix round 2: with an idle action, A is emulator-controlled from the start, so it
                // registers with its emulator running; the idle check acts only on an instance
                // whose emulator the Runtime read running.
                if spec.idle_close_a && alias == A {
                    state.emulator_control.store(true, Ordering::Release);
                }
                (alias, state)
            })
            .collect::<BTreeMap<_, _>>();
        let mut facts = policy_facts();
        facts.outcomes.clear();
        for (alias, server, host) in [
            (B, "fixture-server-b", "fixture-host-b"),
            (C, "fixture-server-c", "fixture-host-c"),
        ] {
            facts.instances.push(InstanceSnapshot {
                instance_id: alias.to_owned(),
                server_id: server.to_owned(),
                game_id: "fixture-game-a".to_owned(),
                host_id: host.to_owned(),
                available: true,
                capability_operation_ids: vec!["operation.observe".to_owned()],
                preferred_task_ids: Vec::new(),
            });
        }
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
            spec,
            root,
        };
        Self {
            host: parts.start_host(),
            parts,
        }
    }

    /// The same host closed and started again on the same root, devices and clock.
    fn restart(self) -> Self {
        let Self { host, parts } = self;
        host.close().expect("close the fixture host");
        Self {
            host: parts.start_host(),
            parts,
        }
    }

    fn clock(&self) -> &ManualRuntimeClock {
        &self.parts.clock
    }

    fn sample_capacity(&self) {
        let sample = self
            .host
            .capacity_sampler_for_test()
            .expect("capacity owner");
        sample().expect("capacity sample");
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

    /// The run request of every run: the startup package with `RUN_DEADLINE_MS`.
    fn request(&self) -> ContainedTaskRequest {
        ContainedTaskRequest::new(self.parts.package.to_string_lossy().into_owned())
            .and_then(|request| request.with_response_deadline_ms(RUN_DEADLINE_MS))
            .expect("the run's request")
    }

    /// `task_id`'s dispatch in `evaluation`, admitted and granted.
    fn grant(&self, evaluation: &PolicyEvaluation, task_id: &str) -> Box<PolicyRunContext> {
        let (intent, chain) = intent_of(evaluation, task_id);
        record_policy_approval(&self.host, &intent);
        self.sample_capacity();
        match self
            .host
            .admit_scheduled_policy_dispatch(
                &intent,
                &chain,
                &policy_context(&self.host, &intent),
                &self.request(),
            )
            .unwrap_or_else(|error| panic!("admit {task_id}: {error}"))
        {
            PolicyDispatchAdmission::Granted { context } => context,
            other => panic!("{task_id}'s dispatch is granted: {other:?}"),
        }
    }

    /// The granted run of `context` run and settled a success.
    fn run_and_settle(&self, context: &PolicyRunContext) {
        let receipt = self
            .host
            .run_scheduled_contained_task(context, &self.request())
            .unwrap_or_else(|error| panic!("the run succeeds: {error:?}"));
        let (execution, _) = self
            .host
            .complete_scheduled_policy_run(context, &receipt)
            .expect("the run settles");
        assert!(
            matches!(execution.outcome, PolicyExecutionOutcome::Succeeded { .. }),
            "{:?}",
            execution.outcome
        );
    }

    /// `task_id`'s dispatch in `evaluation` admitted, run and settled.
    fn dispatch(&self, evaluation: &PolicyEvaluation, task_id: &str) {
        let context = self.grant(evaluation, task_id);
        self.run_and_settle(&context);
    }

    /// A request on `connection` processed as a client's.
    fn send(&self, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
        send_on(&self.host, operation, connection)
    }

    /// A direct run of the package on `alias` (a run a person starts), on `connection`.
    fn direct_run(&self, alias: &str, connection: u64) -> RuntimeReceipt {
        let holder = IdentifierIssuer::new().expect("identifier issuer");
        self.send(
            RuntimeOperation::run_contained_task(
                alias,
                holder.mint_holder_id().expect("holder"),
                self.request(),
            ),
            connection,
        )
    }

    /// An `ApplicationLifecycle` `launch` of `alias`, on `connection`.
    fn launch(&self, alias: &str, connection: u64) -> RuntimeReceipt {
        lifecycle(
            &self.host,
            alias,
            ApplicationLifecycleAction::Launch,
            connection,
        )
    }

    /// An `ApplicationLifecycle` `stop` of `alias`, on `connection`.
    fn stop(&self, alias: &str, connection: u64) -> RuntimeReceipt {
        lifecycle(
            &self.host,
            alias,
            ApplicationLifecycleAction::Stop,
            connection,
        )
    }

    /// Every application command that reached `alias`'s device.
    fn actions(&self, alias: &str) -> Vec<ApplicationLifecycleAction> {
        self.parts.states[alias]
            .application_actions
            .lock()
            .expect("fake application actions")
            .clone()
    }

    /// `alias`'s foreground answers `before` until its first application command, then `own`.
    fn front_until_started(&self, alias: &str, before: &'static str, own: &'static str) {
        *self.parts.states[alias]
            .foreground_script
            .lock()
            .expect("foreground script") = Some(Arc::new(move |_: usize, commands: usize| {
            Some(if commands == 0 { before } else { own }.to_owned())
        }));
    }

    /// `alias`'s frames show no page of the startup package until the next application command
    /// reaches its device, and the package's home from then on: a task's run takes its first frame
    /// first and runs the package's restart step only on a frame that shows no page of the
    /// package (the kernel's `ApplicationEntryRun::AfterInitialFrame`).
    fn no_page_until_next_command(&self, alias: &str) {
        let state = &self.parts.states[alias];
        let before = state.application_count.load(Ordering::Acquire);
        let device = Arc::downgrade(state);
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, _: usize| {
                let commands = device.upgrade().map_or(before, |live| {
                    live.application_count.load(Ordering::Acquire)
                });
                if commands == before {
                    vec![(0, NO_PAGE)]
                } else {
                    Vec::new()
                }
            }));
    }

    /// A person's device drill of `alias`'s recovery ladder (`DrillRecoveryLadder`, mode
    /// `device`), on `connection`, waited out as the drill's own gate waits for it (its
    /// `recovery_ladder_finished` recorded and its key released): each rung it finished, with
    /// its result, in order.
    fn drill(&self, alias: &str, connection: u64) -> Vec<(RecoveryRung, RecoveryRungOutcome)> {
        let receipt = self.send(
            RuntimeOperation::DrillRecoveryLadder {
                instance_alias: alias.to_owned(),
                mode: RecoveryDrillMode::Device,
            },
            connection,
        );
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::RecoveryLadderDrillQueued { drill }) = receipt.result() else {
            panic!("a drill's receipt names the drill: {receipt:?}");
        };
        let ladder = drill.ladder_request_id;
        let phases = || {
            self.host
                .query_persisted_events_for_test(EventQuery {
                    request_id: Some(ladder),
                    ..EventQuery::default()
                })
                .expect("the drill's records")
                .iter()
                .filter_map(|event| match event.payload() {
                    EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                        Some(value.phase())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        wait_until("the drill's end and its release", || {
            phases()
                .iter()
                .any(|phase| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderFinished { .. }))
                && self
                    .host
                    .instance_claims_for_test(alias)
                    .expect("instance claims")
                    .0
                    .is_none()
        });
        phases()
            .into_iter()
            .filter_map(|phase| match phase {
                RuntimeLifecyclePhase::RecoveryRungFinished {
                    rung, rung_result, ..
                } => Some((rung, rung_result)),
                _ => None,
            })
            .collect()
    }

    /// `alias`'s next foreground read fails, as a transient ADB failure.
    fn fail_next_front_read(&self, alias: &str) {
        *self.parts.states[alias]
            .foreground_error_once
            .lock()
            .expect("one-shot foreground error") =
            Some(DeviceError::transient("fixture foreground query failed"));
    }

    /// The member the host holds in front of `alias`'s device.
    fn front(&self, alias: &str) -> Option<String> {
        self.host
            .device_front_for_test(alias)
            .expect("the member in front")
    }

    /// The start of the queue wait of `task_id` on `alias` the next evaluation passes.
    fn wait(&self, task_id: &str, alias: &str) -> Option<u64> {
        self.host
            .queue_wait_start_for_test(task_id, alias)
            .expect("the queue wait")
    }

    /// The `config.parameters.scheduling` row of `key`: its value, source and whether it is
    /// settable.
    fn parameter_row(&self, key: &str) -> (FactScalar, String, bool) {
        let record = self
            .host
            .runtime_fact_for_test(
                &RuntimeFactScope::Runtime,
                &config_parameters_fact_key("scheduling"),
            )
            .expect("read the fact")
            .expect("config.parameters.scheduling");
        let ContractFactValue::RecordList(rows) = record.value else {
            panic!("config.parameters.scheduling is a record list");
        };
        let row = rows
            .iter()
            .find(|row| row.get("key") == Some(&FactScalar::String(key.to_owned())))
            .unwrap_or_else(|| panic!("no row {key}: {rows:?}"));
        let Some(FactScalar::String(source)) = row.get("source").cloned() else {
            panic!("source is a string: {row:?}");
        };
        let Some(FactScalar::Boolean(settable)) = row.get("settable").cloned() else {
            panic!("settable is a boolean: {row:?}");
        };
        (row.get("value").expect("value").clone(), source, settable)
    }

    /// Every record in the ledger, in order.
    fn events(&self) -> Vec<PersistedEvent> {
        self.host
            .query_persisted_events_for_test(EventQuery::default())
            .expect("ledger events")
    }

    fn close(self) {
        assert!(self.host.fatal_error().expect("health").is_none());
        self.host.close().expect("close the fixture host");
    }
}

/// A request on `connection` processed by `host` as a client's.
fn send_on(host: &RuntimeHost, operation: RuntimeOperation, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    host.process_request_for_test(
        &runtime_request(&ids, operation),
        ConnectionId::new(connection).expect("connection"),
    )
    .expect("the request's receipt")
}

/// An `ApplicationLifecycle` `action` of `alias` on `host`, on `connection`.
fn lifecycle(
    host: &RuntimeHost,
    alias: &str,
    action: ApplicationLifecycleAction,
    connection: u64,
) -> RuntimeReceipt {
    let holder = IdentifierIssuer::new().expect("identifier issuer");
    send_on(
        host,
        RuntimeOperation::ApplicationLifecycle {
            instance_alias: alias.to_owned(),
            holder_id: *holder.mint_holder_id().expect("holder").transport(),
            action,
        },
        connection,
    )
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

/// The (task, alias) of every dispatch intent of `evaluation`.
fn intents(evaluation: &PolicyEvaluation) -> Vec<(String, String)> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| (intent.task_id.clone(), intent.instance_id.clone()))
        .collect()
}

fn pair(task_id: &str, alias: &str) -> (String, String) {
    (task_id.to_owned(), alias.to_owned())
}

/// The last reason with `code` of the decision of `task_id` on `alias` in `evaluation`. Not the
/// decision's last reason: on the first evaluation after a start the host adds
/// `eligible_since_unknown` after the evaluator's own reasons.
fn reason<'a>(
    evaluation: &'a PolicyEvaluation,
    task_id: &str,
    alias: &str,
    code: &str,
) -> &'a Outcome {
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .unwrap_or_else(|| panic!("a decision of {task_id} on {alias}: {evaluation:#?}"));
    decision
        .reasons
        .iter()
        .rev()
        .map(|reason| &reason.outcome)
        .find(|outcome| outcome.code().as_str() == code)
        .unwrap_or_else(|| panic!("a reason {code} of {task_id}: {decision:?}"))
}

/// The `device_group_waiting` row of the decision of `task_id` on `alias` in `evaluation`.
fn waiting<'a>(evaluation: &'a PolicyEvaluation, task_id: &str, alias: &str) -> &'a Outcome {
    reason(
        evaluation,
        task_id,
        alias,
        PolicyCode::DeviceGroupWaiting.as_str(),
    )
}

/// Whether a reason of the decision of `task_id` on `alias` in `evaluation` has `code`.
fn has_reason(evaluation: &PolicyEvaluation, task_id: &str, alias: &str, code: &str) -> bool {
    evaluation
        .decisions
        .iter()
        .filter(|decision| {
            decision.task_id == task_id && decision.instance_id.as_deref() == Some(alias)
        })
        .flat_map(|decision| &decision.reasons)
        .any(|reason| reason.outcome.code().as_str() == code)
}

/// The number value `key` of `outcome`.
fn number(outcome: &Outcome, key: &str) -> Option<u64> {
    outcome.value(key).and_then(Value::as_u64)
}

/// `outcome` is `device_group_waiting` `selected` for `winner`'s `winner_task`, with the order's
/// limit `limit_ms` and its own time left `remaining_ms`.
fn assert_limit_row(
    outcome: &Outcome,
    winner: &str,
    winner_task: &str,
    limit_ms: u64,
    remaining_ms: u64,
) {
    assert!(outcome.is(PolicyCode::DeviceGroupWaiting), "{outcome:?}");
    assert_eq!(
        outcome.value(DEVICE_GROUP_WAIT),
        Some(&json!(DeviceGroupWait::Selected.as_str())),
        "{outcome:?}"
    );
    assert_eq!(outcome.value("alias"), Some(&json!(winner)), "{outcome:?}");
    assert_eq!(
        outcome.value("task_id"),
        Some(&json!(winner_task)),
        "{outcome:?}"
    );
    assert_eq!(number(outcome, LIMIT_MS), Some(limit_ms), "{outcome:?}");
    assert_eq!(
        number(outcome, REMAINING_MS),
        Some(remaining_ms),
        "{outcome:?}"
    );
}

/// A pause of `alias` asked on a new connection, completed.
fn pause(host: &RuntimeHost, alias: &str) {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::PauseScheduling {
        scope: SchedulingPauseScope::Instance {
            instance_alias: alias.to_owned(),
        },
        pause_reason: SchedulingPauseReason::new("ord1c_member_pause").expect("pause reason"),
        drain_timeout_ms: 1_000,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// `ConfigureSettings` of `key` to `value` on a new connection.
fn configure(host: &RuntimeHost, key: &str, value: Value) -> RuntimeReceipt {
    let mut client = TestClient::connect(host);
    let request = client.request(RuntimeOperation::ConfigureSettings {
        changes: vec![ConfigSettingChange {
            key: key.to_owned(),
            value: Some(value),
        }],
        expected_version: None,
    });
    client.send(&request)
}

// ---- The tests.

/// Ruling 4, both entries of the switch: the switch before a run (a direct run of B, its own
/// package in front) puts B in front of the device, as A's device sees it too; the switch before a
/// start outside a run (a `launch` of A) puts A in front. Each entry's failure leaves the device
/// with none in front: B's direct run whose foreground read fails, and, once A's launch has put A
/// in front again, A's launch whose foreground read fails. A start holds none, and C, alone on its
/// device, never has one.
#[test]
fn ord1c_t1_a_switch_that_succeeds_puts_its_member_in_front_and_one_that_fails_forgets_it() {
    let bench = Bench::start(GROUPED);
    assert_eq!(bench.front(A), None, "a start holds none in front");

    let receipt = bench.direct_run(B, 81);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        bench.front(A).as_deref(),
        Some(B),
        "before a run: B in front"
    );
    assert_eq!(bench.front(B).as_deref(), Some(B));

    let receipt = bench.launch(A, 82);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        bench.front(B).as_deref(),
        Some(A),
        "before a start outside a run: A in front"
    );

    bench.fail_next_front_read(B);
    let receipt = bench.direct_run(B, 83);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::ApplicationSwitchFailed.as_str(),
        "{receipt:?}"
    );
    assert_eq!(bench.front(A), None, "a failed switch before a run: none");

    let receipt = bench.launch(A, 84);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(bench.front(B).as_deref(), Some(A));
    bench.fail_next_front_read(A);
    let receipt = bench.launch(A, 85);
    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(bench.front(B), None, "a failed switch before a start: none");

    let receipt = bench.direct_run(C, 86);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(bench.front(C), None, "an instance alone has none in front");
    bench.close();
}

/// Ruling 1: with A in front, B's due task waits from its first queued round (T0), through a round
/// in which A's lease holds the device (`device_group_waiting` `held`) and a later round, where its
/// row's time left counts from T0, to the round its limit passes, where it is selected and still
/// waits; its admission ends the wait. A's task, of the member in front, has no wait in any round.
#[test]
fn ord1c_t2_a_due_task_waits_from_its_first_queued_round_across_selected_and_held_until_admission()
{
    let bench = Bench::start(GROUPED);
    let launched = bench.launch(A, 91);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.front(A).as_deref(), Some(A));

    let first = bench.evaluate(T0);
    assert_eq!(intents(&first), vec![pair(TASK_A, A)]);
    assert_limit_row(
        waiting(&first, TASK_B, B),
        A,
        TASK_A,
        DEFAULT_LIMIT_MS,
        DEFAULT_LIMIT_MS,
    );
    assert_eq!(
        bench.wait(TASK_B, B),
        Some(T0),
        "B waits from its first round"
    );
    assert_eq!(bench.wait(TASK_A, A), None, "the member in front waits not");
    bench.dispatch(&first, TASK_A);
    assert_eq!(bench.front(A).as_deref(), Some(A));

    let mut holder = TestClient::connect(&bench.host);
    let (_, token) = holder.acquire(A);
    let held = bench.evaluate(T0 + MINUTE_MS);
    assert!(intents(&held).is_empty(), "{:?}", intents(&held));
    let row = waiting(&held, TASK_B, B);
    assert_eq!(
        row.value("alias"),
        Some(&json!(A)),
        "held by A's lease: {row:?}"
    );
    assert_eq!(
        row.value(DEVICE_GROUP_WAIT),
        Some(&json!(DeviceGroupWait::Held.as_str())),
        "{row:?}"
    );
    assert_eq!(bench.wait(TASK_B, B), Some(T0), "kept across a held row");
    assert_eq!(bench.wait(TASK_A, A), None);
    let release = holder.request(RuntimeOperation::ReleaseLease { token });
    assert_eq!(
        holder.send(&release).state(),
        RuntimeReceiptState::Completed
    );
    drop(holder);

    let later = T0 + 2 * MINUTE_MS;
    let again = bench.evaluate(later);
    assert_eq!(intents(&again), vec![pair(TASK_A, A)]);
    assert_limit_row(
        waiting(&again, TASK_B, B),
        A,
        TASK_A,
        DEFAULT_LIMIT_MS,
        DEFAULT_LIMIT_MS - 2 * MINUTE_MS,
    );
    assert_eq!(bench.wait(TASK_B, B), Some(T0));

    let due = bench.evaluate(T0 + DEFAULT_LIMIT_MS);
    assert_eq!(
        intents(&due),
        vec![pair(TASK_B, B)],
        "at its limit B's task goes: nothing of A ends by it"
    );
    assert_eq!(
        bench.wait(TASK_B, B),
        Some(T0),
        "selected, not admitted: it waits"
    );
    assert_eq!(bench.wait(TASK_A, A), None);
    let context = bench.grant(&due, TASK_B);
    assert_eq!(bench.wait(TASK_B, B), None, "its admission ends the wait");
    bench.run_and_settle(&context);
    assert_eq!(bench.front(A).as_deref(), Some(B));
    bench.close();
}

/// Ruling 1 and its consequence, with the default limit set to five minutes by the file and B's
/// two tasks (TASK_B before TASK_B2 in B's order). Every round, one minute apart, the device's one
/// pick is dispatched, run and settled. A, first in the allocation order, wins the first round (no
/// member in front, every wait fresh) and runs while its task ends by B's limit (rounds 0 to 4);
/// in round 5 nothing of A ends by it and B's TASK_B goes, while TASK_B2 still waits from round 0;
/// B's switch puts B in front and forgets TASK_B2's wait. A lost the front, so its task waits anew
/// from round 6, and B runs until A's limit, counted from round 6, would pass (rounds 5 to 10);
/// round 11 switches back. Twelve dispatches, two switches.
#[test]
fn ord1c_t3_taking_the_front_forgets_the_waits_and_two_backlogs_switch_by_the_limit_not_per_dispatch()
 {
    const LIMIT: u64 = 5 * MINUTE_MS;
    let bench = Bench::start(Spec {
        tasks: two_tasks_of_b,
        limit_ms: Some(LIMIT),
        ..GROUPED
    });
    let mut picked = Vec::new();
    for round in 0..12_u64 {
        let now = T0 + round * MINUTE_MS;
        let evaluation = bench.evaluate(now);
        let intents = intents(&evaluation);
        let [(task_id, alias)] = intents.as_slice() else {
            panic!("round {round}: one pick per device: {intents:?}");
        };
        match round {
            0 => assert_eq!(bench.wait(TASK_A, A), Some(T0), "no member in front yet"),
            5 => {
                assert_eq!(pair(task_id, alias), pair(TASK_B, B), "round 5");
                assert_eq!(
                    bench.wait(TASK_B2, B),
                    Some(T0),
                    "TASK_B2 waits behind B's pick"
                );
                assert_eq!(bench.wait(TASK_A, A), None, "A is in front");
            }
            6 => assert_eq!(
                bench.wait(TASK_A, A),
                Some(now),
                "A, no longer in front, waits from the first round after"
            ),
            _ => {}
        }
        bench.dispatch(&evaluation, task_id);
        if round == 0 {
            assert_eq!(bench.front(B).as_deref(), Some(A));
            assert_eq!(
                bench.wait(TASK_A, A),
                None,
                "taking the front forgets A's waits"
            );
            assert_eq!(bench.wait(TASK_B, B), Some(T0));
            assert_eq!(bench.wait(TASK_B2, B), Some(T0));
        }
        if round == 5 {
            assert_eq!(bench.front(A).as_deref(), Some(B));
            assert_eq!(
                bench.wait(TASK_B2, B),
                None,
                "taking the front forgets every wait of B"
            );
            assert_eq!(bench.wait(TASK_B, B), None);
        }
        picked.push(alias.clone());
    }
    let expected = [A, A, A, A, A, B, B, B, B, B, B, A];
    assert_eq!(
        picked, expected,
        "the device stays with the front until a limit would pass"
    );
    let switches = picked.windows(2).filter(|two| two[0] != two[1]).count();
    assert_eq!(switches, 2, "two switches in twelve dispatches: {picked:?}");
    bench.close();
}

/// Ruling 1's end of a wait and ruling 2, with A in front: B's wait (from T0) ends while B is
/// paused, and B's row then, `device_group_waiting` `selected` behind A's pick without the
/// order's values (a member held as the cycle starts), starts none; with the pause lifted it starts
/// anew. A person's hold of B ends it too, and it starts anew once the hold ends. A failed switch
/// of B (its direct run's foreground read) forgets the front and starts B's switch cooldown, which
/// ends the wait; once the cooldown is over B waits anew.
#[test]
fn ord1c_t4_a_pause_a_player_hold_or_a_switch_cooldown_ends_the_wait_and_it_starts_anew() {
    let bench = Bench::start(GROUPED);
    let launched = bench.launch(A, 101);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    let first = bench.evaluate(T0);
    assert_eq!(intents(&first), vec![pair(TASK_A, A)]);
    assert_eq!(bench.wait(TASK_B, B), Some(T0));

    pause(&bench.host, B);
    let paused = bench.evaluate(T0 + MINUTE_MS);
    assert_eq!(intents(&paused), vec![pair(TASK_A, A)]);
    let row = waiting(&paused, TASK_B, B);
    assert_eq!(
        row.value("alias"),
        Some(&json!(A)),
        "behind A's pick: {row:?}"
    );
    assert_eq!(
        row.value(DEVICE_GROUP_WAIT),
        Some(&json!(DeviceGroupWait::Selected.as_str())),
        "{row:?}"
    );
    assert_eq!(
        row.value(LIMIT_MS),
        None,
        "a held member's row has no order values"
    );
    assert_eq!(
        bench.wait(TASK_B, B),
        None,
        "a pause ends the wait and starts none"
    );
    bench
        .host
        .lift_instance_pause_for_test(B)
        .expect("lift B's pause");
    let lifted = T0 + 2 * MINUTE_MS;
    bench.evaluate(lifted);
    assert_eq!(bench.wait(TASK_B, B), Some(lifted), "anew after the pause");

    bench
        .host
        .hold_player_for_test(B, true)
        .expect("a person holds B");
    let held = bench.evaluate(T0 + 3 * MINUTE_MS);
    assert!(intents(&held).is_empty(), "{:?}", intents(&held));
    assert_eq!(bench.wait(TASK_B, B), None, "a person's hold ends the wait");
    bench
        .host
        .hold_player_for_test(B, false)
        .expect("the hold ends");
    let freed = T0 + 4 * MINUTE_MS;
    bench.evaluate(freed);
    assert_eq!(bench.wait(TASK_B, B), Some(freed), "anew after the hold");

    bench.fail_next_front_read(B);
    let failed = bench.direct_run(B, 102);
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(bench.front(A), None, "B's failed switch forgets the front");
    let cooling = bench.evaluate(T0 + 5 * MINUTE_MS);
    assert!(
        has_reason(
            &cooling,
            TASK_B,
            B,
            HostCode::DispatchInstanceCooldown.as_str()
        ),
        "B waits for its switch cooldown: {cooling:#?}"
    );
    assert_eq!(
        bench.wait(TASK_B, B),
        None,
        "the switch cooldown ends the wait"
    );
    bench.clock().advance(COOLDOWN_MS);
    let cooled = T0 + 7 * MINUTE_MS;
    let after = bench.evaluate(cooled);
    assert!(
        !has_reason(
            &after,
            TASK_B,
            B,
            HostCode::DispatchInstanceCooldown.as_str()
        ),
        "the cooldown is over: {after:#?}"
    );
    assert_eq!(
        bench.wait(TASK_B, B),
        Some(cooled),
        "anew after the cooldown"
    );
    bench.close();
}

/// A restart forgets the member in front and every wait (fix round 1, point 4: the round after the
/// restart tells a kept front and a kept wait from forgotten ones). Before it B is in front, B's
/// one-minute task is picked, and A's task, which declares a limit of two minutes, waits from T0.
/// After it none is in front and no pair waits. At T0 plus two minutes the round picks A's task
/// (the allocation order, none in front): with B kept in front it would pick B's task, which ends
/// by A's fresh limit. A's wait begins at that round: kept, it would begin at T0. B's row shows a
/// fresh wait (its time left the whole default limit).
#[test]
fn ord1c_t5_a_restart_forgets_the_front_and_every_wait() {
    let bench = Bench::start(Spec {
        tasks: a_declares_two_minutes,
        ..GROUPED
    });
    let launched = bench.launch(B, 111);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    let before = bench.evaluate(T0);
    assert_eq!(
        intents(&before),
        vec![pair(TASK_B, B)],
        "B's task ends by A's limit"
    );
    assert_eq!(bench.front(A).as_deref(), Some(B));
    assert_eq!(bench.wait(TASK_A, A), Some(T0));

    let bench = bench.restart();
    assert_eq!(bench.front(A), None, "the front is forgotten");
    assert_eq!(bench.front(B), None);
    assert_eq!(bench.wait(TASK_A, A), None, "every wait is forgotten");
    assert_eq!(bench.wait(TASK_B, B), None);
    // Past any takeover cooldown the restart gives the device.
    bench.clock().advance(MINUTE_MS);
    let after = T0 + 2 * MINUTE_MS;
    let evaluation = bench.evaluate(after);
    assert_eq!(
        intents(&evaluation),
        vec![pair(TASK_A, A)],
        "none in front: the allocation order, not B's task in front"
    );
    assert_eq!(
        bench.wait(TASK_A, A),
        Some(after),
        "A's wait begins at the first round after the restart, not at T0"
    );
    assert_limit_row(
        waiting(&evaluation, TASK_B, B),
        A,
        TASK_A,
        DEFAULT_LIMIT_MS,
        DEFAULT_LIMIT_MS,
    );
    assert_eq!(bench.wait(TASK_B, B), Some(after));
    bench.close();
}

/// The key `scheduling.device_group.max_queue_wait_ms`: milliseconds `60000..=86400000`,
/// settable, default `1800000` from the shipped defaults file; the manifest row the assembly
/// renders from the defaults file, and the `config.parameters.scheduling` row of a host started
/// with it; a value outside the range refused `config_value_invalid` and the value in effect kept;
/// a value set while the Runtime runs applies from the next round (B's wait, from T0, then counts
/// against the new limit).
#[test]
fn ord1c_t6_the_key_has_its_range_default_and_rows_and_applies_from_the_next_round() {
    let (row, _) = find_row(DEVICE_GROUP_MAX_QUEUE_WAIT_MS).expect("the key's row");
    assert_eq!(
        row.range,
        KeyRange::Integer {
            min: 60_000,
            max: 86_400_000
        }
    );
    assert_eq!(row.settable, Settable::Yes);
    assert!(matches!(row.default, DefaultSource::File));
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    assert_eq!(
        defaults["scheduling"]["device_group"]["max_queue_wait_ms"],
        json!(DEFAULT_LIMIT_MS)
    );
    let rendered = render_parameters(&mechanism_rows(), &[], &defaults, &json!({}), &|_| None)
        .expect("every key resolves")
        .into_iter()
        .filter(|parameter| parameter.key == DEVICE_GROUP_MAX_QUEUE_WAIT_MS)
        .collect::<Vec<_>>();
    let [manifest_row] = rendered.as_slice() else {
        panic!("one manifest row of the key: {rendered:?}");
    };
    assert_eq!(manifest_row.value, FactScalar::DurationMs(DEFAULT_LIMIT_MS));
    assert_eq!(manifest_row.source, ConfigParameterSource::Default);
    assert!(manifest_row.settable);

    let bench = Bench::start(GROUPED);
    assert_eq!(
        bench.parameter_row(DEVICE_GROUP_MAX_QUEUE_WAIT_MS),
        (
            FactScalar::DurationMs(DEFAULT_LIMIT_MS),
            ConfigParameterSource::Default.as_str().to_owned(),
            true
        )
    );
    for outside in [59_999_u64, 86_400_001] {
        let receipt = configure(&bench.host, DEVICE_GROUP_MAX_QUEUE_WAIT_MS, json!(outside));
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
        assert_eq!(
            receipt.outcome().code().as_str(),
            ConfigCode::ConfigValueInvalid.as_str(),
            "{receipt:?}"
        );
        assert_eq!(
            receipt.outcome().value("config_key"),
            Some(&json!(DEVICE_GROUP_MAX_QUEUE_WAIT_MS))
        );
    }
    assert_eq!(
        bench.parameter_row(DEVICE_GROUP_MAX_QUEUE_WAIT_MS).0,
        FactScalar::DurationMs(DEFAULT_LIMIT_MS),
        "the value in effect is kept"
    );

    let launched = bench.launch(A, 121);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    let first = bench.evaluate(T0);
    assert_limit_row(
        waiting(&first, TASK_B, B),
        A,
        TASK_A,
        DEFAULT_LIMIT_MS,
        DEFAULT_LIMIT_MS,
    );
    const SET_MS: u64 = 2 * MINUTE_MS;
    let receipt = configure(&bench.host, DEVICE_GROUP_MAX_QUEUE_WAIT_MS, json!(SET_MS));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        bench.parameter_row(DEVICE_GROUP_MAX_QUEUE_WAIT_MS),
        (
            FactScalar::DurationMs(SET_MS),
            ConfigParameterSource::Instruction.as_str().to_owned(),
            true
        )
    );
    let next = bench.evaluate(T0 + MINUTE_MS);
    assert_eq!(
        intents(&next),
        vec![pair(TASK_A, A)],
        "A still ends by B's new limit"
    );
    assert_limit_row(
        waiting(&next, TASK_B, B),
        A,
        TASK_A,
        SET_MS,
        SET_MS - MINUTE_MS,
    );
    bench.close();
}

/// The coordinator's order, end to end, with the default limit set to ten minutes by the file:
/// A in front; B's TASK_B declares a limit of two minutes and TASK_B2 takes the configured
/// default, as their rows show. A's task (one minute) runs while it ends by TASK_B's deadline: at
/// T0, and one minute later, when it ends exactly there; at T0 plus two minutes it would not, and
/// the device switches: TASK_B goes, A's row shows the time its run needs against the time the
/// waiting task had left, and B is in front after its run.
#[test]
fn ord1c_t7_front_tasks_run_until_a_waiting_task_would_pass_its_limit_then_the_device_switches() {
    const DEFAULT: u64 = 10 * MINUTE_MS;
    const DECLARED: u64 = 2 * MINUTE_MS;
    let bench = Bench::start(Spec {
        tasks: one_declared_limit,
        limit_ms: Some(DEFAULT),
        ..GROUPED
    });
    let launched = bench.launch(A, 131);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );

    let first = bench.evaluate(T0);
    assert_eq!(intents(&first), vec![pair(TASK_A, A)]);
    assert_limit_row(waiting(&first, TASK_B, B), A, TASK_A, DECLARED, DECLARED);
    assert_limit_row(waiting(&first, TASK_B2, B), A, TASK_A, DEFAULT, DEFAULT);
    bench.dispatch(&first, TASK_A);

    let second = bench.evaluate(T0 + MINUTE_MS);
    assert_eq!(
        intents(&second),
        vec![pair(TASK_A, A)],
        "A's run ends exactly at TASK_B's limit"
    );
    assert_limit_row(
        waiting(&second, TASK_B, B),
        A,
        TASK_A,
        DECLARED,
        DECLARED - MINUTE_MS,
    );
    bench.dispatch(&second, TASK_A);

    let third = bench.evaluate(T0 + 2 * MINUTE_MS);
    assert_eq!(
        intents(&third),
        vec![pair(TASK_B, B)],
        "nothing of A ends by TASK_B's limit: the switch"
    );
    let row = waiting(&third, TASK_A, A);
    assert_eq!(row.value("alias"), Some(&json!(B)), "{row:?}");
    assert_eq!(row.value("task_id"), Some(&json!(TASK_B)), "{row:?}");
    assert_eq!(number(row, REQUIRED_MS), Some(E_MS), "{row:?}");
    assert_eq!(number(row, REMAINING_MS), Some(0), "{row:?}");
    assert!(
        has_reason(
            &third,
            TASK_B2,
            B,
            PolicyCode::InstanceAlreadySelected.as_str()
        ),
        "TASK_B2 waits behind its own member's pick: {third:#?}"
    );
    bench.dispatch(&third, TASK_B);
    assert_eq!(bench.front(A).as_deref(), Some(B));
    bench.close();
}

/// Whether a member is named `key`, whose value is a time or a place in the ledger: an instant on
/// either clock, a measurement on the process clock, a local day, a ledger position, revision or
/// record index, or the evaluation's `seed` (the list of `gate_mapp2b` and `gate_453_s2`).
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

/// `text` with each run of 32 or more lowercase hexadecimal digits replaced: a minted id, a
/// snapshot id, a digest.
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

/// The texts of `root`'s path a record may hold: as given, canonical, with forward slashes, and
/// its last component alone (the start record names the state root by its base name); the
/// longest first.
fn root_texts(root: &TempDir) -> Vec<String> {
    let mut texts = vec![root.path().display().to_string()];
    if let Ok(canonical) = root.path().canonicalize() {
        texts.push(canonical.display().to_string());
    }
    let slashed = texts
        .iter()
        .map(|text| text.replace('\\', "/"))
        .collect::<Vec<_>>();
    texts.extend(slashed);
    if let Some(name) = root.path().file_name() {
        texts.push(name.to_string_lossy().into_owned());
    }
    texts.sort_by_key(|text| std::cmp::Reverse(text.len()));
    texts
}

/// The members of the performance summary record (`EventType::PerformanceSummary`) whose value
/// samples the machine, not the Runtime's work (fix round 1, point 2: named, not every number). Fix
/// round 2 (the review): replaced inside that record only, since the same names occur in other
/// records (fact snapshots, timing, artifact failures), which are compared as they are. The ledger
/// open's phases (`sql_read_ms` to `import_ms`), which fix round 1 named too, are in no record of
/// the ledger: the start reports them as its `ledger_opened` daemon line and in its start report
/// (`host.rs`, `ledger_open_outcome`), so nothing of them is replaced. An evaluation holds none of
/// these members, and its numbers are compared as they are.
const PERFORMANCE_SAMPLES: [&str; 7] = [
    "context",
    "foreground",
    "owned_processes",
    "third_party_high_load",
    "ledger_commits",
    "capacity",
    "pacing",
];

/// `value` with its ids, times and places normalised: every member [`time_or_place`] names and
/// every member `measured` names is replaced whole, and every string loses its root path and its
/// identities. Every other number is kept.
fn normalised(value: &mut Value, roots: &[String], measured: &[&str]) {
    match value {
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                if time_or_place(key) || measured.contains(&key.as_str()) {
                    *member = json!("<normalised>");
                } else {
                    normalised(member, roots, measured);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalised(item, roots, measured);
            }
        }
        Value::String(text) => {
            let mut written = text.clone();
            for root in roots {
                written = written.replace(root.as_str(), "<root>");
            }
            *text = without_identities(&written);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// One record as the comparison reads it: its type, its severity and its whole payload,
/// normalised.
#[derive(Debug, PartialEq)]
struct Compared {
    event_type: EventType,
    severity: EventSeverity,
    payload: Value,
}

/// What one host without a device of several did: its picks, each round's evaluation and every
/// record of its ledger, the last two normalised.
struct Rounds {
    picks: Vec<Vec<(String, String)>>,
    evaluations: Vec<Value>,
    records: Vec<Compared>,
}

/// A host whose instances A, B and C (`ids`) are each alone on their devices, TASK_A on A and
/// TASK_C on C: its start, then three rounds one minute apart, each round's every intent admitted,
/// run and settled. With `off` every entry the slice adds returns at once on this thread
/// ([`QUEUE_ORDER_OFF_FOR_TEST`]), as a build without the slice.
fn lone_rounds(ids: &BTreeMap<&'static str, InstanceId>, off: bool) -> Rounds {
    QUEUE_ORDER_OFF_FOR_TEST.with(|switch| switch.set(off));
    let bench = Bench::start_with(
        ids.clone(),
        Spec {
            tasks: lone_tasks,
            grouped: false,
            ..GROUPED
        },
    );
    let roots = root_texts(&bench.parts.root);
    let mut picks = Vec::new();
    let mut evaluations = Vec::new();
    for round in 0..3_u64 {
        let evaluation = bench.evaluate(T0 + round * MINUTE_MS);
        let mut picked = intents(&evaluation);
        let mut value = serde_json::to_value(&evaluation).expect("an evaluation encodes");
        normalised(&mut value, &roots, &[]);
        evaluations.push(value);
        for (task_id, _) in &picked {
            bench.dispatch(&evaluation, task_id);
        }
        picked.sort();
        picks.push(picked);
    }
    for alias in [A, B, C] {
        assert_eq!(bench.front(alias), None, "{alias}: no member in front");
    }
    for (task_id, alias) in [(TASK_A, A), (TASK_C, C)] {
        assert_eq!(bench.wait(task_id, alias), None, "{alias}: no queue wait");
    }
    let records = bench
        .events()
        .iter()
        .map(|event| {
            let mut payload =
                serde_json::to_value(event.payload()).expect("a record's payload encodes");
            let measured: &[&str] = if event.event_type() == EventType::PerformanceSummary {
                &PERFORMANCE_SAMPLES
            } else {
                &[]
            };
            normalised(&mut payload, &roots, measured);
            Compared {
                event_type: event.event_type(),
                severity: event.severity(),
                payload,
            }
        })
        .collect();
    bench.close();
    QUEUE_ORDER_OFF_FOR_TEST.with(|switch| switch.set(false));
    Rounds {
        picks,
        evaluations,
        records,
    }
}

/// Every index at which `with` (the slice's entries reachable) and `without` (returning at once)
/// differ, each named with both values, so one message names them all.
fn differing<T: std::fmt::Debug + PartialEq>(what: &str, with: &[T], without: &[T]) -> Vec<String> {
    (0..with.len().max(without.len()))
        .filter(|index| with.get(*index) != without.get(*index))
        .map(|index| {
            format!(
                "{what} #{index}: with the slice {:#?}, without {:#?}",
                with.get(index),
                without.get(index)
            )
        })
        .collect()
}

/// A host without a device of several evaluates and writes the same with and without the slice:
/// the whole start and three rounds (each intent admitted, run and settled) of a host whose three
/// instances are each alone on their devices, against the same on a host of this build whose
/// entries of the slice return at once (the test-only switch, one function per build). The
/// picks are equal as they are; each round's evaluation and every record of the ledger, record by
/// record (type, severity and whole payload), are equal with ids, times, the evaluation's `seed`
/// and the temporary root normalised, and in the performance summary records only their named
/// samples of the machine ([`PERFORMANCE_SAMPLES`], fix round 2: inside those records alone); every
/// other number is compared as it is, and every differing one is named in one message. The two
/// hosts are started alike with the same instance ids; nothing else is compared between them.
///
/// What it proves (fix round 1, point 2): two starts of this build, one with the slice's entries
/// reachable and one with them returning at once, evaluate and write the same. That a host
/// without a device of several evaluates and writes as main does rests on reading the code: the
/// test-only switch turns off each entry the slice adds, and those entries are the slice's only
/// change on that path.
#[test]
fn ord1c_t8_a_host_without_a_device_of_several_evaluates_and_writes_the_same_with_and_without_the_slice()
 {
    let ids = fresh_ids();
    let with = lone_rounds(&ids, false);
    let without = lone_rounds(&ids, true);
    assert_eq!(
        with.picks,
        vec![vec![pair(TASK_C, C), pair(TASK_A, A)]; 3],
        "each round dispatches both instances alone (sorted by task)"
    );
    assert_eq!(with.picks, without.picks);
    assert!(!with.records.is_empty(), "the host wrote records");
    let mut differences = differing("evaluation", &with.evaluations, &without.evaluations);
    differences.extend(differing("record", &with.records, &without.records));
    assert!(
        differences.is_empty(),
        "the host without a device of several evaluates and writes the same; these differ:\n{}",
        differences.join("\n")
    );
}

/// Fix round 1, point 1 (the review's race): a switch between an evaluation's read of the members
/// in front and its staging. With A in front, B waits from T0. The next evaluation reads A in front
/// (B's row counts B's wait from T0); after that read and before it stages its verdicts, the
/// evaluation's test hook lets a launch of B on another thread run to its end, which puts B in
/// front and forgets B's waits. The evaluation then stages none of B's pairs, so the member in
/// front keeps no wait. The next apply clears what it skipped: once A has taken the front again,
/// the round after stages B's wait anew. Two channels order the threads; nothing sleeps.
#[test]
fn ord1c_t9_a_switch_between_the_front_read_and_the_staging_leaves_its_member_without_a_wait() {
    let bench = Bench::start(GROUPED);
    let launched = bench.launch(A, 141);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    let first = bench.evaluate(T0);
    assert_eq!(intents(&first), vec![pair(TASK_A, A)]);
    assert_eq!(bench.wait(TASK_B, B), Some(T0));
    bench.dispatch(&first, TASK_A);

    let (reached, at_hook) = mpsc::channel::<()>();
    let (ended, launch_ended) = mpsc::channel::<()>();
    bench
        .host
        .run_between_policy_sample_and_validation_for_test(Box::new(move || {
            reached.send(()).expect("the launcher listens");
            launch_ended
                .recv_timeout(GATE_WAIT)
                .expect("B's launch ends while the evaluation waits");
        }))
        .expect("the hook");
    let host = &bench.host;
    let t1 = T0 + MINUTE_MS;
    let (raced, relaunched) = thread::scope(|scope| {
        let launcher = scope.spawn(move || {
            at_hook
                .recv_timeout(GATE_WAIT)
                .expect("the evaluation read the members in front");
            let receipt = lifecycle(host, B, ApplicationLifecycleAction::Launch, 142);
            ended.send(()).expect("the evaluation waits");
            receipt
        });
        let raced = bench.evaluate(t1);
        (raced, launcher.join().expect("the launcher's thread"))
    });
    assert_eq!(
        relaunched.state(),
        RuntimeReceiptState::Completed,
        "{relaunched:?}"
    );
    assert_eq!(
        intents(&raced),
        vec![pair(TASK_A, A)],
        "the evaluation read A in front"
    );
    assert_limit_row(
        waiting(&raced, TASK_B, B),
        A,
        TASK_A,
        DEFAULT_LIMIT_MS,
        DEFAULT_LIMIT_MS - MINUTE_MS,
    );
    assert_eq!(
        bench.front(A).as_deref(),
        Some(B),
        "B took the front while the evaluation ran"
    );
    assert_eq!(
        bench.wait(TASK_B, B),
        None,
        "the evaluation stages no wait of a member that took the front after its read"
    );

    let back = bench.launch(A, 143);
    assert_eq!(back.state(), RuntimeReceiptState::Completed, "{back:?}");
    assert_eq!(bench.front(B).as_deref(), Some(A));
    let t2 = T0 + 2 * MINUTE_MS;
    bench.evaluate(t2);
    assert_eq!(
        bench.wait(TASK_B, B),
        Some(t2),
        "the next apply cleared what was skipped: B, out of the front again, waits anew"
    );
    bench.close();
}

/// Fix round 1, point 3 (the coordinator's ruling): a Runtime-level failure of the switch (its
/// `Err`; here the test's failure after the switch's steps ran) forgets the device's front,
/// whoever it names: with A in front, B's launch, whose switch so fails, leaves the device with
/// none in front.
#[test]
fn ord1c_t10_a_runtime_level_failure_of_the_switch_forgets_the_devices_front() {
    let bench = Bench::start(GROUPED);
    let launched = bench.launch(A, 151);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.front(B).as_deref(), Some(A));
    bench
        .host
        .fail_next_switch_steps_for_test()
        .expect("the switch's failure");
    let failed = bench.launch(B, 152);
    assert_ne!(failed.state(), RuntimeReceiptState::Completed, "{failed:?}");
    assert_eq!(
        bench.front(A),
        None,
        "the switch's Runtime-level failure forgets the front, though it named A"
    );
    bench.close();
}

/// Fix round 1, point 3, the `ApplicationLifecycle` request: with A in front, a stop of B, which
/// is not in front, leaves A there, and a stop of A leaves the device with none. Once B's launch has
/// put B in front, a launch of A whose switch puts A in front (A's own application in front) and
/// whose own command then fails leaves the device with none in front.
#[test]
fn ord1c_t11_a_stop_or_a_failed_start_of_the_member_in_front_forgets_it() {
    let bench = Bench::start(GROUPED);
    let launched = bench.launch(A, 161);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    let stopped = bench.stop(B, 162);
    assert_eq!(
        stopped.state(),
        RuntimeReceiptState::Completed,
        "{stopped:?}"
    );
    assert_eq!(
        bench.front(A).as_deref(),
        Some(A),
        "a stop of another member leaves the member in front"
    );
    let stopped = bench.stop(A, 163);
    assert_eq!(
        stopped.state(),
        RuntimeReceiptState::Completed,
        "{stopped:?}"
    );
    assert_eq!(bench.front(B), None, "a stop of the member in front");

    let launched = bench.launch(B, 164);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.front(A).as_deref(), Some(B));
    *bench.parts.states[A]
        .application_error
        .lock()
        .expect("one-shot application error") =
        Some(DeviceError::transient("fixture launch failed"));
    let failed = bench.launch(A, 165);
    assert_ne!(failed.state(), RuntimeReceiptState::Completed, "{failed:?}");
    assert_ne!(
        failed.outcome().code().as_str(),
        HostCode::ApplicationSwitchFailed.as_str(),
        "the switch passed and the launch's own command failed: {failed:?}"
    );
    assert_eq!(
        bench.front(B),
        None,
        "a start that failed after the switch put A in front"
    );
    bench.close();
}

/// Fix round 1, point 3, the ladder's application restart rung: a device drill of B's ladder with
/// A in front. The rung stops A first (B's device answers A's application in front until B's first
/// application command), then B's own application, and B's startup run then fails (B's captures
/// match no page): the rung fails after it stopped the others, and the device has none in front.
/// B has no emulator control, so the emulator restart rung is skipped and only the failed rung
/// forgot the front.
#[test]
fn ord1c_t12_a_ladder_rung_that_fails_after_it_stopped_the_others_forgets_the_front() {
    let bench = Bench::start(GROUPED);
    let launched = bench.launch(A, 171);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.front(B).as_deref(), Some(A));
    bench.front_until_started(B, APP_A, APP_B);
    bench.parts.states[B]
        .unknown_capture
        .store(true, Ordering::Release);
    let rungs = bench.drill(B, 172);
    assert!(
        rungs.contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Failed
        )),
        "the application restart rung fails: {rungs:?}"
    );
    assert!(
        rungs.contains(&(RecoveryRung::EmulatorRestart, RecoveryRungOutcome::Skipped)),
        "no emulator restart: {rungs:?}"
    );
    assert_eq!(
        bench.actions(A).last(),
        Some(&ApplicationLifecycleAction::Stop),
        "the rung stopped A, the member in front"
    );
    assert_eq!(
        bench.front(A),
        None,
        "a rung that failed after it stopped the others"
    );
    bench.close();
}

/// Fix round 1, point 3, the ladder's emulator restart rung: it forgets the device's front,
/// whoever it names. B has no startup package and is bound to the fake emulator, so a device drill
/// of B's ladder skips the application restart and restarts B's emulator; with A in front before
/// it, the device has none in front after it.
#[test]
fn ord1c_t13_an_emulator_restart_by_a_rung_forgets_the_devices_front() {
    let bench = Bench::start(Spec {
        startup_b: false,
        ..GROUPED
    });
    let launched = bench.launch(A, 181);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.front(B).as_deref(), Some(A));
    let state = &bench.parts.states[B];
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    bind_fake_emulator(&bench.host, B, bench.parts.ids[B], state);
    let rungs = bench.drill(B, 182);
    assert!(
        rungs.contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Skipped
        )),
        "B has no startup package: {rungs:?}"
    );
    assert!(
        rungs
            .iter()
            .any(|(rung, result)| *rung == RecoveryRung::EmulatorRestart
                && *result != RecoveryRungOutcome::Skipped),
        "the emulator restart rung ran: {rungs:?}"
    );
    assert_eq!(
        state
            .emulator_actions
            .lock()
            .expect("fake emulator actions")
            .first(),
        Some(&EmulatorInstanceAction::Stop),
        "the rung restarted B's emulator"
    );
    assert_eq!(
        bench.front(A),
        None,
        "an emulator restart forgets the front, though it named A"
    );
    bench.close();
}

/// Fix round 1, point 3, the idle action: closing the application of the member in front forgets
/// it. A's own `idle.action` is `close_app` and the standard-pack root declares A's application;
/// with A in front and its next work four hours away, the idle check closes A's application on the
/// action's key, and the device has none in front. A member of a group of several names no idle
/// action today (actingd refuses it at assembly, "Device-level actions"); the host is handed one
/// here directly, so the forget is in place before the idle action is made group-aware. Fix round
/// 2: the configuration carries the `idle` section's keys, which the start's check of A's own keys
/// reads, and A registers with its emulator running (emulator-controlled from the start), without
/// which the idle check takes no step on A. Fix round 3: so A is a device self-checked policy
/// instance, whose start preparation takes the clock's millisecond ([`Parts::start_host`]).
#[test]
fn ord1c_t14_the_idle_action_closing_the_member_in_front_forgets_it() {
    let bench = Bench::start(Spec {
        idle_close_a: true,
        ..GROUPED
    });
    let launched = bench.launch(A, 191);
    assert_eq!(
        launched.state(),
        RuntimeReceiptState::Completed,
        "{launched:?}"
    );
    assert_eq!(bench.front(B).as_deref(), Some(A));
    bench.clock().set_unix_ms(T0);
    let plans = BTreeMap::from([(
        A.to_owned(),
        IdlePlan {
            evaluated_at_unix_ms: T0,
            due_now: false,
            next_wake_unix_ms: Some(T0 + IDLE_GAP_MS),
            reading_due_unix_ms: None,
        },
    )]);
    let identities =
        BTreeMap::from([(A.to_owned(), (IDLE_GAME.to_owned(), IDLE_SERVER.to_owned()))]);
    bench
        .host
        .check_idle_instances_with_identities_for_test(&plans, &identities)
        .expect("the idle check");
    assert_eq!(
        bench.host.await_idle_action_ends_for_test(1, GATE_WAIT),
        Some(1),
        "A's idle action ended"
    );
    assert_eq!(
        bench.actions(A),
        [
            ApplicationLifecycleAction::Launch,
            ApplicationLifecycleAction::Stop
        ],
        "the idle action closed A's application"
    );
    assert_eq!(
        bench.front(B),
        None,
        "the idle action closed the member in front"
    );
    bench.close();
}

/// Fix round 2 (the review): a task step's own application effect follows the rule of the
/// `ApplicationLifecycle` request (`HostShared::front_after_own_application_effect`, called by the
/// run's `control_application`). A direct run of A whose first frame shows no page of its package
/// runs the package's one step, a restart of A's application, and the frames show the package's
/// home once the restart reached the device: the restart succeeds, the run completes and A is in
/// front. In the next such run the switch finds A's own application in front and the step's
/// restart command fails (the fake device records the command, then fails it): the run fails, not
/// at its switch, and the device has none in front. The switch before each run has already noted
/// A in front (its own application was in front), so the first run pins that a successful restart
/// leaves A there and the second that a failed one forgets it. Fix round 3: the first frames;
/// before, they showed the home, so each run ended there without a step and no restart reached
/// the device.
#[test]
fn ord1c_t15_a_task_steps_failed_restart_forgets_the_member_in_front() {
    let bench = Bench::start(GROUPED);
    bench.no_page_until_next_command(A);
    let receipt = bench.direct_run(A, 201);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        bench.actions(A).last(),
        Some(&ApplicationLifecycleAction::Restart),
        "the run's step restarted A's application"
    );
    assert_eq!(
        bench.front(B).as_deref(),
        Some(A),
        "a run whose restart step succeeded"
    );
    let commands = bench.actions(A).len();

    bench.no_page_until_next_command(A);
    *bench.parts.states[A]
        .application_error
        .lock()
        .expect("one-shot application error") =
        Some(DeviceError::transient("fixture restart failed"));
    let failed = bench.direct_run(A, 202);
    assert_ne!(failed.state(), RuntimeReceiptState::Completed, "{failed:?}");
    assert_ne!(
        failed.outcome().code().as_str(),
        HostCode::ApplicationSwitchFailed.as_str(),
        "the switch passed and the step's restart failed: {failed:?}"
    );
    let after = bench.actions(A);
    assert_eq!(
        (after.len(), after.last()),
        (commands + 1, Some(&ApplicationLifecycleAction::Restart)),
        "the step's restart, and only it, reached the device"
    );
    assert_eq!(
        bench.front(B),
        None,
        "a task step whose restart command failed"
    );
    bench.close();
}
