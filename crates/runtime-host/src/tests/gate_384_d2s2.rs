// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §8, the host halves of rows
// T9, T10, T12, T14 and T34): a binding to a package that is no reader is refused with its
// reader problem; readings are published on the frames they were taken on, and a mark that
// gives no reading leaves the key's earlier record and is a warning row of the task diagnostic;
// a mark whose key the configuration claims is refused, and a start runs without it; a realistic
// fact no reader marks is a warning. Every expected value comes from the model rows named on
// each test.

use super::*;

const READING_ALIAS: &str = "neutral.instance";
const READING_PACKAGE_ID: &str = "neutral.reading.task";
const UNRESOLVED: &str = "contained_task_resource_reading_unresolved";

// Reads 120 on the home page and 35 on the terminal page, at confidence 0.99, or 0.5 on the
// terminal page while `terminal_low` is set.
#[derive(Debug, Default)]
struct ReadingNumbers {
    terminal_low: AtomicBool,
}

impl VisionProvider for ReadingNumbers {
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
        let confidence = if terminal && self.terminal_low.load(Ordering::Acquire) {
            0.5
        } else {
            0.99
        };
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence: Some(confidence),
                }],
                confidence: Some(confidence),
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: "reading-ocr".to_owned(),
                session_id: "reading-session".to_owned(),
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

fn reading_ocr_target(id: &str, region: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "ocr", "id": id, "region": region, "languages": ["en"], "timeout_ms": 1000,
        "match_mode": "exact", "expected": ["0"], "case_sensitive": false,
        "minimum_confidence": 0.0, "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

fn reading_mark(id: &str, page: &str, target: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "fact_key": format!("resource.{id}"), "page_id": page, "target_id": target,
        "trim": "whitespace_v1",
        "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
        "minimum_confidence_milli": 900, "valid_for_ms": 60000
    })
}

// A guarded tap from `home` to `terminal`; with `marks`, the home page's region `k1` and the
// terminal page's whole page `k2`; with `designated`, the tap is the designated operation.
fn reading_package(designated: bool, marks: bool) -> Vec<u8> {
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
    let mut task = serde_json::json!({
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
        }]
    });
    if marks {
        task["resource_readings"] = serde_json::json!([
            reading_mark("k1", "home", "count/head"),
            reading_mark("k2", "terminal", "count/page")
        ]);
    }
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": READING_PACKAGE_ID,
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
            reading_ocr_target("count/head", serde_json::json!({"x": 1, "y": 0, "width": 1, "height": 1})),
            reading_ocr_target("count/page", serde_json::json!({"x": 0, "y": 0, "width": 2, "height": 1}))
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
            .expect("reading package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("reading package JSON"))
            .expect("reading package content");
    }
    zip.finish().expect("finish reading package").into_inner()
}

fn reading_descriptor(
    designated: bool,
    marks: bool,
) -> actingcommand_execution_kernel::TaskPackageDescriptor {
    let bytes = reading_package(designated, marks);
    let recorded = actingcommand_contract::PackageRef::LegacyZipSha256(
        actingcommand_pack_containment::Sha256Hash::digest(&bytes).to_string(),
    );
    PreparedContainedTask::load_with_vision_provider(
        READING_ALIAS,
        &bytes,
        &recorded,
        Arc::new(ReadingNumbers::default()),
    )
    .expect("the reading package is admitted")
    .package_descriptor()
}

fn reader_problem(outcome: &actingcommand_contract::outcome::Outcome) -> Option<&str> {
    outcome
        .value("reader_problem")
        .and_then(serde_json::Value::as_str)
}

// T9 (D2M:589; §3.10), the code: a binding to a package with a designated operation is
// `fact_reader_binding_invalid(reader_problem=designated_operation)`, to one without marks
// `no_marks`; a package that taps into a page and reads it is a reader.
#[test]
fn d2s2_t9_a_binding_to_a_package_that_is_no_reader_is_invalid() {
    let path = Path::new("packages").join("reader");
    for (designated, marks, problem) in [
        (true, true, "designated_operation"),
        (false, false, "no_marks"),
    ] {
        let refused = recognition_binding_qualification(
            "node.a",
            &path,
            &reading_descriptor(designated, marks),
        )
        .expect_err("the package is no reader");
        assert_eq!(
            refused.code().as_str(),
            HostCode::FactReaderBindingInvalid.as_str()
        );
        assert_eq!(
            refused.category(),
            actingcommand_contract::outcome::Category::Error
        );
        assert_eq!(reader_problem(&refused), Some(problem));
        assert_eq!(
            refused.value("package_id"),
            Some(&serde_json::json!(READING_PACKAGE_ID))
        );
        assert_eq!(
            refused.value("instance_id"),
            Some(&serde_json::json!("node.a"))
        );
    }
    recognition_binding_qualification("node.a", &path, &reading_descriptor(false, true))
        .expect("a package that navigates and reads is a reader");
}

