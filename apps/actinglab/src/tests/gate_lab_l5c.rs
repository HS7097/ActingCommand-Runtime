// SPDX-License-Identifier: AGPL-3.0-only

// #411 Lab L5c (exec gate key 114; register §1.141; plan-lab-l5 decisions Q8 (a) and Q2;
// brief-L5c requirements 1-4): the Lab's `--capture` client forms take the frames target
// consensus needs.
// - R1: `do --capture --dry-run`, `ensure --capture` and `wait --capture` take F Runtime captures
//   at the declared interval, oldest first, the last the current frame, and decide on them as
//   offline `--scene` frames are decided on (#411 Lab B-2): a target consensus of k = 2 of 3 is
//   recognised on (A, B, A) and not on (B, B, A); do's target is the current frame's.
// - R2: an input on the instance between the first and the last capture fails the frame set with
//   `observation_samples_interrupted`; a quiet set passes.
// - R3: a package without target consensus takes one capture per observation, as before.
// - R4: each wait poll of `ensure` and `wait` takes one frame set.
// A Runtime host runs in the test over a fake instance whose 3x1 frames are scripted per capture:
// pixel 0 shows the page, pixel 1 the button or not, pixel 2 the capture's number. Every expected
// value comes from those requirement lines. Packages are synthetic and neutral; the declared
// interval is 1 ms.

use super::*;
use actingcommand_contract::{
    ApplicationLifecycleAction, BackendOpenEntry, FencedWrite, InputAction, ObservationFramePlan,
};
use actingcommand_device::{
    CaptureBackendName, DeviceCloseAuthority, DeviceResourceCloseOutcome, Frame, OpenedBackend,
    PixelFormat,
};
use actingcommand_runtime_host::ForegroundApplicationObservation;
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use zip::{ZipWriter, write::FileOptions};

const ALIAS: &str = "node.a";
const ENTRY: [u8; 3] = [40, 90, 160];
const OTHER: [u8; 3] = [90, 90, 90];
const PAGE_A: [u8; 3] = [200, 60, 60];
const BUTTON: [u8; 3] = [60, 200, 60];
const BLANK: [u8; 3] = [0, 0, 0];
const ENTRY_PAGE: &str = "neutral/entry_page";
const PAGE_A_PAGE: &str = "neutral/page_a";

/// What the fake instance shows, one screen (its page and button pixels) per capture in order,
/// the last one repeated, and what it was asked to do.
#[derive(Default)]
struct FakeScreen {
    script: Mutex<Vec<([u8; 3], [u8; 3])>>,
    captures: AtomicUsize,
    taps: AtomicUsize,
}

impl FakeScreen {
    /// Shows `screens` from the next capture on, the counts starting from zero again.
    fn show(&self, screens: &[([u8; 3], [u8; 3])]) {
        *self.script.lock().expect("screen script") = screens.to_vec();
        self.captures.store(0, Ordering::SeqCst);
        self.taps.store(0, Ordering::SeqCst);
    }

    fn captures(&self) -> usize {
        self.captures.load(Ordering::SeqCst)
    }

    fn taps(&self) -> usize {
        self.taps.load(Ordering::SeqCst)
    }
}

struct FakeCapture {
    screen: Arc<FakeScreen>,
}

