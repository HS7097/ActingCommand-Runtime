// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 K2 (register §1.380; contracts/linear-steps.md, "Endings"), the host's half: a
// `linear_steps` package whose one step `buy` leads from `shop` to its target page `done`, and whose
// scheduling outcome names the ending page `short` beside it, runs directly through the host. After
// the tap the screen shows `short`: the kernel finishes the run Completed on it with no key of its
// own, and the host derives the terminal's scheduling disposition from the one mapping of that page,
// the ending's key. After the same tap the screen shows `done`: the usual ending keeps its own key.
// The host's rule is unchanged; this pins what it gives an ending. Fix round 1
// (`rd-1115/brief-1727-fix1.md`, item 1, the coordinator's ruling): an operation that arrives on an
// ending page confirms no resource adjustment, so the adjusted key's earlier record stands, while
// the ending page's own reading is published.
//
// The package is a `linear_steps` source package of game `neutral`, server `test`, 2 x 1: `shop`
// is a red first pixel (as the fake device shows it before any input), `done` a blue one and `short`
// a white one. The run is on the test's manual clock (test plan E-3: the kernel waits on it, a
// capture spends no time).

use super::instance_workers::all_events;
use super::*;
use serde_json::json;

const BOUGHT: &str = "task_bought";
const SHORT_BALANCE: &str = "task_short_balance";

// Writes the package as a source package directory at `directory`: one tap `buy` from `shop` to
// `done`, expected within 200 ms, no retry; the mappings of `done` and of the ending page `short`.
fn write_package(directory: &Path) {
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.k2.buy",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 50,
        "step_timeout_ms": 600,
        "timeout_ms": 8000,
        "max_steps": 1
    });
    let probe = |id: &str, expected: [u8; 3]| {
        json!({"id": id,
               "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
               "expected": expected})
    };
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "locale": "en-US",
        "goal": "K2 fixture",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
        "color_probes": [
            probe("first_red", [255, 0, 0]),
            probe("first_blue", [0, 0, 255]),
            probe("first_white", [255, 255, 255])
        ],
        "page_rules": {
            "shop": {"required": ["first_red"]},
            "done": {"required": ["first_blue"]},
            "short": {"required": ["first_white"]}
        },
        "entry_page": "shop",
        "target_page": "done",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": BOUGHT, "effect": "no_designated_effect", "terminal_pages": ["done"]},
            {"outcome_key": SHORT_BALANCE, "effect": "no_designated_effect",
             "terminal_pages": ["short"]}
        ]},
        "operations": [{
            "id": "buy",
            "from": "shop",
            "to": "done",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": "done", "timeout_ms": 200}
        }]
    });
    for (path, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(path);
        fs::create_dir_all(path.parent().expect("package file parent")).expect("package dir");
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("package JSON"),
        )
        .expect("write package file");
    }
}

// One direct run: its receipt, the host's ledger and the inputs the device took.
struct DirectRun {
    receipt: RuntimeReceipt,
    events: Vec<PersistedEvent>,
    inputs: usize,
}

// Runs the package directly on a device instance whose first pixel turns `after` once the tap is
// made.
fn run_with_screen_after_tap(after: [u8; 3]) -> DirectRun {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("k2-buy");
    write_package(&package);
    let state = Arc::new(FakeState::default());
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            if inputs == 0 {
                Vec::<(usize, [u8; 3])>::new()
            } else {
                vec![(0_usize, after)]
            }
        }));
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let run = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            POLICY_INSTANCE_ALIAS,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.to_string_lossy().into_owned())
                .expect("task request"),
        ),
    );
    let receipt = host
        .process_request_for_test(&run, ConnectionId::new(463).expect("connection"))
        .expect("run receipt");
    let events = all_events(&host);
    let inputs = state.input_count.load(Ordering::Acquire);
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close runtime host");
    DirectRun {
        receipt,
        events,
        inputs,
    }
}

