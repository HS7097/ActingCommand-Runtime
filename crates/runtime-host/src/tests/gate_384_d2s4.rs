// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-CHAIN (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.2, §3.4, §3.5, §3.8;
// additions-v012 §8.2; host gate key 82).
//
// D2-CHAIN, D2-S4's merge gate (ADD §8.2): with a fake recognition maintenance package and fake
// facts, the five rings run end to end and each is visible in the ledger: an instance fact is
// unknown, the instance's recognition maintenance package is dispatched to read it, the reading
// is written to the instance fact store, the next round scores the task that reads it, and the
// task is dispatched.
//
// Every expected value comes from the model rows named above.

use super::*;
use actingcommand_contract::FactPayload;
use actingcommand_policy::{FACT_REFRESH_TASK_ID, PolicyEvaluation};

// The key the business task's trigger reads and the recognition package's terminal mark writes.
const READ_KEY: &str = "resource.k2";
const RECOGNITION_PACKAGE_ID: &str = "neutral.recognition.task";
const REQUESTED: &str = "fact_refresh_requested";
const PENDING: &str = "fact_refresh_pending";
const BINDING_INVALID: &str = "fact_reader_binding_invalid";
const BUSINESS_TASK: &str = "fixture.observe";

// Reads 35 on the terminal page (blue first pixel) and 120 elsewhere, at confidence 0.99.
#[derive(Debug, Default)]
struct TerminalReadings;

impl VisionProvider for TerminalReadings {
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
        let terminal = request.frame.rgb8_pixels.get(..3) == Some([0_u8, 0, 255].as_slice());
        let text = String::from(if terminal { "35" } else { "120" });
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
                invocation_id: "refresh-ocr".to_owned(),
                session_id: "refresh-session".to_owned(),
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

fn recognition_ocr_target(id: &str, region: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
        "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

fn recognition_mark(id: &str, page: &str, target: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page, "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    })
}

// The fake recognition maintenance package: a guarded tap from `home` to `terminal`, the home
// page's region read into `resource.k1` and the terminal page's whole page into `resource.k2`.
// With `designated` the tap is the package's designated operation, so the package is no reader.
fn recognition_package(designated: bool) -> Vec<u8> {
    let outcome = if designated {
        serde_json::json!({"designated_operation": "open_terminal", "mappings": [
            {"outcome_key": "read_done", "effect": "designated_effect_completed",
             "terminal_pages": ["terminal"]},
            {"outcome_key": "read_skipped", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]})
    } else {
        serde_json::json!({"mappings": [
            {"outcome_key": "read", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]})
    };
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": outcome,
        "operations": [{
            "id": "open_terminal",
            "from": "home",
            "to": "terminal",
            "click": {"kind": "point", "x": 1, "y": 0},
            "guard": {
                "page_id": "home",
                "target_id": "guard/ready",
                "expected_rect": {"x": 1, "y": 0, "width": 1, "height": 1},
                "color_probe": "guard/ready"
            },
            "retryable": false
        }],
        "resource_readings": [
            recognition_mark("k1", "home", "count/head"),
            recognition_mark("k2", "terminal", "count/page")
        ]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": RECOGNITION_PACKAGE_ID,
        "execution_mode": "navigable_route",
        "game": "neutral",
        "server": "test",
        "resolution": {"width": 2, "height": 1},
        "entry_task_id": "task",
        "capture_interval_ms": 1,
        "step_timeout_ms": 50,
        "timeout_ms": 5000,
        "max_steps": 2
    });
    let pack = serde_json::json!({
        "schema_version": "0.7",
        "game": "neutral",
        "server": "test",
        "coordinate_space": {"width": 2, "height": 1},
        "defaults": {"color_max_distance": 0.0},
        "targets": [
            {"type": "color", "id": "page/home",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [255, 0, 0]},
            {"type": "color", "id": "page/terminal",
             "region": {"x": 0, "y": 0, "width": 1, "height": 1}, "expected": [0, 0, 255]},
            {"type": "color", "id": "guard/ready",
             "region": {"x": 1, "y": 0, "width": 1, "height": 1}, "expected": [0, 255, 0]},
            recognition_ocr_target("count/head", serde_json::json!({"x": 1, "y": 0, "width": 1, "height": 1})),
            recognition_ocr_target("count/page", serde_json::json!({"x": 0, "y": 0, "width": 2, "height": 1}))
        ]
    });
    let pages = serde_json::json!({
        "schema_version": "0.3",
        "pages": [
            {"id": "neutral/home", "required": ["page/home"], "optional": [], "forbidden": []},
            {"id": "neutral/terminal", "required": ["page/terminal"], "optional": [],
             "forbidden": []}
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
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options)
            .expect("recognition package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("recognition package JSON"))
            .expect("recognition package content");
    }
    zip.finish()
        .expect("finish recognition package")
        .into_inner()
}

// The fixture catalog whose task is triggered by `resource.k2` below 50 in the instance's own
// scope, read within fifteen minutes. Its activity profile is open all day and paces dispatches
// one millisecond apart, so the wall clock this test runs on admits both dispatches.
fn chain_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("chain tasks JSON");
    tasks["tasks"][0]["trigger"] = serde_json::json!({
        "kind": "fact",
        "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
        "fact_key": READ_KEY,
        "comparison": "less_than",
        "value": {"type": "integer", "value": 50},
        "max_age_ms": 900000
    });
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("chain tasks bytes");
    let mut activity: serde_json::Value =
        serde_json::from_slice(&sources.activity.bytes).expect("chain activity JSON");
    activity["profiles"][0]["windows"] = serde_json::json!([{
        "weekdays": [1, 2, 3, 4, 5, 6, 7],
        "utc_offset_minutes": 0,
        "start_minute_of_day": 0,
        "end_minute_of_day": 0
    }]);
    activity["profiles"][0]["minimum_interval_ms"] = serde_json::json!(1);
    activity["profiles"][0]["maximum_interval_ms"] = serde_json::json!(1);
    sources.activity.bytes = serde_json::to_vec_pretty(&activity).expect("chain activity bytes");
    sources
}

// A host whose instance binds the recognition maintenance package at `package`.
fn chain_host(root: &TempDir, package: &Path, state: &Arc<FakeState>) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root).with_recognition_packages(BTreeMap::from([(
            POLICY_INSTANCE_ALIAS.to_owned(),
            package.to_path_buf(),
        )])),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(state))
                .fixture_simulation()
                .with_vision_provider(Arc::new(TerminalReadings)),
        ),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&chain_sources())
        .expect("activate the chain catalog");
    host
}

