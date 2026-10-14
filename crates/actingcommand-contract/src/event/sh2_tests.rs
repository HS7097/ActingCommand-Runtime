// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 SH-2, the second v0.12.0 shape slice: the shapes of `contracts/v012-register.md`
// §1.25 (rows X1-X14 but the register-only X10) are decoded, validated and encoded again by
// their readers. Each test names its register rows. No writer exists before the slices the
// register names, so every stored record here is a record an existing writer makes, extended
// with the new member, or a JSON record of the registered shape.

use super::sh1::{
    AT_MS, admission, dispatch_data, epoch, holder, ids, lifecycle, outcome, read_back, record,
    refused, token, wake, with, written,
};
use super::*;
use crate::{
    CaptureBackendName, CaptureExtent, CaptureGeometry, CaptureGeometryObservation,
    CaptureGeometrySource, CaptureInsets, CaptureRotation, CaptureRotationObservation,
    CaptureRotationSource, CaptureWmSizeKind,
};
use serde_json::{Value, json};
use std::time::{Duration, SystemTime};

fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(AT_MS)
}

fn pressure(
    kind: PerformancePressureKind,
    severity: PerformancePressureSeverity,
    peak: PerformancePressureValue,
) -> Value {
    written(
        PerformancePayloadDraft::pressure_started(
            PerformancePressureEventData {
                observed_at_unix_ms: AT_MS,
                pressure: PerformancePressureRecord {
                    kind,
                    severity,
                    started_at_unix_ms: AT_MS,
                    last_observed_at_unix_ms: AT_MS,
                    peak,
                },
            },
            AuditInput::new(),
        )
        .into(),
    )
}

#[test]
fn x1_fullscreen_contention_and_ledger_backlog_pressures_read_back_in_pairs() {
    assert_eq!(
        token(PerformancePressureKind::FullscreenContention),
        json!("fullscreen_contention")
    );
    assert_eq!(
        token(PerformancePressureKind::LedgerBacklog),
        json!("ledger_backlog")
    );

    // A fullscreen contention's peak is the third-party load behind it.
    let contention = pressure(
        PerformancePressureKind::FullscreenContention,
        PerformancePressureSeverity::High,
        PerformancePressureValue::ProcessLoad {
            process_count: 2,
            peak_cpu_basis_points: 6_500,
            peak_io_bytes_per_second: 1_048_576,
        },
    );
    read_back(&contention);
    let mut utilization = contention.clone();
    holder(&mut utilization, "peak").expect("the pressure")["peak"] =
        json!({"kind": "utilization", "basis_points": 9_000});
    let error = refused(utilization);
    assert_eq!(
        error.code(),
        SanitizationCode::InvalidPerformancePressureValue.code()
    );

    // A ledger backlog's tier names its severity: 1 elevated, 2 high, 3 critical.
    for (tier, severity) in [
        (1_u8, PerformancePressureSeverity::Elevated),
        (2, PerformancePressureSeverity::High),
        (3, PerformancePressureSeverity::Critical),
    ] {
        let backlog = pressure(
            PerformancePressureKind::LedgerBacklog,
            severity,
            PerformancePressureValue::LedgerBacklog {
                tier,
                buffer_bytes: 48 * 1_048_576,
                buffer_records: 12_000,
            },
        );
        let payload = read_back(&backlog);
        let EventPayload::Performance(PerformancePayload::PressureStarted(started)) = &payload
        else {
            panic!("a pressure record");
        };
        assert_eq!(
            started.pressure().kind,
            PerformancePressureKind::LedgerBacklog
        );
        assert_eq!(
            holder(&mut backlog.clone(), "peak").expect("the pressure")["peak"],
            json!({
                "kind": "ledger_backlog",
                "tier": tier,
                "buffer_bytes": 48 * 1_048_576,
                "buffer_records": 12_000,
            })
        );
    }

    let backlog = pressure(
        PerformancePressureKind::LedgerBacklog,
        PerformancePressureSeverity::High,
        PerformancePressureValue::LedgerBacklog {
            tier: 2,
            buffer_bytes: 1_024,
            buffer_records: 3,
        },
    );
    let edited = |member: &str, value: Value| {
        let mut stored = backlog.clone();
        let held = holder(&mut stored, "peak").expect("the pressure");
        match member {
            "tier" => held["peak"]["tier"] = value,
            other => held[other] = value,
        }
        stored
    };
    for bad in [
        edited("severity", json!("critical")),
        edited("tier", json!(0)),
        edited("tier", json!(4)),
        edited("kind", json!("cpu")),
    ] {
        let error = refused(bad);
        assert_eq!(
            error.code(),
            SanitizationCode::InvalidPerformancePressureValue.code()
        );
    }
}