// B.2, end to end: after the tap the screen shows the ending page `short`, then, in a second run,
// the target page `done`. Each run is Completed after its one step and one tap on the page shown,
// and its terminal's scheduling disposition carries that page's mapping's key, with no designated
// effect: the ending's key for `short`, the usual key for `done`.
#[test]
fn k2_the_host_keys_a_run_that_ends_on_an_ending_by_its_mapping() {
    for (case, after, page, key) in [
        (
            "the ending",
            [255_u8, 255, 255],
            "neutral/short",
            SHORT_BALANCE,
        ),
        ("the usual ending", [0_u8, 0, 255], "neutral/done", BOUGHT),
    ] {
        let DirectRun {
            receipt,
            events,
            inputs,
        } = run_with_screen_after_tap(after);

        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{case}: {:?}",
            receipt.outcome()
        );
        assert!(
            matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    final_page: Some(final_page),
                    executed_steps: 1,
                    ..
                }) if final_page == page
            ),
            "{case}: {:?}",
            receipt.result()
        );
        assert_eq!(inputs, 1, "{case}");
        let terminals = events
            .iter()
            .filter_map(|event| match event.payload() {
                EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
                    TaskSemanticFact::TerminalCommitted {
                        task_result,
                        final_page,
                        scheduling_disposition,
                        ..
                    } => Some((
                        *task_result,
                        final_page.as_deref(),
                        scheduling_disposition.as_ref(),
                    )),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        let [(task_result, final_page, disposition)] = terminals.as_slice() else {
            panic!("{case}: one terminal, got {}", terminals.len());
        };
        assert_eq!(*task_result, TaskOutcome::Success, "{case}");
        assert_eq!(*final_page, Some(page), "{case}");
        let disposition = disposition.expect("a scheduling disposition");
        assert_eq!(disposition.outcome_key(), key, "{case}");
        assert!(
            matches!(
                disposition.effect(),
                SchedulingEffectEvidence::NoDesignatedEffect
            ),
            "{case}: {:?}",
            disposition.effect()
        );
    }
}

// Fix round 1, item 1: the instance of the adjusted package's runs, the key `buy` adjusts and the
// key the ending page reads.
const ADJUSTED_ALIAS: &str = "neutral.instance";
const ADJUSTED_KEY: &str = "resource.k1";
const READ_KEY: &str = "resource.k2";

// Every OCR region reads "20" at confidence 0.99, with the execution evidence a reading and an
// adjustment's amount ask for.
#[derive(Debug, Default)]
struct ReadsTwenty;

