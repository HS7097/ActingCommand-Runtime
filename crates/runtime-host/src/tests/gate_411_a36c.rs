// SPDX-License-Identifier: AGPL-3.0-only

// #411 A36c (0.12.9, the remainder of A36b): a PP-OCR diagnostic archive failure of a kind that
// does not stop the Runtime ends only the work that owns the archive. The failure here is A36b's:
// the seal finds the staged report changed under it (`artifact_hash_mismatch` at
// `seal_artifact_stream`, with the byte counts it compared). The work records its terminal with
// that code and those counts, and the host keeps serving.
//
// T1 is a direct contained-task run on instance A, whose page recognition archives its report;
// instance B's run is accepted and completes afterwards. T2a and T2b are the two other callers
// of the archive: an online observation of a page and the OCR of a saved frame.
//
// The foreign write lands between the report's writes and its seal (the host's test hook
// `before_next_ppocr_seal_for_test`, on the archiving thread). It appends to the staged report,
// so the staged count is the written count plus the foreign bytes.

use super::instance_workers::all_events;
use super::*;
use actingcommand_artifact_store::{StoreCode, StoreLocation};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ContainedObservationRequest, PPOCR_NODE_PLACEMENT_RECORD_TYPE, PpocrNodePlacementDiagnostic,
    SavedArtifactOcrRequest, SavedArtifactOcrSource,
};
use actingcommand_recognition_pack::{
    OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
};
use serde_json::Value as JsonValue;
use std::sync::Mutex;

const INSTANCE_A: &str = "a36c.instance.a";
const INSTANCE_B: &str = "a36c.instance.b";
// The file name an open artifact stream stages its bytes under.
const STAGING_PREFIX: &str = ".artifact-stream.partial-";
// How a staged PP-OCR diagnostic report begins.
const REPORT_PREFIX: &[u8] = br#"{"schema_version":"actingcommand.runtime.ppocr-diagnostic.v1""#;
const FOREIGN: &[u8] = b"foreign";

// The fixture OCR provider whose every reading carries one PP-OCR node-placement report, as the
// first reading of a provider session does, and its execution evidence.
#[derive(Debug, Default)]
struct ReportingVision(FakeVisionProvider);

fn placement_report() -> Arc<PpocrNodePlacementDiagnostic> {
    Arc::new(PpocrNodePlacementDiagnostic {
        record_type: PPOCR_NODE_PLACEMENT_RECORD_TYPE.to_owned(),
        model_role: "recognizer".to_owned(),
        diagnostic_scope: "session_initialization_only".to_owned(),
        inference_executed: false,
        cpu_assigned_node_count: 0,
        nodes: Vec::new(),
    })
}

impl VisionProvider for ReportingVision {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        self.0.require_ocr_model(model_ref, model_sha256)
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        self.0.require_nn_model(model_ref, model_sha256)
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        let mut result = self.0.read_text(request)?;
        result.ppocr_diagnostics.push(placement_report());
        Ok(result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let model_ref = request.model_ref.to_owned();
        let model_sha256 = request.model_sha256.to_owned();
        Ok(OcrProviderObservation {
            result: self.read_text(request)?,
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("a36c-ocr-{}", self.0.ocr_calls.load(Ordering::Acquire)),
                session_id: "a36c-session".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref,
                model_sha256,
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
        })
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        self.0.classify(request)
    }
}

fn reporting_vision() -> Arc<dyn VisionProvider> {
    Arc::new(ReportingVision::default())
}

fn send(host: &RuntimeHost, request: &RuntimeRequest, connection: u64) -> RuntimeReceipt {
    host.process_request_for_test(request, ConnectionId::new(connection).expect("connection"))
        .expect("a receipt")
}

// The staging files of the artifact streams open below `root`.
fn staging_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(STAGING_PREFIX))
            {
                found.push(path);
            }
        }
    }
    found
}

// What the foreign write met: per staged PP-OCR report, its path, its length before the write and
// the write's result; per staging file it could not read, its path and the read's error.
type ForeignAppends = Arc<Mutex<Vec<(PathBuf, u64, Result<(), std::io::ErrorKind>)>>>;