#[test]
fn x2_x3_a_wake_may_name_no_trigger_record() {
    let ids = ids();
    assert_eq!(token(AgentWakeKind::DailySummary), json!("daily_summary"));
    let requested = |wake_data: Value| {
        record(
            "agent",
            "wake_requested",
            json!({"action": "agent.wake", "wake": wake_data, "audit": {}}),
        )
    };
    // Today's writer names the trigger record and its sequence.
    let triggered = token(wake(&ids, AgentWakeKind::DailySummary));
    assert!(triggered.get("trigger_event_id").is_some());
    assert_eq!(triggered["trigger_sequence"], 1);
    read_back(&requested(triggered.clone()));

    // A wake without a trigger sequence (AO-4), and a timer's wake without a trigger record.
    let mut unsequenced = triggered.clone();
    unsequenced
        .as_object_mut()
        .expect("a wake")
        .remove("trigger_sequence");
    let payload = read_back(&requested(unsequenced.clone()));
    let EventPayload::Agent(AgentPayload::WakeRequested(read)) = &payload else {
        panic!("a wake record");
    };
    assert_eq!(read.wake().trigger_sequence(), None);
    assert!(read.wake().trigger_event_id().is_some());

    let mut timed = unsequenced;
    timed
        .as_object_mut()
        .expect("a wake")
        .remove("trigger_event_id");
    let payload = read_back(&requested(timed));
    let EventPayload::Agent(AgentPayload::WakeRequested(read)) = &payload else {
        panic!("a wake record");
    };
    assert_eq!(read.wake().kind(), AgentWakeKind::DailySummary);
    assert_eq!(read.wake().trigger_event_id(), None);
    assert_eq!(read.wake().trigger_sequence(), None);

    // A present sequence is never 0.
    let mut zero = triggered;
    zero["trigger_sequence"] = json!(0);
    let error = refused(requested(zero));
    assert_eq!(error.code(), SanitizationCode::InvalidAgentWake.code());
}

#[test]
fn x4_tagged_state_phases_may_carry_their_report() {
    let ids = ids();
    let stores = json!([{"source": "instance_facts", "lineage": epoch(&ids), "loaded_count": 3}]);
    let loaded = |report: Value| {
        lifecycle(
            &ids,
            json!({"kind": "tagged_state_loaded", "stores": stores, "outcome": report}),
        )
    };
    let report = json!({
        "code": "ledger_tagged_state_loaded",
        "category": "info",
        "values": {"count": 3, "elapsed_ms": 12},
    });
    let payload = read_back(&loaded(report));
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(recorded)) = &payload else {
        panic!("a lifecycle record");
    };
    assert_eq!(
        recorded.outcome().map(|report| report.code().as_str()),
        Some("ledger_tagged_state_loaded")
    );
    assert_eq!(
        record_severity(EventSeverity::Info, &payload),
        EventSeverity::Info
    );

    let seeded = lifecycle(
        &ids,
        json!({
            "kind": "tagged_state_enabled",
            "outcome": {
                "code": "ledger_tagged_state_seeded",
                "category": "info",
                "values": {"count": 40},
            },
        }),
    );
    let payload = read_back(&seeded);
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(recorded)) = &payload else {
        panic!("a lifecycle record");
    };
    assert_eq!(
        recorded.outcome().map(|report| report.code().as_str()),
        Some("ledger_tagged_state_seeded")
    );

    // The report of a load or a seeding is info.
    refused(loaded(outcome("ledger_tag_index_mismatch", "fatal")));
    refused(lifecycle(
        &ids,
        json!({
            "kind": "tagged_state_enabled",
            "outcome": outcome("ledger_tag_key_unclassified", "warning"),
        }),
    ));
}

