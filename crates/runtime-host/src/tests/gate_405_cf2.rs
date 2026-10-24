// SPDX-License-Identifier: AGPL-3.0-only

// SCH-M slice CF-2, the host (frozen model `rd-1024/sched-design/model.md` §3.3 D-9 and its §6
// row CF-2; `contracts/resource-readings.md`, "Host"): a reading mark declared
// `{"type": "timestamp_ms", "format": "clock_hms", "base": "observation"}` publishes the frame's
// capture time plus the countdown it shows as a `timestamp_ms` instance fact, and the W1
// predicate `fact_time_reached` on that key turns true at that instant. One manual run on the
// policy fixture's instance: a guarded tap from `home` to `terminal`, whose whole page shows the
// countdown `1:05:00`; the mark writes `resource.neutral.ready.slot`.

use super::*;
use actingcommand_policy::{PolicyEvaluation, SCHEDULING_SCHEMA_VERSION_V2};

const READY_AT: &str = "resource.neutral.ready.slot";
// `1:05:00` in milliseconds.
const COUNTDOWN_MS: u64 = 3_900_000;
// The mark's `valid_for_ms`: twelve hours, past every evaluation below.
const VALID_FOR_MS: u64 = 43_200_000;
const PENDING: &str = "fact_time_pending";
const TASK: &str = "fixture.observe";

// Reads the countdown `1:05:00` at confidence 0.99.
#[derive(Debug, Default)]
struct ShownCountdown;

impl VisionProvider for ShownCountdown {
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
        let text = String::from("1:05:00");
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
                invocation_id: "countdown-ocr".to_owned(),
                session_id: "countdown-session".to_owned(),
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

// A guarded tap from `home` to `terminal`, whose whole page is the countdown mark `slot`.
fn countdown_package() -> Vec<u8> {
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "read", "effect": "no_designated_effect",
             "terminal_pages": ["terminal"]}
        ]},
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
        "resource_readings": [{
            "id": "slot", "fact_key": READY_AT, "page_id": "terminal",
            "target_id": "count/page", "trim": "whitespace_v1",
            "value": {"type": "timestamp_ms", "format": "clock_hms", "base": "observation"},
            "minimum_confidence_milli": 900, "valid_for_ms": VALID_FOR_MS
        }]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.countdown.task",
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
            {"type": "ocr", "id": "count/page",
             "region": {"x": 0, "y": 0, "width": 2, "height": 1}, "languages": ["en"],
             "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
             "case_sensitive": false, "minimum_confidence": 0.0,
             "model_ref": "PP-OCRv6_medium",
             "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
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
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, contents) in files {
        zip.start_file(path, options)
            .expect("countdown package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("countdown package JSON"))
            .expect("countdown package content");
    }
    zip.finish().expect("finish countdown package").into_inner()
}

// A Runtime whose instance, the policy fixture's, shows `home` until its first input and
// `terminal` after it, after one manual run of the countdown package; and the record the run
// published under `READY_AT`.
fn published_countdown(root: &TempDir) -> (RuntimeHost, FactRecord) {
    let package = root.path().join("countdown.zip");
    fs::write(&package, countdown_package()).expect("write the countdown package");
    let state = Arc::new(FakeState::default());
    // A direct run needs a device instance, and a device's first frame passes the task geometry
    // only with its display observed (Workflow #392 DA-A).
    state.physical_task_geometry.store(true, Ordering::Release);
    *state
        .physical_task_extent
        .lock()
        .expect("fake physical task extent") = Some((2, 1));
    state
        .transition_capture_after_inputs
        .store(1, Ordering::Release);
    let host = RuntimeHost::start(
        config(root),
        Arc::new(
            FakeProvider::one(POLICY_INSTANCE_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(Arc::new(ShownCountdown)),
        ),
    )
    .expect("runtime host");

    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::run_contained_task(
        POLICY_INSTANCE_ALIAS,
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(package.display().to_string()).expect("countdown task request"),
    ));
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    drop(client);
    let record = host
        .instance_fact_snapshot(InstanceFactContext {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
            server_id: "test".to_owned(),
            game_id: "neutral".to_owned(),
        })
        .expect("instance facts")
        .records
        .into_iter()
        .find(|record| record.key == READY_AT)
        .unwrap_or_else(|| panic!("no record of {READY_AT}"));
    (host, record)
}

