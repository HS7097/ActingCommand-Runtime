// SPDX-License-Identifier: AGPL-3.0-only

// #402 HS-L (register §1.275; design rd-1031/backend-switch/model-backend-switch.md §2.2, §3.1 and
// §7 row HS-L; work order rd-1031/backend-switch/brief-hs-l.md): `actinglab --instance <alias>
// session backend [--touch <name>] [--capture <name>] [--clear]` sends one ConfigureSettings of the
// instance's backend keys on the Lab channel and prints the receipt, each key in effect and the
// settings' version; `--clear` returns both keys to the file's values, which is a switch too; a
// refusal prints the Runtime's outcome with its problem word and exits non-zero; after a switch a
// frame of the closed session is refused `input_frame_unavailable`, and the next `do` observes
// first and presses on its own frame; the instance listing's `runtime_backends` is the Runtime's.
// A Runtime host runs in the test over one fake instance, an emulator to its configuration (the
// file layer names both backends): its registration takes the backends a switch names, as the
// device registry does, and its input open fails for the touch backend a test names. Its 3x1
// frames are numbered per capture. No device, adb or emulator is touched; the names are neutral;
// nothing sleeps.

use super::*;
use actingcommand_contract::{
    ApplicationLifecycleAction, BackendOpenEntry, CommandPayload, ConfigParameter,
    ConfigParameterSource, ConfiguredSetting, ContainedLabOperationResult,
    EffectiveDeviceConfiguration, EventPayload, EventQuery, EventType, FactScalar, FencedWrite,
    InputAction, InputFrameReference, ProjectionPayload, ProjectionProfile, RuntimeConfigManifest,
    RuntimeResult,
};
use actingcommand_device::{
    CaptureBackendChoice, CaptureBackendName, DeviceCloseAuthority, DeviceResourceCloseOutcome,
    Frame, OpenedBackend, PixelFormat, TouchBackendChoice,
};
use actingcommand_runtime_client::RuntimeChannel;
use actingcommand_runtime_host::ForegroundApplicationObservation;
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use zip::{ZipWriter, write::FileOptions};

const ALIAS: &str = "node.a";
/// The backends the instance's configuration file names.
const FILE_TOUCH: &str = "adb_shell_input";
const FILE_CAPTURE: &str = "adb";
const ENTRY: [u8; 3] = [40, 90, 160];
const PAGE_A: [u8; 3] = [200, 60, 60];
const BUTTON: [u8; 3] = [60, 200, 60];

/// What the fake instance was asked to do.
#[derive(Default)]
struct HslScreen {
    captures: AtomicUsize,
    taps: AtomicUsize,
}

impl HslScreen {
    fn captures(&self) -> usize {
        self.captures.load(Ordering::SeqCst)
    }

    fn taps(&self) -> usize {
        self.taps.load(Ordering::SeqCst)
    }
}

/// The entry page with its button, pixel 2 the capture's number.
struct HslCapture {
    screen: Arc<HslScreen>,
}

impl CaptureBackend for HslCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let number = self.screen.captures.fetch_add(1, Ordering::SeqCst) + 1;
        let marker = (number % 256) as u8;
        Frame::from_pixels(
            3,
            1,
            [ENTRY, BUTTON, [marker; 3]].concat(),
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }
}

struct HslInput {
    screen: Arc<HslScreen>,
}

impl InputBackend for HslInput {
    fn tap(&mut self, _witness: &FencedWrite, _x: i32, _y: i32) -> DeviceResult<()> {
        self.screen.taps.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn long_tap(
        &mut self,
        _witness: &FencedWrite,
        _x: i32,
        _y: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal("the HS-L fake instance only taps"))
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
        Err(DeviceError::fatal("the HS-L fake instance only taps"))
    }

    fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal("the HS-L fake instance only taps"))
    }

    fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal("the HS-L fake instance only taps"))
    }

    fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
        Ok(())
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(1))
    }
}

