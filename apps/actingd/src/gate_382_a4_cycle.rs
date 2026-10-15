// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-4, apps key 6 "Policy cycle (#369, #372)" (test plan v2.2 §3.7 row A-4,
// test-ci-plan.md:202; inventory-apps.md key 6 and gaps G5, G6; APPS-I7, the actingd cycle
// harness). Wave-17 ruling 11: G5 row 1 cannot occur (the evaluator selects at most one intent per
// instance per round) and is not written; G5 rows 2-3 and G6a are written here; G6b is not written
// in 0.12 (no admission hook is defined). Every expected value comes from the cited requirement,
// not from what the code does today.
//
// G5 row 2: in one cycle a scheduled run on instance A fails, and instance B's intent in the same
// cycle still runs (model-369-queue v3.1 §3.3 W-1 "After a failed run (review M-6)": "Other
// instances' intents still run"; §7 S4 test; test plan A-4 "另一实例照常").
// G5 row 3: `failed_runs` makes the driver evaluate again at once, and that evaluation defers the
// instance its ladder holds after the failed run's lease hand-off with `dispatch_instance_held`
// and writes nothing for it (model-369-queue v3.1 §3.3 W-1, §6 steps 1-3; test plan A-4
// "failed_runs 立即重评时把被持有的实例按 dispatch_instance_held 延期").
// G6a: when the cycle's budget runs out, the intents not yet attempted are reported in
// `actingd policy_cycle_yielded attempted=<i> remaining=<N>` and write no ledger fact, and the
// first intent is always attempted (test plan A-4 G6 "预算让出时未尝试的意图记进
// policy_cycle_yielded … remaining=N"; inventory-apps G6; Workflow #191 A1, "Every cycle attempts
// its first intent"). The budget is read through the seam `execute_policy_cycle_with`.
//
// The harness (APPS-I7): actingd's own cycle (`initialize_policy`, `execute_policy_cycle`, the
// driver's `apply_policy_cycle_result`) against an in-process Runtime on a manual Runtime clock
// and fake physical devices, with the policy bootstrap of a configured scheduled run per
// procedure (PKG-S4: the request names its package by its path). The catalog is
// contracts/scheduling/examples/catalog-a as the runtime-host policy fixtures use it. Every wait
// returns as soon as its message arrives; `GATE_WAIT` only bounds a broken run.

use super::*;
use crate::config::ScheduledProcedureTask;
use actingcommand_contract::{
    ApplicationLifecycleAction, BackendOpenEntry, CaptureGeometryObservation,
    ContainedTaskRecoveryBinding, ContainedTaskRequest, EventPayload, EventType, FencedWrite,
    IdentifierIssuer, InstanceId, InstanceStuckRecovery, LeaseId, PolicyPayload,
};
use actingcommand_device::{
    CaptureBackend, DeviceCloseAuthority, DeviceError, DeviceResourceCloseOutcome, DeviceResult,
    Frame, FrameMemoryBudget, InputBackend, OpenedBackend,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig, PersistedEvent};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, EvaluationFacts, EvaluationResources,
    HostResourceSnapshot, InstanceSnapshot, PoolValueSnapshot, SchedulingDecisionState,
};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, ForegroundApplicationObservation, PolicyInputSnapshot,
    ProcedureBinding, ProcedureManifest, ResolvedExecutionInstance, RuntimeClock,
    RuntimeClockSample, RuntimeHostConfig, RuntimeHostResult,
};
use serde_json::{Value, json};
use std::fs::File;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

const GATE_WAIT: Duration = Duration::from_secs(120);
// The runtime-host policy fixtures' instant, 2023-11-14 12:00 UTC: inside catalog-a's activity
// windows (08:00-22:00 UTC), at an occurrence of its hourly clock trigger.
const POLICY_NOW_UNIX_MS: u64 = 1_699_963_200_000;
const SALT: &[u8] = b"gate-382-a4-salt";
const APPROVAL: &str = "approval:fixture-a";
const GAME: &str = "fixture-game-a";
const OPERATION: &str = "operation.observe";
// The first pixel of each page the packages recognize (c4_support::page_frame).
const HOME: [u8; 3] = [255, 0, 0];
const TERMINAL: [u8; 3] = [0, 0, 255];

