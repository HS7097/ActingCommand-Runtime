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

use super::gate_403::{fence_records, status_fence};
use super::instance_workers::all_events;
use super::*;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, DeviceBackendSwitchedPayload,
    FactScalar, NemuAppIndexSource, RuntimeConfigManifest, RuntimePayload,
};
use actingcommand_device::{
    AdbConfig, CaptureBackendChoice, CaptureBackendConfig, DeviceTarget, MaaTouchConfig,
    TouchBackendChoice, TouchBackendConfig, mumu_instance_port, nemu_paired_display,
};
use actingcommand_execution_kernel::DiscoveredInstanceBinding;
use actingcommand_runtime_config::{CAPTURE_BACKEND, TOUCH_BACKEND, instance_key};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::atomic::AtomicI32;

const A: &str = "hs5.instance.a";
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
