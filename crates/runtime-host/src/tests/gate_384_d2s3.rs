// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #384 D2-S3 (`rd-1009/d2/model-d2-fact-refresh.md` v1.4.2 §3.6, §8, the host halves of
// rows T15, T16, T17 and T40): what a run's confirmed resource adjustments do to its instance's
// facts. A change read on the confirmation frame is added to or subtracted from the key's
// current record, which keeps its expiry and takes the lower confidence; a reading after the
// operation is the new value; a change that cannot be read, a reading that does not follow and
// a result below zero drop the key's record; a missing, expired or newer current record writes
// nothing. The run succeeds in every case. Every expected value comes from the model rows named
// on each test.

use super::*;
use crate::codes::AdjustmentReason;
use crate::resource_adjustment::{
    AdjustmentPlan, ConfirmationInput, PlannedAdjustment, PlannedBase, PlannedInvalidation,
    PlannedPublication, PreRunRecord, ReadingFrameInput, plan_resource_adjustments,
};
use actingcommand_contract::{
    EventId, ResourceAdjustmentAmount, ResourceAdjustmentDeclaration, ResourceAdjustmentDirection,
    ResourceReadingDeclaration,
};
use actingcommand_execution_kernel::{
    ResourceAdjustmentAmountValue, ResourceAdjustmentResult, ResourceReadingProblem,
    ResourceReadingValue,
};
use std::time::{SystemTime, UNIX_EPOCH};

const ADJUSTMENT_ALIAS: &str = "neutral.instance";
const SKIPPED: &str = "fact_adjustment_skipped";
const UNRESOLVED: &str = "contained_task_resource_reading_unresolved";

// On the terminal page (blue first pixel) the whole page reads 35, the change on its first pixel
// "+120" and the change on its second pixel "20", at confidence 0.99, or 0.5 while
// `terminal_low` is set; anything else reads 0.
#[derive(Debug, Default)]
struct TerminalChanges {
    terminal_low: AtomicBool,
}