// The policy fixture's catalog declared V2, its task triggered by `fact_time_reached` on
// `READY_AT` in the instance's scope with no offset and no age limit; its activity profile is
// open all day and paces dispatches one millisecond apart.
fn countdown_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: serde_json::Value =
            serde_json::from_slice(&source.bytes).expect("policy fixture JSON");
        document["schema_version"] = serde_json::json!(SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document
            .get_mut("events")
            .and_then(serde_json::Value::as_array_mut)
        {
            for event in events {
                event["validity"] = serde_json::json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if let Some(profiles) = document
            .get_mut("profiles")
            .and_then(serde_json::Value::as_array_mut)
        {
            for profile in profiles {
                profile["windows"] = serde_json::json!([{
                    "weekdays": [1, 2, 3, 4, 5, 6, 7],
                    "utc_offset_minutes": 0,
                    "start_minute_of_day": 0,
                    "end_minute_of_day": 0
                }]);
                profile["minimum_interval_ms"] = serde_json::json!(1);
                profile["maximum_interval_ms"] = serde_json::json!(1);
            }
        }
        if let Some(tasks) = document
            .get_mut("tasks")
            .and_then(serde_json::Value::as_array_mut)
        {
            tasks[0]["trigger"] = serde_json::json!({
                "kind": "fact_time_reached",
                "scope": {"kind": "instance", "instance_id": POLICY_INSTANCE_ALIAS},
                "fact_key": READY_AT,
                "offset_ms": 0,
                "max_age_ms": null
            });
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("policy fixture bytes");
    }
    sources
}

// One full policy cycle at `unix_ms`.
fn evaluate_at(host: &RuntimeHost, unix_ms: u64) -> PolicyEvaluation {
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &policy_resources(),
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
        },
        11,
        PolicyTrigger::Recovery,
    )
    .expect("policy cycle")
    .evaluation
    .expect("a full evaluation")
}

// D-9, row item "frame T reading `1:05:00` → `TimestampMs(T + 3 900 000)`": the record of the
// countdown mark is the `timestamp_ms` value of its frame's capture time T (its
// `observed_at_unix_ms`) plus 3 900 000 ms, valid for the declared `valid_for_ms` from T as every
// reading is, in the instance's scope.
#[test]
fn cf2_a_countdown_read_on_frame_t_publishes_t_plus_its_countdown() {
    let root = TempDir::new().expect("tempdir");
    let (host, record) = published_countdown(&root);
    let frame_time = record.observed_at_unix_ms;
    assert_eq!(
        record.content,
        FactContent::Inline {
            value: ContractFactValue::TimestampMs(frame_time + COUNTDOWN_MS)
        }
    );
    assert_eq!(
        record.scope,
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned()
        }
    );
    assert_eq!(record.expires_at_unix_ms, Some(frame_time + VALID_FOR_MS));
    host.close().expect("close the host");
}

// D-9, row item "with W1 the predicate turns true at that instant": with the published record,
// the catalog task triggered by `fact_time_reached` on its key is not due one millisecond before
// T + 3 900 000 (its decision carries `fact_time_pending` with that instant as
// `next_eligible_unix_ms`), and is dispatched at that instant.
#[test]
fn cf2_the_published_time_turns_fact_time_reached_true_at_that_instant() {
    let root = TempDir::new().expect("tempdir");
    let (host, record) = published_countdown(&root);
    let ready = record.observed_at_unix_ms + COUNTDOWN_MS;
    host.activate_policy_catalog(&countdown_sources())
        .expect("activate the countdown catalog");

    let before = evaluate_at(&host, ready - 1);
    assert!(
        before
            .dispatch_intents
            .iter()
            .all(|intent| intent.task_id != TASK),
        "not due before the instant: {:#?}",
        before.dispatch_intents
    );
    let decision = before
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == TASK
                && decision.instance_id.as_deref() == Some(POLICY_INSTANCE_ALIAS)
        })
        .expect("the task's decision on the instance");
    let pending = decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.code().as_str() == PENDING)
        .unwrap_or_else(|| panic!("fact_time_pending in {:#?}", decision.reasons));
    assert_eq!(
        pending.outcome.values().get("next_eligible_unix_ms"),
        Some(&serde_json::json!(ready))
    );

    let at = evaluate_at(&host, ready);
    assert!(
        at.dispatch_intents
            .iter()
            .any(|intent| intent.task_id == TASK),
        "due at the instant: {:#?}",
        at.decisions
    );
    host.close().expect("close the host");
}