impl VisionProvider for ReadsTwenty {
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
        _request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "readings request execution evidence",
        ))
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<actingcommand_recognition_pack::OcrProviderObservation, VisionProviderError> {
        use actingcommand_recognition_pack::{
            OcrExecutionProviderKind, OcrProviderExecutionEvidence, OcrProviderObservation,
            OcrProviderTextBlock,
        };
        let text = "20".to_owned();
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
                invocation_id: "k2-ocr".to_owned(),
                session_id: "k2-session".to_owned(),
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

// Writes the adjusted package as a zip at `path`, 2 x 1 as above: one tap `buy` from `shop` (red)
// to `done` (blue), expected within 1 000 ms, no retry; the mappings of `done` and of the ending page
// `short` (white). `buy` declares the adjustment `spent`, which consumes the amount read on its
// confirmation frame (`count/spent`) from `resource.k1`; the mark `stock` reads `resource.k2` on
// `short` (`count/stock`).
fn write_adjusted_package(path: &Path) {
    let space = json!({"width": 2, "height": 1});
    let control = json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.k2.adjusted",
        "execution_mode": "linear_steps",
        "game": "neutral",
        "server": "test",
        "resolution": space,
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 2_000,
        "timeout_ms": 20_000,
        "max_steps": 1
    });
    let task = json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": space,
        "entry_page": "shop",
        "target_page": "done",
        "max_steps": 1,
        "scheduling_outcome": {"mappings": [
            {"outcome_key": BOUGHT, "effect": "no_designated_effect", "terminal_pages": ["done"]},
            {"outcome_key": SHORT_BALANCE, "effect": "no_designated_effect",
             "terminal_pages": ["short"]}
        ]},
        "operations": [{
            "id": "buy",
            "from": "shop",
            "to": "done",
            "click": {"kind": "point", "x": 1, "y": 0},
            "unguarded_trusted_coordinate": true,
            "expect_after": {"page_id": "done", "timeout_ms": 1_000}
        }],
        "resource_readings": [{
            "id": "stock", "fact_key": READ_KEY, "page_id": "short",
            "target_id": "count/stock", "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
            "minimum_confidence_milli": 900, "valid_for_ms": 60000
        }],
        "resource_adjustments": [
            {"id": "spent", "fact_key": ADJUSTED_KEY, "operation_id": "buy",
             "amount": {"target_id": "count/spent", "direction": "consume"}}
        ]
    });
    let color = |id: &str, expected: [u8; 3]| {
        json!({"type": "color", "id": id,
               "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": expected})
    };
    let ocr = |id: &str, width: u32| {
        json!({"type": "ocr", "id": id, "region": {"x": 0, "y": 0, "width": width, "height": 1},
               "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
               "case_sensitive": false, "minimum_confidence": 0.9,
               "model_ref": "PP-OCRv6_medium", "model_sha256": "a".repeat(64)})
    };
    let pack = json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": space,
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            color("page/shop", [255, 0, 0]),
            color("page/done", [0, 0, 255]),
            color("page/short", [255, 255, 255]),
            ocr("count/spent", 1),
            ocr("count/stock", 2)
        ]
    });
    let page = |id: &str| {
        json!({"id": format!("neutral/{id}"), "required": [format!("page/{id}")],
               "optional": [], "forbidden": []})
    };
    let pages = json!({
        "schema_version": "0.3",
        "pages": [page("shop"), page("done"), page("short")]
    });
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, value) in [
        ("control.json", control),
        (
            "resources/manifest.json",
            json!({"schema_version": "0.3", "entry_task_id": "task"}),
        ),
        ("resources/operations/task/task.json", task),
        ("resources/recognition/neutral.test.pack.json", pack),
        ("resources/recognition/neutral.test.pages.json", pages),
    ] {
        zip.start_file(name, options).expect("package entry");
        zip.write_all(&serde_json::to_vec(&value).expect("package JSON"))
            .expect("package content");
    }
    fs::write(path, zip.finish().expect("finish package").into_inner()).expect("write the package");
}