// One manual run of the reading package; its receipt and correlation.
fn run_reading_package(
    host: &RuntimeHost,
    package: &Path,
    bytes: &[u8],
) -> (RuntimeReceipt, actingcommand_contract::CorrelationId) {
    let mut client = TestClient::connect(host);
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            READING_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(
                package.display().to_string(),
                actingcommand_pack_containment::Sha256Hash::digest(bytes).to_string(),
            )
            .expect("reading task request"),
        ),
    );
    (client.send(&request), correlation_id)
}

fn reading_record(host: &RuntimeHost, key: &str) -> FactRecord {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: READING_ALIAS.to_owned(),
        server_id: "test".to_owned(),
        game_id: "neutral".to_owned(),
    })
    .expect("instance facts")
    .records
    .into_iter()
    .find(|record| record.key == key)
    .unwrap_or_else(|| panic!("no record of {key}"))
}

// The run and the frame a reading's `source_snapshot_id` (`run:<run>/frame:<frame>/<id>`) names.
fn snapshot_run_and_frame(record: &FactRecord) -> (String, String) {
    let mut parts = record.source_snapshot_id.split('/');
    let run = parts.next().expect("run part").to_owned();
    let frame = parts.next().expect("frame part").to_owned();
    assert!(
        run.starts_with("run:") && frame.starts_with("frame:"),
        "{}",
        record.source_snapshot_id
    );
    (run, frame)
}

// T10 (D2M:590) and T12 (D2M:592; §3.5; Q-D3), the host: a run publishes the home page's region
// reading and the terminal page's whole-page reading, each observed when its own frame was
// captured. A second run whose terminal mark reads below its floor succeeds; the key's earlier
// record stays as it was, and the miss is one warning row of the task diagnostic.
#[test]
fn d2s2_t10_t12_readings_publish_on_their_frames_and_a_miss_keeps_the_record() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("reading.zip");
    let bytes = reading_package(false, true);
    fs::write(&package, &bytes).expect("write reading package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance, and a device's first frame passes the task geometry
    // only with its display observed (Workflow #392 DA-A): the fake device observes a display of
    // the package's 2x1 resolution and serves frames of that size.
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let provider = Arc::new(ReadingNumbers::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(READING_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(provider.clone()),
        ),
    )
    .expect("runtime host");

    let (receipt, _) = run_reading_package(&host, &package, &bytes);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the first run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    let k1 = reading_record(&host, "resource.k1");
    let k2 = reading_record(&host, "resource.k2");
    assert_eq!(
        k1.content,
        FactContent::Inline {
            value: ContractFactValue::Integer(120)
        }
    );
    assert_eq!(
        k2.content,
        FactContent::Inline {
            value: ContractFactValue::Integer(35)
        }
    );
    let (k1_run, k1_frame) = snapshot_run_and_frame(&k1);
    let (k2_run, k2_frame) = snapshot_run_and_frame(&k2);
    assert_eq!(k1_run, k2_run);
    assert_ne!(k1_frame, k2_frame, "each reading names its own frame");
    assert!(k1.observed_at_unix_ms <= k2.observed_at_unix_ms);
    for record in [&k1, &k2] {
        assert_eq!(
            record.expires_at_unix_ms,
            Some(record.observed_at_unix_ms + 60_000)
        );
    }

    // The second run starts on the home page again and reads the terminal page below its floor.
    state
        .transition_capture_after_inputs
        .store(2, Ordering::Release);
    provider.terminal_low.store(true, Ordering::Release);
    let (receipt, correlation_id) = run_reading_package(&host, &package, &bytes);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the second run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    assert_eq!(reading_record(&host, "resource.k2"), k2);
    let (k1_rerun, _) = snapshot_run_and_frame(&reading_record(&host, "resource.k1"));
    assert_ne!(k1_rerun, k1_run, "the home page is read again");

    let mut client = TestClient::connect(&host);
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
        &read_projected_verified(root.path(), stream).expect("verified stream bytes"),
    )
    .expect("task diagnostic JSON");
    let misses = document["records"]
        .as_array()
        .expect("diagnostic records")
        .iter()
        .filter(|record| {
            record["kind"] == "error" && record["data"]["error"]["outcome"]["code"] == UNRESOLVED
        })
        .collect::<Vec<_>>();
    let [miss] = misses.as_slice() else {
        panic!("exactly one miss row: {misses:?}");
    };
    let outcome = &miss["data"]["error"]["outcome"];
    assert_eq!(outcome["category"], "warning");
    assert_eq!(outcome["values"]["reading_id"], "k2");
    assert_eq!(outcome["values"]["fact_key"], "resource.k2");
    assert_eq!(outcome["values"]["reading_problem"], "low_confidence");
    assert_eq!(miss["data"]["target_id"], "count/page");
    drop(client);
    host.close().expect("close the host");
}