// Once the next PP-OCR report is written and before its seal, appends FOREIGN to every staged
// PP-OCR report below `root`. Other open streams (a run's task diagnostic) are left alone.
fn append_before_next_ppocr_seal(host: &RuntimeHost, root: &Path) -> ForeignAppends {
    let appended = ForeignAppends::default();
    let record = Arc::clone(&appended);
    let root = root.to_path_buf();
    host.before_next_ppocr_seal_for_test(move || {
        for staging in staging_files(&root) {
            let staged = match fs::read(&staging) {
                Ok(staged) => staged,
                Err(error) => {
                    record
                        .lock()
                        .expect("foreign appends")
                        .push((staging, 0, Err(error.kind())));
                    continue;
                }
            };
            if !staged.starts_with(REPORT_PREFIX) {
                continue;
            }
            let written = OpenOptions::new()
                .append(true)
                .open(&staging)
                .and_then(|mut file| file.write_all(FOREIGN));
            record.lock().expect("foreign appends").push((
                staging,
                staged.len() as u64,
                written.map_err(|error| error.kind()),
            ));
        }
    })
    .expect("install the foreign write");
    appended
}

// The one foreign append into one staged report, as the counts its seal compares: the written
// stream and the staged file.
fn one_foreign_append(appended: &ForeignAppends) -> (u64, u64) {
    let appends = appended.lock().expect("foreign appends").clone();
    let [(_, written, Ok(()))] = appends.as_slice() else {
        panic!("one foreign write into one staged PP-OCR report: {appends:?}");
    };
    (*written, *written + FOREIGN.len() as u64)
}

// The byte counts an outcome says the seal compared: `expected_bytes` (the written stream) and
// `actual_bytes` (the staged file).
fn compared_counts(outcome: &Outcome) -> (u64, u64) {
    let count = |key| {
        outcome
            .value(key)
            .and_then(JsonValue::as_u64)
            .unwrap_or_else(|| panic!("{key} of {outcome}"))
    };
    (count("expected_bytes"), count("actual_bytes"))
}

// The failed receipt names the seal's mismatch with the counts it compared, and it is not fatal.
fn assert_failed_by_the_mismatch(failed: &RuntimeReceipt, compared: (u64, u64)) {
    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    assert_eq!(
        failed.outcome().code().as_str(),
        StoreCode::ArtifactHashMismatch.as_str(),
        "{failed:?}"
    );
    assert!(
        !failed.error_projection().expect("the failure").fatal,
        "{failed:?}"
    );
    assert_eq!(compared_counts(failed.outcome()), compared);
}

// The outcome a record carries is the seal's mismatch at `seal_artifact_stream`, with the counts
// it compared.
fn assert_the_mismatch(outcome: &Outcome, compared: (u64, u64)) {
    assert_eq!(
        outcome.code().as_str(),
        StoreCode::ArtifactHashMismatch.as_str(),
        "{outcome}"
    );
    assert_eq!(
        outcome.value("operation"),
        Some(&JsonValue::from(StoreLocation::SealArtifactStream.as_str())),
        "{outcome}"
    );
    assert_eq!(compared_counts(outcome), compared);
}

// The events recorded under the request's correlation (each request here has its own).
fn correlated_events(host: &RuntimeHost, request: &RuntimeRequest) -> Vec<PersistedEvent> {
    host.query_persisted_events_for_test(EventQuery {
        correlation_id: Some(request.correlation_id()),
        ..EventQuery::default()
    })
    .expect("the request's events")
}

// The recorded event the receipt's terminal names.
fn recorded_terminal(
    host: &RuntimeHost,
    request: &RuntimeRequest,
    receipt: &RuntimeReceipt,
) -> PersistedEvent {
    let terminal = receipt.terminal().expect("the receipt names its terminal");
    correlated_events(host, request)
        .into_iter()
        .find(|event| {
            event.sequence() == terminal.sequence && *event.event_id() == terminal.event_id
        })
        .unwrap_or_else(|| panic!("the recorded terminal {terminal:?}"))
}

// A direct run of `package` on `instance_alias`, as `actingctl` sends it.
fn direct_run(instance_alias: &str, package: &str) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            instance_alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    )
}

// Instance A's package: the neutral task whose pages are read by OCR, with the frame settings of
// `gate_369s5::direct_run_package`.
fn ocr_run_package(root: &TempDir, state: &FakeState) -> String {
    let package = root.path().join("neutral-ocr-task.zip");
    fs::write(&package, neutral_vision_contained_task_package(true)).expect("write package");
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    package.display().to_string()
}

