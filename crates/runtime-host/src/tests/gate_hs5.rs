// SPDX-License-Identifier: AGPL-3.0-only

//! #402 HS-5b (register §1.277; design `rd-1031/backend-switch/model-hs5.md` §1, §2, §4.3, §5.2;
//! coordinator work order `brief-hs-5ab.md`): a Nemu-touch instance whose application does not
//! run behaves as an adb-touch one. The paired open takes the main display when the package has
//! no display (HS-5a), `MuMuManager`'s `stopped` resolves the application index 0, and the
//! resolution is dropped only by a rebinding; so every place that opens a session (a start's,
//! a resume's or an emulator control's preparation, a self-check, a fence's recheck, a client's,
//! a scheduled or a host's own run, a ladder rung, the Lab, a read-only observation, a backend
//! switch) opens it without the application and nothing is fenced.
//!
//! The morning of 11-04 (board #402 6621962566), in host terms: a switch to Nemu touch while
//! the application ran (one `app info` read, index 0 kept), a warm run, `ac_app stop`, then a
//! client's run of the startup package. Its first capture found no display for the package; the
//! open could not release the connection it had made, the instance was fenced and, the run being
//! a person's, waited for one.
//!
//! The device is the fake provider's, with a paired device (`FakePairedDevice`) that each test
//! sets: the paired open goes through a real registry entry of the instance (its index read and
//! its cache, the read answering from the fake application's state through the device layer's
//! own parser) and a fake open that asks the device layer which display a paired session takes
//! ([`actingcommand_device::nemu_paired_display`]); when it opens, the fake provider's own
//! backends serve the session. Each capture of the session looks the package's display up the
//! same way. The application runs or not as the application commands say; while it does not,
//! Android has the launcher in front and the screen is the desktop, which no package knows. No
//! device, adb, `MuMuManager` or emulator is touched; names, indices and ports are made up.

use super::gate_387_d1c::ladder_manifest;
use super::gate_403::{fence_records, status_fence};
use super::gate_hs3::{self as hs3, Opened, Reads};
use super::instance_workers::{
    admit_ladder_run, all_events, ladder_setup_with_packages, phases, rung_outcomes,
    wait_for_ladder_end, wait_until,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, ContainedLabOperationRequest,
    ContainedObservationRequest, DeviceBackendSwitchedPayload, EmulatorInstanceAction, FactScalar,
    LabOperationSelection, LabOperationStage, LabProjectionHint, NemuAppIndexSource,
    ObservationFramePlan, RecoveryRung, RecoveryRungOutcome, RuntimeConfigManifest,
    RuntimeLifecyclePhase, RuntimePayload, SchedulingPauseScope, SchedulingResumeSelfCheck,
    StartupPackageDisposition,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, CaptureOpenCause, CaptureOpenFailure,
    DeviceResourceQuiescence, DeviceTarget, MaaTouchConfig, TouchBackendChoice, TouchBackendConfig,
    mumu_instance_port, nemu_paired_display,
};
use actingcommand_execution_kernel::DiscoveredInstanceBinding;
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_config::{CAPTURE_BACKEND, TOUCH_BACKEND, instance_key};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::AtomicI32;

const A: &str = "hs5.instance.a";
const B: &str = "hs5.instance.b";
const PAIRED_OPEN_FAILED: &str = "paired_backend_open_failed";
const NOT_FOREGROUND: &str = "application_not_foreground";
/// A page the application shows that no package knows.
const STUCK: [u8; 3] = [0, 128, 0];
/// The application every instance is assigned (the fake provider's).
const APPLICATION: &str = "neutral.application";
/// What Android has in front while the application does not run.
const LAUNCHER: &str = "neutral.launcher";
/// The MuMu index and `MuMuManager` of the registry entries' discovery bindings.
const INDEX: u16 = 3;
const MUMU_MANAGER: &str = "neutral-mumu-manager";
/// The display Nemu answers for the application's package while it runs.
const PACKAGE_DISPLAY: i32 = 7;
/// The connection's main display (`NemuIpcConfig::display_id`, 0 when no key sets it).
const MAIN_DISPLAY: i32 = 0;
/// Pixel 0 of the application's home page, and of the desktop, which no package knows.
const HOME: [u8; 3] = [255, 255, 0];
const DESKTOP: [u8; 3] = [9, 9, 9];

// ---- The paired device.

/// The device of one instance: its application, its package's display, and every paired open,
/// `app info` read, capture and application command it saw.
#[derive(Default)]
struct Device {
    /// The application runs.
    running: AtomicBool,
    /// The instance's touch backend is Nemu IPC: its sessions are paired.
    nemu_touch: AtomicBool,
    /// After the application starts, this many captures still find no display for its package
    /// (the package's display comes up after the start).
    display_delay: AtomicUsize,
    pending: AtomicUsize,
    /// The page the application shows; a start shows its home.
    page: Mutex<Option<[u8; 3]>>,
    /// The display the last capture addressed.
    last_display: AtomicI32,
    /// `app info` reads.
    reads: AtomicUsize,
    /// Every paired open: the application index it was handed and the display it took, `None`
    /// for one that failed.
    opens: Mutex<Vec<(u32, Option<i32>)>>,
    /// The last paired open took a display (the fake provider's backends serve it).
    opened: AtomicBool,
    /// Every capture of a paired session: its display and the paired opens before it.
    captures: Mutex<Vec<(i32, usize)>>,
    /// Every application command: its action and the captures before it.
    commands: Mutex<Vec<(ApplicationLifecycleAction, usize)>>,
    /// While set, a paired open fails with it after the index is resolved.
    open_failure: Mutex<Option<DeviceError>>,
}