/// The instance `node.a`: a physical one to the Runtime, registered with the backends its last
/// switch named (the file's before any).
struct HslInstance {
    instance_id: InstanceId,
    screen: Arc<HslScreen>,
    /// The registration's (touch, capture).
    backends: Mutex<(String, String)>,
    /// Every switch the registration took, in order.
    switches: Mutex<Vec<(String, String)>>,
    /// The touch backend whose input open fails, when a test names one.
    failing_touch: Mutex<Option<String>>,
}

impl HslInstance {
    fn backends(&self) -> (String, String) {
        self.backends.lock().expect("fake backends").clone()
    }

    fn switches(&self) -> Vec<(String, String)> {
        self.switches.lock().expect("fake switches").clone()
    }

    fn fail_touch(&self, touch: &str) {
        *self.failing_touch.lock().expect("fake failing touch") = Some(touch.to_owned());
    }
}

impl ExecutionBackendProvider for HslInstance {
    fn instance_aliases(&self) -> Vec<String> {
        vec![ALIAS.to_string()]
    }

    // The configuration names the registration's backends, as the device registry's does.
    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == ALIAS).then(|| {
            let (touch, capture) = self.backends();
            ResolvedExecutionInstance::new(self.instance_id, "<lab-hsl-test>").with_configuration(
                EffectiveDeviceConfiguration {
                    input_backend: touch,
                    capture_backend: capture,
                    input_adb: "<sealed-test>".to_owned(),
                    capture_adb: "<sealed-test>".to_owned(),
                    configured_serial: None,
                    resolved_serial: "TESTSERIAL0001".to_owned(),
                    input_command_timeout_ms: 5_000,
                    capture_command_timeout_ms: 5_000,
                    capture_timeout_ms: 5_000,
                    configured_mumu_root: None,
                    configured_capture_dll: None,
                },
            )
        })
    }

    fn open_input(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        let (touch, _) = self.backends();
        if self
            .failing_touch
            .lock()
            .expect("fake failing touch")
            .as_deref()
            == Some(touch.as_str())
        {
            return Err(DeviceError::fatal("the fake touch backend does not open"));
        }
        let backend: Box<dyn InputBackend> = Box::new(HslInput {
            screen: Arc::clone(&self.screen),
        });
        Ok(OpenedBackend::unobserved(backend, BackendOpenEntry::Input))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        let backend: Box<dyn CaptureBackend> = Box::new(HslCapture {
            screen: Arc::clone(&self.screen),
        });
        Ok(OpenedBackend::unobserved(
            backend,
            BackendOpenEntry::Capture,
        ))
    }

    // The assigned application is always in the foreground, so the Runtime's foreground gate
    // passes before an input.
    fn observe_foreground_application(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<ForegroundApplicationObservation> {
        Ok(ForegroundApplicationObservation {
            foreground: Some("neutral.application".to_owned()),
            assigned: "neutral.application".to_owned(),
        })
    }

    fn control_application(
        &self,
        _witness: &FencedWrite,
        _instance_alias: &str,
        _action: ApplicationLifecycleAction,
    ) -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "the HS-L fake instance controls no application",
        ))
    }

    // #402 HS-2: the registration takes the switch's backends; nothing is opened.
    fn switch_instance_backends(
        &self,
        _instance_alias: &str,
        touch: TouchBackendChoice,
        capture: CaptureBackendChoice,
    ) -> DeviceResult<Option<u32>> {
        let switched = (touch.as_str().to_owned(), capture.as_str().to_owned());
        self.switches
            .lock()
            .expect("fake switches")
            .push(switched.clone());
        *self.backends.lock().expect("fake backends") = switched;
        Ok(None)
    }
}

/// The JSON text of an instance id, the `<instance id>` of its configuration keys.
fn hsl_id_text(instance: InstanceId) -> String {
    match serde_json::to_value(instance).expect("an instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is a string: {other}"),
    }
}

/// The file layer's two backend rows of `instance`, explicit, as actingd reports them (both
/// settable since HS-2).
fn hsl_file_rows(instance: InstanceId) -> Vec<ConfigParameter> {
    let id = hsl_id_text(instance);
    [
        ("touch_backend", FILE_TOUCH),
        ("capture_backend", FILE_CAPTURE),
    ]
    .into_iter()
    .map(|(member, value)| ConfigParameter {
        key: format!("instance.{id}.{member}"),
        value: FactScalar::String(value.to_owned()),
        source: ConfigParameterSource::Explicit,
        settable: true,
    })
    .collect()
}