// One configured instance: its alias, its static identity, its catalog task and procedure, and
// its fake device's serial.
struct Fixture {
    alias: &'static str,
    server: &'static str,
    host: &'static str,
    task: &'static str,
    procedure: &'static str,
    serial: &'static str,
}

// Instance A carries catalog-a's task; instance B the same task at a lower priority (the
// runtime-host `pending_policy_sources` pair), so that A's intent is the cycle's first.
static INSTANCES: [Fixture; 2] = [
    Fixture {
        alias: "fixture-instance-a",
        server: "fixture-server-a",
        host: "fixture-host-a",
        task: "fixture.observe",
        procedure: "procedure.observe",
        serial: "TESTSERIAL0001",
    },
    Fixture {
        alias: "fixture-instance-b",
        server: "fixture-server-b",
        host: "fixture-host-b",
        task: "fixture.observe-b",
        procedure: "procedure.observe-b",
        serial: "TESTSERIAL0002",
    },
];

// The Runtime clock (HOST-I2, EXEC-I3): its wall time moves 1 ms at every reading, so records keep
// strictly increasing times; its monotonic time moves only by the contained-task kernel's waits
// (`pause`), which return at once. It never reads the wall clock.
struct CycleClock {
    unix_ms: AtomicU64,
    monotonic_ms: AtomicU64,
}

impl RuntimeClock for CycleClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        Ok(RuntimeClockSample {
            unix_ms: self.unix_ms.fetch_add(1, Ordering::SeqCst) + 1,
            monotonic_ms: self.monotonic_ms.load(Ordering::SeqCst),
        })
    }

    fn pause(&self, duration: Duration) {
        let duration_ms = duration.as_nanos().div_ceil(1_000_000);
        self.monotonic_ms.fetch_add(
            u64::try_from(duration_ms).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
    }
}

// What a fake device shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    // `home` at every capture: a tap never opens `terminal`, so a run fails
    // `page_confirmation_failed`.
    Stuck,
    // `home` until the device's first tap, `terminal` from then on: a run completes.
    Opens,
}

// The taps a fake device saw: how many, the lease of the first, and whether its hold was taken.
#[derive(Default)]
struct Taps {
    count: usize,
    first_lease: Option<LeaseId>,
    held: bool,
}

// Row 3's hold: the first tap under a lease other than the device's first tap's lease (a rung run
// on the ladder's key, model-369 §6 step 3) reports itself on `reached` and waits for `release`.
// Until then the ladder holds its key.
struct Hold {
    reached: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Hold {
    fn wait(&self) -> DeviceResult<()> {
        self.reached
            .lock()
            .map_err(|_| DeviceError::fatal("hold signal poisoned"))?
            .send(())
            .map_err(|_| DeviceError::fatal("the test no longer waits for the hold"))?;
        self.release
            .lock()
            .map_err(|_| DeviceError::fatal("hold release poisoned"))?
            .recv_timeout(GATE_WAIT)
            .map_err(|_| DeviceError::fatal("the test did not release the hold"))
    }
}

// A fake physical device; its sessions share its state.
struct Device {
    instance_id: InstanceId,
    serial: &'static str,
    screen: Screen,
    taps: Mutex<Taps>,
    hold: Option<Hold>,
}

impl Device {
    fn frame(&self) -> DeviceResult<Frame> {
        let opened = self.screen == Screen::Opens
            && self
                .taps
                .lock()
                .map_err(|_| DeviceError::fatal("fake device state poisoned"))?
                .count
                > 0;
        crate::c4_support::page_frame(if opened { TERMINAL } else { HOME })
    }

    fn tap(&self, witness: &FencedWrite) -> DeviceResult<()> {
        let lease = witness.token().lease_id();
        let hold = {
            let mut taps = self
                .taps
                .lock()
                .map_err(|_| DeviceError::fatal("fake device state poisoned"))?;
            taps.count += 1;
            let first = *taps.first_lease.get_or_insert(lease);
            let hold = first != lease && !taps.held;
            taps.held |= hold;
            hold
        };
        match &self.hold {
            Some(point) if hold => point.wait(),
            _ => Ok(()),
        }
    }
}

struct DeviceInput(Arc<Device>);

impl InputBackend for DeviceInput {
    fn tap(&mut self, witness: &FencedWrite, _x: i32, _y: i32) -> DeviceResult<()> {
        self.0.tap(witness)
    }