impl Device {
    fn running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::Acquire)
    }

    fn captures(&self) -> Vec<(i32, usize)> {
        self.captures.lock().expect("captures").clone()
    }

    fn commands(&self) -> Vec<(ApplicationLifecycleAction, usize)> {
        self.commands.lock().expect("commands").clone()
    }

    fn opens(&self) -> Vec<(u32, Option<i32>)> {
        self.opens.lock().expect("opens").clone()
    }

    /// What Nemu answers now for the package's display: none while the application does not
    /// run or its display is not up yet.
    fn package_display(&self) -> i32 {
        if self.running() && self.pending.load(Ordering::Acquire) == 0 {
            PACKAGE_DISPLAY
        } else {
            -1
        }
    }

    /// The fake paired open, after the registry resolved `index`: the display the device layer
    /// takes, or the open's failure. A successful open still answers an error, which the paired
    /// device turns into an open the fake provider's backends serve (a test opens no device).
    fn open(&self, index: u32) -> DeviceError {
        let mut opens = self.opens.lock().expect("opens");
        if let Some(failure) = self.open_failure.lock().expect("open failure").clone() {
            opens.push((index, None));
            return failure;
        }
        match nemu_paired_display(self.package_display(), MAIN_DISPLAY) {
            Ok(display) => {
                opens.push((index, Some(display)));
                self.opened.store(true, Ordering::Release);
                DeviceError::fatal("fixture paired open: the fake provider's backends serve it")
            }
            Err(failure) => {
                opens.push((index, None));
                failure
            }
        }
    }

    /// What the screen shows: the application's page on its package's display, the desktop
    /// on the main display (or, before Nemu touch, with the application not running).
    fn shown(&self) -> [u8; 3] {
        let application = if self.nemu_touch.load(Ordering::Acquire) {
            self.last_display.load(Ordering::Acquire) == PACKAGE_DISPLAY
        } else {
            self.running()
        };
        match *self.page.lock().expect("page") {
            Some(page) if application => page,
            _ => DESKTOP,
        }
    }

    fn foreground(&self) -> &'static str {
        if self.running() {
            APPLICATION
        } else {
            LAUNCHER
        }
    }
}

/// The paired device of one instance: its registry entry and its device.
struct Paired {
    registry: ExecutionBackendRegistry,
    device: Arc<Device>,
}

impl FakePairedDevice for Paired {
    fn open(
        &self,
        instance_alias: &str,
        memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<()> {
        self.device.opened.store(false, Ordering::Release);
        match self.registry.open_nemu_session(instance_alias, memory) {
            Ok(_) => Ok(()),
            Err(_) if self.device.opened.swap(false, Ordering::AcqRel) => Ok(()),
            Err(failure) => Err(failure),
        }
    }

    fn switch(
        &self,
        instance_alias: &str,
        touch: TouchBackendChoice,
        capture: CaptureBackendChoice,
    ) -> DeviceResult<Option<u32>> {
        let index = self
            .registry
            .switch_instance_backends(instance_alias, touch, capture)?;
        self.device
            .nemu_touch
            .store(touch == TouchBackendChoice::NemuIpc, Ordering::Release);
        Ok(index)
    }

    fn resolved_index(&self, instance_alias: &str) -> Option<u32> {
        self.registry.resolved_nemu_app_index(instance_alias)
    }

    fn application(&self, action: ApplicationLifecycleAction) {
        let device = &self.device;
        let captures = device.captures.lock().expect("captures").len();
        device
            .commands
            .lock()
            .expect("commands")
            .push((action, captures));
        match action {
            ApplicationLifecycleAction::Launch | ApplicationLifecycleAction::Restart => {
                device.running.store(true, Ordering::Release);
                device.pending.store(
                    device.display_delay.load(Ordering::Acquire),
                    Ordering::Release,
                );
                *device.page.lock().expect("page") = Some(HOME);
            }
            ApplicationLifecycleAction::Stop => device.running.store(false, Ordering::Release),
        }
    }

    fn capture(&self) -> DeviceResult<()> {
        let device = &self.device;
        if !device.nemu_touch.load(Ordering::Acquire) {
            return Ok(());
        }
        let package = device.package_display();
        if device.running() {
            let pending = device.pending.load(Ordering::Acquire);
            device
                .pending
                .store(pending.saturating_sub(1), Ordering::Release);
        }
        let display = nemu_paired_display(package, MAIN_DISPLAY)?;
        device.last_display.store(display, Ordering::Release);
        let opens = device.opens.lock().expect("opens").len();
        device
            .captures
            .lock()
            .expect("captures")
            .push((display, opens));
        Ok(())
    }
}

/// The registry entry of `alias`: touch `touch` and Nemu capture, bound through discovery to
/// `INDEX` on its base port (so the CAP-IDX port check reads nothing), without
/// `nemu_app_index`. Its `app info` read answers the device's application state through the
/// device layer's parser; its paired open is the device's.
fn registry(
    alias: &str,
    instance: InstanceId,
    touch: TouchBackendChoice,
    device: &Arc<Device>,
) -> ExecutionBackendRegistry {
    let target = DeviceTarget {
        port: mumu_instance_port(INDEX).expect("base port"),
        ..DeviceTarget::default()
    };
    let adb = AdbConfig {
        adb_path: "neutral-adb".to_owned(),
        ..AdbConfig::default()
    };
    let registration = ExecutionBackendRegistration::new(
        alias,
        instance,
        APPLICATION,
        TouchBackendConfig::new(adb.clone(), target.clone(), MaaTouchConfig::default())
            .with_requested(touch),
        CaptureBackendConfig::new(adb, target).with_requested(CaptureBackendChoice::NemuIpc),
    )
    .expect("registration")
    .with_discovered_binding(DiscoveredInstanceBinding::new(
        INDEX,
        "neutral-instance",
        "neutral-provider-version",
        MUMU_MANAGER,
    ));
    let reader = Arc::clone(device);
    let opener = Arc::clone(device);
    ExecutionBackendRegistry::new([registration])
        .expect("registry")
        .with_instance_state_reader(|_, _| {
            Err(DeviceError::fatal("fixture: no MuMuManager is read here"))
        })
        .with_application_state_reader(move |_, index, package| {
            reader.reads.fetch_add(1, Ordering::AcqRel);
            let state = if reader.running() {
                "running"
            } else {
                "stopped"
            };
            actingcommand_device::parse_application_state(
                &json!({ "state": state }),
                index,
                package,
            )
        })
        .with_nemu_session_opener(move |_, application, _, _| {
            Err(opener.open(application.app_index().index()))
        })
}

/// One instance of a test: its alias, id, fake device state and paired device.
struct Node {
    alias: &'static str,
    id: InstanceId,
    state: Arc<FakeState>,
    device: Arc<Device>,
}

impl Node {
    /// An instance with touch `touch` and Nemu capture whose application runs or not; its
    /// opens report a passed native open.
    fn new(alias: &'static str, touch: TouchBackendChoice, running: bool) -> Self {
        let id = instance_id();
        let state = Arc::new(FakeState::default());
        state.observed_open.store(true, Ordering::Release);
        state.physical_task_geometry.store(true, Ordering::Release);
        *state.backends.lock().expect("fake backends") =
            Some((touch.as_str().to_owned(), "nemu_ipc".to_owned()));
        let device = Arc::new(Device::default());
        device.running.store(running, Ordering::Release);
        device
            .nemu_touch
            .store(touch == TouchBackendChoice::NemuIpc, Ordering::Release);
        if running {
            *device.page.lock().expect("page") = Some(HOME);
        }
        *state.paired.lock().expect("fake paired device") = Some(Arc::new(Paired {
            registry: registry(alias, id, touch, &device),
            device: Arc::clone(&device),
        }));
        let shown = Arc::clone(&device);
        *state.pixel_script.lock().expect("pixel script") =
            Some(Arc::new(move |_: usize, _: usize| vec![(0, shown.shown())]));
        let front = Arc::clone(&device);
        *state.foreground_script.lock().expect("foreground script") =
            Some(Arc::new(move |_: usize, _: usize| {
                Some(front.foreground().to_owned())
            }));
        Self {
            alias,
            id,
            state,
            device,
        }
    }