// T1: instance A's direct run archives the PP-OCR report of its page recognition, and a foreign
// write changes the staged report before its seal. The run fails with a `task.failed` terminal
// carrying the mismatch and the counts; the receipt is not fatal; the run's lease is released
// once. The host is not latched: B's direct run is accepted and completes, and the host closes
// normally.
#[test]
fn a36c_a_run_whose_ocr_diagnostic_seal_mismatches_fails_only_itself_and_the_host_keeps_serving() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = ocr_run_package(&root, &state);
    let other_package = super::gate_369s5::direct_run_package(&root, &other_state);
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::from_entries([
                (INSTANCE_A.to_owned(), instance_id(), Arc::clone(&state)),
                (
                    INSTANCE_B.to_owned(),
                    instance_id(),
                    Arc::clone(&other_state),
                ),
            ])
            .with_vision_provider(reporting_vision()),
        ),
    )
    .expect("runtime host with two instances");
    let appended = append_before_next_ppocr_seal(&host, root.path());
    let request = direct_run(INSTANCE_A, &package);

    let failed = send(&host, &request, 1371);

    let compared = one_foreign_append(&appended);
    assert_failed_by_the_mismatch(&failed, compared);
    let terminals = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request.request_id()),
            event_type: Some(EventType::TaskFailed),
            ..EventQuery::default()
        })
        .expect("the run's terminals");
    let [terminal] = terminals.as_slice() else {
        panic!("one task.failed of the run: {terminals:#?}");
    };
    assert_the_mismatch(
        terminal
            .payload()
            .outcome()
            .expect("the terminal's failure"),
        compared,
    );
    let lease = terminal
        .links()
        .lease_id()
        .copied()
        .expect("the terminal names its lease");
    let released = all_events(&host)
        .iter()
        .filter(|event| {
            event.event_type() == EventType::LeaseReleased
                && event.links().lease_id() == Some(&lease)
        })
        .count();
    assert_eq!(released, 1, "the run's lease is released once");
    assert!(host.fatal_error().expect("runtime health").is_none());

    let other = send(&host, &direct_run(INSTANCE_B, &other_package), 1372);
    assert_eq!(other.state(), RuntimeReceiptState::Completed, "{other:?}");
    assert!(matches!(
        other.result(),
        Some(RuntimeResult::ContainedTaskCompleted {
            task_result: TaskOutcome::Success,
            ..
        })
    ));
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// The observation package of `online_observation`'s recognized case: a page of a colour anchor
// and an OCR text, the text a personal field.
fn observation_package(root: &TempDir) -> String {
    let files: &[(&str, &[u8])] = &[
        (
            "control.json",
            br#"{"game":"neutral","server":"test","package_id":"neutral.observation","entry_task_id":"task"}"#,
        ),
        ("resources/manifest.json", br#"{"entry_task_id":"task"}"#),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{"schema_version":"0.6","coordinate_space":{"width":2,"height":1},"targets":[{"type":"color","id":"anchor","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},{"type":"ocr","id":"text","region":"full_frame","languages":["en"],"timeout_ms":1000,"match_mode":"exact","expected":["home"],"case_sensitive":false,"minimum_confidence":0.9,"model_ref":"PP-OCRv6_medium","model_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"type":"color","id":"later","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]}]}"#,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{"schema_version":"0.6","pages":[{"id":"page","required":["anchor","text"],"optional":["later"]}]}"#,
        ),
        (
            "resources/navigation/neutral.test.navigation.json",
            br#"{"navigation":[]}"#,
        ),
        (
            "resources/operations/task/task.json",
            br#"{"task_id":"task","post_admission_ocr":{"mode":"fields_v1","fields":[{"id":"name","target_id":"text","privacy":"personal"}]}}"#,
        ),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in files {
        zip.start_file(*name, FileOptions::default())
            .expect("package entry");
        zip.write_all(bytes).expect("package entry bytes");
    }
    let path = root.path().join("observation.zip");
    fs::write(&path, zip.finish().expect("package zip").into_inner()).expect("write package");
    path.display().to_string()
}

// `operation` as the Lab sends it.
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

fn observe(package: &str) -> RuntimeRequest {
    lab_request(RuntimeOperation::ObserveContainedPage {
        instance_alias: INSTANCE_A.to_owned(),
        request: ContainedObservationRequest::new(package, vec!["text".to_owned()])
            .expect("observation request"),
    })
}

// T2a: an online observation archives the PP-OCR report of its recognition, and a foreign write
// changes the staged report before its seal. The observation fails with its terminal recorded,
// carrying the mismatch and the counts; the receipt is not fatal and the host is not latched: the
// same observation is then observed.
#[test]
fn a36c_an_observation_whose_ocr_diagnostic_seal_mismatches_records_its_terminal() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(INSTANCE_A, instance_id(), Arc::new(FakeState::default()))
                .with_vision_provider(reporting_vision()),
        ),
    )
    .expect("runtime host");
    let package = observation_package(&root);
    let appended = append_before_next_ppocr_seal(&host, root.path());
    let request = observe(&package);

    let failed = send(&host, &request, 1373);

    let compared = one_foreign_append(&appended);
    assert_failed_by_the_mismatch(&failed, compared);
    let terminal = recorded_terminal(&host, &request, &failed);
    assert_the_mismatch(
        terminal
            .payload()
            .outcome()
            .expect("the terminal's failure"),
        compared,
    );
    assert!(host.fatal_error().expect("runtime health").is_none());

    let observed = send(&host, &observe(&package), 1374);
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Observed,
        "{observed:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