/// A Runtime host over the fake instance with the file's two backend rows, its state root the
/// Lab's, and directories for a package and a Lab configuration. The fields drop in order: the
/// host first, the environment lock last.
struct HslLab {
    host: Option<RuntimeHost>,
    device: Arc<HslInstance>,
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    runtime: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl HslLab {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        let runtime = TempDir::new().expect("Runtime state root");
        let runtime_env = use_runtime_state_root(runtime.path());
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let device = Arc::new(HslInstance {
            instance_id,
            screen: Arc::new(HslScreen::default()),
            backends: Mutex::new((FILE_TOUCH.to_owned(), FILE_CAPTURE.to_owned())),
            switches: Mutex::default(),
            failing_touch: Mutex::default(),
        });
        let provider = Arc::clone(&device);
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(runtime.path(), b"actinglab-hsl-test").with_config_manifest(
                RuntimeConfigManifest {
                    subsystems: Vec::new(),
                    parameters: hsl_file_rows(instance_id),
                },
            ),
            provider,
        )
        .expect("Runtime host");
        Self {
            host: Some(host),
            device,
            files: TempDir::new().expect("test files"),
            _runtime_env: runtime_env,
            runtime,
            _app_env: app_env,
            _guard: guard,
        }
    }

    /// `words` run against the Runtime with `--json` and the instance.
    fn run(&self, words: &[&str]) -> CliResult {
        let mut line = vec!["--json", "--instance", ALIAS];
        line.extend_from_slice(words);
        run_cli(line, true)
    }

    /// A Lab client of the Runtime.
    fn client(&self) -> RuntimeClient {
        RuntimeClient::connect(lab_client_config(self.runtime.path())).expect("Runtime client")
    }

    /// The instance's configuration key of `member`.
    fn key(&self, member: &str) -> String {
        format!("instance.{}.{member}", hsl_id_text(self.device.instance_id))
    }

    /// A Lab configuration that lists the instance, with no backend of its own.
    fn list_instance(&self) {
        set_config_env(self.files.path().join("config.json"));
        let mut config = UserConfig::default();
        config
            .instances
            .insert(ALIAS.to_owned(), InstanceConfig::default());
        write_user_config(&config).expect("Lab configuration");
    }

    /// The package `name`: its entry page told by pixel 0, the button `ui/button` on pixel 1.
    fn package(&self, name: &str) -> String {
        let colour = |id: &str, x: i32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let mut button = colour("ui/button", 1, BUTTON);
        button["click"] = json!({"x": 1, "y": 0, "width": 1, "height": 1});
        let pack = json!({
            "schema_version": "0.7",
            "game": "neutral",
            "server": "test",
            "coordinate_space": {"width": 3, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                colour("page/entry_page", 0, ENTRY),
                colour("page/page_a", 0, PAGE_A),
                button
            ]
        });
        let entries = [
            (
                "control.json",
                json!({"game": "neutral", "server": "test", "entry_task_id": "task"}),
            ),
            (
                "resources/manifest.json",
                json!({"schema_version": "0.3", "entry_task_id": "task"}),
            ),
            (
                "resources/operations/task/task.json",
                json!({
                    "schema_version": "0.6",
                    "task_id": "task",
                    "game": "neutral",
                    "server_scope": ["test"],
                    "goal": "neutral fixture",
                    "coordinate_space": {"width": 3, "height": 1},
                    "entry_page": "entry_page",
                    "target_page": "page_a",
                    "operations": [{
                        "id": "press_button",
                        "purpose": "neutral fixture",
                        "from": "entry_page",
                        "to": "page_a",
                        "click": {"kind": "point", "x": 1, "y": 0},
                        "guard": {
                            "page_id": "entry_page",
                            "target_id": "ui/button",
                            "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                            "color_probe": "ui/button"
                        }
                    }]
                }),
            ),
            ("resources/recognition/neutral.test.pack.json", pack),
            (
                "resources/recognition/neutral.test.pages.json",
                json!({"schema_version": "0.3", "pages": [
                    {"id": "neutral/entry_page", "required": ["page/entry_page"]},
                    {"id": "neutral/page_a", "required": ["page/page_a"]}
                ]}),
            ),
            (
                "resources/navigation/neutral.test.navigation.json",
                json!({
                    "schema_version": "0.3",
                    "game": "neutral",
                    "server": "test",
                    "navigation": [{
                        "id": "open_page_a",
                        "from_page": "neutral/entry_page",
                        "to_page": "neutral/page_a",
                        "click": {"kind": "rect", "x": 1, "y": 0, "width": 1, "height": 1}
                    }],
                    "destructive_actions": []
                }),
            ),
        ];
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, value) in &entries {
            zip.start_file(*path, options).expect("zip entry");
            zip.write_all(&serde_json::to_vec(value).expect("JSON"))
                .expect("zip bytes");
        }
        let path = self.files.path().join(name);
        fs::write(&path, zip.finish().expect("finish zip").into_inner()).expect("write package");
        path.display().to_string()
    }

    /// Stops the Runtime host, which must close cleanly.
    fn close(mut self) {
        self.host
            .take()
            .expect("the Runtime host")
            .close()
            .expect("close the Runtime host");
        set_missing_config_env();
    }
}