    fn long_tap(
        &mut self,
        _witness: &FencedWrite,
        _x: i32,
        _y: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn swipe(
        &mut self,
        _witness: &FencedWrite,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Ok(())
    }

    fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
        Ok(())
    }

    fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
        Ok(())
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }
}

struct DeviceCapture(Arc<Device>);

impl CaptureBackend for DeviceCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.0.frame()
    }

    fn observe_geometry(
        &mut self,
        _deadline: std::time::Instant,
    ) -> DeviceResult<CaptureGeometryObservation> {
        // The same 16x9 extent as the frames.
        Ok(crate::c4_support::physical_geometry_16x9(self.0.serial))
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }
}

// The fake physical instances by alias. The assigned application is always in the foreground, so
// the foreground gate passes; application control is never asked for.
struct CycleProvider(BTreeMap<String, Arc<Device>>);

impl CycleProvider {
    fn device(&self, instance_alias: &str) -> DeviceResult<Arc<Device>> {
        self.0
            .get(instance_alias)
            .cloned()
            .ok_or_else(|| DeviceError::fatal("unknown fake instance"))
    }
}

impl ExecutionBackendProvider for CycleProvider {
    fn instance_aliases(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        self.0
            .get(instance_alias)
            .map(|device| ResolvedExecutionInstance::new(device.instance_id, device.serial))
    }

    fn open_input(
        &self,
        instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        let input: Box<dyn InputBackend> = Box::new(DeviceInput(self.device(instance_alias)?));
        Ok(OpenedBackend::unobserved(input, BackendOpenEntry::Input))
    }

    fn open_capture(
        &self,
        instance_alias: &str,
        _memory: Option<&FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        let capture: Box<dyn CaptureBackend> =
            Box::new(DeviceCapture(self.device(instance_alias)?));
        Ok(OpenedBackend::unobserved(
            capture,
            BackendOpenEntry::Capture,
        ))
    }

    fn observe_foreground_application(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        Ok(crate::c4_support::neutral_foreground())
    }

    fn control_application(
        &self,
        _witness: &FencedWrite,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "the fake device controls no application",
        ))
    }
}

// A neutral navigable package on a 16x9 screen: one click from `home` (first pixel red) to
// `terminal` (first pixel blue). The scheduled package requires `home` as its entry page; the
// return-home package, the ladder's first rung, has no entry page.
fn write_package(path: &Path, package_id: &str, entry_page: bool) {
    let mut task = json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 16, "height": 9},
        "target_page": "terminal",
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true
        }]
    });
    if entry_page {
        task["entry_page"] = json!("home");
    }
    let files = [
        (
            "control.json",
            json!({
                "schema_version": "Lab-1y.control.v1",
                "package_id": package_id,
                "execution_mode": "navigable_route",
                "game": "neutral",
                "server": "test",
                "resolution": {"width": 16, "height": 9},
                "entry_task_id": "task",
                "capture_interval_ms": 50,
                "step_timeout_ms": 50,
                "timeout_ms": 1000,
                "max_steps": 2
            }),
        ),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        (
            "resources/recognition/neutral.test.pack.json",
            json!({
                "schema_version": "0.3",
                "game": "neutral",
                "server": "test",
                "coordinate_space": {"width": 16, "height": 9},
                "defaults": {"color_max_distance": 0.0},
                "targets": [
                    {"type": "color", "id": "page/home", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": HOME},
                    {"type": "color", "id": "page/terminal", "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": TERMINAL}
                ]
            }),
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            json!({
                "schema_version": "0.3",
                "pages": [
                    {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
                    {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [], "forbidden": []}
                ]
            }),
        ),
    ];
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = ZipWriter::new(File::create(path).expect("create the package"));
    for (name, content) in files {
        writer
            .start_file(name, options)
            .expect("start a package entry");
        writer
            .write_all(&serde_json::to_vec(&content).expect("package entry JSON"))
            .expect("write a package entry");
    }
    writer.finish().expect("finish the package");
}

fn document(name: &str, bytes: Vec<u8>) -> CatalogDocumentSource {
    CatalogDocumentSource::new(format!("memory://gate-382-a4/{name}"), bytes)
}

// catalog-a with its feedback stop at a clock instant that never comes (the runtime-host
// `policy_sources`) and, for two instances, instance B's copy of the task at priority 50.
fn catalog(two_instances: bool) -> CatalogSources {
    let mut tasks: Value = serde_json::from_slice(include_bytes!(
        "../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ))
    .expect("catalog-a tasks");
    tasks["tasks"][0]["feedback_stop"] = json!({
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
            "at_ms": 4_102_444_800_000_u64
        }
    });
    if two_instances {
        let b = &INSTANCES[1];
        let mut second = tasks["tasks"][0].clone();
        second["id"] = json!(b.task);
        second["scope"] = json!({"kind": "instance", "instance_id": b.alias});
        second["procedure_ref"] = json!(b.procedure);
        second["priority"] = json!(50);
        second["produces"] = json!([]);
        second["instance_overrides"] = json!([]);
        tasks["tasks"]
            .as_array_mut()
            .expect("catalog-a task list")
            .push(second);
    }
    CatalogSources {
        tasks: document(
            "tasks.json",
            serde_json::to_vec(&tasks).expect("catalog tasks bytes"),
        ),
        pools: document(
            "pools.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/pools.json").to_vec(),
        ),
        activity: document(
            "activity.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/activity.json")
                .to_vec(),
        ),
        timeline: document(
            "timeline.json",
            include_bytes!("../../../contracts/scheduling/examples/catalog-a/timeline.json")
                .to_vec(),
        ),
        selection: None,
    }
}