    /// The file layer's two backend rows of the instance (explicit, as actingd reports them).
    fn rows(&self, touch: &str) -> Vec<ConfigParameter> {
        [
            (touch_key(self.id), touch),
            (capture_key(self.id), "nemu_ipc"),
        ]
        .into_iter()
        .map(|(key, value)| ConfigParameter {
            key,
            value: FactScalar::String(value.to_owned()),
            source: ConfigParameterSource::Explicit,
            settable: false,
        })
        .collect()
    }
}

fn id_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

fn touch_key(instance: InstanceId) -> String {
    instance_key(&id_text(instance), TOUCH_BACKEND)
}

fn capture_key(instance: InstanceId) -> String {
    instance_key(&id_text(instance), CAPTURE_BACKEND)
}

fn manifest(rows: Vec<ConfigParameter>) -> RuntimeConfigManifest {
    let mut parameters = rows;
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn provider(nodes: &[&Node]) -> FakeProvider {
    FakeProvider::from_entries(
        nodes
            .iter()
            .map(|node| (node.alias.to_owned(), node.id, Arc::clone(&node.state))),
    )
}

/// The application entry package (design §7 P3, neutral): from any screen it restarts the
/// instance's application, to `home`; its recognition pack knows only `home`, by pixel 0.
fn app_entry_package(root: &TempDir) -> PathBuf {
    let space = json!({"width": 16, "height": 9});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.test.startup",
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
    let path = root.path().join("app-entry.zip");
    fs::write(&path, zip.finish().expect("finish zip").into_inner())
        .expect("write the application entry package");
    path
}

fn send(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("the request's receipt")
}

/// A person's run of `package` on `alias` (`run start`, `ac_run_pack`: a direct task run).
fn direct_run(host: &RuntimeHost, alias: &str, package: &Path, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::run_contained_task(
                alias,
                ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
            ),
        ),
        connection,
    )
}

/// `actingctl app <action>` (`ac_app`) of `alias`.
fn application(
    host: &RuntimeHost,
    alias: &str,
    action: ApplicationLifecycleAction,
    connection: u64,
) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::ApplicationLifecycle {
                instance_alias: alias.to_owned(),
                holder_id: *ids.mint_holder_id().expect("holder").transport(),
                action,
            },
        ),
        connection,
    )
}

/// The switch of `node` to Nemu touch and capture, as `actingctl config set` sends it.
fn switch_to_nemu(host: &RuntimeHost, node: &Node, connection: u64) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let set = |key: String| ConfigSettingChange {
        key,
        value: Some(json!("nemu_ipc")),
    };
    send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::ConfigureSettings {
                changes: vec![set(touch_key(node.id)), set(capture_key(node.id))],
                expected_version: None,
            },
        ),
        connection,
    )
}

/// The `device.backend_switched` records so far.
fn switched_records(host: &RuntimeHost) -> Vec<DeviceBackendSwitchedPayload> {
    all_events(host)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::DeviceBackendSwitched(payload)) => {
                Some(payload.clone())
            }
            _ => None,
        })
        .collect()
}

fn assert_completed_on_home(receipt: &RuntimeReceipt) {
    assert!(
        matches!(receipt.result(), Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success, final_page: Some(page), ..
        }) if page == "neutral/home"),
        "the run completes on home: {receipt:?}"
    );
}

/// No fence anywhere, and `alias` neither fenced nor waiting for a person.
fn assert_unfenced(host: &RuntimeHost, alias: &str) {
    let events = all_events(host);
    let fences = fence_records(&events);
    assert!(fences.is_empty(), "no instance is fenced: {fences:#?}");
    assert_eq!(status_fence(host, alias), None);
    assert!(host.fatal_error().expect("runtime health").is_none());
}

/// The displays of the captures after the application command `index` (in order), and of the
/// one just before it, if any.
fn around_command(device: &Device, index: usize) -> (Option<i32>, Vec<(i32, usize)>) {
    let (_, before) = device.commands()[index];
    let captures = device.captures();
    (
        before
            .checked_sub(1)
            .and_then(|last| captures.get(last))
            .map(|capture| capture.0),
        captures[before..].to_vec(),
    )
}