// One frame a closed Runtime under `root` captured, as a saved OCR names it.
fn saved_frame(root: &TempDir) -> SavedArtifactOcrSource {
    let host = RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(
            INSTANCE_A,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("source runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let observe = runtime_request(
        &ids,
        RuntimeOperation::ObserveReadonly {
            instance_alias: INSTANCE_A.to_owned(),
        },
    );
    let observed = send(&host, &observe, 1375);
    assert_eq!(
        observed.state(),
        RuntimeReceiptState::Completed,
        "{observed:?}"
    );
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(observe.correlation_id()),
            ..EventQuery::default()
        })
        .expect("the observation's events");
    let frame_event = |event_type: EventType| {
        events
            .iter()
            .find(|event| {
                event.event_type() == event_type
                    && (event_type == EventType::CaptureCompleted
                        || event
                            .artifacts()
                            .iter()
                            .any(|artifact| artifact.kind() == ArtifactKind::CaptureFrame))
            })
            .unwrap_or_else(|| panic!("the frame's {event_type:?}"))
    };
    let created = frame_event(EventType::ArtifactCreated);
    let verified = frame_event(EventType::ArtifactVerified);
    let captured = frame_event(EventType::CaptureCompleted);
    let artifact = created
        .artifacts()
        .iter()
        .find(|artifact| artifact.kind() == ArtifactKind::CaptureFrame)
        .expect("the captured frame")
        .project(true);
    let position = |event: &PersistedEvent| TerminalEvent {
        sequence: event.sequence(),
        event_id: *event.event_id(),
    };
    let source = SavedArtifactOcrSource {
        state_root: root.path().to_str().expect("UTF-8 state root").to_owned(),
        through_sequence: captured.sequence(),
        frame_id: artifact.frame_id.expect("the frame's identity"),
        artifact,
        created: position(created),
        verified: position(verified),
        captured: position(captured),
    };
    host.close().expect("close the source host");
    source
}

fn recognize(source: &SavedArtifactOcrSource, package: &Path) -> RuntimeRequest {
    lab_request(RuntimeOperation::RecognizeArtifact {
        request: Box::new(SavedArtifactOcrRequest {
            source: source.clone(),
            package_path: package.to_str().expect("UTF-8 package path").to_owned(),
            target_id: "fixture/ocr".to_owned(),
        }),
    })
}

// T2b: the OCR of a saved frame archives the PP-OCR report of its reading, and a foreign write
// changes the staged report before its seal. The request fails with its terminal recorded and
// the failure's record carrying the mismatch and the counts; the receipt is not fatal and the
// host is not latched: the same OCR then completes.
#[test]
fn a36c_a_saved_frame_ocr_whose_diagnostic_seal_mismatches_records_its_terminal() {
    let source_root = TempDir::new().expect("source tempdir");
    let source = saved_frame(&source_root);
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("saved-ocr.zip");
    fs::write(
        &package,
        neutral_post_admission_ocr_contained_task_package(false),
    )
    .expect("write package");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(INSTANCE_A, instance_id(), Arc::new(FakeState::default()))
                .with_vision_provider(reporting_vision()),
        ),
    )
    .expect("runtime host");
    let appended = append_before_next_ppocr_seal(&host, root.path());
    let request = recognize(&source, &package);

    let failed = send(&host, &request, 1376);

    let compared = one_foreign_append(&appended);
    assert_failed_by_the_mismatch(&failed, compared);
    let terminal = recorded_terminal(&host, &request, &failed);
    assert_eq!(terminal.event_type(), EventType::RecognitionFailed);
    let records = correlated_events(&host, &request);
    let failures = records
        .iter()
        .filter_map(|event| event.payload().outcome())
        .filter(|outcome| outcome.code().as_str() == StoreCode::ArtifactHashMismatch.as_str())
        .collect::<Vec<_>>();
    let [failure] = failures.as_slice() else {
        panic!("one record of the failure: {failures:#?}");
    };
    assert_the_mismatch(failure, compared);
    assert!(host.fatal_error().expect("runtime health").is_none());

    let recognized = send(&host, &recognize(&source, &package), 1377);
    assert_eq!(
        recognized.state(),
        RuntimeReceiptState::Completed,
        "{recognized:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