#[test]
fn x5_a_probe_finding_may_report_an_unclassified_workbench_key() {
    let finding = record(
        "ledger",
        "health_observed",
        json!({
            "action": "ledger.probe",
            "outcome": {
                "code": "ledger_tag_key_unclassified",
                "category": "warning",
                "values": {"fact_key": "host.example_prefix"},
            },
            "audit": {},
        }),
    );
    let payload = read_back(&finding);
    assert_eq!(payload.event_type(), EventType::LedgerHealthObserved);
    assert_eq!(
        record_severity(EventSeverity::Info, &payload),
        EventSeverity::Warning
    );
}

fn geometry(safe_insets: Option<CaptureInsets>) -> CaptureGeometryObservation {
    let display = CaptureExtent::new(2340, 1080).expect("an extent");
    CaptureGeometryObservation::Observed(CaptureGeometry {
        backend: CaptureBackendName::AdbScreencap,
        source: CaptureGeometrySource::AdbDefaultDisplay {
            serial: "handset:neutral.handset".to_owned(),
            wm_extent: display,
            wm_size_kind: CaptureWmSizeKind::Physical,
        },
        logical_display_extent: display,
        rotation: CaptureRotationObservation::Observed {
            rotation: CaptureRotation::R0,
            source: CaptureRotationSource::DumpsysDisplayOrientation,
        },
        sampled_at: at(),
        frame_transform: None,
        safe_insets,
    })
}

fn observed(
    ids: &IdentifierIssuer,
    phase: TaskGeometryPhase,
    trigger: Option<TaskGeometryRecheckTrigger>,
    safe_insets: Option<CaptureInsets>,
    display_map: Option<TaskGeometryDisplayMap>,
) -> Value {
    let observation = geometry(safe_insets);
    let frame = TaskGeometryFrame {
        frame_id: *ids.mint_frame_id().expect("frame id").transport(),
        extent: CaptureExtent::new(2340, 1080).expect("an extent"),
        backend: CaptureBackendName::AdbScreencap,
        captured_at: at(),
        producer: observation.clone(),
    };
    let conclusion = TaskGeometryObservation::assess(&frame, &observation);
    assert_eq!(conclusion, TaskGeometryConclusion::Pass);
    written(
        TaskPayloadDraft::semantic(
            TaskSemanticFact::GeometryObserved {
                observation: Box::new(TaskGeometryObservation {
                    phase,
                    frame: Some(frame),
                    observation: Some(observation),
                    conclusion,
                    trigger,
                    original_failure: None,
                    unavailable: None,
                    display_map,
                }),
            },
            AuditInput::new(),
        )
        .into(),
    )
}

fn observation_of(stored: &mut Value) -> &mut serde_json::Map<String, Value> {
    holder(stored, "conclusion").expect("a geometry observation")
}