impl CaptureBackend for FakeCapture {
    fn capture(&mut self) -> DeviceResult<Frame> {
        let number = self.screen.captures.fetch_add(1, Ordering::SeqCst) + 1;
        let (page, button) = {
            let script = self.screen.script.lock().expect("screen script");
            script
                .get(number - 1)
                .or_else(|| script.last())
                .copied()
                .unwrap_or((BLANK, BLANK))
        };
        let marker = (number % 256) as u8;
        Frame::from_pixels(
            3,
            1,
            [page, button, [marker; 3]].concat(),
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

struct FakeInput {
    screen: Arc<FakeScreen>,
}

impl InputBackend for FakeInput {
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
        Err(DeviceError::fatal("the L5c fake instance only taps"))
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
        Err(DeviceError::fatal("the L5c fake instance only taps"))
    }

    fn key(&mut self, _witness: &FencedWrite, _key: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal("the L5c fake instance only taps"))
    }

    fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
        Err(DeviceError::fatal("the L5c fake instance only taps"))
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

/// The instance `node.a`: a physical one to the Runtime, served by the fake screen.
struct FakeInstance {
    instance_id: InstanceId,
    screen: Arc<FakeScreen>,
}

impl ExecutionBackendProvider for FakeInstance {
    fn instance_aliases(&self) -> Vec<String> {
        vec![ALIAS.to_string()]
    }

    fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
        (instance_alias == ALIAS)
            .then(|| ResolvedExecutionInstance::new(self.instance_id, "<lab-l5c-test>"))
    }

    fn open_input(
        &self,
        _instance_alias: &str,
    ) -> DeviceResult<OpenedBackend<Box<dyn InputBackend>>> {
        let backend: Box<dyn InputBackend> = Box::new(FakeInput {
            screen: Arc::clone(&self.screen),
        });
        Ok(OpenedBackend::unobserved(backend, BackendOpenEntry::Input))
    }

    fn open_capture(
        &self,
        _instance_alias: &str,
        _memory: Option<&actingcommand_device::FrameMemoryBudget>,
    ) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        let backend: Box<dyn CaptureBackend> = Box::new(FakeCapture {
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
            "the L5c fake instance controls no application",
        ))
    }
}

/// A Runtime host over the fake instance, its state root the Lab's, no configuration and no
/// recording session, and a directory for the packages. The fields drop in order: the host
/// first, the environment lock last.
struct Lab {
    host: Option<RuntimeHost>,
    screen: Arc<FakeScreen>,
    files: TempDir,
    _runtime_env: RuntimeStateEnvGuard,
    runtime: TempDir,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Lab {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
        }
        let runtime = TempDir::new().expect("Runtime state root");
        let runtime_env = use_runtime_state_root(runtime.path());
        let screen = Arc::new(FakeScreen::default());
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(runtime.path(), b"actinglab-lab-l5c-test"),
            Arc::new(FakeInstance {
                instance_id,
                screen: Arc::clone(&screen),
            }),
        )
        .expect("Runtime host");
        Self {
            host: Some(host),
            screen,
            files: TempDir::new().expect("package directory"),
            _runtime_env: runtime_env,
            runtime,
            _app_env: app_env,
            _guard: guard,
        }
    }

    /// A Lab client of the Runtime.
    fn client(&self) -> RuntimeClient {
        RuntimeClient::connect(lab_client_config(self.runtime.path())).expect("Runtime client")
    }

    /// `words` with `--capture`, run against the Runtime with `--json`, the instance, the whole
    /// answer (`--pretty`) and the package.
    fn run(&self, words: &[&str], package: &str) -> CliResult {
        let mut line = vec!["--json", "--instance", ALIAS];
        line.extend_from_slice(words);
        line.extend(["--capture", "--pretty", "--zip", package]);
        run_cli(line, true)
    }

    /// The package `name`: its pages told apart by pixel 0 (`entry_page`, `page_a`), the button
    /// `ui/button` on pixel 1, and the route `open_page_a`, which taps the button. With
    /// `consensus` the entry page's target needs its colour on 2 of 3 frames, 1 ms apart.
    fn package(&self, name: &str, consensus: bool) -> String {
        let colour = |id: &str, x: i32, expected: [u8; 3]| {
            json!({"type": "color", "id": id,
                   "region": {"x": x, "y": 0, "width": 1, "height": 1}, "expected": expected})
        };
        let mut button = colour("ui/button", 1, BUTTON);
        button["click"] = json!({"x": 1, "y": 0, "width": 1, "height": 1});
        let mut pack = json!({
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
        if consensus {
            pack["target_consensus"] = json!({"page/entry_page": {
                "samples": [{"frame": 0}, {"frame": 1}, {"frame": 2}],
                "k": 2,
                "sample_interval_ms": 1
            }});
        }
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
                    {"id": ENTRY_PAGE, "required": ["page/entry_page"]},
                    {"id": PAGE_A_PAGE, "required": ["page/page_a"]}
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
                        "from_page": ENTRY_PAGE,
                        "to_page": PAGE_A_PAGE,
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
    }
}

/// The answer is a success; its data.
#[track_caller]
fn data(result: &CliResult, what: &str) -> Value {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), 0, "{what}: {shown}");
    result
        .envelope
        .data
        .clone()
        .unwrap_or_else(|| panic!("{what}: no data: {shown}"))
}

