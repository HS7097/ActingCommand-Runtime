// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 B1 (slice BT-B1; B-train plan `rd-1021/train-0129/b-plan/plan.md` §2.1 items 2
// and 3, test rows T2 and T3 of §5.3; register §1.153 LR-5 and LR-6, §1.163): a value that the
// frames of an agreement do not read is no value. End to end through the Runtime: a value test
// whose side declares an agreement of three frames reads 100, 100 and 180, so its page does not
// match, and the run's task diagnostic stream carries the side's `no_agreement` (LR-5) and one
// `agreement_sample` row per frame (LR-6); a reading mark whose three frames read two different
// values writes no fact and records the warning with `no_agreement`. The scripted OCR provider
// answers its calls in turn from a fixed list; the frames are the fake device's neutral frames.

use super::*;

const AGREEMENT_ALIAS: &str = "neutral.instance";
const UNRESOLVED: &str = "contained_task_resource_reading_unresolved";

// Each OCR call reads the next text of `texts`, round after round, at confidence 0.99.
#[derive(Debug)]
struct TextsInTurn {
    texts: &'static [&'static str],
    calls: AtomicUsize,
}

impl TextsInTurn {
    fn new(texts: &'static [&'static str]) -> Arc<Self> {
        Arc::new(Self {
            texts,
            calls: AtomicUsize::new(0),
        })
    }
}

impl VisionProvider for TextsInTurn {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.read_text_with_execution_evidence(request)
            .map(|observation| observation.result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<actingcommand_recognition_pack::OcrProviderObservation, VisionProviderError> {
        use actingcommand_recognition_pack::{
            OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
            OcrProviderTextBlock,
        };
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let text = self.texts[call % self.texts.len()].to_owned();
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(0.99),
                }],
                confidence: Some(0.99),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("agreement-ocr-{call}"),
                session_id: "agreement-session".to_owned(),
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
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
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
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// A guarded tap from `page_home` to `page_a`, the target page. With `value_test`, `page_a` also
// requires the value test `check/count`, whose left side reads `ocr/count` on three frames that
// must all read one value (`parses` holds once the side reads); `readings`, when given, are the
// task's reading marks.
fn agreement_package(value_test: bool, readings: Option<serde_json::Value>) -> Vec<u8> {
    let mut task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "page_home",
        "target_page": "page_a",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "done", "effect": "no_designated_effect",
             "terminal_pages": ["page_a"]}
        ]},
        "operations": [{
            "id": "op_a",
            "from": "page_home",
            "to": "page_a",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "page_home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }]
    });
    if let Some(readings) = readings {
        task["resource_readings"] = readings;
    }
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.agreement.task",
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 1000,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    let mut pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/a",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            {"type": "ocr", "id": "ocr/count",
             "region": {"x": 0, "y": 0, "width": 2, "height": 1}, "languages": ["en"],
             "timeout_ms": 10, "match_mode": "exact", "expected": ["0"],
             "case_sensitive": false, "minimum_confidence": 0.5,
             "model_ref": "PP-OCRv6_medium",
             "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        ]
    });
    let mut page_a = vec!["page/a"];
    if value_test {
        pack["targets"]
            .as_array_mut()
            .expect("the pack's targets")
            .push(serde_json::json!({
                "type": "value_test", "id": "check/count",
                "left": {"target_id": "ocr/count", "trim": "whitespace_v1",
                         "value": {"type": "unsigned_integer", "min": 0, "max": 999},
                         "agreement": {"frames": 3, "interval_ms": 1}},
                "op": "parses"
            }));
        page_a.push("check/count");
    }
    let pages = serde_json::json!({
        "schema_version": "0.6",
        "pages": [
            {"id": "neutral/page_home", "required": ["page/home"], "optional": [],
             "forbidden": []},
            {"id": "neutral/page_a", "required": page_a, "optional": [], "forbidden": []}
        ]
    });
    let files = [
        ("control.json", control),
        (
            "resources/manifest.json",
            serde_json::json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options)
            .expect("agreement package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("agreement package JSON"))
            .expect("agreement package content");
    }
    zip.finish().expect("finish agreement package").into_inner()
}