// The morning of 11-04: the switch to Nemu touch while the application ran read `app info` once
// and kept index 0; a warm run completed; `ac_app stop`; then a person's run of the startup
// package. Required: it completes. Its first frame is the main display's (the desktop, no page),
// its application effect restarts the application, and the frames after it are the package's
// display, on home. Nothing is fenced, nobody is waited for, and `app info` was read only once.
#[test]
fn hs5_morning_sequence_client_startup_after_stop() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::AdbShellInput, true);
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(manifest(a.rows("adb_shell_input"))),
        Arc::new(provider(&[&a])),
    )
    .expect("runtime host");
    let package = app_entry_package(&root);

    let switched = switch_to_nemu(&host, &a, 5101);
    assert_eq!(
        switched.state(),
        RuntimeReceiptState::Completed,
        "{switched:?}"
    );
    let [record]: [DeviceBackendSwitchedPayload; 1] = switched_records(&host)
        .try_into()
        .expect("one switch record");
    assert_eq!(record.nemu_app_index(), Some(0));
    assert_eq!(
        record.nemu_app_index_source(),
        Some(NemuAppIndexSource::Resolved)
    );
    assert_eq!(a.device.reads(), 1, "the switch read app info once");

    assert_completed_on_home(&direct_run(&host, A, &package, 5102));
    let stopped = application(&host, A, ApplicationLifecycleAction::Stop, 5103);
    assert_eq!(
        stopped.state(),
        RuntimeReceiptState::Completed,
        "{stopped:?}"
    );
    assert!(!a.device.running());

    let receipt = direct_run(&host, A, &package, 5104);

    assert_completed_on_home(&receipt);
    let commands = a.device.commands();
    assert_eq!(
        commands
            .iter()
            .map(|(action, _)| *action)
            .collect::<Vec<_>>(),
        [
            ApplicationLifecycleAction::Stop,
            ApplicationLifecycleAction::Restart
        ],
        "the stop, then the run's application effect"
    );
    let (before, after) = around_command(&a.device, 1);
    assert_eq!(
        before,
        Some(MAIN_DISPLAY),
        "the run's first frame is the main display's: {:?}",
        a.device.captures()
    );
    assert!(
        !after.is_empty() && after.iter().all(|(display, _)| *display == PACKAGE_DISPLAY),
        "after the effect the frames are the package's display: {after:?}"
    );
    assert_eq!(a.device.reads(), 1, "app info was read only once");
    assert!(
        a.device.opens().iter().all(|(index, _)| *index == 0),
        "every paired open took index 0: {:?}",
        a.device.opens()
    );
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// ---- More of the host's ways in.

/// A host of `nodes` on the real Runtime clock; with `endpoints`, each instance has a bound ADB
/// endpoint of its own, so the start prepares it.
fn start(root: &TempDir, nodes: &[&Node], endpoints: bool) -> RuntimeHost {
    let provider = provider(nodes);
    let provider = if endpoints {
        provider.with_device_endpoints()
    } else {
        provider
    };
    RuntimeHost::start(config(root), Arc::new(provider)).expect("runtime host")
}

/// A paired open that acquired its connection and could not release it (the 11-04 open, before
/// HS-5a): unconfirmed.
fn unreleased_open() -> DeviceError {
    DeviceError::fatal("fixture paired open: its own connection stayed open")
        .with_resource_summary(DeviceResourceQuiescence::Unconfirmed, 1)
}

/// A paired open that failed after its connection and released it (HS-5a R1): confirmed.
fn released_open() -> DeviceError {
    DeviceError::fatal(
        "fixture paired open: it failed after connecting and released the connection",
    )
    .with_resource_summary(DeviceResourceQuiescence::Confirmed, 1)
}

/// The preparations of `instance` that finished so far, in ledger order: whether each passed.
fn preparations(host: &RuntimeHost, instance: InstanceId) -> Vec<bool> {
    phases(&all_events(host))
        .into_iter()
        .filter(|(event, _)| event.links().instance_id() == Some(&instance))
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::InstancePreparationFinished { outcome, .. } => {
                Some(outcome.is_none())
            }
            _ => None,
        })
        .collect()
}

fn ladder_started(host: &RuntimeHost) -> bool {
    phases(&all_events(host))
        .iter()
        .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. }))
}

/// The outcomes of the fence lifts so far.
fn lifts(host: &RuntimeHost) -> Vec<Outcome> {
    phases(&all_events(host))
        .into_iter()
        .filter_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::ResourceFenceLifted { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect()
}

/// How many contained tasks ended (completed, failed or cancelled), and how many completed.
fn task_ends(host: &RuntimeHost) -> (usize, usize) {
    let events = all_events(host);
    let ended = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type(),
                EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
            )
        })
        .count();
    let completed = events
        .iter()
        .filter(|event| event.event_type() == EventType::TaskCompleted)
        .count();
    (ended, completed)
}

fn assert_passed(selfcheck: &SchedulingResumeSelfCheck) {
    assert!(
        selfcheck.capture.ok && selfcheck.touch.ok && selfcheck.failure_code.is_none(),
        "{selfcheck:?}"
    );
}

/// `actingctl selfcheck` of `alias`.
fn self_check(host: &RuntimeHost, alias: &str, connection: u64) -> SchedulingResumeSelfCheck {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::SelfCheckInstance {
                instance_alias: alias.to_owned(),
            },
        ),
        connection,
    );
    let Some(RuntimeResult::InstanceSelfChecked { selfcheck, .. }) = receipt.result() else {
        panic!("a self-check result: {receipt:?}");
    };
    selfcheck.clone()
}

/// `actingctl pause` of `alias`, as the operator sends it before a resume (a resume needs the
/// instance paused): it completes.
fn pause(host: &RuntimeHost, alias: &str, connection: u64) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::PauseScheduling {
                scope: SchedulingPauseScope::Instance {
                    instance_alias: alias.to_owned(),
                },
                pause_reason: actingcommand_contract::SchedulingPauseReason::new("hs5_check")
                    .expect("pause reason"),
                drain_timeout_ms: 1_000,
            },
        ),
        connection,
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
}

/// `actingctl resume` of `alias` (paused before): the self-check its reconnect made.
fn resume(host: &RuntimeHost, alias: &str, connection: u64) -> SchedulingResumeSelfCheck {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let receipt = send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::ResumeScheduling {
                scope: SchedulingPauseScope::Instance {
                    instance_alias: alias.to_owned(),
                },
                expected: None,
            },
        ),
        connection,
    );
    let Some(RuntimeResult::SchedulingResumed {
        selfcheck: Some(selfcheck),
        ..
    }) = receipt.result()
    else {
        panic!("an instance resume with its self-check: {receipt:?}");
    };
    selfcheck.clone()
}

