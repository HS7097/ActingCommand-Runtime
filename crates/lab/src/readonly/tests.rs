// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::ports::{DisabledLedger, DisabledSemanticInput};
use crate::{CaptureBackendFactory, Clock, ConfigSource, InputBackendFactory, LabPorts};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_recognition::ScenePixelFormat;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

struct DisabledInputFactory;

impl InputBackendFactory for DisabledInputFactory {
    fn open(
        &self,
        _request: crate::InputBackendRequest,
    ) -> LabResult<Box<dyn actingcommand_device::InputBackend>> {
        Err(LabError::device(
            "input must not be opened in readonly tests",
        ))
    }
}

struct DisabledCaptureFactory;

impl CaptureBackendFactory for DisabledCaptureFactory {
    fn open(
        &self,
        _request: crate::CaptureBackendRequest,
    ) -> LabResult<Box<dyn actingcommand_device::CaptureBackend>> {
        Err(LabError::device(
            "capture must not be opened in readonly tests",
        ))
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
        Err(LabError::device(
            "config must not be loaded in readonly tests",
        ))
    }

    fn state_root(&self) -> LabResult<PathBuf> {
        Err(LabError::device(
            "config must not be loaded in readonly tests",
        ))
    }
}

struct TestPorts {
    input: DisabledInputFactory,
    semantic_input: DisabledSemanticInput,
    capture: DisabledCaptureFactory,
    ledger: DisabledLedger,
    clock: FixedClock,
    config: DisabledConfig,
}

impl LabPorts for TestPorts {
    type InputFactory = DisabledInputFactory;
    type SemanticInput = DisabledSemanticInput;
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

#[test]
fn recognize_evaluates_color_target() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let response = lab
        .recognize(crate::RecognizeRequest {
            input: fixture.input(red_scene(), false),
            target: "home_anchor".to_string(),
        })
        .expect("recognize");

    let crate::RecognizeResponse::Evaluated(response) = response else {
        panic!("color target must be evaluated");
    };
    assert!(response.passed);
    assert_eq!(response.target, "home_anchor");
    assert!(response.evaluation.color.is_some());
}

#[test]
fn recognize_click_only_does_not_require_scene() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let response = lab
        .recognize(crate::RecognizeRequest {
            input: fixture.input(None, false),
            target: "home_button".to_string(),
        })
        .expect("click-only recognize");

    let crate::RecognizeResponse::ClickOnly(response) = response else {
        panic!("click-only target must not be evaluated");
    };
    assert!(!response.evaluated);
    assert_eq!(response.click.x, 10);
}

#[test]
fn detect_page_and_current_page_share_typed_detection() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let detected = lab
        .detect_page(crate::DetectPageRequest {
            input: fixture.input(red_scene(), true),
            check_pages: false,
        })
        .expect("detect page");
    let crate::DetectPageResponse::Detection(detected) = detected.response else {
        panic!("detect-page must return page detection");
    };
    assert_eq!(detected.page, "fixture/home");
    assert!(detected.matched);

    let current = lab
        .current_page(crate::CurrentPageRequest {
            input: fixture.input(red_scene(), true),
        })
        .expect("current page");
    assert_eq!(current.page, "fixture/home");
    assert!(current.matched);
}

#[test]
fn is_visible_reports_failed_evaluation_without_fake_success() {
    let fixture = Fixture::new();
    let mut lab = fixture.lab();
    let response = lab
        .is_visible(crate::IsVisibleRequest {
            input: fixture.input(blue_scene(), false),
            target: "home_anchor".to_string(),
        })
        .expect("is visible");

    assert!(!response.visible);
    assert!(!response.evaluation.passed);
}

struct Fixture {
    temp: TempDir,
    resources: Arc<crate::ExternallyVerifiedBundle>,
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
                    {"type":"click_only","id":"home_button","click":{"x":10,"y":20,"width":4,"height":6}}
                ]
            }"#
                .as_bytes(),
            ),
            (
                "resources/recognition/fixture.test.pages.json",
            r#"{
                "schema_version":"0.3",
                "pages":[{"id":"fixture/home","required":["home_anchor"]}]
            }"#
                .as_bytes(),
            ),
        ];
        write_zip(&zip_path, &files);
        let bytes = std::fs::read(&zip_path).expect("bundle bytes");
        let expected =
            crate::ExternalExpectedSha256::parse_hex(&Sha256Hash::digest(&bytes).to_string())
                .expect("expected hash");
        let resources = crate::ExternallyVerifiedBundle::load("readonly_fixture", &bytes, expected)
            .map(Arc::new)
            .expect("verified bundle");
        Self { temp, resources }
    }

    fn lab(&self) -> Lab<TestPorts> {
        Lab::new(
            TestPorts {
                input: DisabledInputFactory,
                semantic_input: DisabledSemanticInput,
                capture: DisabledCaptureFactory,
                ledger: DisabledLedger,
                clock: FixedClock,
                config: DisabledConfig,
            },
            crate::LabState::open(self.temp.path()).expect("state"),
        )
        .expect("lab")
    }

    fn input(&self, scene: Option<Scene>, _pages: bool) -> crate::ReadonlyRecognitionInput {
        crate::ReadonlyRecognitionInput {
            resources: Arc::clone(&self.resources),
            scene,
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

fn red_scene() -> Option<Scene> {
    Some(scene([255, 0, 0]))
}

fn blue_scene() -> Option<Scene> {
    Some(scene([0, 0, 255]))
}

fn scene(pixel: [u8; 3]) -> Scene {
    Scene::from_pixels(1, 1, &pixel, ScenePixelFormat::Rgb8).expect("scene")
}