/// The coverage of the consensus package's three frames.
fn three_frames() -> Value {
    json!({"provided_frames": 3, "required_frames": 3, "aggregation": "production_owner"})
}

/// R1: `do --capture --dry-run` takes three Runtime captures for the consensus package and plans
/// on them as offline `do` plans on three `--scene` frames: the entry page is recognised on
/// (A, B, A) and not on (B, B, A), whose current frame alone shows it; the button, visible on the
/// third capture only, is planned on, so the current frame is the last capture; the answer
/// reports the three frames.
#[test]
fn l5c_do_capture_dry_run_takes_the_consensus_frames() {
    let lab = Lab::new();
    let package = lab.package("consensus.zip", true);

    for (screens, page, matched) in [
        (
            [(ENTRY, BLANK), (OTHER, BLANK), (ENTRY, BUTTON)],
            ENTRY_PAGE,
            true,
        ),
        (
            [(OTHER, BLANK), (OTHER, BLANK), (ENTRY, BUTTON)],
            "standby",
            false,
        ),
    ] {
        lab.screen.show(&screens);
        let planned = data(
            &lab.run(&["do", "ui/button", "--dry-run"], &package),
            "do --capture --dry-run",
        );
        assert_eq!(planned["state"], "planned", "{planned}");
        assert_eq!(planned["executed"], false, "{planned}");
        assert_eq!(planned["page"], page, "{planned}");
        assert_eq!(planned["observation"]["matched"], matched, "{planned}");
        assert_eq!(planned["guard_result"]["passed"], true, "{planned}");
        assert_eq!(planned["recognition_coverage"], three_frames(), "{planned}");
        assert_eq!(
            planned["frame_coordinates"]["device_pixels"],
            json!([3, 1]),
            "{planned}"
        );
        assert_eq!(lab.screen.captures(), 3, "{planned}");
        assert_eq!(lab.screen.taps(), 0, "{planned}");
    }
    lab.close();
}

/// R1, R4: every wait poll takes one frame set. `wait --capture` polls (B, B, A), which is not
/// the entry page, then (A, B, A), which is: six captures. `ensure --capture` decides its start
/// and its route's guard on a set each, taps once and arrives on one more set: nine captures.
#[test]
fn l5c_ensure_and_wait_capture_sample_frame_sets() {
    let lab = Lab::new();
    let package = lab.package("consensus.zip", true);

    lab.screen.show(&[
        (OTHER, BUTTON),
        (OTHER, BUTTON),
        (ENTRY, BUTTON),
        (ENTRY, BUTTON),
        (OTHER, BUTTON),
        (ENTRY, BUTTON),
    ]);
    let waited = data(
        &lab.run(
            &[
                "wait",
                "--page",
                ENTRY_PAGE,
                "--timeout-ms",
                "10000",
                "--poll-ms",
                "1",
            ],
            &package,
        ),
        "wait --capture",
    );
    assert_eq!(waited["state"], "arrived", "{waited}");
    assert_eq!(waited["page"], ENTRY_PAGE, "{waited}");
    assert_eq!(
        lab.screen.captures(),
        6,
        "two polls of three frames: {waited}"
    );

    lab.screen.show(&[
        (ENTRY, BUTTON),
        (OTHER, BUTTON),
        (ENTRY, BUTTON),
        (OTHER, BUTTON),
        (ENTRY, BUTTON),
        (ENTRY, BUTTON),
        (PAGE_A, BLANK),
    ]);
    let ensured = data(
        &lab.run(
            &[
                "ensure",
                "--to",
                PAGE_A_PAGE,
                "--step-timeout-ms",
                "10000",
                "--poll-ms",
                "1",
            ],
            &package,
        ),
        "ensure --capture",
    );
    assert_eq!(ensured["state"], "arrived", "{ensured}");
    assert_eq!(ensured["from"], ENTRY_PAGE, "{ensured}");
    assert_eq!(ensured["page"], PAGE_A_PAGE, "{ensured}");
    assert_eq!(lab.screen.taps(), 1, "{ensured}");
    assert_eq!(
        lab.screen.captures(),
        9,
        "the start, the guard and the arrival, three frames each: {ensured}"
    );
    lab.close();
}