fn facts(instances: &[Fixture]) -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 0,
        fact_snapshot_id: "snapshot:gate-382-a4".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: instances
            .iter()
            .map(|instance| InstanceSnapshot {
                instance_id: instance.alias.to_owned(),
                server_id: instance.server.to_owned(),
                game_id: GAME.to_owned(),
                host_id: instance.host.to_owned(),
                available: true,
                capability_operation_ids: vec![OPERATION.to_owned()],
                preferred_task_ids: Vec::new(),
            })
            .collect(),
        priority_offsets: Vec::new(),
    }
}

fn resources(instances: &[Fixture]) -> EvaluationResources {
    EvaluationResources {
        pools: vec![PoolValueSnapshot {
            pool_id: "fixture-pool-a".to_owned(),
            value: 10,
            observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        }],
        hosts: instances
            .iter()
            .map(|instance| HostResourceSnapshot {
                host_id: instance.host.to_owned(),
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: actingcommand_policy::DEFAULT_HEAVY_THRESHOLD_MILLI,
            })
            .collect(),
    }
}

// Starts the in-process Runtime on one fake device per screen (instance A, then B) with what
// actingd's configuration assembles for a scheduled run per procedure (config.rs: the policy
// inputs, the procedure manifest at the package's path, the scheduled procedures, and the
// bootstrap's scheduled tasks and registry modes). The first device takes `hold`. With `ladder`
// each instance keeps its default stuck recovery (`recovery_ladder.enabled`) and the scheduled
// request binds the return-home package, the ladder's first rung; without it stuck recovery is
// off for every instance.
fn start(
    root: &Path,
    screens: &[Screen],
    mut hold: Option<Hold>,
    ladder: bool,
) -> (RuntimeHost, PolicyBootstrap) {
    let instances = &INSTANCES[..screens.len()];
    let scheduled = root.join("scheduled-task.zip");
    write_package(&scheduled, "neutral.cycle.task", true);
    let mut request = ContainedTaskRequest::new(scheduled.to_string_lossy().into_owned())
        .and_then(|request| {
            request.with_response_deadline_ms(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS)
        })
        .expect("the scheduled request");
    if ladder {
        let return_home = root.join("return-home.zip");
        write_package(&return_home, "neutral.cycle.return-home", false);
        request = ContainedTaskRecoveryBinding::new(return_home.to_string_lossy().into_owned())
            .and_then(|binding| request.with_recovery(binding))
            .expect("the scheduled request with its return-home package");
    }
    let manifest = ProcedureManifest::new(instances.iter().map(|instance| {
        ProcedureBinding::new(
            instance.procedure,
            OPERATION,
            vec!["after_observation".to_owned()],
        )
        .expect("procedure binding")
        .with_package_path(request.package_path())
    }))
    .expect("procedure manifest");
    let mut config = RuntimeHostConfig::new(root, SALT)
        .with_runtime_clock(Arc::new(CycleClock {
            unix_ms: AtomicU64::new(POLICY_NOW_UNIX_MS),
            monotonic_ms: AtomicU64::new(0),
        }))
        .with_policy_inputs(PolicyInputSnapshot::new(
            facts(instances),
            resources(instances),
        ))
        .with_procedure_manifest(manifest)
        .with_scheduled_procedures(
            instances
                .iter()
                .map(|instance| instance.procedure.to_owned()),
        );
    if !ladder {
        config = config.with_stuck_recovery(
            instances
                .iter()
                .map(|instance| {
                    (
                        instance.alias.to_owned(),
                        InstanceStuckRecovery {
                            enabled: false,
                            cooldown_secs: 600,
                        },
                    )
                })
                .collect(),
        );
    }
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let devices = instances
        .iter()
        .zip(screens)
        .map(|(instance, screen)| {
            let device = Device {
                instance_id: *issuer.mint_instance_id().expect("instance id").transport(),
                serial: instance.serial,
                screen: *screen,
                taps: Mutex::default(),
                hold: hold.take(),
            };
            (instance.alias.to_owned(), Arc::new(device))
        })
        .collect();
    let host = RuntimeHost::start(config, Arc::new(CycleProvider(devices)))
        .expect("the in-process Runtime starts");
    let policy = PolicyBootstrap {
        catalog_approval_ids: vec![APPROVAL.to_owned()],
        catalog: catalog(instances.len() > 1),
        catalog_transition: None,
        scheduled_tasks: instances
            .iter()
            .map(|instance| {
                (
                    instance.procedure.to_owned(),
                    ScheduledProcedureTask {
                        request: request.clone(),
                        mode: ScheduledExecutionMode::DeviceRegistry,
                    },
                )
            })
            .collect(),
        registry_modes: instances
            .iter()
            .map(|instance| {
                (
                    instance.alias.to_owned(),
                    ScheduledExecutionMode::DeviceRegistry,
                )
            })
            .collect(),
        cadence: PolicyCadence::default(),
    };
    (host, policy)
}

