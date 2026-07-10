// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{DriveRecord, LedgerProjection};
use actingcommand_page_detector::PageTargetEvaluation;
use actingcommand_recognition_pack::load_pack_from_json_str;
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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

#[derive(Default)]
struct DeviceResolverCounters {
    resolved_ids: Mutex<Vec<String>>,
    provenance: AtomicUsize,
    capture: AtomicUsize,
    touch: AtomicUsize,
}

struct TestDeviceResolver {
    selected: crate::LabRunSelectedDevice,
    counters: Arc<DeviceResolverCounters>,
    lease_root: PathBuf,
    require_lease_before_capture: bool,
    fail_provenance: bool,
}

struct SequencedRuntimeCommitSource {
    values: Mutex<VecDeque<Option<String>>>,
}

impl SequencedRuntimeCommitSource {
    fn new(values: impl IntoIterator<Item = Option<&'static str>>) -> Self {
        Self {
            values: Mutex::new(
                values
                    .into_iter()
                    .map(|value| value.map(str::to_string))
                    .collect(),
            ),
        }
    }
}

impl crate::RuntimeCommitSource for SequencedRuntimeCommitSource {
    fn sample(&self) -> Option<String> {
        self.values
            .lock()
            .expect("commit source")
            .pop_front()
            .flatten()
    }
}

impl crate::LabRunDeviceResolver for TestDeviceResolver {
    fn resolve_serial(&mut self, instance_id: &str) -> CliOutcome<crate::LabRunSelectedDevice> {
        self.counters
            .resolved_ids
            .lock()
            .expect("resolver ids")
            .push(instance_id.to_string());
        if instance_id != self.selected.id {
            return Err(CliError::instance(format!(
                "unexpected selected instance '{instance_id}'"
            )));
        }
        Ok(self.selected.clone())
    }

    fn global_adb_provenance(&mut self) -> CliOutcome<String> {
        self.counters.provenance.fetch_add(1, Ordering::SeqCst);
        if self.fail_provenance {
            return Err(CliError::device("synthetic global provenance failure"));
        }
        Ok("adb".to_string())
    }

    fn capture_config(
        &mut self,
        device: &crate::LabRunSelectedDevice,
    ) -> CliOutcome<actingcommand_device::CaptureBackendConfig> {
        self.counters.capture.fetch_add(1, Ordering::SeqCst);
        if self.require_lease_before_capture {
            let lock = self
                .lease_root
                .join(format!("{}.lock", sanitize_path_segment(&device.serial)));
            if !lock.is_file() {
                return Err(CliError::device(
                    "capture config resolved before the Lab lease was acquired",
                ));
            }
        }
        Ok(actingcommand_device::CaptureBackendConfig::new(
            actingcommand_device::AdbConfig::default(),
            actingcommand_device::DeviceTarget {
                serial: Some(device.serial.clone()),
                ..Default::default()
            },
        ))
    }

    fn touch_config(
        &mut self,
        device: &crate::LabRunSelectedDevice,
    ) -> CliOutcome<actingcommand_device::TouchBackendConfig> {
        self.counters.touch.fetch_add(1, Ordering::SeqCst);
        Ok(actingcommand_device::TouchBackendConfig::new(
            actingcommand_device::AdbConfig::default(),
            actingcommand_device::DeviceTarget {
                serial: Some(device.serial.clone()),
                ..Default::default()
            },
            actingcommand_device::MaaTouchConfig::default(),
        ))
    }
}