// A Runtime whose one instance shows `page_home` until its first input and `page_a` after it,
// and whose OCR provider reads `texts` in turn.
fn agreement_host(root: &TempDir, texts: &'static [&'static str]) -> RuntimeHost {
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    RuntimeHost::start(
        config(root),
        Arc::new(
            FakeProvider::one(AGREEMENT_ALIAS, instance_id(), state)
                .with_vision_provider(TextsInTurn::new(texts)),
        ),
    )
    .expect("runtime host")
}

// One manual run of `package`; its receipt and correlation.
fn run_package(
    host: &RuntimeHost,
    package: &Path,
) -> (RuntimeReceipt, actingcommand_contract::CorrelationId) {
    let mut client = TestClient::connect(host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            AGREEMENT_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string())
                .expect("agreement task request"),
        ),
    );
    (client.send(&request), correlation_id)
}

// Every row of the task diagnostic stream of the run `correlation_id`.
fn stream_rows(
    host: &RuntimeHost,
    root: &Path,
    correlation_id: actingcommand_contract::CorrelationId,
) -> Vec<serde_json::Value> {
    let mut client = TestClient::connect(host);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let stream = events
        .iter()
        .filter(|event| event.event_type == EventType::ArtifactVerified)
        .flat_map(|event| &event.artifacts)
        .find(|artifact| {
            artifact.kind == ArtifactKind::DiagnosticJson
                && artifact.redaction_state
                    == actingcommand_contract::ArtifactRedactionState::Pending
        })
        .expect("the run's task diagnostic stream");
    let document: serde_json::Value = serde_json::from_slice(
        &read_projected_verified(root, stream).expect("verified stream bytes"),
    )
    .expect("task diagnostic JSON");
    document["records"]
        .as_array()
        .expect("diagnostic records")
        .clone()
}

// T2 (plan §5.3 BT-B1 T2; §2.1 items 2 and 3; Q6): the value test's agreement of three frames, k
// three by default, reads 100, 100 and 180: its side is unread with `no_agreement`, so `page_a`,
// whose color passed, does not match and the step after the tap is never confirmed. The stream
// carries the side's `problem` `no_agreement` exactly as LR-5 registers it, and each frame's
// evaluation as a row of the guard phase `agreement_sample` (LR-6), the frames in order.
#[test]
fn bt_b1_t2_frames_that_do_not_agree_leave_the_side_unread_and_the_page_unmatched() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("agreement.zip");
    fs::write(&package, agreement_package(true, None)).expect("write the agreement package");
    let host = agreement_host(&root, &["100", "100", "180"]);

    let (receipt, correlation_id) = run_package(&host, &package);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "page_a never matches: {:?}",
        receipt.outcome()
    );
    let rows = stream_rows(&host, root.path(), correlation_id);
    let of_test =
        |row: &serde_json::Value| row["kind"] == "target" && row["data"]["id"] == "check/count";
    let sample = |row: &serde_json::Value| {
        of_test(row)
            && row["data"]["source"]["phase"]
                == actingcommand_contract::TASK_DIAGNOSTIC_PHASE_AGREEMENT_SAMPLE
    };
    // A page's evaluation of the test (a page-target row), not one it skipped.
    let evaluation = |row: &serde_json::Value| {
        of_test(row)
            && row["data"]["source"].get("role").is_some()
            && row["data"].get("skipped").is_none()
    };
    // LR-5: each evaluation of the test by `page_a` carries the side's `no_agreement`, with no
    // value and no text, and its failure names it.
    let evaluated = rows
        .iter()
        .filter(|row| evaluation(row))
        .collect::<Vec<_>>();
    assert!(!evaluated.is_empty(), "page_a's frames evaluated the test");
    for row in &evaluated {
        assert_eq!(row["data"]["kind"], "value_test");
        assert_eq!(row["data"]["passed"], false);
        assert_eq!(
            row["data"]["value_test"]["left"],
            serde_json::json!({"target_id": "ocr/count", "problem": "no_agreement"})
        );
        assert_eq!(row["data"]["failure"]["code"], "value_test_unresolved");
        assert_eq!(
            row["data"]["failure"]["values"]["value_problem"],
            "no_agreement"
        );
    }
    // LR-6: the first evaluation follows one `agreement_sample` row per frame, oldest first,
    // each with its own read.
    let first = rows
        .iter()
        .position(evaluation)
        .expect("the first evaluation");
    let samples = rows[..first]
        .iter()
        .filter(|row| sample(row))
        .collect::<Vec<_>>();
    assert_eq!(samples.len(), 3, "one row per frame: {samples:?}");
    for (row, (text, value)) in samples
        .iter()
        .zip([("100", 100), ("100", 100), ("180", 180)])
    {
        assert_eq!(
            row["data"]["source"],
            serde_json::json!({"phase": "agreement_sample"})
        );
        assert_eq!(row["data"]["kind"], "value_test");
        assert_eq!(row["data"]["passed"], true);
        assert_eq!(
            row["data"]["value_test"]["left"],
            serde_json::json!({"target_id": "ocr/count", "raw_text": text, "value": value})
        );
    }
    // `page_a`'s color passed on those frames, and the page never matched.
    let page_a = rows
        .iter()
        .filter(|row| row["kind"] == "page" && row["data"]["page_id"] == "neutral/page_a")
        .collect::<Vec<_>>();
    assert!(page_a.iter().all(|row| row["data"]["matched"] != true));
    assert!(
        page_a.iter().any(|row| {
            row["data"]["required_total"] == 2 && row["data"]["required_passed"] == 1
        })
    );
    host.close().expect("close the host");
}