// The catalog transition and the configured approvals `initialize_policy` makes before its first
// cycle (Workflow #361 C2, #389 U-2), through the same host calls.
fn activate(host: &RuntimeHost, policy: &PolicyBootstrap) {
    let plan = host
        .plan_policy_catalog_transition(
            &policy.catalog,
            policy.catalog_transition.as_ref(),
            &policy.catalog_approval_ids,
        )
        .expect("the catalog transition is planned");
    host.apply_policy_catalog_transition(&plan)
        .expect("the catalog is activated");
    let approvals = plan.approvals();
    for decision in approvals
        .revoke()
        .iter()
        .chain(approvals.record())
        .chain(approvals.reapprove())
    {
        host.transcribe_configured_approval(decision)
            .expect("the configured approval is recorded");
    }
}

// The ledger as a read-only reader sees it now.
fn ledger_events(root: &Path) -> Vec<PersistedEvent> {
    GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root), |_| None)
        .expect("the ledger opens read-only")
        .events()
        .to_vec()
}

// The policy dispatch records (intent, admitted, rejected, completed) of one decision.
fn dispatch_records<'a>(
    events: &'a [PersistedEvent],
    decision_id: &str,
) -> Vec<&'a PersistedEvent> {
    events
        .iter()
        .filter(|event| match event.payload() {
            EventPayload::Policy(
                PolicyPayload::DispatchIntent(payload)
                | PolicyPayload::DispatchAdmitted(payload)
                | PolicyPayload::DispatchRejected(payload)
                | PolicyPayload::DispatchCompleted(payload),
            ) => payload.decision_id() == decision_id,
            _ => false,
        })
        .collect()
}

// The record of one decision of `event_type`.
fn dispatch_record<'a>(
    events: &'a [PersistedEvent],
    decision_id: &str,
    event_type: EventType,
) -> Option<&'a PersistedEvent> {
    dispatch_records(events, decision_id)
        .into_iter()
        .find(|event| event.event_type() == event_type)
}