impl VisionProvider for TerminalChanges {
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
            "adjustments request execution evidence",
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
        let text = match (terminal, request.region.x, request.region.width) {
            (true, 0, 2) => "35",
            (true, 0, 1) => "+120",
            (true, 1, 1) => "20",
            _ => "0",
        }
        .to_owned();
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
                invocation_id: "adjustment-ocr".to_owned(),
                session_id: "adjustment-session".to_owned(),
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

fn ocr_target(id: &str, x: u32, width: u32, minimum_confidence: f64) -> serde_json::Value {
    serde_json::json!({
        "type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": width, "height": 1},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["0"],
        "case_sensitive": false, "minimum_confidence": minimum_confidence,
        "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    })
}

// A guarded tap from `home` to `terminal`. The terminal page's whole page is the mark of
// `resource.k2`, which `spent` reads after the tap; `gained` adds the change on the terminal
// page's first pixel to `resource.k1`, `used` subtracts the one on its second pixel from
// `resource.k3`.
fn adjustment_package() -> Vec<u8> {
    let task = serde_json::json!({
        "schema_version": "0.9",
        "task_id": "task",
        "game": "neutral",
        "server_scope": ["test"],
        "coordinate_space": {"width": 2, "height": 1},
        "entry_page": "home",
        "target_page": "terminal",
        "scheduling_outcome": {"mappings": [
            {"outcome_key": "done", "effect": "no_designated_effect",
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
            "id": "k2", "fact_key": "resource.k2", "page_id": "terminal",
            "target_id": "count/page", "trim": "whitespace_v1",
            "value": {"type": "unsigned_integer", "min": 0, "max": 100000},
            "minimum_confidence_milli": 900, "valid_for_ms": 60000
        }],
        "resource_adjustments": [
            {"id": "spent", "fact_key": "resource.k2", "operation_id": "open_terminal"},
            {"id": "gained", "fact_key": "resource.k1", "operation_id": "open_terminal",
             "amount": {"target_id": "count/gain", "direction": "gain"}},
            {"id": "used", "fact_key": "resource.k3", "operation_id": "open_terminal",
             "amount": {"target_id": "count/used", "direction": "consume"}}
        ]
    });
    let control = serde_json::json!({
        "schema_version": "Lab-1y.control.v2",
        "package_id": "neutral.adjustment.task",
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
            ocr_target("count/page", 0, 2, 0.0),
            ocr_target("count/gain", 0, 1, 0.9),
            ocr_target("count/used", 1, 1, 0.9)
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
            .expect("adjustment package entry");
        zip.write_all(&serde_json::to_vec(&contents).expect("adjustment package JSON"))
            .expect("adjustment package content");
    }
    zip.finish()
        .expect("finish adjustment package")
        .into_inner()
}

// One manual run of the package; its receipt and correlation.
fn run_adjustment_package(
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
            ADJUSTMENT_ALIAS,
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(
                package.display().to_string(),
                actingcommand_pack_containment::Sha256Hash::digest(bytes).to_string(),
            )
            .expect("adjustment task request"),
        ),
    );
    (client.send(&request), correlation_id)
}

fn record_of(host: &RuntimeHost, key: &str) -> Option<FactRecord> {
    host.instance_fact_snapshot(InstanceFactContext {
        instance_id: ADJUSTMENT_ALIAS.to_owned(),
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

// The frame a reading's or an adjustment's `source_snapshot_id` names.
fn snapshot_frame(record: &FactRecord) -> String {
    let mut parts = record.source_snapshot_id.split('/');
    let run = parts.next().expect("run part");
    let frame = parts.next().expect("frame part").to_owned();
    assert!(
        run.starts_with("run:") && frame.starts_with("frame:"),
        "{}",
        record.source_snapshot_id
    );
    frame
}

// A key's record before the run: `value`, observed a second ago, valid for an hour.
fn before_the_run(key: &str, value: i64, confidence_milli: u16) -> FactRecord {
    let observed_at_unix_ms = unix_ms_now().expect("the clock") - 1_000;
    FactRecord {
        scope: FactScope::Instance {
            instance_id: ADJUSTMENT_ALIAS.to_owned(),
        },
        key: key.to_owned(),
        content: FactContent::Inline {
            value: ContractFactValue::Integer(value),
        },
        observed_at_unix_ms,
        expires_at_unix_ms: Some(observed_at_unix_ms + 3_600_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 3_600_000,
            maximum_ms: 3_600_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli,
        source_detector: "fixture.before".to_owned(),
        source_snapshot_id: format!("snapshot:before:{key}"),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "c".repeat(64),
        invalidate_on: Vec::new(),
    }
}

// The task diagnostic rows of the run `correlation_id` whose outcome is `code`.
fn diagnostic_rows(
    host: &RuntimeHost,
    root: &Path,
    correlation_id: actingcommand_contract::CorrelationId,
    code: &str,
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
        .iter()
        .filter(|record| {
            record["kind"] == "error" && record["data"]["error"]["outcome"]["code"] == code
        })
        .cloned()
        .collect()
}

// T15 (D2M:595), T17 (D2M:597), T40 (D2M:620) and T16 (D2M:596, the unreadable change), end to
// end. Before the first run `resource.k1` is 100 (confidence 800) and `resource.k3` 100. The
// first run reads "+120" for `gained` and "20" for `used` on its confirmation frame: k1 becomes
// 220 at confidence 800 and k3 80, each observed on that frame and keeping its record's expiry;
// `spent` writes nothing, and the terminal reading 35 is k2's new value. The second run reads
// nothing on the terminal page: each key's record is dropped, each adjustment is one
// `fact_adjustment_skipped` row (`after_read_missing`, `amount_unresolved` twice) beside the
// misses, and the run succeeds.
#[test]
fn d2s3_t15_t16_t17_t40_confirmed_adjustments_write_or_drop_their_keys() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("adjustment.zip");
    let bytes = adjustment_package();
    fs::write(&package, &bytes).expect("write adjustment package");
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
    let provider = Arc::new(TerminalChanges::default());
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(
            FakeProvider::one(ADJUSTMENT_ALIAS, instance_id(), Arc::clone(&state))
                .with_vision_provider(provider.clone()),
        ),
    )
    .expect("runtime host");
    let k1_before = before_the_run("resource.k1", 100, 800);
    let k3_before = before_the_run("resource.k3", 100, 1_000);
    host.publish_fact(k1_before.clone())
        .expect("publish resource.k1");
    host.publish_fact(k3_before.clone())
        .expect("publish resource.k3");

    let (receipt, correlation_id) = run_adjustment_package(&host, &package, &bytes);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the first run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    let k1 = record_of(&host, "resource.k1").expect("resource.k1 after the first run");
    let k2 = record_of(&host, "resource.k2").expect("resource.k2 after the first run");
    let k3 = record_of(&host, "resource.k3").expect("resource.k3 after the first run");
    assert_eq!(integer_of(&k2), Some(35));
    assert_eq!(k2.source_detector, "resource_reading:task/k2");
    assert_eq!(integer_of(&k1), Some(220));
    assert_eq!(k1.confidence_milli, 800);
    assert_eq!(k1.source_detector, "resource_adjustment:task/gained");
    assert_eq!(k1.expires_at_unix_ms, k1_before.expires_at_unix_ms);
    assert_eq!(integer_of(&k3), Some(80));
    assert_eq!(k3.confidence_milli, 990);
    assert_eq!(k3.source_detector, "resource_adjustment:task/used");
    assert_eq!(k3.expires_at_unix_ms, k3_before.expires_at_unix_ms);
    for adjusted in [&k1, &k3] {
        assert_eq!(adjusted.observed_at_unix_ms, k2.observed_at_unix_ms);
        assert_eq!(snapshot_frame(adjusted), snapshot_frame(&k2));
        assert!(
            adjusted.source_snapshot_id.contains("/base:evt_"),
            "{}",
            adjusted.source_snapshot_id
        );
        let expires = adjusted.expires_at_unix_ms.expect("an expiry");
        assert_eq!(
            adjusted.ttl_policy,
            Some(FactTtlPolicy {
                minimum_ms: expires - adjusted.observed_at_unix_ms,
                maximum_ms: expires - adjusted.observed_at_unix_ms,
                source: FactTtlSource::DetectorContract,
            })
        );
    }
    assert!(diagnostic_rows(&host, root.path(), correlation_id, SKIPPED).is_empty());

    // The second run starts on the home page again and reads nothing on the terminal page.
    state
        .transition_capture_after_inputs
        .store(2, Ordering::Release);
    provider.terminal_low.store(true, Ordering::Release);
    let (receipt, correlation_id) = run_adjustment_package(&host, &package, &bytes);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the second run's outcome: {:?}; the Runtime's fatal error: {:?}",
        receipt.outcome(),
        host.fatal_error()
    );
    for key in ["resource.k1", "resource.k2", "resource.k3"] {
        assert_eq!(record_of(&host, key), None, "{key} is dropped");
    }
    let mut skipped = diagnostic_rows(&host, root.path(), correlation_id, SKIPPED)
        .iter()
        .map(|row| {
            let outcome = &row["data"]["error"]["outcome"];
            assert_eq!(outcome["category"], "info");
            assert_eq!(outcome["values"]["instance_id"], ADJUSTMENT_ALIAS);
            assert_eq!(outcome["values"]["task_id"], "task");
            (
                outcome["values"]["fact_key"]
                    .as_str()
                    .expect("fact_key")
                    .to_owned(),
                outcome["values"]["adjustment_reason"]
                    .as_str()
                    .expect("adjustment_reason")
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    skipped.sort();
    assert_eq!(
        skipped,
        [
            ("resource.k1", "amount_unresolved"),
            ("resource.k2", "after_read_missing"),
            ("resource.k3", "amount_unresolved"),
        ]
        .map(|(key, reason)| (key.to_owned(), reason.to_owned()))
    );
    let mut misses = diagnostic_rows(&host, root.path(), correlation_id, UNRESOLVED)
        .iter()
        .map(|row| {
            let outcome = &row["data"]["error"]["outcome"];
            assert_eq!(outcome["category"], "warning");
            assert_eq!(outcome["values"]["reading_problem"], "low_confidence");
            outcome["values"]["reading_id"]
                .as_str()
                .expect("reading_id")
                .to_owned()
        })
        .collect::<Vec<_>>();
    misses.sort();
    assert_eq!(misses, ["gained", "k2", "used"]);
    host.close().expect("close the host");
}

const CONFIRMED_AT_MS: u64 = 10_000;
const STEP_SEQUENCE: u64 = 50;
const NOW_MS: u64 = 20_000;

fn at(unix_ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(unix_ms)
}

fn event(n: u8) -> EventId {
    serde_json::from_value(serde_json::json!(format!("evt_{:032x}", n))).expect("an event id")
}

fn consume(amount: Result<u64, ResourceReadingProblem>) -> ResourceAdjustmentResult {
    ResourceAdjustmentResult {
        declaration: ResourceAdjustmentDeclaration {
            id: "used".to_owned(),
            fact_key: "resource.k1".to_owned(),
            operation_id: "spend".to_owned(),
            amount: Some(ResourceAdjustmentAmount {
                target_id: "count/used".to_owned(),
                direction: ResourceAdjustmentDirection::Consume,
            }),
        },
        amount: Some(amount.map(|value| ResourceAdjustmentAmountValue {
            value,
            confidence_milli: 990,
        })),
    }
}

fn read_after() -> ResourceAdjustmentResult {
    ResourceAdjustmentResult {
        declaration: ResourceAdjustmentDeclaration {
            id: "spent".to_owned(),
            fact_key: "resource.k1".to_owned(),
            operation_id: "spend".to_owned(),
            amount: None,
        },
        amount: None,
    }
}

fn reading(value: u64) -> ResourceReadingValue {
    ResourceReadingValue {
        declaration: ResourceReadingDeclaration {
            id: "k1".to_owned(),
            fact_key: "resource.k1".to_owned(),
            page_id: "home".to_owned(),
            target_id: "count/balance".to_owned(),
            trim: actingcommand_contract::OcrFieldTrim::WhitespaceV1,
            value: actingcommand_contract::ResourceReadingType::UnsignedInteger {
                min: 0,
                max: 100_000,
                format: actingcommand_contract::OcrUnsignedIntegerFormat::AsciiDecimal,
            },
            minimum_confidence_milli: 900,
            valid_for_ms: 60_000,
        },
        value,
        confidence_milli: 990,
    }
}

// `resource.k1` before the run: `value`, observed at `observed_at_ms`, until `expires_at_ms`,
// published at ledger sequence 10.
fn k1_before(
    value: i64,
    observed_at_ms: u64,
    expires_at_ms: u64,
) -> BTreeMap<String, PreRunRecord> {
    let mut record = before_the_run("resource.k1", value, 800);
    record.observed_at_unix_ms = observed_at_ms;
    record.expires_at_unix_ms = Some(expires_at_ms);
    record.ttl_policy = Some(FactTtlPolicy {
        minimum_ms: expires_at_ms - observed_at_ms,
        maximum_ms: expires_at_ms - observed_at_ms,
        source: FactTtlSource::DetectorContract,
    });
    BTreeMap::from([(
        "resource.k1".to_owned(),
        PreRunRecord {
            record,
            sequence: 10,
            event_id: event(1),
        },
    )])
}

// Plans one confirmation of `adjustment` at `CONFIRMED_AT_MS` after the frames `frames` (capture
// time, reading of `resource.k1`), with `pre_run` as the key's records before the run.
fn plan(
    adjustment: ResourceAdjustmentResult,
    frames: &[(u64, ResourceReadingValue)],
    pre_run: &BTreeMap<String, PreRunRecord>,
) -> AdjustmentPlan {
    let readings = frames
        .iter()
        .map(|(_, reading)| vec![reading.clone()])
        .collect::<Vec<_>>();
    let frames = frames
        .iter()
        .zip(&readings)
        .map(|((captured_at_ms, _), readings)| ReadingFrameInput {
            captured_at: at(*captured_at_ms),
            observed_at_unix_ms: *captured_at_ms,
            readings,
        })
        .collect::<Vec<_>>();
    let adjustments = [adjustment];
    plan_resource_adjustments(
        ADJUSTMENT_ALIAS,
        "task",
        NOW_MS,
        &frames,
        &[ConfirmationInput {
            captured_at: at(CONFIRMED_AT_MS),
            observed_at_unix_ms: CONFIRMED_AT_MS,
            step_sequence: STEP_SEQUENCE,
            adjustments: &adjustments,
        }],
        pre_run,
    )
}

// The one outcome of a plan, as (code, adjustment_reason).
fn outcome_of(plan: &AdjustmentPlan) -> Option<(String, Option<String>)> {
    match plan.outcomes.as_slice() {
        [] => None,
        [planned] => {
            assert_eq!(planned.confirmation, 0);
            assert_eq!(
                planned.outcome.value("instance_id"),
                Some(&serde_json::json!(ADJUSTMENT_ALIAS))
            );
            assert_eq!(
                planned.outcome.value("fact_key"),
                Some(&serde_json::json!("resource.k1"))
            );
            assert_eq!(
                planned.outcome.value("task_id"),
                Some(&serde_json::json!("task"))
            );
            Some((
                planned.outcome.code().as_str().to_owned(),
                planned
                    .outcome
                    .value("adjustment_reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            ))
        }
        more => panic!("one outcome at most: {more:?}"),
    }
}

fn skipped(reason: AdjustmentReason) -> Option<(String, Option<String>)> {
    Some((
        HostCode::FactAdjustmentSkipped.as_str().to_owned(),
        Some(reason.as_str().to_owned()),
    ))
}

// T16 (D2M:596; §3.6), the plan. A consumption of 20 confirmed at 10 s, `now` at 20 s:
// - no current record: `base_missing`, nothing written or dropped;
// - a record that expired at 5 s: `base_expired`;
// - a reading of the key on the confirmation frame: `newer_observation`, the reading published;
// - a record before the run observed at 15 s: `newer_observation`;
// - a record of 10: `fact_adjustment_underflow` (warning), the record dropped by the step;
// - an unreadable change: `amount_unresolved`, the record dropped;
// - a record of 100 (confidence 800, until 100 s): the write of 80 observed at 10 s, confidence
//   800, until 100 s, adjusting that record;
// - T40, read after its operation: a reading of 100 at 5 s and none after the confirmation is
//   `after_read_missing`; the earlier reading is not published and the record is dropped.
#[test]
fn d2s3_t16_t40_the_plan_writes_skips_or_drops_as_the_model_says() {
    let none = BTreeMap::new();
    let dropped = |base_sequence: u64, adjustment_id: &str| PlannedInvalidation {
        adjustment_id: adjustment_id.to_owned(),
        fact_key: "resource.k1".to_owned(),
        base_sequence,
        step_sequence: STEP_SEQUENCE,
    };

    let missing = plan(consume(Ok(20)), &[], &none);
    assert_eq!(outcome_of(&missing), skipped(AdjustmentReason::BaseMissing));
    assert!(missing.publications.is_empty() && missing.invalidations.is_empty());

    let expired = plan(consume(Ok(20)), &[], &k1_before(100, 1_000, 5_000));
    assert_eq!(outcome_of(&expired), skipped(AdjustmentReason::BaseExpired));
    assert!(expired.publications.is_empty() && expired.invalidations.is_empty());

    let read_on_frame = plan(
        consume(Ok(20)),
        &[(CONFIRMED_AT_MS, reading(90))],
        &k1_before(100, 1_000, 100_000),
    );
    assert_eq!(
        outcome_of(&read_on_frame),
        skipped(AdjustmentReason::NewerObservation)
    );
    assert_eq!(
        read_on_frame.publications,
        [PlannedPublication::Reading {
            frame: 0,
            reading: 0
        }]
    );
    assert!(read_on_frame.invalidations.is_empty());

    let newer = plan(consume(Ok(20)), &[], &k1_before(100, 15_000, 100_000));
    assert_eq!(
        outcome_of(&newer),
        skipped(AdjustmentReason::NewerObservation)
    );
    assert!(newer.publications.is_empty() && newer.invalidations.is_empty());

    let underflow = plan(consume(Ok(20)), &[], &k1_before(10, 1_000, 100_000));
    let [planned] = underflow.outcomes.as_slice() else {
        panic!("one outcome: {:?}", underflow.outcomes);
    };
    assert_eq!(
        planned.outcome.code().as_str(),
        HostCode::FactAdjustmentUnderflow.as_str()
    );
    assert_eq!(
        planned.outcome.category(),
        actingcommand_contract::outcome::Category::Warning
    );
    assert!(underflow.publications.is_empty());
    assert_eq!(underflow.invalidations, [dropped(10, "used")]);

    let unreadable = plan(
        consume(Err(ResourceReadingProblem::LowConfidence)),
        &[],
        &k1_before(100, 1_000, 100_000),
    );
    assert_eq!(
        outcome_of(&unreadable),
        skipped(AdjustmentReason::AmountUnresolved)
    );
    assert_eq!(
        unreadable.outcomes[0].target_id.as_deref(),
        Some("count/used")
    );
    assert!(unreadable.publications.is_empty());
    assert_eq!(unreadable.invalidations, [dropped(10, "used")]);

    let written = plan(consume(Ok(20)), &[], &k1_before(100, 1_000, 100_000));
    assert_eq!(outcome_of(&written), None);
    assert!(written.invalidations.is_empty());
    assert_eq!(
        written.publications,
        [PlannedPublication::Adjustment(PlannedAdjustment {
            confirmation: 0,
            adjustment_id: "used".to_owned(),
            fact_key: "resource.k1".to_owned(),
            value: Some(80),
            confidence_milli: 800,
            observed_at_unix_ms: CONFIRMED_AT_MS,
            expires_at_unix_ms: Some(100_000),
            base: PlannedBase::PreRun {
                sequence: 10,
                event_id: event(1),
            },
        })]
    );

    let unread = plan(
        read_after(),
        &[(5_000, reading(100))],
        &k1_before(100, 1_000, 100_000),
    );
    assert_eq!(
        outcome_of(&unread),
        skipped(AdjustmentReason::AfterReadMissing)
    );
    assert!(unread.publications.is_empty());
    assert_eq!(unread.invalidations, [dropped(10, "spent")]);
}