// T3 (plan §5.3 BT-B1 T3; §2.1 items 2 and 3): a reading mark on `page_a` whose agreement of three
// frames reads 35 on two of them and 36 on the third writes no fact: the run completes, the key
// has no record, and the mark's miss is the warning `contained_task_resource_reading_unresolved`
// with `reading_problem` `no_agreement`.
#[test]
fn bt_b1_t3_a_mark_whose_frames_do_not_agree_writes_no_fact_and_warns() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("reading.zip");
    let readings = serde_json::json!([{
        "id": "count", "fact_key": "resource.count", "page_id": "page_a",
        "target_id": "ocr/count", "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 999},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000,
        "agreement": {"frames": 3, "interval_ms": 1}
    }]);
    fs::write(&package, agreement_package(false, Some(readings)))
        .expect("write the reading package");
    let host = agreement_host(&root, &["35", "35", "36"]);

    let (receipt, correlation_id) = run_package(&host, &package);

    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "a mark's miss never fails the run: {:?}",
        receipt.outcome()
    );
    let record = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: AGREEMENT_ALIAS.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        })
        .expect("instance facts")
        .records
        .into_iter()
        .find(|record| record.key == "resource.count");
    assert_eq!(record, None, "no fact, no default, no last value");
    let misses = stream_rows(&host, root.path(), correlation_id)
        .into_iter()
        .filter(|row| {
            row["kind"] == "error" && row["data"]["error"]["outcome"]["code"] == UNRESOLVED
        })
        .collect::<Vec<_>>();
    assert_eq!(misses.len(), 1, "one miss: {misses:?}");
    let outcome = &misses[0]["data"]["error"]["outcome"];
    assert_eq!(outcome["category"], "warning");
    assert_eq!(outcome["values"]["reading_id"], "count");
    assert_eq!(outcome["values"]["fact_key"], "resource.count");
    assert_eq!(outcome["values"]["reading_problem"], "no_agreement");
    host.close().expect("close the host");
}

// BT-B1-FIX (host key 342): the package's task timeout (`timeout_ms` of `agreement_package`).
const FIX_TASK_TIMEOUT_MS: u64 = 5_000;
// How far the Runtime clock is set ahead of the process clock before the run, so the recognition
// pack's own deadline checks, which read the process clock, never decide.
const FIX_AHEAD_OF_PROCESS_CLOCK_MS: u64 = 60_000;
// Where the provider's one move leaves the Runtime clock: this long before the task's deadline,
// far past the confirmation window's end and with time left for the wait to observe again.
const FIX_BEFORE_TASK_DEADLINE_MS: u64 = 100;

// Reads `texts` in turn; its first read after the instance's first input first moves the Runtime
// clock to `to_ms`, once.
struct ClockMovedAtFirstReadAfterInput {
    texts: Arc<TextsInTurn>,
    state: Arc<FakeState>,
    clock: Arc<ManualRuntimeClock>,
    to_ms: AtomicU64,
    moved: AtomicBool,
}

impl std::fmt::Debug for ClockMovedAtFirstReadAfterInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClockMovedAtFirstReadAfterInput")
            .field("texts", &self.texts)
            .field("to_ms", &self.to_ms)
            .field("moved", &self.moved)
            .finish_non_exhaustive()
    }
}