#[test]
fn x6_capture_geometry_records_its_safe_insets() {
    let ids = ids();
    let insets = CaptureInsets {
        left: 90,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let stored = observed(&ids, TaskGeometryPhase::Initial, None, Some(insets), None);
    let payload = read_back(&stored);
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = &payload else {
        panic!("a task record");
    };
    let TaskSemanticFact::GeometryObserved { observation } = semantic.fact() else {
        panic!("a geometry record");
    };
    let Some(CaptureGeometryObservation::Observed(read)) = &observation.observation else {
        panic!("an observed geometry");
    };
    assert_eq!(read.safe_insets, Some(insets));
    assert_eq!(
        observation_of(&mut stored.clone())["observation"]["detail"]["safe_insets"],
        json!({"left": 90, "top": 0, "right": 0, "bottom": 0})
    );
    // Absent when not read: today's records keep their shape.
    let plain = observed(&ids, TaskGeometryPhase::Initial, None, None, None);
    assert!(!plain.to_string().contains("safe_insets"));

    // The insets leave a safe area inside the display.
    let mut wide = stored.clone();
    holder(&mut wide, "safe_insets").expect("a geometry")["safe_insets"] =
        json!({"left": 1200, "top": 0, "right": 1140, "bottom": 0});
    refused(wide);
    let mut tall = stored.clone();
    holder(&mut tall, "safe_insets").expect("a geometry")["safe_insets"] =
        json!({"left": 0, "top": 1080, "right": 0, "bottom": 0});
    refused(tall);
    let mut unknown = stored;
    holder(&mut unknown, "safe_insets").expect("a geometry")["safe_insets"] =
        json!({"left": 0, "top": 0, "right": 0, "bottom": 0, "source": "reported"});
    assert!(serde_json::from_value::<EventPayload>(unknown).is_err());
}

#[test]
fn x7_a_geometry_observation_names_the_display_map_it_used() {
    let ids = ids();
    // Model 392 §4.2's examples: base 1280x720 on 1280x720 (identity), on 1920x1080 (scale
    // 3/2), on 2340x1080 by height (translate, scale frame 1560x720).
    let identity = json!({
        "kind": "identity",
        "scale_num": 1,
        "scale_den": 1,
        "scale_width": 1280,
        "scale_height": 720,
        "base_width": 1280,
        "base_height": 720,
        "scale_by": "height",
        "scale_area": "screen",
        "anchor_area": "screen",
        "declared": false,
    });
    let map = |edits: &[(&str, Value)]| {
        let mut value = identity.clone();
        for (member, replacement) in edits {
            value[*member] = replacement.clone();
        }
        value
    };
    let scale = map(&[
        ("kind", json!("scale")),
        ("scale_num", json!(3)),
        ("scale_den", json!(2)),
    ]);
    let translate = map(&[
        ("kind", json!("translate")),
        ("scale_num", json!(3)),
        ("scale_den", json!(2)),
        ("scale_width", json!(1560)),
        ("declared", json!(true)),
    ]);
    let safe_area = map(&[
        ("kind", json!("translate")),
        ("scale_num", json!(3)),
        ("scale_den", json!(2)),
        ("scale_width", json!(1560)),
        ("scale_area", json!("safe_area")),
        ("anchor_area", json!("safe_area")),
        ("insets_source", json!("reported")),
        ("declared", json!(true)),
    ]);
    let plain = observed(&ids, TaskGeometryPhase::Initial, None, None, None);
    assert!(!plain.to_string().contains("display_map"));
    let with_map = |value: &Value| {
        let mut stored = plain.clone();
        observation_of(&mut stored).insert("display_map".to_owned(), value.clone());
        stored
    };
    for good in [&identity, &scale, &translate, &safe_area] {
        let payload = read_back(&with_map(good));
        let EventPayload::Task(TaskPayload::Semantic(semantic)) = &payload else {
            panic!("a task record");
        };
        let TaskSemanticFact::GeometryObserved { observation } = semantic.fact() else {
            panic!("a geometry record");
        };
        assert_eq!(observation.display_map.map(token), Some(good.clone()));
    }
    let typed = observed(
        &ids,
        TaskGeometryPhase::Initial,
        None,
        None,
        Some(TaskGeometryDisplayMap {
            kind: DisplayMapKind::Translate,
            scale_num: 3,
            scale_den: 2,
            scale_width: 1560,
            scale_height: 720,
            base_width: 1280,
            base_height: 720,
            scale_by: DisplayScaleBy::Height,
            scale_area: DisplayArea::Screen,
            anchor_area: DisplayArea::Screen,
            insets_source: None,
            declared: true,
        }),
    );
    assert_eq!(observation_of(&mut typed.clone())["display_map"], translate);

    let bad = [
        // identity is the factor 1 on the base size
        map(&[("scale_num", json!(3)), ("scale_den", json!(2))]),
        // a pure scale keeps the base size
        map(&[
            ("kind", json!("scale")),
            ("scale_num", json!(3)),
            ("scale_den", json!(2)),
            ("scale_width", json!(1560)),
        ]),
        // a translation needs a declaration
        map(&[
            ("kind", json!("translate")),
            ("scale_num", json!(3)),
            ("scale_den", json!(2)),
            ("scale_width", json!(1560)),
        ]),
        // the factor is reduced and never below 1
        map(&[
            ("kind", json!("scale")),
            ("scale_num", json!(6)),
            ("scale_den", json!(4)),
        ]),
        map(&[
            ("kind", json!("scale")),
            ("scale_num", json!(2)),
            ("scale_den", json!(3)),
        ]),
        // a size is never 0
        map(&[("scale_den", json!(0))]),
        // the safe area names its insets' source
        map(&[
            ("kind", json!("translate")),
            ("scale_num", json!(3)),
            ("scale_den", json!(2)),
            ("scale_width", json!(1560)),
            ("anchor_area", json!("safe_area")),
            ("declared", json!(true)),
        ]),
    ];
    for value in &bad {
        refused(with_map(value));
    }
}

#[test]
fn x8_a_changed_observation_names_the_change_and_a_recheck_a_failure() {
    let ids = ids();
    for (trigger, spelling) in [
        (TaskGeometryRecheckTrigger::ExtentChanged, "extent_changed"),
        (
            TaskGeometryRecheckTrigger::RotationChanged,
            "rotation_changed",
        ),
        (TaskGeometryRecheckTrigger::InsetsChanged, "insets_changed"),
    ] {
        let stored = observed(&ids, TaskGeometryPhase::Changed, Some(trigger), None, None);
        let observation = observation_of(&mut stored.clone()).clone();
        assert_eq!(observation["phase"], json!("changed"));
        assert_eq!(observation["trigger"], json!(spelling));
        read_back(&stored);
    }

    let changed = observed(
        &ids,
        TaskGeometryPhase::Changed,
        Some(TaskGeometryRecheckTrigger::ExtentChanged),
        None,
        None,
    );
    let edited = |edits: &[(&str, Value)]| {
        let mut stored = changed.clone();
        let observation = observation_of(&mut stored);
        for (member, value) in edits {
            observation.insert((*member).to_owned(), value.clone());
        }
        stored
    };
    let failure = json!({"outcome": outcome("recognition_failed", "error"), "event_id": null});
    for bad in [
        // a change names what changed, never a failure
        edited(&[("trigger", json!("recognition_failed"))]),
        edited(&[("trigger", Value::Null)]),
        edited(&[("original_failure", failure.clone())]),
        edited(&[("frame", Value::Null)]),
        // a recheck follows a failure, never a change
        edited(&[("phase", json!("recheck")), ("original_failure", failure)]),
    ] {
        let error = refused(bad);
        assert_eq!(
            error.code(),
            SanitizationCode::InvalidTaskGeometryPhase.code()
        );
    }
}

#[test]
fn x9_a_diagnostic_target_row_records_its_correction() {
    let row = json!({
        "index": 3,
        "frame_id": null,
        "step_action_id": null,
        "physical_action_id": null,
        "parent_index": 1,
        "kind": "target",
        "data": {
            "id": "anchor-a",
            "kind": "template",
            "passed": true,
            "template": {
                "x": 150,
                "y": 40,
                "width": 100,
                "height": 40,
                "raw_score": 0.875,
                "score": 0.875,
                "threshold": 0.75,
            },
            "color": null,
            "source": {
                "role": "required",
                "group_index": null,
                "target_index": 0,
                "target_id": "anchor-a",
                "passed": true,
            },
            "correction": {"dx": -6, "dy": 4},
        },
    });
    let read: TaskDiagnosticRecord = serde_json::from_value(row.clone()).expect("the row decodes");
    let TaskDiagnosticPayload::Target(target) = &read.payload else {
        panic!("a target row");
    };
    assert_eq!(
        target.correction,
        Some(TaskDiagnosticCorrectionData { dx: -6, dy: 4 })
    );
    assert_eq!(token(&read), row);

    let mut plain = row.clone();
    plain["data"]
        .as_object_mut()
        .expect("a target row")
        .remove("correction");
    let read: TaskDiagnosticRecord =
        serde_json::from_value(plain.clone()).expect("the row decodes");
    assert_eq!(token(&read), plain);

    let mut unknown = row;
    unknown["data"]["correction"]["radius_px"] = json!(16);
    assert!(serde_json::from_value::<TaskDiagnosticRecord>(unknown).is_err());
}

#[test]
fn x11_x12_a_centre_hotspot_draw_names_its_algorithm_and_parameters() {
    assert_eq!(
        token(InputSamplingAlgorithm::CenterHotspotNormalV1),
        json!("center_hotspot_normal_v1")
    );
    assert_eq!(
        token(InputSamplingAlgorithm::CenterHotspotLeastRecentV1),
        json!("center_hotspot_least_recent_v1")
    );
    // Today's writer records the uniform draw without parameters.
    let uniform = written(
        TaskPayloadDraft::semantic_with_sampling(
            TaskSemanticFact::EffectIntent {
                frame_extent: None,
                step_index: 3,
                operation_label: "collect".to_owned(),
                action: InputAction::Tap { x: 30, y: 40 },
            },
            InputSamplingEvidence::new(
                77,
                vec![InputSamplingRegion::new(10, 20, 40, 40).expect("a region")],
            )
            .expect("sampling evidence"),
            AuditInput::new(),
        )
        .into(),
    );
    read_back(&uniform);
    assert!(
        holder(&mut uniform.clone(), "action_seed")
            .expect("the sampling")
            .get("parameters")
            .is_none()
    );
    let sampled = |algorithm: &str, parameters: Option<Value>| {
        let mut stored = uniform.clone();
        let sampling = holder(&mut stored, "action_seed").expect("the sampling");
        sampling.insert("algorithm".to_owned(), json!(algorithm));
        if let Some(parameters) = parameters {
            sampling.insert("parameters".to_owned(), parameters);
        }
        stored
    };

    let normal = json!({"sigma_ratio_milli": 250, "max_redraws": 8});
    let least_recent = json!({
        "sigma_ratio_milli": 250,
        "max_redraws": 8,
        "candidate_count": 4,
        "chosen_candidate": 2,
    });
    let payload = read_back(&sampled("center_hotspot_normal_v1", Some(normal.clone())));
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = &payload else {
        panic!("a task record");
    };
    let sampling = semantic.sampling().expect("the sampling");
    assert_eq!(
        sampling.algorithm(),
        InputSamplingAlgorithm::CenterHotspotNormalV1
    );
    assert_eq!(
        sampling.parameters().map(|read| read.sigma_ratio_milli),
        Some(250)
    );
    let payload = read_back(&sampled(
        "center_hotspot_least_recent_v1",
        Some(least_recent.clone()),
    ));
    let EventPayload::Task(TaskPayload::Semantic(semantic)) = &payload else {
        panic!("a task record");
    };
    assert_eq!(
        semantic
            .sampling()
            .and_then(InputSamplingEvidence::parameters)
            .map(|read| (read.candidate_count, read.chosen_candidate)),
        Some((Some(4), Some(2)))
    );

    let mut chosen_past_the_end = least_recent.clone();
    chosen_past_the_end["chosen_candidate"] = json!(4);
    let mut no_spread = normal.clone();
    no_spread["sigma_ratio_milli"] = json!(0);
    for bad in [
        sampled("xorshift64_uniform_rect_v1", Some(normal.clone())),
        sampled("center_hotspot_normal_v1", None),
        sampled("center_hotspot_normal_v1", Some(least_recent)),
        sampled("center_hotspot_normal_v1", Some(no_spread)),
        sampled("center_hotspot_least_recent_v1", Some(normal)),
        sampled("center_hotspot_least_recent_v1", Some(chosen_past_the_end)),
    ] {
        refused(bad);
    }
}

#[test]
fn x13_adb_recovery_evidence_may_carry_outcomes_instead_of_text() {
    let ids = ids();
    let owner = *ids.mint_owner_epoch().expect("owner epoch").transport();
    let recovery = written(
        RuntimePayloadDraft::adb_target_recovery(
            owner,
            AdbTargetRecovery {
                endpoint: Some(LifecycleNativeDetail::new(
                    "[device:handset:neutral]",
                    false,
                )),
                initial_error: Some(LifecycleNativeDetail::new("device offline", false)),
                path: AdbRecoveryPath::Connect,
                budget_ms: 1_000,
                steps: vec![AdbRecoveryStep {
                    phase: AdbRecoveryPhase::InitialState,
                    attempt: 1,
                    elapsed_ms: 10,
                    command: Some(AdbCommandEvidence {
                        succeeded: false,
                        exit_code: Some(1),
                        stdout: None,
                        stderr: Some(LifecycleNativeDetail::new("error: device offline", false)),
                        stdout_lossy_decode: false,
                        stderr_lossy_decode: false,
                        state: AdbTransportState::Offline,
                        raw_text_redacted: false,
                        outcome: None,
                    }),
                    error: None,
                    outcome: None,
                }],
                final_state: AdbTransportState::Offline,
                recovered: false,
                dropped_count: 0,
                outcome: None,
            },
        )
        .into(),
    );
    read_back(&recovery);
    // Today's writer keeps the text members and writes no outcome.
    assert!(!recovery.to_string().contains("\"outcome\""));

    let foreign = |sensitivity: &str| {
        json!({
            "code": "foreign_adb_error",
            "category": "error",
            "values": {
                "raw_source": "adb",
                "raw_text": "error: device offline",
                "raw_text_sensitivity": sensitivity,
            },
        })
    };
    let mut moved = recovery.clone();
    let report = holder(&mut moved, "initial_error").expect("the recovery report");
    report.remove("endpoint");
    report.remove("initial_error");
    report.insert("outcome".to_owned(), foreign("sensitive"));
    let command = holder(&mut moved, "stderr").expect("the command evidence");
    command.insert("stderr".to_owned(), Value::Null);
    command.insert("outcome".to_owned(), foreign("secret"));
    holder(&mut moved, "elapsed_ms")
        .expect("the step")
        .insert("outcome".to_owned(), foreign("internal"));
    let payload = read_back(&moved);
    let EventPayload::Runtime(RuntimePayload::LifecycleObserved(recorded)) = &payload else {
        panic!("a lifecycle record");
    };
    let read = recorded.adb_recovery().expect("the recovery report");
    assert_eq!(read.endpoint, None);
    assert_eq!(read.initial_error, None);
    assert!(read.outcome.is_some());
    // The record is as sensitive as the foreign text on its evidence.
    assert_eq!(payload.sensitivity(), Sensitivity::Secret);

    // The error that started the recovery is recorded, as text or as an outcome.
    let mut unstarted = moved;
    holder(&mut unstarted, "budget_ms")
        .expect("the recovery report")
        .remove("outcome");
    refused(unstarted);
}

#[test]
fn x14_a_dispatch_intent_records_its_random_delay() {
    let intent =
        written(PolicyPayloadDraft::dispatch_intent(dispatch_data(), AuditInput::new()).into());
    assert!(intent["payload"]["data"].get("random_delay").is_none());
    let delay = |due: u64, planned: u64, delay_ms: u64| {
        json!({
            "due_at_unix_ms": due,
            "planned_start_unix_ms": planned,
            "delay_ms": delay_ms,
        })
    };
    let payload = read_back(&with(
        intent.clone(),
        "random_delay",
        delay(AT_MS, AT_MS + 600_000, 600_000),
    ));
    let EventPayload::Policy(PolicyPayload::DispatchIntent(dispatch)) = &payload else {
        panic!("a dispatch intent");
    };
    assert_eq!(
        dispatch.random_delay(),
        Some(&PolicyRandomDelay {
            due_at_unix_ms: AT_MS,
            planned_start_unix_ms: AT_MS + 600_000,
            delay_ms: 600_000,
        })
    );
    // A delay shortened to nothing by the allowed period still reads.
    read_back(&with(
        intent.clone(),
        "random_delay",
        delay(AT_MS, AT_MS, 0),
    ));

    for bad in [
        delay(AT_MS, AT_MS + 600_001, 600_000),
        delay(0, 600_000, 600_000),
        delay(
            AT_MS,
            AT_MS + MAX_POLICY_RANDOM_DELAY_MS + 1,
            MAX_POLICY_RANDOM_DELAY_MS + 1,
        ),
    ] {
        let error = refused(with(intent.clone(), "random_delay", bad));
        assert_eq!(error.code(), SanitizationCode::InvalidPolicyPayload.code());
    }

    // Only a dispatch intent carries it.
    let admitted = written(
        PolicyPayloadDraft::dispatch_admitted(dispatch_data(), admission(), AuditInput::new())
            .into(),
    );
    let error = refused(with(
        admitted,
        "random_delay",
        delay(AT_MS, AT_MS + 600_000, 600_000),
    ));
    assert_eq!(error.code(), SanitizationCode::InvalidPolicyPayload.code());
}