/// R2: a client frame set of two frames, with nothing between its captures, is one screen;
/// another client that presses the instance between the two captures makes the set fail with
/// `observation_samples_interrupted` (`device_error`).
#[test]
fn l5c_an_input_between_client_captures_fails_the_observation() {
    let lab = Lab::new();
    lab.screen.show(&[(ENTRY, BUTTON)]);
    let session = lab.client().begin_debug_session().expect("debug session");
    let plan = ObservationFramePlan::new(2, 1).expect("two frames");

    if let Err(error) =
        crate::lab2_cli::capture_runtime_frame_set(&session, ALIAS, plan, &mut |_| {})
    {
        panic!("frames with no input between them are one screen: {error}");
    }
    assert_eq!(lab.screen.captures(), 2);

    let presser = lab
        .client()
        .begin_debug_session()
        .expect("pressing session");
    let interrupted =
        crate::lab2_cli::capture_runtime_frame_set(&session, ALIAS, plan, &mut |_| {
            let token = presser.acquire_lease(ALIAS).expect("lease");
            presser
                .input(&token, InputAction::Tap { x: 1, y: 0 })
                .expect("tap");
            presser.release_lease(&token).expect("release the lease");
        });
    let Err(error) = interrupted else {
        panic!("frames that straddle an input are not one screen");
    };
    assert_eq!(error.code, "device_error", "{error}");
    assert_eq!(
        error
            .details
            .as_ref()
            .map(|details| details["error"].clone()),
        Some(json!("observation_samples_interrupted")),
        "{error}"
    );
    assert_eq!(lab.screen.captures(), 4);
    assert_eq!(lab.screen.taps(), 1);
    drop(presser);
    drop(session);
    lab.close();
}

/// R3: for a package without target consensus every observation of the client forms is one
/// capture, as before, and do's answer has no `recognition_coverage`: `do --capture --dry-run`
/// one capture, `wait --capture` one, `ensure --capture` its start, its guard and its arrival.
#[test]
fn l5c_a_package_without_consensus_captures_one_frame_as_before() {
    let lab = Lab::new();
    let package = lab.package("single.zip", false);

    lab.screen.show(&[(ENTRY, BUTTON)]);
    let planned = data(
        &lab.run(&["do", "ui/button", "--dry-run"], &package),
        "do --capture --dry-run",
    );
    assert_eq!(planned["state"], "planned", "{planned}");
    assert_eq!(planned["page"], ENTRY_PAGE, "{planned}");
    assert!(planned.get("recognition_coverage").is_none(), "{planned}");
    assert_eq!(lab.screen.captures(), 1, "{planned}");

    lab.screen.show(&[(ENTRY, BUTTON)]);
    let waited = data(
        &lab.run(
            &[
                "wait",
                "--page",
                ENTRY_PAGE,
                "--timeout-ms",
                "10000",
                "--poll-ms",
                "1",
            ],
            &package,
        ),
        "wait --capture",
    );
    assert_eq!(waited["state"], "arrived", "{waited}");
    assert_eq!(lab.screen.captures(), 1, "{waited}");

    lab.screen
        .show(&[(ENTRY, BUTTON), (ENTRY, BUTTON), (PAGE_A, BLANK)]);
    let ensured = data(
        &lab.run(
            &[
                "ensure",
                "--to",
                PAGE_A_PAGE,
                "--step-timeout-ms",
                "10000",
                "--poll-ms",
                "1",
            ],
            &package,
        ),
        "ensure --capture",
    );
    assert_eq!(ensured["state"], "arrived", "{ensured}");
    assert_eq!(ensured["page"], PAGE_A_PAGE, "{ensured}");
    assert_eq!(lab.screen.taps(), 1, "{ensured}");
    assert_eq!(lab.screen.captures(), 3, "{ensured}");
    lab.close();
}
