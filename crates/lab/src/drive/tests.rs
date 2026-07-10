// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::{
    CaptureBackendFactory, ConfigSource, InputBackendAttemptReport, InputBackendReport, LedgerSink,
    SemanticRequestContext,
};
use actingcommand_contract::{DriveRecord, LedgerProjection};
use actingcommand_device::{
    AdbConfig, DeviceError, DeviceResult, DeviceTarget, InputBackend, MaaTouchConfig,
    TouchBackendConfig,
};
use actingcommand_recognition::{Scene, ScenePixelFormat};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[test]
fn tap_target_dry_run_returns_typed_plan() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let mut ledger = ledger("tap-target");
    let response = lab
        .tap_target(fixture.tap_request(red_scene(), true), &mut ledger)
        .expect("tap plan");

    assert_eq!(response.status, "planned");
    assert!(!response.executed);
    assert_eq!(response.point.x, 12);
    assert_eq!(response.point.y, 23);
}

#[test]
fn tap_target_failed_recognition_is_visible_error() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let mut ledger = ledger("tap-target");
    let error = lab
        .tap_target(fixture.tap_request(blue_scene(), true), &mut ledger)
        .expect_err("invisible target must be blocked");

    assert_eq!(error.code, "target_not_visible");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.pointer("/evaluation/passed"))
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn navigate_dry_run_uses_typed_route() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let mut ledger = ledger("navigate");
    let response = lab
        .navigate(fixture.navigate_request(), &mut ledger)
        .expect("navigate plan");

    assert_eq!(response.status, "planned");
    let route = response.route.expect("route");
    assert_eq!(route.len(), 1);
    assert_eq!(route[0].id, "home_to_target");
    assert!(route[0].action_id.is_some());
}

#[test]
fn tap_target_real_execution_uses_input_port() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let mut ledger = ledger("tap-target");
    let response = lab
        .tap_target(fixture.tap_request(red_scene(), false), &mut ledger)
        .expect("real tap");

    assert_eq!(response.status, "sent");
    assert!(response.executed);
    assert_eq!(
        fixture.actions.lock().expect("actions").as_slice(),
        ["tap:12:23"]
    );
    assert_eq!(
        response.device.expect("device").report.backend,
        "test_input"
    );
}

#[test]
fn absolute_coordinate_derivation_preserves_existing_translation() {
    let derived = derive_absolute_coordinate_rect_from_match(
        "fixture",
        PackRect {
            x: 100,
            y: 200,
            width: 10,
            height: 20,
        },
        PackRect {
            x: 5,
            y: 7,
            width: 1,
            height: 1,
        },
        PackRect {
            x: 8,
            y: 11,
            width: 1,
            height: 1,
        },
    )
    .expect("translated rect");

    assert_eq!(derived.x, 103);
    assert_eq!(derived.y, 204);
    assert_eq!(derived.width, 10);
    assert_eq!(derived.height, 20);
}

struct Fixture {
    temp: TempDir,
    pack_path: PathBuf,
    pages_path: PathBuf,
    navigation_path: PathBuf,
    actions: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let pack_path = temp.path().join("fixture.pack.json");
        let pages_path = temp.path().join("fixture.pages.json");
        let navigation_path = temp.path().join("fixture.navigation.json");
        std::fs::write(
            &pack_path,
            r#"{
                "schema_version":"0.3",
                "coordinate_space":{"width":1,"height":1},
                "targets":[
                    {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
                ]
            }"#,
        )
        .expect("pack");
        std::fs::write(
            &pages_path,
            r#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"fixture/home","required":["home_anchor"]},
                    {"id":"fixture/target","required":["home_button"]}
                ]
            }"#,
        )
        .expect("pages");
        std::fs::write(
            &navigation_path,
            r#"{
                "schema_version":"0.3",
                "game":"fixture",
                "navigation":[{
                    "id":"home_to_target",
                    "from_page":"fixture/home",
                    "to_page":"fixture/target",
                    "click":{"kind":"rect","x":10,"y":20,"width":4,"height":6}
                }],
                "destructive_actions":[]
            }"#,
        )
        .expect("navigation");
        Self {
            temp,
            pack_path,
            pages_path,
            navigation_path,
            actions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn lab(&self) -> Lab<TestPorts> {
        Lab::new(
            TestPorts {
                input: RecordingInputFactory {
                    actions: self.actions.clone(),
                },
                capture: DisabledCaptureFactory,
                ledger: DisabledLedger,
                clock: FixedClock,
                config: DisabledConfig,
            },
            crate::LabState::open(self.temp.path()).expect("state"),
        )
        .expect("lab")
    }

    fn tap_request(&self, scene: Scene, dry_run: bool) -> crate::TapTargetRequest {
        crate::TapTargetRequest {
            input: self.input(scene, false),
            target: "home_button".to_string(),
            allow_destructive: false,
            dry_run,
            capture_requested: !dry_run,
            touch_config: (!dry_run).then(|| Ok(touch_config())),
        }
    }

    fn navigate_request(&self) -> crate::NavigateRequest {
        crate::NavigateRequest {
            input: self.input(red_scene(), true),
            navigation_path: Ok(self.navigation_path.clone()),
            to: "target".to_string(),
            allow_destructive: false,
            dry_run: true,
            capture_requested: false,
            touch_config: None,
            step_timeout: None,
            poll: None,
        }
    }

    fn input(&self, scene: Scene, pages: bool) -> crate::ReadonlyRecognitionInput {
        crate::ReadonlyRecognitionInput {
            pack_path: self.pack_path.clone(),
            pack_root: self.temp.path().to_path_buf(),
            pages_path: pages.then(|| self.pages_path.clone()),
            marker_request: crate::EnvMarkerResolutionRequest {
                resource_root: self.temp.path().to_path_buf(),
                instance: None,
                game: None,
                server: None,
                env_task: None,
            },
            scene: Some(scene),
            scene_path: None,
            capture_config: None,
            require_fresh: false,
            fresh_delay: Duration::from_millis(160),
        }
    }
}