// The run a decision's intent opened.
fn run_of(events: &[PersistedEvent], decision_id: &str) -> RunId {
    dispatch_record(events, decision_id, EventType::PolicyDispatchIntent)
        .and_then(|event| event.links().run_id().copied())
        .unwrap_or_else(|| panic!("the run of decision {decision_id}"))
}

// The records of one run, in ledger order.
fn run_records(events: &[PersistedEvent], run_id: RunId) -> Vec<&PersistedEvent> {
    events
        .iter()
        .filter(|event| event.links().run_id() == Some(&run_id))
        .collect()
}

// G5 row 2 (apps key 6): instance A's intent is the cycle's first and its scheduled run fails;
// instance B's intent, after it in the same cycle, is still written, admitted and run to its
// completion (model-369 W-1, review M-6: "Other instances' intents still run").
#[test]
fn gate_382_a4_g5_a_failed_run_leaves_the_other_instances_intent_to_run_in_the_same_cycle() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy) = start(root.path(), &[Screen::Stuck, Screen::Opens], None, false);

    let execution = initialize_policy(&host, &policy).expect("the cycle runs");
    let pending = &execution.cycle.pending_dispatch_intents;
    assert_eq!(
        pending
            .iter()
            .map(|intent| intent.instance_id.as_str())
            .collect::<Vec<_>>(),
        [INSTANCES[0].alias, INSTANCES[1].alias],
        "one intent per instance, A's first"
    );
    let (failed, other) = (&pending[0].decision_id, &pending[1].decision_id);

    let events = ledger_events(root.path());
    let failed_run = run_of(&events, failed);
    let failed_records = run_records(&events, failed_run);
    let task_failed = failed_records
        .iter()
        .find(|event| event.event_type() == EventType::TaskFailed)
        .unwrap_or_else(|| panic!("A's scheduled run fails: {failed_records:#?}"));
    assert!(
        failed_records
            .iter()
            .all(|event| event.event_type() != EventType::TaskCompleted),
        "A's scheduled run does not complete"
    );

    let intent = dispatch_record(&events, other, EventType::PolicyDispatchIntent)
        .expect("B's intent is still written in the same cycle");
    assert!(
        intent.sequence() > task_failed.sequence(),
        "B's intent follows A's failed run"
    );
    assert!(
        dispatch_record(&events, other, EventType::PolicyDispatchAdmitted).is_some(),
        "B's intent is admitted"
    );
    assert!(
        dispatch_record(&events, other, EventType::PolicyDispatchRejected).is_none(),
        "B's intent is not rejected"
    );
    let other_run = run_of(&events, other);
    assert!(
        run_records(&events, other_run)
            .iter()
            .any(|event| event.event_type() == EventType::TaskCompleted),
        "B's scheduled run completes"
    );
    assert!(
        dispatch_record(&events, other, EventType::PolicyDispatchCompleted).is_some(),
        "B's dispatch completes"
    );
    host.close().expect("the Runtime closes");
}