/// `actingctl emulator <action>` of `alias`.
fn control_emulator(
    host: &RuntimeHost,
    alias: &str,
    action: EmulatorInstanceAction,
    connection: u64,
) -> RuntimeReceipt {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    send(
        host,
        &runtime_request(
            &ids,
            RuntimeOperation::ControlEmulatorInstance {
                instance_alias: alias.to_owned(),
                action,
            },
        ),
        connection,
    )
}

/// A request from the Lab (`actinglab`).
fn lab_request(operation: RuntimeOperation) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        unix_ms_now().expect("wall clock"),
        operation,
    )
    .expect("runtime request")
}

/// `actinglab do --tap 1,0 --package <package>` on `alias`: the operation's failure, if any.
fn lab_tap(root: &TempDir, alias: &str, package: &Path) -> Option<(String, LabOperationStage)> {
    let lab = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        RuntimeChannel::Lab,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .expect("Lab client");
    let verified = lab
        .begin_debug_session()
        .expect("debug session")
        .run_contained_lab_operation(
            alias,
            ContainedLabOperationRequest {
                package_path: package.display().to_string(),
                selection: LabOperationSelection::Coordinates {
                    action: InputAction::Tap { x: 1, y: 0 },
                },
                projection_hint: LabProjectionHint {
                    sequence: None,
                    content_sha256: None,
                },
                after: None,
            },
            ObservationFramePlan::SINGLE,
        )
        .expect("the Lab client verifies the operation");
    let failure = verified
        .operation()
        .record
        .failure
        .as_ref()
        .map(|failure| (failure.code.clone(), failure.stage));
    drop(lab);
    failure
}

/// The application command `action`'s captures after it: the first one's display, and the
/// first one of the package's display, each with the paired opens before it.
fn captures_after(device: &Device, action: ApplicationLifecycleAction) -> Vec<(i32, usize)> {
    let index = device
        .commands()
        .iter()
        .position(|(command, _)| *command == action)
        .expect("the application command");
    around_command(device, index).1
}

/// After the application's start: the first frame is the main display's, a later one the
/// package's display's, and no paired open came between them (one session).
fn assert_main_then_package(after: &[(i32, usize)]) {
    assert_eq!(
        after.first().map(|capture| capture.0),
        Some(MAIN_DISPLAY),
        "the first frame after the start: {after:?}"
    );
    let package = after
        .iter()
        .find(|capture| capture.0 == PACKAGE_DISPLAY)
        .expect("a frame of the package's display");
    assert_eq!(
        package.1, after[0].1,
        "no paired open between the two frames: {after:?}"
    );
}

// §2 rows 1–3: a start's preparation, a resume's reconnect and an emulator restart's
// preparation, the application not running: each self-check passes, the instance is usable,
// nothing is fenced and no ladder starts.
#[test]
fn hs5_preparations_pass_without_application() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], true);
    wait_until("the start's preparation", || {
        !preparations(&host, a.id).is_empty()
    });
    assert_eq!(
        preparations(&host, a.id),
        [true],
        "the start's self-check passes"
    );
    pause(&host, A, 5110);
    assert_passed(&resume(&host, A, 5111));
    assert_eq!(a.device.reads(), 1, "the index was read once and kept");
    assert!(
        a.device
            .opens()
            .iter()
            .all(|open| *open == (0, Some(MAIN_DISPLAY))),
        "every open took the main display: {:?}",
        a.device.opens()
    );
    assert!(!ladder_started(&host));
    assert!(!a.device.running(), "nothing started the application");
    assert_unfenced(&host, A);
    host.close().expect("close host");

    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    bind_fake_emulator(&host, A, a.id, &a.state);
    let before = preparations(&host, a.id).len();
    let restarted = control_emulator(&host, A, EmulatorInstanceAction::Restart, 5112);
    assert_eq!(
        restarted.state(),
        RuntimeReceiptState::Completed,
        "{restarted:?}"
    );
    wait_until("the restart's preparation", || {
        preparations(&host, a.id).len() > before
    });
    assert_eq!(
        preparations(&host, a.id)[before..].to_vec(),
        vec![true],
        "the restart's self-check passes"
    );
    assert!(!ladder_started(&host));
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §2 row 4: a person's run fenced the instance (its paired open could not release its
// connection) and it waits for a person. With the application still not running, the fence's
// recheck passes and lifts it (`instance_resources_fence_recheck_passed`), and a person's
// self-check passes too.
#[test]
fn hs5_selfcheck_and_recheck_pass_without_application() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(clock.clone())
            .with_config_manifest(ladder_manifest(json!({
                "fence_recheck": {"interval_ms": 60_000, "max_attempts": 3}
            }))),
        Arc::new(provider(&[&a])),
    )
    .expect("runtime host");
    let package = app_entry_package(&root);
    *a.device.open_failure.lock().expect("open failure") = Some(unreleased_open());
    let fenced = direct_run(&host, A, &package, 5121);
    assert_eq!(
        fenced.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "{fenced:?}"
    );
    let fence = status_fence(&host, A).expect("the fence");
    assert!(fence.awaiting_person, "{fence:?}");
    assert_eq!(fence.cause.as_str(), PAIRED_OPEN_FAILED);
    *a.device.open_failure.lock().expect("open failure") = None;

    let next = fence
        .person_wait
        .and_then(|wait| wait.next_recheck_unix_ms)
        .expect("a next recheck");
    clock.advance(next.saturating_sub(clock.sample().expect("manual clock").unix_ms));
    host.fence_recheck_tick_for_test().expect("sweep tick");
    wait_until("the fence's lift", || !lifts(&host).is_empty());
    let lifted = lifts(&host);
    let [outcome] = lifted.as_slice() else {
        panic!("one lift: {lifted:#?}");
    };
    assert_eq!(
        outcome.code().as_str(),
        HostCode::InstanceResourcesFenceRecheckPassed.as_str()
    );
    assert_eq!(status_fence(&host, A), None);
    assert!(!a.device.running(), "the recheck started no application");

    assert_passed(&self_check(&host, A, 5122));
    assert_eq!(status_fence(&host, A), None);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// §2 row 5 (the morning's kind of run): a person's run of the application entry package, the