fn test_device_resolver(
    id: &str,
    serial: &str,
    counters: Arc<DeviceResolverCounters>,
    lease_root: PathBuf,
    require_lease_before_capture: bool,
) -> Box<dyn crate::LabRunDeviceResolver> {
    Box::new(TestDeviceResolver {
        selected: crate::LabRunSelectedDevice {
            id: id.to_string(),
            serial: serial.to_string(),
        },
        counters,
        lease_root,
        require_lease_before_capture,
        fail_provenance: false,
    })
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

struct TestLedgerSink {
    run_starts: Arc<AtomicUsize>,
}

struct TestRunLedgerSession {
    ledger: Option<LabLedger>,
    run_starts: Arc<AtomicUsize>,
}

impl TestLedgerSink {
    fn new() -> Self {
        Self {
            run_starts: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl crate::LedgerSink for TestLedgerSink {
    type RunSession = TestRunLedgerSession;

    fn append_drive<T: Serialize>(&mut self, _record: &DriveRecord<T>) -> CliOutcome<()> {
        Err(CliError::device("unexpected test ledger effect"))
    }

    fn finish<T: Serialize>(&mut self, _response: &T) -> CliOutcome<LedgerProjection> {
        Err(CliError::device("unexpected test ledger effect"))
    }

    fn run_session(&mut self) -> Self::RunSession {
        TestRunLedgerSession {
            ledger: None,
            run_starts: self.run_starts.clone(),
        }
    }

    fn start_run_session(
        session: &mut Self::RunSession,
        request: crate::RunLedgerSessionRequest,
    ) -> actingcommand_ledger::LabLogResult<PathBuf> {
        session.run_starts.fetch_add(1, Ordering::SeqCst);
        if session.ledger.is_some() {
            return Err(LabLogError::InvalidInput(
                "runtime ledger session is already started".to_string(),
            ));
        }
        let ledger = LabLedger::create_runtime_shard(
            request.run_root,
            &request.run_id,
            &request.instance,
            request.header,
        )?;
        let path = ledger.ledger_path().to_path_buf();
        session.ledger = Some(ledger);
        Ok(path)
    }

    fn append_run_record(
        session: &mut Self::RunSession,
        record: LedgerRecord,
    ) -> actingcommand_ledger::LabLogResult<()> {
        test_run_ledger_mut(session)?.append(record)
    }

    fn append_run_event(
        session: &mut Self::RunSession,
        event: LightEvent,
    ) -> actingcommand_ledger::LabLogResult<()> {
        test_run_ledger_mut(session)?.append_event(event)
    }

    fn sync_run_session(session: &Self::RunSession) -> actingcommand_ledger::LabLogResult<()> {
        test_run_ledger(session)?.sync()
    }

    fn read_run_session(
        session: &Self::RunSession,
    ) -> actingcommand_ledger::LabLogResult<actingcommand_ledger::LedgerRead> {
        LabLedger::read(test_run_ledger(session)?.ledger_path())
    }

    fn write_run_last_resort(
        run_root: Option<&Path>,
        error: &LastResortError,
    ) -> actingcommand_ledger::LabLogResult<PathBuf> {
        actingcommand_ledger::write_last_resort_error(run_root, error)
    }
}

fn test_run_ledger(
    session: &TestRunLedgerSession,
) -> actingcommand_ledger::LabLogResult<&LabLedger> {
    session.ledger.as_ref().ok_or_else(|| {
        LabLogError::InvalidInput("runtime ledger handle is unavailable".to_string())
    })
}

fn test_run_ledger_mut(
    session: &mut TestRunLedgerSession,
) -> actingcommand_ledger::LabLogResult<&mut LabLedger> {
    session.ledger.as_mut().ok_or_else(|| {
        LabLogError::InvalidInput("runtime ledger handle is unavailable".to_string())
    })
}

impl LabRunContext<'static, TestLedgerSink> {
    fn create(run_root: &Path, input_zip: &Path) -> CliOutcome<Self> {
        let mut ledger = TestLedgerSink::new();
        let ledger_session = ledger.run_session();
        Self::create_with_context(
            run_root,
            input_zip,
            crate::LabRunProcessContext {
                current_dir: None,
                lease_root: run_root.join("locks"),
                os: "test".to_string(),
                app_version: "actinglab-test".to_string(),
                runtime_commit_source: Arc::new(EmptyRuntimeCommitSource),
                memory_source: crate::MemorySampleSource::fixed(crate::MemorySample {
                    total_bytes: 8 * 1024 * 1024 * 1024,
                    available_bytes: 4 * 1024 * 1024 * 1024,
                }),
            },
            &TEST_CLOCK,
            ledger_session,
        )
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
    ledger: TestLedgerSink,
    clock: TestClock,
    config: TestConfigSource,
}

impl crate::LabPorts for TestPorts {
    type InputFactory = TestInputFactory;
    type CaptureFactory = TestCaptureFactory;
    type Ledger = TestLedgerSink;
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
            ledger: TestLedgerSink::new(),
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
        device_resolver: test_device_resolver(
            "127.0.0.1:1",
            "127.0.0.1:1",
            Arc::new(DeviceResolverCounters::default()),
            root.join("locks"),
            false,
        ),
        capture_interval_override: None,
        capture_backend_override: None,
        frame_store_override: FrameStoreControl::default(),
        expected_input_sha256: None,
        process: crate::LabRunProcessContext {
            current_dir: Some(root.to_path_buf()),
            lease_root: root.join("locks"),
            os: "test".to_string(),
            app_version: "actinglab-test".to_string(),
            runtime_commit_source: Arc::new(EmptyRuntimeCommitSource),
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
