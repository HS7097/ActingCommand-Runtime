// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::LabRunDeviceCandidate;
use actingcommand_contract::{DriveRecord, LedgerProjection};
use actingcommand_page_detector::PageTargetEvaluation;
use actingcommand_recognition_pack::load_pack_from_json_str;
use serde::Serialize;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use zip::ZipArchive;

struct TestInputFactory {
    opens: Arc<AtomicUsize>,
}

impl crate::InputBackendFactory for TestInputFactory {
    fn open(&self, _request: crate::InputBackendRequest) -> CliOutcome<Box<dyn InputBackend>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(NoopInputBackend))
    }
}

struct NoopInputBackend;

impl InputBackend for NoopInputBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }

    fn long_tap(
        &mut self,
        _x: i32,
        _y: i32,
        _duration_ms: u64,
    ) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }

    fn key(&mut self, _key: &str) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }

    fn text(&mut self, _text: &str) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }

    fn reset(&mut self) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }

    fn close(&mut self) -> actingcommand_device::DeviceResult<()> {
        Ok(())
    }
}

struct TestCaptureFactory {
    opens: Arc<AtomicUsize>,
}

impl crate::CaptureBackendFactory for TestCaptureFactory {
    fn open(&self, request: crate::CaptureBackendRequest) -> CliOutcome<Box<dyn CaptureBackend>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        if let Some(observation) = request.observation {
            observation.record(crate::CaptureBackendReport {
                requested: request.config.requested,
                used: CaptureBackendName::AdbScreencap,
                attempts: Vec::new(),
            })?;
        }
        let frame = Frame::from_pixels(
            1280,
            720,
            vec![0; 1280 * 720 * 3],
            PixelFormat::Rgb8,
            CaptureBackendName::AdbScreencap,
        )
        .map_err(|error| CliError::device(error.to_string()))?;
        Ok(Box::new(StaticCapture { frame }))
    }
}

struct UnusedLedger;

impl crate::LedgerSink for UnusedLedger {
    fn append_drive<T: Serialize>(&mut self, _record: &DriveRecord<T>) -> CliOutcome<()> {
        Err(CliError::device("unexpected test ledger effect"))
    }

    fn finish<T: Serialize>(&mut self, _response: &T) -> CliOutcome<LedgerProjection> {
        Err(CliError::device("unexpected test ledger effect"))
    }
}

struct TestConfigSource;

impl crate::ConfigSource for TestConfigSource {
    fn load(&self) -> CliOutcome<crate::UserConfig> {
        Ok(crate::UserConfig::default())
    }

    fn state_root(&self) -> CliOutcome<PathBuf> {
        Ok(PathBuf::from("."))
    }
}

struct TestPorts {
    input: TestInputFactory,
    capture: TestCaptureFactory,
    ledger: UnusedLedger,
    clock: TestClock,
    config: TestConfigSource,
}

impl crate::LabPorts for TestPorts {
    type InputFactory = TestInputFactory;
    type CaptureFactory = TestCaptureFactory;
    type Ledger = UnusedLedger;
    type Time = TestClock;
    type Config = TestConfigSource;

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

fn test_lab(root: &Path) -> Lab<TestPorts> {
    Lab::new(
        TestPorts {
            input: TestInputFactory {
                opens: Arc::new(AtomicUsize::new(0)),
            },
            capture: TestCaptureFactory {
                opens: Arc::new(AtomicUsize::new(0)),
            },
            ledger: UnusedLedger,
            clock: TestClock,
            config: TestConfigSource,
        },
        crate::LabState::open(root).expect("test Lab state"),
    )
    .expect("test Lab")
}

fn test_run_request(zip: PathBuf, out: PathBuf, root: &Path) -> LabRunRequest {
    LabRunRequest {
        zip_path: zip,
        out_path: out,
        run_root: root.join("runs"),
        game: None,
        server: None,
        instance: Some("127.0.0.1:1".to_string()),
        device_candidates: Vec::new(),
        capture_interval_override: None,
        capture_backend_override: None,
        frame_store_override: FrameStoreControl::default(),
        expected_input_sha256: None,
        process: crate::LabRunProcessContext {
            current_dir: Some(root.to_path_buf()),
            lease_root: root.join("locks"),
            os: "test".to_string(),
            runtime_commit: None,
            memory_source: crate::MemorySampleSource::fixed(crate::MemorySample {
                total_bytes: 8 * 1024 * 1024 * 1024,
                available_bytes: 4 * 1024 * 1024 * 1024,
            }),
        },
    }
}

include!("bundle_and_actions.rs");
include!("guards_and_recovery.rs");
include!("context_and_output.rs");
include!("fixtures.rs");