// application not running. Its first frame is the main display's (the desktop, no page), the
// application effect starts the application, and the run arrives on home on the package's
// display. Nothing is fenced.
#[test]
fn hs5_direct_run_application_entry_without_application() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let package = app_entry_package(&root);

    assert_completed_on_home(&direct_run(&host, A, &package, 5131));

    let (before, after) = around_command(&a.device, 0);
    assert_eq!(before, Some(MAIN_DISPLAY), "{:?}", a.device.captures());
    assert!(
        !after.is_empty() && after.iter().all(|capture| capture.0 == PACKAGE_DISPLAY),
        "{after:?}"
    );
    assert_eq!(
        a.state.application_count.load(Ordering::Acquire),
        1,
        "one application effect"
    );
    assert_eq!(a.device.reads(), 1);
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §2 row 5, a client's lease: the run waits behind the client's lease and runs once the client
// gives it back, as above.
#[test]
fn hs5_leased_run_application_entry_without_application() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let package = app_entry_package(&root);
    let mut holder = TestClient::connect(&host);
    let (_, token) = holder.acquire(A);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            A,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let release = holder.request(RuntimeOperation::ReleaseLease { token });

    let receipt = thread::scope(|scope| {
        let waiting = scope.spawn(|| {
            host.process_request_for_test(&run, ConnectionId::new(5141).expect("connection"))
        });
        wait_until("the run queued behind the client's lease", || {
            !host
                .query_persisted_events_for_test(EventQuery {
                    event_type: Some(EventType::SchedulerQueued),
                    request_id: Some(run.request_id()),
                    ..EventQuery::default()
                })
                .expect("queued run")
                .is_empty()
        });
        assert_eq!(
            holder.send(&release).state(),
            RuntimeReceiptState::Completed
        );
        waiting
            .join()
            .expect("the waiting run")
            .expect("the run's receipt")
    });

    assert_completed_on_home(&receipt);
    let (before, after) = around_command(&a.device, 0);
    assert_eq!(before, Some(MAIN_DISPLAY), "{:?}", a.device.captures());
    assert!(
        after.iter().any(|capture| capture.0 == PACKAGE_DISPLAY),
        "{after:?}"
    );
    assert_unfenced(&host, A);
    drop(holder);
    host.close().expect("close host");
}

// §2 row 5, a scheduled run: the policy's run of the application entry package, the application
// not running, completes as a person's does; no ladder starts.
#[test]
fn hs5_scheduled_run_application_entry_without_application() {
    let root = TempDir::new().expect("tempdir");
    let entry = fs::read(app_entry_package(&root)).expect("the package's bytes");
    let (host_config, request) = ladder_setup_with_packages(
        root.path(),
        &entry,
        &neutral_non_home_start_contained_task_package(true),
    );
    let a = Node::new(POLICY_INSTANCE_ALIAS, TouchBackendChoice::NemuIpc, false);
    let host = RuntimeHost::start(
        host_config,
        Arc::new(provider(&[&a]).with_device_endpoints()),
    )
    .expect("runtime host");
    wait_until("the start's preparation", || {
        !preparations(&host, a.id).is_empty()
    });
    let context = admit_ladder_run(&host, &request);

    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the scheduled run");

    assert_completed_on_home(&receipt);
    let (before, after) = around_command(&a.device, 0);
    assert_eq!(before, Some(MAIN_DISPLAY), "{:?}", a.device.captures());
    assert!(
        after.iter().any(|capture| capture.0 == PACKAGE_DISPLAY),
        "{after:?}"
    );
    assert!(!ladder_started(&host), "no ladder");
    assert_unfenced(&host, POLICY_INSTANCE_ALIAS);
    host.close().expect("close host");
}

// §2 row 6: the ladder's application restart rung. The application shows a page no package
// knows, so the scheduled run and the return-home rung fail; the restart rung starts the
// application before any capture, and its package's display comes up a frame later. The rung's
// first frame is the main display's, the next the package's display's, on one session, and the
// rung recovers.
//
// #402 HS-5d (register §1.279): on a manual Runtime clock. The rung's arrival wait is the entry
// package's step window (600 ms) on the Runtime clock. On the system clock the work around the
// rung's first frame (its capture, the frame's records, the test thread reading the ledger) could
// spend the window before the second frame was taken, and the rung failed
// `page_confirmation_failed` (the CI of #1521). Here only the kernel's own waits move the clock,
// 10 ms between two frames, and the fake's display follows the count of captures since the
// start, so the frames and the window are the same on every run.
#[test]
fn hs5_ladder_application_rung_opens_after_entry() {
    let root = TempDir::new().expect("tempdir");
    let entry = app_entry_package(&root);
    let (host_config, request) = ladder_setup_with_packages(
        root.path(),
        &neutral_contained_task_package(true),
        &neutral_non_home_start_contained_task_package(true),
    );
    let host_config = host_config
        .with_runtime_clock(Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0)))
        .with_startup_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            ContainedTaskRequest::new(entry.display().to_string()).expect("startup package"),
        )]));
    let a = Node::new(POLICY_INSTANCE_ALIAS, TouchBackendChoice::NemuIpc, true);
    *a.device.page.lock().expect("page") = Some(STUCK);
    a.device.display_delay.store(1, Ordering::Release);
    let host = RuntimeHost::start(
        host_config,
        Arc::new(provider(&[&a]).with_device_endpoints()),
    )
    .expect("runtime host");
    wait_until("the start's preparation", || {
        !preparations(&host, a.id).is_empty()
    });
    let context = admit_ladder_run(&host, &request);
    host.run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails on a page no package knows");
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);

    let rungs = rung_outcomes(&all_events(&host));
    assert!(
        rungs.contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Recovered,
            None
        )),
        "{rungs:?}"
    );
    assert_main_then_package(&captures_after(
        &a.device,
        ApplicationLifecycleAction::Restart,
    ));
    assert_unfenced(&host, POLICY_INSTANCE_ALIAS);
    host.close().expect("close host");
}