// G5 row 3 (apps key 6): the cycle's scheduled run on A fails and its lease end hands A's key to
// A's ladder (model-369 §6 step 1); the cycle reports `failed_runs`, the driver takes the cycle
// and is woken at once, and the evaluation it runs, while the ladder holds A in its first rung,
// defers A's candidate `dispatch_instance_held` with the ladder as its holder and writes no intent
// or rejection for it (§3.3 W-1, §6 step 2).
#[test]
fn gate_382_a4_g5_failed_runs_re_evaluates_at_once_and_defers_the_instance_its_ladder_holds() {
    let root = TempDir::new().expect("tempdir");
    let (reached_sender, reached) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let hold = Hold {
        reached: Mutex::new(reached_sender),
        release: Mutex::new(release_receiver),
    };
    let (host, policy) = start(root.path(), &[Screen::Stuck], Some(hold), true);
    // Declared after the host, so that a failing assertion drops it first and ends the hold.
    let release = release_sender;
    let instance = INSTANCES[0].alias;

    let execution = initialize_policy(&host, &policy).expect("the cycle runs");
    let [intent] = execution.cycle.pending_dispatch_intents.as_slice() else {
        panic!(
            "one intent: {:#?}",
            execution.cycle.pending_dispatch_intents
        )
    };
    assert_eq!(intent.instance_id, instance);
    assert!(execution.failed_runs, "the cycle reports its failed run");
    reached
        .recv_timeout(GATE_WAIT)
        .expect("A's ladder runs its first rung on its own key");

    let control = PolicyDriverControl::default();
    let mut schedule =
        PolicyDriverSchedule::new(policy.cadence.clone(), &execution.cycle, POLICY_NOW_UNIX_MS)
            .expect("the driver's schedule");
    apply_policy_cycle_result(
        &control,
        &mut schedule,
        PolicyTrigger::Recovery,
        Vec::new(),
        POLICY_NOW_UNIX_MS,
        Ok(execution),
    )
    .expect("the driver takes the cycle");
    let PolicyDriverWake::Trigger(trigger, recomputes) =
        control.wait(Duration::ZERO).expect("the driver's wake")
    else {
        panic!("failed_runs wakes the driver at once")
    };
    assert_eq!(trigger, PolicyTrigger::FactsChanged);
    assert!(recomputes.is_empty());

    let again = execute_policy_cycle(&host, &policy, trigger).expect("the evaluation again");
    assert!(
        again
            .cycle
            .pending_dispatch_intents
            .iter()
            .all(|pending| pending.instance_id != instance),
        "no intent for the held instance: {:#?}",
        again.cycle.pending_dispatch_intents
    );
    let evaluation = again.cycle.evaluation.as_ref().expect("an evaluation");
    let decision = evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == INSTANCES[0].task
                && decision.instance_id.as_deref() == Some(instance)
        })
        .expect("A's decision");
    assert_eq!(
        decision.state,
        SchedulingDecisionState::Deferred,
        "{decision:#?}"
    );
    let held = decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == "dispatch_instance_held")
        .unwrap_or_else(|| panic!("deferred as held: {decision:#?}"));
    assert_eq!(
        held.outcome.value("holder_kind"),
        Some(&json!("ladder")),
        "{held:#?}"
    );

    let events = ledger_events(root.path());
    let intents = events
        .iter()
        .filter(|event| event.event_type() == EventType::PolicyDispatchIntent)
        .count();
    assert_eq!(intents, 1, "only the failed run's intent is written");
    assert!(
        events
            .iter()
            .all(|event| event.event_type() != EventType::PolicyDispatchRejected),
        "nothing is rejected for the held instance"
    );
    release.send(()).expect("release the ladder's hold");
    host.close().expect("the Runtime closes");
}

// G6a (apps key 6): a cycle whose budget is spent attempts its first intent and yields before the
// second. The yield line reports one attempted and one remaining intent; the attempted intent is
// written and completes; the yielded one writes no ledger fact.
#[test]
fn gate_382_a4_g6a_a_budget_yield_reports_the_unattempted_intents_and_writes_no_fact_for_them() {
    let root = TempDir::new().expect("tempdir");
    let (host, policy) = start(root.path(), &[Screen::Opens, Screen::Opens], None, false);
    activate(&host, &policy);

    let mut out: Vec<u8> = Vec::new();
    let execution = execute_policy_cycle_with(
        &host,
        &policy,
        PolicyTrigger::Recovery,
        &|| MAX_POLICY_CYCLE_DURATION,
        &mut out,
    )
    .expect("the cycle runs");
    let pending = &execution.cycle.pending_dispatch_intents;
    assert_eq!(pending.len(), 2, "one intent per instance");
    assert_eq!(execution.yielded_intents, 1);

    let report = String::from_utf8(out).expect("stdout text");
    let lines = report.lines().collect::<Vec<_>>();
    let [line] = lines.as_slice() else {
        panic!("one yield line: {report:?}")
    };
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    assert_eq!(tokens[..2], ["actingd", "policy_cycle_yielded"], "{line}");
    assert!(
        tokens.contains(&"attempted=1") && tokens.contains(&"remaining=1"),
        "{line}"
    );

    let events = ledger_events(root.path());
    let first = &pending[0].decision_id;
    assert!(
        dispatch_record(&events, first, EventType::PolicyDispatchIntent).is_some(),
        "the first intent is attempted"
    );
    assert!(
        dispatch_record(&events, first, EventType::PolicyDispatchCompleted).is_some(),
        "the first intent completes"
    );
    assert!(
        dispatch_records(&events, &pending[1].decision_id).is_empty(),
        "the yielded intent writes no ledger fact"
    );
    host.close().expect("the Runtime closes");
}