struct RecordingInputFactory {
    actions: Arc<Mutex<Vec<String>>>,
}

impl InputBackendFactory for RecordingInputFactory {
    fn open(&self, request: InputBackendRequest) -> LabResult<Box<dyn InputBackend>> {
        if let Some(observation) = request.observation {
            observation.record(InputBackendReport {
                backend: "test_input".to_string(),
                requested_backend: request.config.requested.as_str().to_string(),
                adb_source: "test".to_string(),
                adb_warning: None,
                attempts: Vec::<InputBackendAttemptReport>::new(),
                warnings: Vec::new(),
                serial: request.config.target.resolved_serial(),
                device_state: "device".to_string(),
                screen_size: "Physical size: 1280x720".to_string(),
                handshake: None,
            })?;
        }
        Ok(Box::new(RecordingInput {
            actions: self.actions.clone(),
        }))
    }
}

struct RecordingInput {
    actions: Arc<Mutex<Vec<String>>>,
}

impl RecordingInput {
    fn record(&self, action: String) -> DeviceResult<()> {
        self.actions
            .lock()
            .map_err(|_| DeviceError::fatal("recording input lock poisoned"))?
            .push(action);
        Ok(())
    }
}

impl InputBackend for RecordingInput {
    fn tap(&mut self, x: i32, y: i32) -> DeviceResult<()> {
        self.record(format!("tap:{x}:{y}"))
    }

    fn long_tap(&mut self, x: i32, y: i32, duration_ms: u64) -> DeviceResult<()> {
        self.record(format!("long_tap:{x}:{y}:{duration_ms}"))
    }

    fn swipe(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, duration_ms: u64) -> DeviceResult<()> {
        self.record(format!("swipe:{x1}:{y1}:{x2}:{y2}:{duration_ms}"))
    }

    fn key(&mut self, key: &str) -> DeviceResult<()> {
        self.record(format!("key:{key}"))
    }

    fn text(&mut self, text: &str) -> DeviceResult<()> {
        self.record(format!("text:{text}"))
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.record("reset".to_string())
    }

    fn close(&mut self) -> DeviceResult<()> {
        Ok(())
    }
}

struct DisabledCaptureFactory;

impl CaptureBackendFactory for DisabledCaptureFactory {
    fn open(
        &self,
        _request: crate::CaptureBackendRequest,
    ) -> LabResult<Box<dyn actingcommand_device::CaptureBackend>> {
        Err(LabError::device("capture must not open in drive tests"))
    }
}

struct DisabledLedger;

impl LedgerSink for DisabledLedger {
    fn append_drive<T: Serialize>(&mut self, _record: &DriveRecord<T>) -> LabResult<()> {
        Err(LabError::device("ledger port must not open in drive tests"))
    }

    fn finish<T: Serialize>(&mut self, _response: &T) -> LabResult<LedgerProjection> {
        Err(LabError::device("ledger port must not open in drive tests"))
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now_unix_ms(&self) -> LabResult<u64> {
        Ok(1_750_000_000_000)
    }

    fn sleep(&self, _duration: Duration) {}
}

struct DisabledConfig;

impl ConfigSource for DisabledConfig {
    fn load(&self) -> LabResult<crate::UserConfig> {
        Err(LabError::device("config must not load in drive tests"))
    }

    fn state_root(&self) -> LabResult<PathBuf> {
        Err(LabError::device("config must not load in drive tests"))
    }
}

struct TestPorts {
    input: RecordingInputFactory,
    capture: DisabledCaptureFactory,
    ledger: DisabledLedger,
    clock: FixedClock,
    config: DisabledConfig,
}

impl LabPorts for TestPorts {
    type InputFactory = RecordingInputFactory;
    type CaptureFactory = DisabledCaptureFactory;
    type Ledger = DisabledLedger;
    type Time = FixedClock;
    type Config = DisabledConfig;

    fn input_factory(&self) -> &Self::InputFactory {
        &self.input
    }

    fn capture_factory(&self) -> &Self::CaptureFactory {
        &self.capture
    }

    fn ledger(&mut self) -> &mut Self::Ledger {
        &mut self.ledger
    }

    fn clock(&self) -> &Self::Time {
        &self.clock
    }

    fn config(&self) -> &Self::Config {
        &self.config
    }
}

fn ledger(command: &str) -> SemanticLedgerContext {
    SemanticLedgerContext::new(SemanticRequestContext {
        command: command.to_string(),
        instance: "fixture".to_string(),
        arguments: Vec::new(),
        dry_run: true,
    })
}

fn touch_config() -> TouchBackendConfig {
    TouchBackendConfig::new(
        AdbConfig::default(),
        DeviceTarget::default(),
        MaaTouchConfig::default(),
    )
}

fn red_scene() -> Scene {
    scene([255, 0, 0])
}

fn blue_scene() -> Scene {
    scene([0, 0, 255])
}

fn scene(pixel: [u8; 3]) -> Scene {
    Scene::from_pixels(1, 1, &pixel, ScenePixelFormat::Rgb8).expect("scene")
}