// §2 row 6: the host's own run of the startup package, the application not running: the
// application effect comes first, its package's display a frame later; the first frame is the
// main display's, the next the package's display's, on one session, and the package completes.
//
// #402 HS-5d (register §1.279): on a manual Runtime clock, for the reason the ladder's test above
// gives: its arrival wait also needs the second frame inside the package's 600 ms step window.
#[test]
fn hs5_startup_package_opens_after_entry() {
    let root = TempDir::new().expect("tempdir");
    let package = app_entry_package(&root);
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    a.device.display_delay.store(1, Ordering::Release);
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
            .with_startup_packages(BTreeMap::from([(
                A.to_owned(),
                ContainedTaskRequest::new(package.display().to_string()).expect("startup package"),
            )])),
        Arc::new(provider(&[&a])),
    )
    .expect("runtime host");

    assert_eq!(
        host.schedule_startup_package_for_test(A)
            .expect("schedule the startup package"),
        StartupPackageDisposition::Scheduled
    );
    wait_until("the startup package's end", || task_ends(&host).0 > 0);

    assert_eq!(task_ends(&host), (1, 1), "the startup package completed");
    assert_main_then_package(&captures_after(
        &a.device,
        ApplicationLifecycleAction::Restart,
    ));
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §2 row 7, §4.4: a tap while the application does not run is refused by the foreground gate
// (`application_not_foreground`, nothing pressed, nothing fenced), as on adb touch; the reset
// completes.
#[test]
fn hs5_pointer_input_before_application_refused_by_foreground_gate() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let package = app_entry_package(&root);

    assert_eq!(
        lab_tap(&root, A, &package),
        Some((NOT_FOREGROUND.to_owned(), LabOperationStage::Input))
    );
    assert!(
        a.state
            .input_actions
            .lock()
            .expect("input actions")
            .is_empty(),
        "nothing was pressed"
    );
    assert_unfenced(&host, A);

    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let reset = send(
        &host,
        &runtime_request(
            &ids,
            RuntimeOperation::safe_reset(A, ids.mint_holder_id().expect("holder")),
        ),
        5171,
    );
    assert_eq!(reset.state(), RuntimeReceiptState::Completed, "{reset:?}");
    assert!(
        matches!(
            reset.result(),
            Some(RuntimeResult::SafeResetCompleted { .. })
        ),
        "{reset:?}"
    );
    assert_eq!(
        *a.state.input_actions.lock().expect("input actions"),
        [InputAction::Reset],
        "only the reset reached the device"
    );
    assert!(!a.device.running());
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §2 row 8: the switch of an instance to Nemu touch and capture while its application does not
// run: the candidate's paired open reads `stopped`, resolves index 0, takes the main display,
// and the switch commits with the index's source `resolved`.
#[test]
fn hs5_switch_to_nemu_without_application_commits() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::AdbShellInput, false);
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(manifest(a.rows("adb_shell_input"))),
        Arc::new(provider(&[&a])),
    )
    .expect("runtime host");

    let receipt = switch_to_nemu(&host, &a, 5181);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let [record]: [DeviceBackendSwitchedPayload; 1] = switched_records(&host)
        .try_into()
        .expect("one switch record");
    assert_eq!(record.nemu_app_index(), Some(0));
    assert_eq!(
        record.nemu_app_index_source(),
        Some(NemuAppIndexSource::Resolved)
    );
    assert_eq!(a.device.reads(), 1);
    assert_eq!(a.device.opens(), [(0, Some(MAIN_DISPLAY))]);
    assert!(!a.device.running());
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §2 row 9: the Lab, the application not running: `observe --capture --package` gets a frame
// (the main display's, no page); `do --tap` is refused by the foreground gate; nothing is fenced.
#[test]
fn hs5_lab_observe_and_do_without_application() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let package = app_entry_package(&root);

    let observed = send(
        &host,
        &lab_request(RuntimeOperation::ObserveContainedPage {
            instance_alias: A.to_owned(),
            request: ContainedObservationRequest::new(package.display().to_string(), Vec::new())
                .expect("observation request"),
        }),
        5191,
    );
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Observed,
        "{observed:?}"
    );
    assert!(
        matches!(
            observed.result(),
            Some(RuntimeResult::ContainedPageObserved { .. })
        ),
        "{observed:?}"
    );
    assert_eq!(
        a.device.captures().first().map(|capture| capture.0),
        Some(MAIN_DISPLAY)
    );
    assert_eq!(
        lab_tap(&root, A, &package),
        Some((NOT_FOREGROUND.to_owned(), LabOperationStage::Input))
    );
    assert!(
        a.state
            .input_actions
            .lock()
            .expect("input actions")
            .is_empty()
    );
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §2 row 10: a read-only observation (the console's single-frame refresh), the application not
// running: it gets a frame, the main display's; nothing is fenced.
#[test]
fn hs5_readonly_observe_without_application() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let mut client = TestClient::connect(&host);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: A.to_owned(),
    });

    let receipt = client.send(&observe);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ReadonlyObservationCompleted { .. })
        ),
        "{receipt:?}"
    );
    assert_eq!(
        a.device.captures().first().map(|capture| capture.0),
        Some(MAIN_DISPLAY)
    );
    assert_unfenced(&host, A);
    drop(client);
    host.close().expect("close host");
}

// HS-5b (§4.3, HS3-2, HS3-3): `MuMuManager` reads the package `stopped`: the paired open gets
// index 0 as for `running`, and the resolution is the entry's. Replaces
// `hs3_stopped_refused_named`.
#[test]
fn hs5_stopped_resolves_zero() {
    let (registry, reads, opens) = hs3::answering_registry(
        hs3::registration(TouchBackendChoice::NemuIpc),
        json!({"state": "stopped"}),
        hs3::no_device,
    );

    let error = hs3::open_fails(&registry, hs3::ALIAS);

    assert_eq!(error.nemu_app_index_problem(), None, "{error:?}");
    hs3::assert_one_read(&reads);
    assert_eq!(hs3::opened_indices(&opens), vec![0]);
    assert_eq!(registry.resolved_nemu_app_index(hs3::ALIAS), Some(0));
}