// One full policy cycle at the wall clock's current instant (the readings are observed on it).
fn evaluate_now(host: &RuntimeHost, seed: u64) -> PolicyEvaluation {
    let now = unix_ms_now().expect("wall clock");
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms: now,
            monotonic_ms: now,
        },
        seed,
        PolicyTrigger::Recovery,
    )
    .expect("policy cycle")
    .evaluation
    .expect("a full evaluation")
}

// The evaluation's intent of `task_id` with its reason chain.
fn intent_of(
    evaluation: &PolicyEvaluation,
    task_id: &str,
) -> (DispatchIntent, DecisionReasonChain) {
    let intent = evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.task_id == task_id)
        .unwrap_or_else(|| panic!("an intent of {task_id}: {evaluation:#?}"))
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("the intent's reason chain")
        .clone();
    (intent, chain)
}

// Admits the fact refresh intent with the request the Runtime builds for it.
fn admit_refresh(
    host: &RuntimeHost,
    intent: &DispatchIntent,
    chain: &DecisionReasonChain,
) -> (Box<PolicyRunContext>, ContainedTaskRequest) {
    let request = host
        .fact_refresh_request(intent)
        .expect("the fact refresh request")
        .expect("a request for the fact refresh intent");
    let admission = host
        .admit_scheduled_policy_dispatch(intent, chain, &policy_context(host, intent), &request)
        .expect("the fact refresh is admitted");
    let PolicyDispatchAdmission::Granted { context } = admission else {
        panic!("expected the fact refresh's run context")
    };
    (context, request)
}

