// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_page_detector::PageTargetEvaluation;
use actingcommand_recognition_pack::load_pack_from_json_str;
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
    validation_ledger_starts: Mutex<Vec<usize>>,
    validation_lease_present: Mutex<Vec<bool>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SelectedConfigFailure {
    Capture,
    Touch,
    Provenance,
}

struct TestDeviceResolver {
    selected: crate::LabRunSelectedDevice,
    counters: Arc<DeviceResolverCounters>,
    lease_root: PathBuf,
    failure: Option<SelectedConfigFailure>,
    ledger_starts: Option<Arc<AtomicUsize>>,
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
    fn resolve_selected(&mut self, instance_id: &str) -> CliOutcome<crate::LabRunSelectedDevice> {
        self.counters
            .resolved_ids
            .lock()
            .expect("resolver ids")
            .push(instance_id.to_string());
        if instance_id != self.selected.id() {
            return Err(CliError::instance(format!(
                "unexpected selected instance '{instance_id}'"
            )));
        }
        self.record_validation_boundary();
        self.counters.capture.fetch_add(1, Ordering::SeqCst);
        if self.failure == Some(SelectedConfigFailure::Capture) {
            return Err(CliError::device("synthetic selected capture failure"));
        }
        self.counters.touch.fetch_add(1, Ordering::SeqCst);
        if self.failure == Some(SelectedConfigFailure::Touch) {
            return Err(CliError::device("synthetic selected touch failure"));
        }
        self.counters.provenance.fetch_add(1, Ordering::SeqCst);
        if self.failure == Some(SelectedConfigFailure::Provenance) {
            return Err(CliError::device("synthetic global provenance failure"));
        }
        Ok(self.selected.clone())
    }
}

impl TestDeviceResolver {
    fn record_validation_boundary(&self) {
        let ledger_starts = self
            .ledger_starts
            .as_ref()
            .map_or(0, |starts| starts.load(Ordering::SeqCst));
        self.counters
            .validation_ledger_starts
            .lock()
            .expect("validation ledger starts")
            .push(ledger_starts);
        let lock = self.lease_root.join(format!(
            "{}.lock",
            sanitize_path_segment(self.selected.serial())
        ));
        self.counters
            .validation_lease_present
            .lock()
            .expect("validation lease state")
            .push(lock.is_file());
    }
}

fn test_device_resolver(
    id: &str,
    serial: &str,
    counters: Arc<DeviceResolverCounters>,
    lease_root: PathBuf,
) -> Box<dyn crate::LabRunDeviceResolver> {
    Box::new(TestDeviceResolver {
        selected: test_selected_device(id, serial),
        counters,
        lease_root,
        failure: None,
        ledger_starts: None,
    })
}

fn test_selected_device(id: &str, serial: &str) -> crate::LabRunSelectedDevice {
    let adb = actingcommand_device::AdbConfig::default();
    let target = actingcommand_device::DeviceTarget {
        serial: Some(serial.to_string()),
        ..Default::default()
    };
    crate::LabRunSelectedDevice::new(
        id,
        serial,
        "adb",
        actingcommand_device::CaptureBackendConfig::new(adb.clone(), target.clone()),
        actingcommand_device::TouchBackendConfig::new(
            adb,
            target,
            actingcommand_device::MaaTouchConfig::default(),
        ),
    )
}

struct TestCaptureFactory {
    opens: Arc<AtomicUsize>,
    lease_root: PathBuf,
}