// A catalog whose pool observes `resource.primary` in `fixture-instance-a`'s scope through the
// ledger (a `ledger_fact` pool).
fn live_pool_catalog() -> actingcommand_policy::CompiledCatalog {
    let mut sources = policy_sources(1);
    // As the live fact pool fixture writes it (facts.rs): the task is triggered by the pool's
    // projection, the pool by the ledger.
    let mut tasks: serde_json::Value =
        serde_json::from_slice(&sources.tasks.bytes).expect("fixture tasks JSON");
    tasks["tasks"][0]["trigger"] = serde_json::json!({"kind": "resource_projection",
        "pool_id": "fixture-pool-a", "comparison": "greater_than_or_equal", "value": 11});
    sources.tasks.bytes = serde_json::to_vec_pretty(&tasks).expect("fixture tasks bytes");
    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("fixture pools JSON");
    pools["pools"][0]["value_source"] =
        serde_json::json!({"kind": "ledger_fact", "minimum_confidence_milli": 900});
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("fixture pools bytes");
    compile_catalog(&sources).expect("the live pool catalog compiles")
}

// T14 (D2M:594; §3.2 checks 2-3; 簿 L648): a mark whose key the configuration's `policy.facts`
// declares for the instance, or that a `ledger_fact` pool of the instance observes, refuses the
// binding (`check-config` refuses it); a start ignores such a binding with
// `fact_reader_binding_ignored`, its refusal linked as the cause.
#[test]
fn d2s2_t14_a_mark_the_configuration_claims_refuses_the_binding_and_a_start_ignores_it() {
    let path = Path::new("packages").join("reader");
    let policy_fact = ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: "node.a".to_owned(),
        },
        fact_key: "resource.k1".to_owned(),
        value: FactValue::Integer(1),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    };
    let refused = recognition_binding_key_conflict(
        "node.a",
        &path,
        READING_PACKAGE_ID,
        &["resource.k1".to_owned()],
        std::slice::from_ref(&policy_fact),
        None,
    )
    .expect_err("a key policy.facts declares is refused");
    assert_eq!(
        refused.code().as_str(),
        HostCode::FactReaderBindingInvalid.as_str()
    );
    assert_eq!(reader_problem(&refused), Some("policy_fact_authority"));
    assert_eq!(
        refused.value("fact_key"),
        Some(&serde_json::json!("resource.k1"))
    );
    recognition_binding_key_conflict(
        "node.b",
        &path,
        READING_PACKAGE_ID,
        &["resource.k1".to_owned()],
        std::slice::from_ref(&policy_fact),
        None,
    )
    .expect("another instance's policy fact claims nothing of this instance");

    let catalog = live_pool_catalog();
    let live = recognition_binding_key_conflict(
        "fixture-instance-a",
        &path,
        READING_PACKAGE_ID,
        &["resource.primary".to_owned()],
        &[],
        Some(catalog.catalog()),
    )
    .expect_err("a key a ledger_fact pool observes is refused");
    assert_eq!(reader_problem(&live), Some("live_pool_key"));
    assert_eq!(
        live.value("fact_key"),
        Some(&serde_json::json!("resource.primary"))
    );

    let ignored = recognition_binding_ignored("node.a", Some(READING_PACKAGE_ID), refused.clone());
    assert_eq!(
        ignored.code().as_str(),
        HostCode::FactReaderBindingIgnored.as_str()
    );
    assert_eq!(
        ignored.category(),
        actingcommand_contract::outcome::Category::Warning
    );
    assert_eq!(
        ignored.value("instance_id"),
        Some(&serde_json::json!("node.a"))
    );
    let [cause] = ignored.causes() else {
        panic!("one cause: {ignored:?}");
    };
    assert_eq!(cause.relation().as_str(), "caused_by");
    assert_eq!(cause.outcome(), &refused);
}

// T34 (D2M:614), the S2 half: a realistic fact a catalog task reads in an instance's scope is
// `fact_reader_missing` for that instance while its reader does not mark the key, or it has
// none; once the instance's reader marks it, nothing is reported.
#[test]
fn d2s2_t34_a_realistic_fact_no_reader_marks_is_a_warning() {
    let compiled = compile_catalog(&policy_sources(1)).expect("the fixture catalog compiles");
    let catalog = compiled.catalog();
    let marked = |keys: &[&str]| {
        BTreeMap::from([(
            "fixture-instance-a".to_owned(),
            keys.iter()
                .map(|key| (*key).to_owned())
                .collect::<BTreeSet<_>>(),
        )])
    };
    for unmarked in [BTreeMap::new(), marked(&["resource.other"])] {
        let missing = missing_fact_readers(catalog, &unmarked);
        let [warning] = missing.as_slice() else {
            panic!("one missing reader: {missing:?}");
        };
        assert_eq!(
            warning.code().as_str(),
            HostCode::FactReaderMissing.as_str()
        );
        assert_eq!(
            warning.category(),
            actingcommand_contract::outcome::Category::Warning
        );
        assert_eq!(
            warning.value("instance_id"),
            Some(&serde_json::json!("fixture-instance-a"))
        );
        assert_eq!(
            warning.value("fact_key"),
            Some(&serde_json::json!("resource.primary"))
        );
        assert_eq!(
            warning.value("task_id"),
            Some(&serde_json::json!("fixture.observe"))
        );
    }
    assert!(missing_fact_readers(catalog, &marked(&["resource.primary"])).is_empty());
}