/// A paired open failure as one without the package's display reported it before 0.12.40.
fn display_missing() -> DeviceError {
    DeviceError::fatal("fixture Nemu application display query failed").with_capture_open_failure(
        CaptureOpenFailure {
            cause: CaptureOpenCause::DisplayMissing,
            application_id: Some(hs3::PACKAGE.to_owned()),
            instance_index: Some(i64::from(INDEX)),
        },
    )
}

// HS-5b (§4.3, HS3-4): only a rebinding drops the resolution. The application stops (and an
// open reports no display for its package), starts again: no further read; a rebinding drops it
// and the next open reads again. Replaces `hs3_display_missing_clears_cache`.
#[test]
fn hs5_resolution_kept_across_application_stop() {
    let running = Arc::new(AtomicBool::new(true));
    let reads = Reads::default();
    let opens = Opened::default();
    let answered = Arc::clone(&running);
    let counted = Arc::clone(&reads);
    let registry = ExecutionBackendRegistry::new([hs3::registration(TouchBackendChoice::NemuIpc)])
        .expect("registry")
        .with_application_state_reader(move |manager, index, package| {
            counted
                .lock()
                .expect("reads")
                .push((manager.to_path_buf(), index, package.to_owned()));
            let state = if answered.load(Ordering::Acquire) {
                "running"
            } else {
                "stopped"
            };
            actingcommand_device::parse_application_state(
                &json!({ "state": state }),
                index,
                package,
            )
        })
        .with_nemu_session_opener(hs3::opener(Arc::clone(&opens), display_missing));

    hs3::open_fails(&registry, hs3::ALIAS);
    hs3::assert_one_read(&reads);

    running.store(false, Ordering::Release);
    let error = hs3::open_fails(&registry, hs3::ALIAS);
    assert_eq!(
        error.capture_open_failure().map(|failure| failure.cause),
        Some(CaptureOpenCause::DisplayMissing)
    );
    assert_eq!(registry.resolved_nemu_app_index(hs3::ALIAS), Some(0));
    running.store(true, Ordering::Release);
    hs3::open_fails(&registry, hs3::ALIAS);
    assert_eq!(
        hs3::read_count(&reads),
        1,
        "the application's stop and start keep it"
    );
    assert_eq!(hs3::opened_indices(&opens), vec![0, 0, 0]);

    running.store(false, Ordering::Release);
    registry
        .rebind_discovered_endpoint(
            hs3::ALIAS,
            Some(mumu_instance_port(INDEX).expect("base port")),
        )
        .expect("rebind after a start");
    assert_eq!(registry.resolved_nemu_app_index(hs3::ALIAS), None);
    hs3::open_fails(&registry, hs3::ALIAS);
    assert_eq!(hs3::read_count(&reads), 2, "a rebinding drops it");
    assert_eq!(registry.resolved_nemu_app_index(hs3::ALIAS), Some(0));
}

// §5.2: a paired open that fails after it connected and released what it acquired (HS-5a R1)
// fails only the run (#411 A7) and the self-check; nothing is fenced.
#[test]
fn hs5_open_failure_after_connect_fails_only_the_run() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let package = app_entry_package(&root);
    *a.device.open_failure.lock().expect("open failure") = Some(released_open());

    let receipt = direct_run(&host, A, &package, 5201);

    assert_ne!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(
        !receipt.error_projection().expect("the run's failure").fatal,
        "{receipt:?}"
    );
    let selfcheck = self_check(&host, A, 5202);
    assert_eq!(
        selfcheck.failure_code.as_deref(),
        Some(PAIRED_OPEN_FAILED),
        "{selfcheck:?}"
    );
    assert_unfenced(&host, A);
    host.close().expect("close host");
}

// §5.2, Workflow #403 unchanged: a paired open whose own resources stay unconfirmed (a worker
// that timed out) fences its instance; a person's run then waits for a person.
#[test]
fn hs5_unconfirmed_open_still_fences() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a], false);
    let package = app_entry_package(&root);
    *a.device.open_failure.lock().expect("open failure") = Some(unreleased_open());

    let receipt = direct_run(&host, A, &package, 5211);

    assert_eq!(
        receipt.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "{receipt:?}"
    );
    let events = all_events(&host);
    let fences = fence_records(&events);
    let [(record, _)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(record.links().instance_id(), Some(&a.id));
    let fence = status_fence(&host, A).expect("the fence");
    assert_eq!(fence.cause.as_str(), PAIRED_OPEN_FAILED);
    assert!(fence.awaiting_person, "{fence:?}");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// #419: while one instance goes through these sequences (a run of its application entry
// without its application, then a fence), the other's session, self-check facts, queue and
// availability are untouched, and its own work runs.
#[test]
fn hs5_other_instances_untouched() {
    let root = TempDir::new().expect("tempdir");
    let a = Node::new(A, TouchBackendChoice::NemuIpc, false);
    let b = Node::new(B, TouchBackendChoice::NemuIpc, false);
    let host = start(&root, &[&a, &b], false);
    let package = app_entry_package(&root);

    assert_completed_on_home(&direct_run(&host, A, &package, 5221));
    let stopped = application(&host, A, ApplicationLifecycleAction::Stop, 5222);
    assert_eq!(
        stopped.state(),
        RuntimeReceiptState::Completed,
        "{stopped:?}"
    );
    *a.device.open_failure.lock().expect("open failure") = Some(unreleased_open());
    let fenced = direct_run(&host, A, &package, 5223);
    assert_eq!(
        fenced.outcome().code().as_str(),
        HostCode::InstanceResourcesFenced.as_str(),
        "{fenced:?}"
    );

    assert!(b.device.opens().is_empty(), "{:?}", b.device.opens());
    assert!(b.device.captures().is_empty());
    assert!(b.device.commands().is_empty());
    assert_eq!(b.device.reads(), 0);
    assert!(preparations(&host, b.id).is_empty(), "no self-check of B");
    assert_eq!(status_fence(&host, B), None);
    assert_eq!(
        host.instance_claims_for_test(B).expect("claims"),
        (None, Vec::new())
    );
    assert_passed(&self_check(&host, B, 5224));
    assert_completed_on_home(&direct_run(&host, B, &package, 5225));
    assert!(status_fence(&host, A).is_some(), "A stays fenced");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