impl crate::CaptureBackendFactory for TestCaptureFactory {
    fn open(&self, request: crate::CaptureBackendRequest) -> CliOutcome<Box<dyn CaptureBackend>> {
        let serial = request.config.target.resolved_serial();
        let lock = self
            .lease_root
            .join(format!("{}.lock", sanitize_path_segment(&serial)));
        if !lock.is_file() {
            return Err(CliError::device(
                "capture backend opened before the Lab lease was acquired",
            ));
        }
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
    run_records: Arc<AtomicUsize>,
    run_events: Arc<AtomicUsize>,
    run_reads: Arc<AtomicUsize>,
}

struct TestRunLedgerSession {
    ledger: Option<LabLedger>,
    run_starts: Arc<AtomicUsize>,
    run_records: Arc<AtomicUsize>,
    run_events: Arc<AtomicUsize>,
    run_reads: Arc<AtomicUsize>,
}

impl TestLedgerSink {
    fn new() -> Self {
        Self {
            run_starts: Arc::new(AtomicUsize::new(0)),
            run_records: Arc::new(AtomicUsize::new(0)),
            run_events: Arc::new(AtomicUsize::new(0)),
            run_reads: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl crate::LedgerSink for TestLedgerSink {
    type RunSession = TestRunLedgerSession;

    fn run_session(&mut self) -> Self::RunSession {
        TestRunLedgerSession {
            ledger: None,
            run_starts: self.run_starts.clone(),
            run_records: self.run_records.clone(),
            run_events: self.run_events.clone(),
            run_reads: self.run_reads.clone(),
        }
    }

    fn start_run_session(
        session: &mut Self::RunSession,
        request: crate::RunLedgerSessionRequest,
    ) -> CliOutcome<PathBuf> {
        session.run_starts.fetch_add(1, Ordering::SeqCst);
        if session.ledger.is_some() {
            return Err(CliError::package_invalid(
                "invalid lab logging input: runtime ledger session is already started",
            ));
        }
        let header =
            serde_json::from_str(&request.header().encoded_json()?).map_err(test_ledger_error)?;
        let ledger = LabLedger::create_runtime_shard(
            request.run_root(),
            request.run_id(),
            request.instance(),
            header,
        )
        .map_err(test_ledger_error)?;
        let path = ledger.ledger_path().to_path_buf();
        session.ledger = Some(ledger);
        Ok(path)
    }

    fn append_run_record(
        session: &mut Self::RunSession,
        record: crate::LedgerRecordEntry,
    ) -> CliOutcome<()> {
        session.run_records.fetch_add(1, Ordering::SeqCst);
        test_run_ledger_mut(session)?
            .append(record.into_storage())
            .map_err(test_ledger_error)
    }

    fn append_run_event(
        session: &mut Self::RunSession,
        event: crate::LedgerEventEntry,
    ) -> CliOutcome<()> {
        session.run_events.fetch_add(1, Ordering::SeqCst);
        test_run_ledger_mut(session)?
            .append_event(event.into_storage())
            .map_err(test_ledger_error)
    }

    fn sync_run_session(session: &Self::RunSession) -> CliOutcome<()> {
        test_run_ledger(session)?.sync().map_err(test_ledger_error)
    }

    fn read_run_session(session: &Self::RunSession) -> CliOutcome<crate::LedgerReadback> {
        session.run_reads.fetch_add(1, Ordering::SeqCst);
        LabLedger::read(test_run_ledger(session)?.ledger_path())
            .map(crate::LedgerReadback::from_storage)
            .map_err(test_ledger_error)
    }

    fn write_run_last_resort(
        run_root: Option<&Path>,
        error: &crate::LedgerLastResort,
    ) -> CliOutcome<PathBuf> {
        actingcommand_ledger::write_last_resort_error(run_root, error.storage())
            .map_err(test_ledger_error)
    }
}

fn test_run_ledger(session: &TestRunLedgerSession) -> CliOutcome<&LabLedger> {
    session.ledger.as_ref().ok_or_else(|| {
        CliError::package_invalid("invalid lab logging input: runtime ledger handle is unavailable")
    })
}

fn test_run_ledger_mut(session: &mut TestRunLedgerSession) -> CliOutcome<&mut LabLedger> {
    session.ledger.as_mut().ok_or_else(|| {
        CliError::package_invalid("invalid lab logging input: runtime ledger handle is unavailable")
    })
}

fn test_ledger_error(error: impl std::fmt::Display) -> CliError {
    CliError::package_invalid(error.to_string())
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
    semantic_input: crate::ports::DisabledSemanticInput,
    capture: TestCaptureFactory,
    ledger: TestLedgerSink,
    clock: TestClock,
    config: TestConfigSource,
}

impl crate::LabPorts for TestPorts {
    type InputFactory = TestInputFactory;
    type SemanticInput = crate::ports::DisabledSemanticInput;
    type CaptureFactory = TestCaptureFactory;
    type Ledger = TestLedgerSink;
    type Time = TestClock;
    type Config = TestConfigSource;

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

fn test_lab(root: &Path) -> Lab<TestPorts> {
    Lab::new(
        TestPorts {
            input: TestInputFactory {
                opens: Arc::new(AtomicUsize::new(0)),
            },
            semantic_input: crate::ports::DisabledSemanticInput,
            capture: TestCaptureFactory {
                opens: Arc::new(AtomicUsize::new(0)),
                lease_root: root.join("locks"),
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