impl ClockMovedAtFirstReadAfterInput {
    fn move_clock_once(&self) {
        if self.state.input_count.load(Ordering::Acquire) >= 1
            && !self.moved.swap(true, Ordering::AcqRel)
        {
            let to_ms = self.to_ms.load(Ordering::Acquire);
            self.clock
                .advance(to_ms.saturating_sub(self.clock.monotonic_ms()));
        }
    }
}

impl VisionProvider for ClockMovedAtFirstReadAfterInput {
    fn require_ocr_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        self.texts.require_ocr_model(model_ref, model_sha256)
    }

    fn require_nn_model(
        &self,
        model_ref: &str,
        model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        self.texts.require_nn_model(model_ref, model_sha256)
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.move_clock_once();
        self.texts.read_text(request)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<actingcommand_recognition_pack::OcrProviderObservation, VisionProviderError> {
        self.move_clock_once();
        self.texts.read_text_with_execution_evidence(request)
    }

    fn classify(
        &self,
        request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        self.texts.classify(request)
    }
}

// BT-B1-FIX (host key 342; #411 B1, B-train plan §2.1 items 2 and 3; Workflow #386 SR-1,
// "Observe-only retries" in business-identity-consensus.md): T2's package, whose value test's
// agreement makes every observation of its pack a sampled one, so the step's confirmation wait for
// `page_a` (which never matches: its frames read 100, 100 and 180) observes in windows. The
// Runtime clock passes the window's end while the first observation after the tap is being
// recognised, so that observation's sampling timed out and the wait observes again in a new window
// without any input. The Runtime latches no fatal for it (left open, the timed-out recognition made
// the retry's start latch `contained_task_recognition_state_invalid`), the run fails as `page_a`
// never matches, and after the tap the run started recognising at least twice: the timed-out
// observation and its retry. Only the kernel's waits and the provider's one move advance the
// Runtime clock.
#[test]
fn bt_b1_fix_a_recognition_its_sampling_deadline_passed_latches_no_fatal_and_is_observed_again() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("agreement.zip");
    fs::write(&package, agreement_package(true, None)).expect("write the agreement package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let clock = Arc::new(ManualRuntimeClock::new(1_000, 0));
    let vision = Arc::new(ClockMovedAtFirstReadAfterInput {
        texts: TextsInTurn::new(&["100", "100", "180"]),
        state: Arc::clone(&state),
        clock: Arc::clone(&clock),
        to_ms: AtomicU64::new(u64::MAX),
        moved: AtomicBool::new(false),
    });
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(
            FakeProvider::one(AGREEMENT_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(vision.clone()),
        ),
    )
    .expect("runtime host");
    // The run's steps are unpaced, whatever load the machine is under.
    host.pin_pacing_factor_for_test(AGREEMENT_ALIAS, 1_000)
        .expect("pin the pacing factor");
    clock.advance(FIX_AHEAD_OF_PROCESS_CLOCK_MS);
    vision.to_ms.store(
        clock.monotonic_ms() + FIX_TASK_TIMEOUT_MS - FIX_BEFORE_TASK_DEADLINE_MS,
        Ordering::Release,
    );

    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            AGREEMENT_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string())
                .expect("agreement task request"),
        ),
    );
    let receipt = client.send_result(&request);
    drop(client);

    assert!(
        vision.moved.load(Ordering::Acquire),
        "the clock passed the window's end during a recognition after the tap: {receipt:?}"
    );
    let latched = host
        .fatal_error()
        .expect("runtime health")
        .map(|fatal| fatal.code());
    assert_eq!(latched, None, "the Runtime latched no fatal: {receipt:?}");
    let receipt = receipt.expect("the run's receipt");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Failed,
        "page_a never matches: {:?}",
        receipt.outcome()
    );
    let events = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        })
        .expect("run events");
    let facts = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Task(TaskPayload::Semantic(payload)) => Some(payload.fact()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let tap = facts
        .iter()
        .position(|fact| matches!(fact, TaskSemanticFact::EffectCompleted { .. }))
        .expect("the tap");
    let recognitions_after_tap = facts[tap..]
        .iter()
        .filter(|fact| matches!(fact, TaskSemanticFact::RecognitionStarted { .. }))
        .count();
    assert!(
        recognitions_after_tap >= 2,
        "the timed-out observation and its retry: {recognitions_after_tap}"
    );
    host.close().expect("close the host");
}
