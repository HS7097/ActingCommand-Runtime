// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ports::DisabledLedger;
use crate::{
    CaptureBackendFactory, ConfigSource, InputBackendAttemptReport, InputBackendFactory,
    InputBackendReport, InputBackendRequest, SemanticInputExecutor, SemanticRequestContext,
};
use actingcommand_contract::InputAction;
use actingcommand_device::InputBackend;
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition::{Scene, ScenePixelFormat};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

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
    resources: Arc<crate::ExternallyVerifiedBundle>,
    actions: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let zip_path = temp.path().join("fixture.zip");
        let files = [
            (
                "control.json",
                br#"{"game":"fixture","server":"test","entry_task_id":"task"}"#.as_slice(),
            ),
            (
                "resources/manifest.json",
                br#"{"schema_version":"0.3","entry_task_id":"task"}"#.as_slice(),
            ),
            ("resources/operations/task/task.json", br#"{}"#.as_slice()),
            (
                "resources/recognition/fixture.test.pack.json",
            r#"{
                "schema_version":"0.3",
                "coordinate_space":{"width":1,"height":1},
                "targets":[
                    {"type":"color","id":"home_anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"home_button","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0],"click":{"x":10,"y":20,"width":4,"height":6}}
                ]
            }"#
                .as_bytes(),
            ),
            (
                "resources/recognition/fixture.test.pages.json",
            r#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"fixture/home","required":["home_anchor"]},
                    {"id":"fixture/target","required":["home_button"]}
                ]
            }"#
                .as_bytes(),
            ),
            (
                "resources/navigation/fixture.test.navigation.json",
            r#"{
                "schema_version":"0.3",
                "game":"fixture",
                "navigation":[{
                    "id":"home_to_target",
                    "from_page":"fixture/home",
                    "to_page":"fixture/target",
                    "effect":"navigation_only",
                    "click":{"kind":"rect","x":10,"y":20,"width":4,"height":6}
                }],
                "destructive_actions":[]
            }"#
                .as_bytes(),
            ),
        ];
        write_zip(&zip_path, &files);
        let bytes = std::fs::read(&zip_path).expect("bundle bytes");
        let expected =
            crate::ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(&bytes).to_string())
                .expect("expected hash");
        let resources = crate::ExternallyVerifiedBundle::load("drive_fixture", &bytes, expected)
            .map(Arc::new)
            .expect("verified bundle");
        Self {
            temp,
            resources,
            actions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn lab(&self) -> Lab<TestPorts> {
        Lab::new(
            TestPorts {
                input: DisabledInputFactory,
                semantic_input: RecordingSemanticInput {
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
        }
    }

    fn navigate_request(&self) -> crate::NavigateRequest {
        crate::NavigateRequest {
            input: self.input(red_scene(), true),
            to: "target".to_string(),
            allow_destructive: false,
            dry_run: true,
            capture_requested: false,
            step_timeout: None,
            poll: None,
        }
    }

    fn input(&self, scene: Scene, _pages: bool) -> crate::ReadonlyRecognitionInput {
        crate::ReadonlyRecognitionInput {
            resources: Arc::clone(&self.resources),
            scene: Some(scene),
            scene_path: None,
            capture_config: None,
            require_fresh: false,
            fresh_delay: Duration::from_millis(160),
        }
    }
}

fn write_zip(path: &std::path::Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("create bundle");
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        zip.start_file(*name, options).expect("start bundle entry");
        zip.write_all(bytes).expect("write bundle entry");
    }
    zip.finish().expect("finish bundle");
}

struct DisabledInputFactory;

impl InputBackendFactory for DisabledInputFactory {
    fn open(&self, _request: InputBackendRequest) -> LabResult<Box<dyn InputBackend>> {
        Err(LabError::device(
            "drive must use the semantic Runtime command port",
        ))
    }
}

struct RecordingSemanticInput {
    actions: Arc<Mutex<Vec<String>>>,
}

impl SemanticInputExecutor for RecordingSemanticInput {
    fn execute(&self, action: InputAction) -> LabResult<InputBackendReport> {
        let action = match action {
            InputAction::Tap { x, y } => format!("tap:{x}:{y}"),
            InputAction::LongTap { x, y, duration_ms } => {
                format!("long_tap:{x}:{y}:{duration_ms}")
            }
            InputAction::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => format!("swipe:{x1}:{y1}:{x2}:{y2}:{duration_ms}"),
            InputAction::Key { key } => format!("key:{key}"),
            InputAction::Text { text } => format!("text:{text}"),
            InputAction::Reset => "reset".to_string(),
        };
        self.actions
            .lock()
            .map_err(|_| LabError::device("recording semantic input lock poisoned"))?
            .push(action);
        Ok(InputBackendReport {
            backend: "test_input".to_string(),
            requested_backend: "runtime_owned".to_string(),
            adb_source: "runtime_owned".to_string(),
            adb_warning: None,
            attempts: Vec::<InputBackendAttemptReport>::new(),
            warnings: Vec::new(),
            serial: "<runtime-owned>".to_string(),
            device_state: "runtime_owned".to_string(),
            screen_size: "<runtime-owned>".to_string(),
            handshake: None,
        })
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
    input: DisabledInputFactory,
    semantic_input: RecordingSemanticInput,
    capture: DisabledCaptureFactory,
    ledger: DisabledLedger,
    clock: FixedClock,
    config: DisabledConfig,
}

impl LabPorts for TestPorts {
    type InputFactory = DisabledInputFactory;
    type SemanticInput = RecordingSemanticInput;
    type CaptureFactory = DisabledCaptureFactory;
    type Ledger = DisabledLedger;
    type Time = FixedClock;
    type Config = DisabledConfig;

    fn input_factory(&self) -> &Self::InputFactory {
        &self.input
    }

    fn semantic_input(&self) -> &Self::SemanticInput {
        &self.semantic_input
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

fn red_scene() -> Scene {
    scene([255, 0, 0])
}

fn blue_scene() -> Scene {
    scene([0, 0, 255])
}

fn scene(pixel: [u8; 3]) -> Scene {
    Scene::from_pixels(1, 1, &pixel, ScenePixelFormat::Rgb8).expect("scene")
}