// D2-CHAIN (ADD §8.2; D2M §3.4, §3.5, §3.8; T18, T19 end to end): the five rings, each in the
// ledger. (1) The task's trigger reads `resource.k2`, which the instance has never had, so the
// task waits (`fact_refresh_pending`) and (2) the recognition maintenance package is dispatched
// with `fact_refresh_requested(fact_key=resource.k2, unknown_reason=fact_missing)` in its reason
// chain. Its run reads only the requested key: (3) `fact.published` writes resource.k2 = 35 from
// the frame it was read on. (4) The next round scores the task with the key known and (5) the
// task is dispatched after the reading.
#[test]
fn d2s4_chain_an_unknown_fact_is_read_written_rescored_and_dispatched() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("recognition.zip");
    fs::write(&package, recognition_package(false)).expect("write the recognition package");
    let state = Arc::new(FakeState::default());
    // The package starts on its home page; after its tap the device shows the terminal page.
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let host = chain_host(&root, &package, &state);

    let first = evaluate_now(&host, 31);
    let waiting = first
        .decisions
        .iter()
        .find(|decision| decision.task_id == BUSINESS_TASK)
        .expect("the task's decision");
    assert!(
        !first
            .dispatch_intents
            .iter()
            .any(|intent| intent.task_id == BUSINESS_TASK),
        "the task is not dispatched on an unknown fact"
    );
    let pending = waiting
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == PENDING)
        .unwrap_or_else(|| panic!("the task waits for the reader: {:?}", waiting.reasons));
    assert_eq!(
        pending.outcome.value("fact_key"),
        Some(&serde_json::json!(READ_KEY))
    );
    assert_eq!(
        pending.outcome.value("package_id"),
        Some(&serde_json::json!(RECOGNITION_PACKAGE_ID))
    );
    let (refresh, refresh_chain) = intent_of(&first, FACT_REFRESH_TASK_ID);
    record_policy_approval(&host, &refresh);
    let (context, request) = admit_refresh(&host, &refresh, &refresh_chain);
    let receipt = host
        .run_scheduled_contained_task(&context, &request)
        .expect("the recognition maintenance package runs");
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    host.complete_scheduled_policy_run(&context, &receipt)
        .expect("the fact refresh settles");

    let second = evaluate_now(&host, 32);
    let (business, business_chain) = intent_of(&second, BUSINESS_TASK);
    assert!(
        !second
            .dispatch_intents
            .iter()
            .any(|intent| intent.task_id == FACT_REFRESH_TASK_ID),
        "nothing requests the reader once the key is known"
    );
    assert!(
        business_chain.reasons.iter().all(|reason| !reason
            .outcome
            .code()
            .as_str()
            .starts_with("fact_refresh_")),
        "{:?}",
        business_chain.reasons
    );
    let admission = host
        .admit_policy_dispatch(
            &business,
            &business_chain,
            &policy_context(&host, &business),
        )
        .expect("the task is admitted");
    assert!(matches!(admission, PolicyDispatchAdmission::Granted { .. }));

    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    let intents = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => {
                Some((event.sequence(), payload))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    // Rings 1 and 2: the reader's dispatch, requested for the unknown key.
    let (refresh_at, refresh_record) = intents
        .iter()
        .find(|(_, payload)| payload.task_id() == FACT_REFRESH_TASK_ID)
        .expect("the reader's dispatch intent in the ledger");
    let requested = refresh_record
        .reasons()
        .iter()
        .filter(|record| record.0.code().as_str() == REQUESTED)
        .collect::<Vec<_>>();
    let [requested] = requested.as_slice() else {
        panic!("one requested key: {:?}", refresh_record.reasons());
    };
    assert_eq!(
        requested.0.value("fact_key"),
        Some(&serde_json::json!(READ_KEY))
    );
    assert_eq!(
        requested.0.value("unknown_reason"),
        Some(&serde_json::json!("fact_missing"))
    );
    assert_eq!(
        requested.0.value("instance_id"),
        Some(&serde_json::json!(POLICY_INSTANCE_ALIAS))
    );
    // Ring 3: the reading, written by the reader's run after its dispatch; the key not requested
    // is not read.
    let readings = events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Fact(FactPayload::Published(payload))
                if payload.record().key.starts_with("resource.k") =>
            {
                Some((event.sequence(), payload.record()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let [(read_at, reading)] = readings.as_slice() else {
        panic!("one reading, of the requested key: {readings:?}");
    };
    assert_eq!(reading.key, READ_KEY);
    assert_eq!(
        reading.content,
        FactContent::Inline {
            value: ContractFactValue::Integer(35)
        }
    );
    assert!(reading.source_snapshot_id.starts_with("run:"));
    assert!(
        read_at > refresh_at,
        "the reading follows the reader's dispatch"
    );
    // Rings 4 and 5: the task's dispatch, scored after the reading, and its admission.
    let (business_at, _) = intents
        .iter()
        .find(|(_, payload)| payload.task_id() == BUSINESS_TASK)
        .expect("the task's dispatch intent in the ledger");
    assert!(
        business_at > read_at,
        "the task is dispatched after the reading"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Policy(PolicyPayload::DispatchAdmitted(payload))
                if payload.task_id() == BUSINESS_TASK
        )),
        "the task's dispatch is admitted"
    );
    host.close().expect("close the host");
}