/// The answer is a success; its data.
#[track_caller]
fn hsl_data(result: &CliResult, what: &str) -> Value {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{what}: {shown}");
    result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("{what}: no data: {shown}"))
}

/// The answer is a failure with exit code `exit`; its code and details.
#[track_caller]
fn hsl_error(result: &CliResult, exit: i32, what: &str) -> (String, Value) {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), exit, "{what}: {shown}");
    let error = result
        .envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: no error: {shown}"));
    (
        error.code.clone(),
        error.details.clone().unwrap_or(Value::Null),
    )
}

/// The receipt's settings as (key, value, source).
fn hsl_settings(data: &Value) -> Vec<(String, FactScalar, ConfigParameterSource)> {
    serde_json::from_value::<Vec<ConfiguredSetting>>(data["settings"].clone())
        .expect("the receipt's settings")
        .into_iter()
        .map(|setting| (setting.key, setting.value, setting.source))
        .collect()
}

/// The channel of every `command.received` the Runtime recorded.
fn hsl_command_channels(client: &RuntimeClient) -> Vec<Option<RuntimeChannel>> {
    client
        .query_events(
            EventQuery {
                event_type: Some(EventType::CommandReceived),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("command records")
        .iter()
        .map(|event| match &event.payload {
            ProjectionPayload::Full(payload) => match payload.as_ref() {
                EventPayload::Command(CommandPayload::Received(observation)) => {
                    observation.channel()
                }
                other => panic!("a command.received payload: {other:?}"),
            },
            other => panic!("a full projection: {other:?}"),
        })
        .collect()
}

/// One line names both backends: one ConfigureSettings on the Lab channel carries both keys (the
/// registration takes both in one switch), and the answer is its receipt: each key in effect,
/// from an instruction, and the settings' version. The listing's `runtime_backends` is then the
/// Runtime's, beside the file's own columns.
#[test]
fn hsl_session_backend_sends_configure_settings() {
    let lab = HslLab::new();
    lab.list_instance();

    let switched = hsl_data(
        &lab.run(&[
            "session",
            "backend",
            "--touch",
            "maatouch",
            "--capture",
            "droidcast_raw",
        ]),
        "session backend",
    );

    assert_eq!(
        lab.device.switches(),
        vec![("maatouch".to_owned(), "droidcast_raw".to_owned())]
    );
    assert_eq!(switched["instance"], ALIAS, "{switched}");
    assert_eq!(
        switched["instance_id"],
        json!(hsl_id_text(lab.device.instance_id)),
        "{switched}"
    );
    assert_eq!(
        hsl_settings(&switched),
        vec![
            (
                lab.key("touch_backend"),
                FactScalar::String("maatouch".to_owned()),
                ConfigParameterSource::Instruction,
            ),
            (
                lab.key("capture_backend"),
                FactScalar::String("droidcast_raw".to_owned()),
                ConfigParameterSource::Instruction,
            ),
        ]
    );
    assert!(switched["config_version"].is_u64(), "{switched}");
    let channels = hsl_command_channels(&lab.client());
    assert!(
        !channels.is_empty()
            && channels
                .iter()
                .all(|channel| *channel == Some(RuntimeChannel::Lab)),
        "{channels:?}"
    );

    let listed = hsl_data(
        &lab.run(&["session", "instance", "list"]),
        "session instance list",
    );
    assert_eq!(
        listed["runtime_backends_read"],
        json!({"read": true, "config_version": switched["config_version"]}),
        "{listed}"
    );
    let row = &listed["instances"][0];
    assert_eq!(row["id"], ALIAS, "{listed}");
    assert_eq!(row["capture_backend"], Value::Null, "{listed}");
    assert_eq!(
        row["runtime_backends"],
        json!({
            "registered": true,
            "instance_id": hsl_id_text(lab.device.instance_id),
            "touch_backend": {"value": "maatouch", "source": "instruction"},
            "capture_backend": {"value": "droidcast_raw", "source": "instruction"},
        }),
        "{listed}"
    );
    lab.close();
}

/// `--clear` returns both keys to the file's values in one request, which is a switch: the
/// registration takes the file's backends again and the receipt names them from the file. With
/// `--touch` or `--capture` it is refused before anything is sent.
#[test]
fn hsl_session_backend_clear_returns_file_values() {
    let lab = HslLab::new();
    lab.list_instance();
    hsl_data(
        &lab.run(&[
            "session",
            "backend",
            "--touch",
            "maatouch",
            "--capture",
            "droidcast_raw",
        ]),
        "session backend",
    );

    let (code, _) = hsl_error(
        &lab.run(&["session", "backend", "--clear", "--touch", "minitouch"]),
        2,
        "session backend --clear --touch",
    );
    assert_eq!(code, "validation_failed");
    assert_eq!(lab.device.switches().len(), 1);

    let cleared = hsl_data(
        &lab.run(&["session", "backend", "--clear"]),
        "session backend --clear",
    );
    assert_eq!(
        lab.device.switches(),
        vec![
            ("maatouch".to_owned(), "droidcast_raw".to_owned()),
            (FILE_TOUCH.to_owned(), FILE_CAPTURE.to_owned()),
        ]
    );
    assert_eq!(
        hsl_settings(&cleared),
        vec![
            (
                lab.key("touch_backend"),
                FactScalar::String(FILE_TOUCH.to_owned()),
                ConfigParameterSource::Explicit,
            ),
            (
                lab.key("capture_backend"),
                FactScalar::String(FILE_CAPTURE.to_owned()),
                ConfigParameterSource::Explicit,
            ),
        ]
    );
    let listed = hsl_data(
        &lab.run(&["session", "instance", "list"]),
        "session instance list",
    );
    assert_eq!(
        listed["instances"][0]["runtime_backends"]["touch_backend"],
        json!({"value": FILE_TOUCH, "source": "explicit"}),
        "{listed}"
    );
    lab.close();
}

/// A refusal prints the Runtime's outcome with its problem word and exits non-zero: Nemu touch
/// beside another capture is `config_value_invalid` (`backend_pair_required`, the touch key, the
/// registry's `nemu_input_requires_paired_capture` as its cause; exit 2) and switches nothing; a
/// touch backend that does not open is `backend_switch_refused` (`open_failed`, the open's
/// `input_backend_open_failed` as its cause; exit 4) after the previous backends came back.
#[test]
fn hsl_refusal_printed_with_problem() {
    let lab = HslLab::new();

    let (code, details) = hsl_error(
        &lab.run(&["session", "backend", "--touch", "nemu_ipc"]),
        2,
        "session backend --touch nemu_ipc",
    );
    assert_eq!(code, "config_value_invalid", "{details}");
    assert_eq!(details["outcome"], "config_value_invalid", "{details}");
    assert_eq!(
        details["config_value_problem"], "backend_pair_required",
        "{details}"
    );
    assert_eq!(
        details["config_key"],
        json!(lab.key("touch_backend")),
        "{details}"
    );
    assert_eq!(details["receipt_state"], "denied", "{details}");
    assert_eq!(details["instance"], ALIAS, "{details}");
    assert_eq!(
        details["runtime_outcome"]["causes"][0]["code"], "nemu_input_requires_paired_capture",
        "{details}"
    );
    assert!(lab.device.switches().is_empty());

    lab.device.fail_touch("minitouch");
    let (code, details) = hsl_error(
        &lab.run(&["session", "backend", "--touch", "minitouch"]),
        4,
        "session backend --touch minitouch",
    );
    assert_eq!(code, "backend_switch_refused", "{details}");
    assert_eq!(
        details["backend_switch_problem"], "open_failed",
        "{details}"
    );
    assert_eq!(details["receipt_state"], "failed", "{details}");
    let causes = details["runtime_outcome"]["causes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        causes
            .iter()
            .any(|cause| cause["code"] == "input_backend_open_failed"),
        "{details}"
    );
    assert_eq!(
        lab.device.switches(),
        vec![
            ("minitouch".to_owned(), FILE_CAPTURE.to_owned()),
            (FILE_TOUCH.to_owned(), FILE_CAPTURE.to_owned()),
        ]
    );
    lab.close();
}

/// A frame the Lab held from the session before a switch was pressed on before it; after the
/// switch the Runtime refuses it `input_frame_unavailable`, and the next `do` observes the
/// instance first and presses on that new frame.
#[test]
fn hsl_stale_frame_reobserved_after_switch() {
    let lab = HslLab::new();
    let package = lab.package("neutral.zip");
    let session = lab.client().begin_debug_session().expect("debug session");
    let observed = session
        .observe_readonly(ALIAS)
        .expect("an observation before the switch");
    let Some(RuntimeResult::ReadonlyObservationCompleted { observation }) = observed.result()
    else {
        panic!("a read-only observation: {observed:?}");
    };
    let held = InputFrameReference {
        frame_id: *observation
            .artifact()
            .frame_id()
            .expect("the Runtime committed the frame"),
        width: observation.width(),
        height: observation.height(),
    };
    let token = session.acquire_lease(ALIAS).expect("lease");
    session
        .input_with_frame(&token, InputAction::Tap { x: 1, y: 0 }, Some(held))
        .expect("an input on the held frame");
    session.release_lease(&token).expect("release the lease");
    assert_eq!(lab.device.screen.taps(), 1);

    hsl_data(
        &lab.run(&["session", "backend", "--touch", "maatouch"]),
        "session backend",
    );
    assert_eq!(
        lab.device.switches(),
        vec![("maatouch".to_owned(), FILE_CAPTURE.to_owned())]
    );

    let token = session
        .acquire_lease(ALIAS)
        .expect("lease after the switch");
    let refused = session.input_with_frame(&token, InputAction::Tap { x: 1, y: 0 }, Some(held));
    session.release_lease(&token).expect("release the lease");
    let Err(error) = refused else {
        panic!("a frame of the closed session is not pressed on");
    };
    assert_eq!(
        error.outcome().code().as_str(),
        "input_frame_unavailable",
        "{error}"
    );
    assert_eq!(lab.device.screen.taps(), 1);
    drop(session);

    let captures = lab.device.screen.captures();
    let done = hsl_data(
        &lab.run(&[
            "do",
            "--tap",
            "1,0",
            "--capture",
            "--zip",
            package.as_str(),
            "--verbose",
        ]),
        "do --capture",
    );
    assert_eq!(done["executed"], true, "{done}");
    assert_eq!(lab.device.screen.taps(), 2, "{done}");
    assert!(lab.device.screen.captures() > captures, "{done}");
    let operation: ContainedLabOperationResult =
        serde_json::from_value(done["operation_record"].clone()).expect("the operation record");
    let before = operation
        .record
        .prepared
        .before_frame
        .expect("the before frame");
    let pressed_on = *before
        .observation
        .artifact()
        .frame_id()
        .expect("the before frame's id");
    assert_ne!(pressed_on, held.frame_id, "{done}");
    lab.close();
}