// `resource.k1`'s record before a run: 100, observed a second ago, valid for an hour.
fn adjusted_key_before_the_run() -> FactRecord {
    let observed_at_unix_ms = unix_ms_now().expect("the clock") - 1_000;
    FactRecord {
        scope: FactScope::Instance {
            instance_id: ADJUSTED_ALIAS.to_owned(),
        },
        key: ADJUSTED_KEY.to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(100),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: Some(observed_at_unix_ms + 3_600_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 3_600_000,
            maximum_ms: 3_600_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 1_000,
        source_detector: "fixture.before".to_owned(),
        source_snapshot_id: format!("snapshot:before:{ADJUSTED_KEY}"),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "c".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// The instance's current record of `key`, if any.
fn record_of(host: &RuntimeHost, key: &str) -> Option<FactRecord> {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: ADJUSTED_ALIAS.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    })
    .expect("instance facts")
    .records
    .into_iter()
    .find(|record| record.key == key)
}

fn integer_of(record: &FactRecord) -> Option<i64> {
    match record.content {
        FactContent::Inline {
            value: ContractFactValue::Integer(value),
        } => Some(value),
        _ => None,
    }
}

// One run of the adjusted package: its receipt, the adjusted key's record before and after it and
// the read key's record after it.
struct AdjustedRun {
    receipt: RuntimeReceipt,
    adjusted_before: Option<FactRecord>,
    adjusted_after: Option<FactRecord>,
    read_after: Option<FactRecord>,
}

// Runs the adjusted package directly, on a fresh host whose fake device's first pixel turns
// `after` once the tap is made, with `resource.k1` published before the run.
fn run_adjusted(after: [u8; 3]) -> AdjustedRun {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("k2-adjusted.zip");
    write_adjusted_package(&package);
    let state = Arc::new(FakeState::default());
    *state.pixel_script.lock().expect("pixel script") =
        Some(Arc::new(move |_: usize, inputs: usize| {
            if inputs == 0 {
                Vec::<(usize, [u8; 3])>::new()
            } else {
                vec![(0_usize, after)]
            }
        }));
    // A direct run needs a device instance whose display is observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ADJUSTED_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(ReadsTwenty)),
        ),
    )
    .expect("runtime host");
    host.publish_fact(adjusted_key_before_the_run())
        .expect("publish resource.k1");
    let adjusted_before = record_of(&host, ADJUSTED_KEY);
    let mut client = TestClient::connect(&host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            ADJUSTED_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let receipt = client.send(&request);
    let adjusted_after = record_of(&host, ADJUSTED_KEY);
    let read_after = record_of(&host, READ_KEY);
    assert!(
        host.fatal_error().expect("runtime health").is_none(),
        "{:?}",
        host.fatal_error()
    );
    drop(client);
    host.close().expect("close the host");
    AdjustedRun {
        receipt,
        adjusted_before,
        adjusted_after,
        read_after,
    }
}

// Fix round 1, item 1 (the coordinator's ruling; contracts/linear-steps.md, "Endings"): `buy`, the
// last press, declares an adjustment with an amount, and `resource.k1` is 100 before each run.
// After the tap the screen shows the ending page `short`: the run is Completed on it, `buy` is
// confirmed on no frame, so `resource.k1` keeps its earlier record exactly, while the reading of
// `short` publishes `resource.k2` (20); the host's join check passes with no confirmation. After
// the same tap the screen shows the target page `done`: `buy` is confirmed on it as before, and
// `resource.k1` becomes 80 (100 less the 20 read), written by the adjustment, with no reading.
#[test]
fn k2_an_operation_that_arrives_on_an_ending_page_confirms_no_adjustment() {
    let ended = run_adjusted([255, 255, 255]);
    assert_eq!(
        ended.receipt.state(),
        RuntimeReceiptState::Completed,
        "the ending: {:?}",
        ended.receipt.outcome()
    );
    assert!(
        matches!(
            ended.receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(page),
                executed_steps: 1,
                ..
            }) if page == "neutral/short"
        ),
        "{:?}",
        ended.receipt.result()
    );
    let before = ended
        .adjusted_before
        .as_ref()
        .expect("resource.k1 before the run");
    assert_eq!(integer_of(before), Some(100));
    assert_eq!(
        ended.adjusted_after, ended.adjusted_before,
        "resource.k1 keeps its earlier record"
    );
    assert_eq!(ended.read_after.as_ref().and_then(integer_of), Some(20));

    let bought = run_adjusted([0, 0, 255]);
    assert_eq!(
        bought.receipt.state(),
        RuntimeReceiptState::Completed,
        "the usual ending: {:?}",
        bought.receipt.outcome()
    );
    assert!(
        matches!(
            bought.receipt.result(),
            Some(RuntimeResult::ContainedTaskCompleted {
                task_result: TaskOutcome::Success,
                final_page: Some(page),
                executed_steps: 1,
                ..
            }) if page == "neutral/done"
        ),
        "{:?}",
        bought.receipt.result()
    );
    let adjusted = bought
        .adjusted_after
        .as_ref()
        .expect("resource.k1 after the run");
    assert_eq!(integer_of(adjusted), Some(80));
    assert_eq!(adjusted.source_detector, "resource_adjustment:task/spent");
    assert_eq!(bought.read_after, None);
}
