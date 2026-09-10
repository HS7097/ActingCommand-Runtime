// SPDX-License-Identifier: AGPL-3.0-only

#[test]
fn successful_recognize_only_run_uses_injected_ledger_and_lazy_device_ports() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let out = temp.path().join("out.zip");
    let mut request = test_run_request(zip, out.clone(), temp.path());
    request.instance = Some("fixture".to_string());
    let resolver = Arc::new(DeviceResolverCounters::default());
    request.device_resolver = test_device_resolver("fixture", "fixture", resolver.clone());
    let mut lab = test_lab(temp.path());

    let response = lab.lab_run(request).expect("successful Lab run");

    assert!(response.ok);
    assert!(out.is_file());
    assert_eq!(lab.ports().capture.opens.load(Ordering::SeqCst), 1);
    assert_eq!(lab.ports().ledger.run_starts.load(Ordering::SeqCst), 1);
    assert!(lab.ports().ledger.run_records.load(Ordering::SeqCst) >= 3);
    assert!(lab.ports().ledger.run_events.load(Ordering::SeqCst) >= 1);
    assert!(lab.ports().ledger.run_reads.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        *resolver.resolved_ids.lock().expect("resolved ids"),
        vec!["fixture".to_string()]
    );
    assert_eq!(resolver.provenance.load(Ordering::SeqCst), 1);
    assert_eq!(resolver.capture.load(Ordering::SeqCst), 1);
    assert_eq!(resolver.touch.load(Ordering::SeqCst), 1);
}

#[test]
fn recovery_suggestion_is_terminal_and_never_chains_the_successor_task() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_recovery_suggestion_lab_package(&zip);
    let out = temp.path().join("out.zip");
    let mut request = test_run_request(zip, out.clone(), temp.path());
    request.instance = Some("fixture".to_string());
    request.device_resolver = test_device_resolver(
        "fixture",
        "fixture",
        Arc::new(DeviceResolverCounters::default()),
    );
    let mut lab = test_lab(temp.path());

    let error = lab.lab_run(request).expect_err("successor suggestion");

    assert_eq!(error.code, "successor_suggested");
    assert_eq!(
        error
            .details
            .as_ref()
            .and_then(|details| details.pointer("/suggestion/task_id"))
            .and_then(Value::as_str),
        Some("return_home")
    );
    assert_eq!(lab.ports().input.opens.load(Ordering::SeqCst), 1);
    let file = File::open(out).expect("failure zip");
    let mut archive = ZipArchive::new(file).expect("failure archive");
    let events = zip_text(&mut archive, "logs/events.jsonl");
    assert_ordered(
        &events,
        &[
            "operation_recovery_required",
            "run_terminal_decided",
            "successor_suggested",
            "run_failed",
        ],
    );
    assert_eq!(events.matches("\"event\":\"click_started\"").count(), 1);
    assert!(!events.contains("recovery_started"));
    assert!(!events.contains("recovery_result"));
}

#[test]
fn selected_provenance_failure_precedes_context_and_normal_ledger_creation() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let out = temp.path().join("out.zip");
    let mut request = test_run_request(zip, out.clone(), temp.path());
    request.instance = Some("fixture".to_string());
    let mut lab = test_lab(temp.path());
    let ledger_starts = lab.ports().ledger.run_starts.clone();
    let counters = Arc::new(DeviceResolverCounters::default());
    request.device_resolver = Box::new(TestDeviceResolver {
        selected: test_selected_device("fixture", "fixture"),
        counters: counters.clone(),
        failure: Some(SelectedConfigFailure::Provenance),
        ledger_starts: Some(ledger_starts),
    });
    let run_root = request.run_root.clone();

    let error = lab.lab_run(request).expect_err("provenance failure");

    assert!(
        error
            .message
            .contains("synthetic global provenance failure")
    );
    assert_eq!(
        *counters
            .validation_ledger_starts
            .lock()
            .expect("validation ledger starts"),
        vec![0]
    );
    assert_eq!(lab.ports().capture.opens.load(Ordering::SeqCst), 0);
    let file = File::open(&out).expect("failure zip");
    let mut archive = ZipArchive::new(file).expect("failure archive");
    let summary: Value =
        serde_json::from_reader(archive.by_name("logs/summary.json").expect("summary"))
            .expect("summary json");
    assert!(summary.get("instance").expect("instance field").is_null());
    let shard_runs = run_root
        .join("runtime-ledger")
        .join("instances")
        .join("unknown")
        .join("runs");
    let shard = fs::read_dir(shard_runs)
        .expect("ledger runs")
        .next()
        .expect("ledger shard")
        .expect("ledger shard entry")
        .path()
        .join("ledger.jsonl");
    let ledger = LabLedger::read(shard).expect("ledger readback");
    assert_eq!(
        ledger
            .header
            .as_ref()
            .map(|header| header.instance.as_str()),
        Some("unknown")
    );
}

#[test]
fn selected_invalid_touch_rejects_before_ledger_or_runtime_effects() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let out = temp.path().join("out.zip");
    let mut request = test_run_request(zip, out.clone(), temp.path());
    request.instance = Some("fixture".to_string());
    let mut lab = test_lab(temp.path());
    let ledger_starts = lab.ports().ledger.run_starts.clone();
    let counters = Arc::new(DeviceResolverCounters::default());
    request.device_resolver = Box::new(TestDeviceResolver {
        selected: test_selected_device("fixture", "fixture"),
        counters: counters.clone(),
        failure: Some(SelectedConfigFailure::Touch),
        ledger_starts: Some(ledger_starts),
    });

    let error = lab
        .lab_run(request)
        .expect_err("selected touch configuration must be validated");

    assert!(error.message.contains("synthetic selected touch failure"));
    assert_eq!(
        *counters
            .validation_ledger_starts
            .lock()
            .expect("validation ledger starts"),
        vec![0]
    );
    assert_eq!(lab.ports().capture.opens.load(Ordering::SeqCst), 0);
    assert!(out.is_file());
}

#[test]
fn selected_invalid_capture_preserves_pre_effect_failure_archive_and_ledger_order() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let out = temp.path().join("out.zip");
    let mut request = test_run_request(zip, out.clone(), temp.path());
    request.instance = Some("fixture".to_string());
    let run_root = request.run_root.clone();
    let mut lab = test_lab(temp.path());
    let ledger_starts = lab.ports().ledger.run_starts.clone();
    let counters = Arc::new(DeviceResolverCounters::default());
    request.device_resolver = Box::new(TestDeviceResolver {
        selected: test_selected_device("fixture", "fixture"),
        counters: counters.clone(),
        failure: Some(SelectedConfigFailure::Capture),
        ledger_starts: Some(ledger_starts),
    });

    let error = lab
        .lab_run(request)
        .expect_err("selected capture configuration must fail");

    assert!(error.message.contains("synthetic selected capture failure"));
    assert_eq!(
        *counters
            .validation_ledger_starts
            .lock()
            .expect("validation ledger starts"),
        vec![0]
    );
    assert_eq!(lab.ports().capture.opens.load(Ordering::SeqCst), 0);

    let file = File::open(&out).expect("failure zip");
    let mut archive = ZipArchive::new(file).expect("failure archive");
    let summary: Value =
        serde_json::from_reader(archive.by_name("logs/summary.json").expect("summary"))
            .expect("summary json");
    assert!(summary.get("instance").expect("instance field").is_null());

    let shard = fs::read_dir(
        run_root
            .join("runtime-ledger")
            .join("instances")
            .join("unknown")
            .join("runs"),
    )
    .expect("unknown ledger runs")
    .next()
    .expect("ledger shard")
    .expect("ledger shard entry")
    .path()
    .join("ledger.jsonl");
    let ledger = LabLedger::read(shard).expect("ledger readback");
    assert_eq!(
        ledger
            .header
            .as_ref()
            .map(|header| header.instance.as_str()),
        Some("unknown")
    );
    let events = ledger
        .events
        .iter()
        .filter_map(|event| event.payload.get("event").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            "run_started",
            "input_unpacked",
            "producer_missing",
            "control_loaded",
            "resources_loaded",
            "run_failed",
            "frame_store_materialized",
            "output_zip_written",
        ]
    );
    let record_types = ledger
        .records
        .iter()
        .filter_map(|record| record.payload.get("record_type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        record_types,
        ["lab_run_dispatch", "finalizing", "finish_error"]
    );
}

#[test]
fn rejects_missing_control_and_writes_failure_zip() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_test_zip(&zip, &[("resources/manifest.json", br#"{}"#)]);
    let out = temp.path().join("out.zip");
    let mut lab = test_lab(temp.path());
    let result = run_lab(&mut lab, test_run_request(zip, out.clone(), temp.path()));

    assert_eq!(result.expect_err("missing control").code, "package_invalid");
    assert!(out.is_file());
}

#[test]
fn lab_validate_accepts_minimal_self_contained_package() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);

    let response = validate_lab_package_zip(&zip).expect("valid package");

    assert_eq!(response.status, "valid");
    assert_eq!(response.hash_source, "self_computed_provenance_only");
    assert!(!response.externally_verified);
    assert_eq!(response.control.entry_task_id, "task");
    assert_eq!(response.resources.operation_count, 1);
}

#[test]
fn lab_validate_reports_an_externally_supplied_hash_as_verified() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let expected = Sha256Hash::digest(&fs::read(&zip).expect("package bytes"));

    let response = validate_lab_package_zip_with_expected(&zip, Some(expected))
        .expect("externally verified package");

    assert_eq!(response.input_sha256, expected.to_string());
    assert_eq!(response.hash_source, "externally_supplied");
    assert!(response.externally_verified);
}

#[test]
fn lab_validate_rejects_expected_sha256_mismatch() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);

    let result = validate_lab_package_zip_with_expected(
        &zip,
        Some(
            Sha256Hash::parse_hex(
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .expect("hash"),
        ),
    );

    assert_eq!(result.expect_err("hash mismatch").code, "package_invalid");
}

#[test]
fn lab_run_rejects_external_hash_mismatch_before_device_resolution() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let out = temp.path().join("out.zip");
    let mut request = test_run_request(zip, out.clone(), temp.path());
    request.instance = Some("fixture".to_string());
    request.expected_input_sha256 = ExternalExpectedSha256::parse_hex(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .expect("external hash");
    let resolver = Arc::new(DeviceResolverCounters::default());
    request.device_resolver = test_device_resolver("fixture", "fixture", resolver.clone());
    let mut lab = test_lab(temp.path());

    let error = lab.lab_run(request).expect_err("hash mismatch");

    assert_eq!(error.code, "package_invalid");
    assert!(error.message.contains("hash mismatch"));
    assert!(
        resolver
            .resolved_ids
            .lock()
            .expect("resolved ids")
            .is_empty()
    );
    assert_eq!(lab.ports().capture.opens.load(Ordering::SeqCst), 0);
    assert!(out.is_file());
}

#[test]
fn production_bundle_ignores_loose_neighbor_resources() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let loose = temp.path().join("resources/operations/task");
    fs::create_dir_all(&loose).expect("loose resource directory");
    fs::write(
        loose.join("task.json"),
        br#"{"task_id":"malicious-loose-resource"}"#,
    )
    .expect("loose resource");
    let expected = ExternalExpectedSha256::parse_hex(
        &Sha256Hash::digest(&fs::read(&zip).expect("read bundle")).to_string(),
    )
    .expect("external hash");

    let contained = load_lab_package_for_validation(&zip, "fixture", Some(expected.hash()))
        .expect("admitted bundle");

    assert_eq!(
        contained
            .bundle
            .operation()
            .get("task_id")
            .and_then(Value::as_str),
        Some("task")
    );
}

#[test]
fn configured_recovery_task_must_exist_inside_admitted_bundle() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);
    let expected = ExternalExpectedSha256::parse_hex(
        &Sha256Hash::digest(&fs::read(&zip).expect("read bundle")).to_string(),
    )
    .expect("external hash");
    let contained = load_lab_package_for_validation(&zip, "fixture", Some(expected.hash()))
        .expect("admitted bundle");
    let mut operation_bundle = test_operation_bundle(test_operation(Some("terminal"), None));
    operation_bundle.recovery = Some(TaskRecovery::Kind("return_home".to_string()));

    let error =
        validate_recovery_task_entries(&contained.bundle, &test_control(), &operation_bundle)
            .expect_err("missing recovery task");

    assert_eq!(error.code, "package_invalid");
    assert!(error.message.contains("return_home"));
}

#[test]
fn lab_validate_reports_unsupported_recognition_target_count() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_lab_package_with_unsupported_recognition(&zip);

    let response = validate_lab_package_zip(&zip).expect("valid package");

    assert_eq!(response.resources.recognition_unsupported_target_count, 1);
    assert_eq!(
        response.resources.recognition_unsupported_targets[0].id,
        "page/home"
    );
}

#[test]
fn lab_validate_rejects_missing_control() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_test_zip(&zip, &[("resources/manifest.json", br#"{}"#)]);

    let result = validate_lab_package_zip(&zip);

    assert_eq!(result.expect_err("missing control").code, "package_invalid");
}

#[test]
fn rejects_fullscreen_rect_unless_explicitly_allowed() {
    let control = LabControl {
        schema_version: CONTROL_SCHEMA.to_string(),
        package_id: "pkg".to_string(),
        execution_mode: "navigable_route".to_string(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        resolution: Resolution {
            width: 1280,
            height: 720,
        },
        entry_task_id: "task".to_string(),
        capture_interval_ms: None,
        _timeout_ms: None,
        _step_timeout_ms: None,
        _max_steps: None,
        _stop_on_error: None,
        _stop_on_confirmation: None,
        allow_placeholder_coords: None,
        _output: None,
        capture_backend: None,
        frame_store: FrameStoreControl::default(),
        _producer: None,
        _trusted_execution: None,
    };
    let click = OperationClick {
        kind: "rect".to_string(),
        x: Some(0),
        y: Some(0),
        width: Some(1280),
        height: Some(720),
        from_rect: None,
        to_rect: None,
        duration_ms: None,
        offset: None,
        target_id: None,
        extra: BTreeMap::new(),
    };

    let err = click.validate(&control).expect_err("fullscreen rejected");
    assert_eq!(err.code, "package_invalid");
}

#[test]
fn operation_validate_rejects_missing_coordinate_guard() {
    let control = test_control();
    let mut operation = test_operation(None, None);
    operation.unguarded_trusted_coordinate = false;

    let err = operation
        .validate(&control)
        .expect_err("missing guard must fail");

    assert_eq!(err.code, "package_invalid");
    assert!(err.message.contains("missing guard metadata"));
}

#[test]
fn operation_validate_allows_explicit_trusted_unguarded_coordinate() {
    let control = test_control();
    let operation = test_operation(None, None);

    operation
        .validate(&control)
        .expect("explicit trusted unguarded coordinate allowed");
}

#[test]
fn offset_click_rejects_color_probe_guard() {
    let control = test_control();
    let mut operation = test_operation(None, None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(OperationGuard {
        page_id: "home".to_string(),
        target_id: "target/button".to_string(),
        expected_rect: PackRect {
            x: 100,
            y: 200,
            width: 20,
            height: 30,
        },
        verify_template: None,
        color_probe: Some("target/button".to_string()),
    });
    operation.click = OperationClick {
        kind: "offset".to_string(),
        x: None,
        y: None,
        width: None,
        height: None,
        from_rect: None,
        to_rect: None,
        duration_ms: None,
        offset: Some(PackRect {
            x: 3,
            y: 4,
            width: 5,
            height: 6,
        }),
        target_id: Some("target/button".to_string()),
        extra: BTreeMap::new(),
    };

    let err = operation
        .validate(&control)
        .expect_err("color guard cannot drive offset");
    assert!(err.message.contains("color-probe guards cannot produce"));
}

#[test]
fn offset_click_rejects_mismatched_guard_target_at_action_time() {
    let control = test_control();
    let mut operation = test_operation(None, None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(OperationGuard {
        page_id: "home".to_string(),
        target_id: "target/button".to_string(),
        expected_rect: PackRect {
            x: 100,
            y: 200,
            width: 20,
            height: 30,
        },
        verify_template: Some("target/button".to_string()),
        color_probe: None,
    });
    operation.click = OperationClick {
        kind: "offset".to_string(),
        x: None,
        y: None,
        width: None,
        height: None,
        from_rect: None,
        to_rect: None,
        duration_ms: None,
        offset: Some(PackRect {
            x: 3,
            y: 4,
            width: 5,
            height: 6,
        }),
        target_id: Some("target/button".to_string()),
        extra: BTreeMap::new(),
    };

    operation.validate(&control).expect("offset valid");
    let err = operation
        .input_action(
            &control.resolution,
            0,
            Some(&template_target_evaluation(
                "target/other",
                PackRect {
                    x: 300,
                    y: 400,
                    width: 20,
                    height: 30,
                },
            )),
        )
        .expect_err("mismatched target must fail");

    assert!(err.message.contains("does not match guard target_id"));
}

#[test]
fn target_click_rejects_color_probe_guard() {
    let control = test_control();
    let mut operation = test_operation(Some("terminal"), None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(test_color_guard());
    operation.click = OperationClick {
        kind: "target".to_string(),
        x: None,
        y: None,
        width: None,
        height: None,
        from_rect: None,
        to_rect: None,
        duration_ms: None,
        offset: None,
        target_id: Some("target/button".to_string()),
        extra: BTreeMap::new(),
    };

    let err = operation
        .validate(&control)
        .expect_err("target click requires template guard");

    assert!(err.message.contains("requires template guard metadata"));
}

#[test]
fn segmented_swipe_validates_the_closed_three_point_declaration() {
    let control = test_control();
    let click_json = json!({
        "kind": "single_touch_drag_with_vertical_brake_v1",
        "from_rect": {"x": 1090, "y": 350, "width": 20, "height": 20},
        "corner_rect": {"x": 100, "y": 350, "width": 20, "height": 20},
        "horizontal_duration_ms": 200,
        "corner_hold_ms": 150,
        "brake_distance_px": 100,
        "brake_duration_ms": 200
    });
    let click: OperationClick =
        serde_json::from_value(click_json.clone()).expect("formal segmented swipe");
    click
        .validate_for_schema(&control, "0.7")
        .expect("closed segmented swipe");
    for (field, value) in [("x", json!(150)), ("unexpected", json!(true))] {
        let mut invalid = click_json.clone();
        invalid[field] = value;
        let invalid: OperationClick =
            serde_json::from_value(invalid).expect("syntactically valid closed-form probe");
        invalid
            .validate_for_schema(&control, "0.7")
            .expect_err("mixed or unknown segmented swipe fields must fail closed");
    }
}

#[test]
fn guarded_drag_rejects_mismatched_guard_target() {
    let control = test_control();
    let mut operation = test_operation(Some("terminal"), None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(OperationGuard {
        page_id: "home".to_string(),
        target_id: "target/thumb".to_string(),
        expected_rect: PackRect {
            x: 100,
            y: 200,
            width: 20,
            height: 30,
        },
        verify_template: Some("target/thumb".to_string()),
        color_probe: None,
    });
    operation.click = OperationClick {
        kind: "drag".to_string(),
        x: None,
        y: None,
        width: None,
        height: None,
        from_rect: Some(PackRect {
            x: 100,
            y: 200,
            width: 20,
            height: 30,
        }),
        to_rect: Some(PackRect {
            x: 800,
            y: 300,
            width: 10,
            height: 10,
        }),
        duration_ms: Some(500),
        offset: None,
        target_id: None,
        extra: BTreeMap::new(),
    };

    operation.validate(&control).expect("guarded drag valid");
    let err = operation
        .input_action(
            &control.resolution,
            0,
            Some(&template_target_evaluation(
                "target/other",
                PackRect {
                    x: 300,
                    y: 400,
                    width: 20,
                    height: 30,
                },
            )),
        )
        .expect_err("guard target mismatch must fail");

    assert!(err.message.contains("does not match guard target_id"));
}

#[test]
fn guarded_point_rejects_mismatched_guard_target() {
    let control = test_control();
    let mut operation = test_operation(Some("terminal"), None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(OperationGuard {
        page_id: "home".to_string(),
        target_id: "target/button".to_string(),
        expected_rect: PackRect {
            x: 100,
            y: 200,
            width: 20,
            height: 30,
        },
        verify_template: Some("target/button".to_string()),
        color_probe: None,
    });

    operation.validate(&control).expect("guarded point valid");
    let err = operation
        .input_action(
            &control.resolution,
            0,
            Some(&template_target_evaluation(
                "target/other",
                PackRect {
                    x: 300,
                    y: 400,
                    width: 20,
                    height: 30,
                },
            )),
        )
        .expect_err("target mismatch must fail");

    assert!(err.message.contains("does not match guard target_id"));
}

#[test]
fn operation_validate_allows_color_guard_for_absolute_coordinate() {
    let control = test_control();
    let mut operation = test_operation(Some("terminal"), None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(test_color_guard());

    operation
        .validate(&control)
        .expect("color guard can protect absolute coordinates");
}

#[test]
fn flow_policy_retries_navigation_only_but_not_side_effects_by_default() {
    let defaults = OperationDefaults::default();
    let mut navigation = test_operation(Some("terminal"), None);
    navigation.effect = Some("navigation_only".to_string());
    let navigation_policy = navigation.flow_policy(defaults);

    assert!(navigation_policy.retryable);
    assert_eq!(navigation_policy.max_attempts, 3);
    assert_eq!(
        navigation_policy.retry_interval_ms,
        DEFAULT_RETRY_INTERVAL_MS
    );

    let mut side_effect = test_operation(Some("terminal"), None);
    side_effect.consumes = vec!["ap".to_string()];
    side_effect.produces = vec!["reward".to_string()];
    let side_effect_policy = side_effect.flow_policy(defaults);

    assert!(!side_effect_policy.retryable);
    assert_eq!(side_effect_policy.max_attempts, 1);
}

#[test]
fn flow_policy_treats_structural_page_transition_as_navigation() {
    let defaults = OperationDefaults::default();
    let mut navigation = test_operation(Some("depot"), None);
    navigation.purpose = "Navigate from home to the Depot page".to_string();

    let navigation_policy = navigation.flow_policy(defaults);

    assert!(navigation_policy.retryable);
    assert_eq!(navigation_policy.max_attempts, 3);

    let mut no_target_page = test_operation(None, None);
    no_target_page.purpose = "Navigate from home to nowhere".to_string();
    let no_target_policy = no_target_page.flow_policy(defaults);

    assert!(!no_target_policy.retryable);
    assert_eq!(no_target_policy.max_attempts, 1);
}

#[test]
fn flow_policy_explicit_retryable_uses_bounded_attempts_and_cadence() {
    let defaults = OperationDefaults {
        max_attempts: Some(5),
        retry_interval_ms: Some(250),
        post_wait_freezes_ms: Some(700),
        ..OperationDefaults::default()
    };
    let mut operation = test_operation(Some("terminal"), None);
    operation.retryable = Some(true);
    operation.max_attempts = Some(2);

    let policy = operation.flow_policy(defaults);

    assert!(policy.retryable);
    assert_eq!(policy.max_attempts, 2);
    assert_eq!(policy.retry_interval_ms, 250);
    assert_eq!(policy.post_wait_freezes_ms, 700);
}

#[test]
fn recovery_configuration_uses_implicit_return_home_when_available() {
    let operation = test_operation(Some("terminal"), None);
    let mut bundle = test_operation_bundle(operation.clone());

    assert_eq!(operation_recovery_task_id(&bundle, &operation, false), None);
    assert_eq!(
        operation_recovery_task_id(&bundle, &operation, true).as_deref(),
        Some(DEFAULT_RECOVERY_TASK_ID)
    );

    bundle.recovery = Some(TaskRecovery::Kind(DEFAULT_RECOVERY_TASK_ID.to_string()));
    assert_eq!(
        operation_recovery_task_id(&bundle, &operation, false).as_deref(),
        Some(DEFAULT_RECOVERY_TASK_ID)
    );

    bundle.recovery = None;
    let mut operation_with_error_handler = operation.clone();
    operation_with_error_handler.on_error = Some(DEFAULT_RECOVERY_TASK_ID.to_string());
    assert_eq!(
        operation_recovery_task_id(&bundle, &operation_with_error_handler, false).as_deref(),
        Some(DEFAULT_RECOVERY_TASK_ID)
    );
}

#[test]
fn operation_bundle_accepts_schema_0_6_retry_recovery_fields() {
    let control = test_control();
    let bundle: OperationBundle = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "arknights",
        "server_scope": ["cn"],
        "goal": "navigation",
        "coordinate_space": {"width": 1280, "height": 720},
        "defaults": {
            "max_attempts": 2,
            "retry_interval_ms": 100,
            "post_wait_freezes_ms": 0
        },
        "entry_page": "home",
        "target_page": "terminal",
        "error_pages": ["negative_popup"],
        "recovery": {"kind": "return_home", "task_id": "return_home"},
        "max_task_retries": 1,
        "on_exhausted": "pause",
        "operations": [{
            "id": "open_terminal",
            "purpose": "navigation",
            "from": "home",
            "to": "terminal",
            "effect": "navigation_only",
            "retryable": true,
            "click": {"kind": "point", "x": 100, "y": 100},
            "unguarded_trusted_coordinate": true
        }]
    }))
    .expect("operation bundle");

    bundle
        .validate(&control, |_relative| Ok(true))
        .expect("schema 0.6 flow fields valid");
    assert_eq!(bundle.recovery.as_ref().unwrap().task_id(), "return_home");
    assert_eq!(bundle.defaults.max_attempts, Some(2));
}

#[test]
fn lab_bundle_normalizes_finite_target_and_destination_sets() {
    let control = test_control();
    let bundle: OperationBundle = serde_json::from_value(json!({
        "schema_version": "0.6",
        "task_id": "task",
        "game": "arknights",
        "server_scope": ["cn"],
        "coordinate_space": {"width": 1280, "height": 720},
        "target_page": ["terminal", "alternate"],
        "operations": [{
            "id": "open_terminal",
            "purpose": "navigation",
            "from": "home",
            "to": ["terminal", "alternate"],
            "click": {"kind": "point", "x": 100, "y": 100},
            "unguarded_trusted_coordinate": true
        }]
    }))
    .expect("finite sets");

    bundle
        .validate(&control, |_relative| Ok(true))
        .expect("finite set bundle");
    assert_eq!(
        bundle.target_page.as_ref().expect("target").as_slice(),
        ["alternate", "terminal"]
    );
    assert_eq!(
        bundle.operations[0]
            .destination_pages()
            .expect("destinations"),
        ["alternate", "terminal"]
    );
}

#[test]
fn normalized_page_set_serialization_preserves_singleton_and_complete_multi_shape() {
    assert_eq!(
        serde_json::to_string(&NormalizedPageSet(vec!["terminal".to_string()])).expect("singleton"),
        r#""terminal""#
    );
    assert_eq!(
        serde_json::to_string(&NormalizedPageSet(vec![
            "alternate".to_string(),
            "terminal".to_string(),
        ]))
        .expect("multi"),
        r#"["alternate","terminal"]"#
    );
}

#[test]
fn expect_after_without_to_does_not_infer_retry() {
    let mut operation = test_operation(None, None);
    operation.expect_after = Some(OperationExpectation {
        page_id: NormalizedPageSet(vec!["alternate".to_string(), "terminal".to_string()]),
        timeout_ms: Some(50),
        interval_ms: Some(5),
    });

    let policy = operation.flow_policy(OperationDefaults::default());

    assert!(!policy.retryable);
    assert_eq!(policy.max_attempts, 1);
    assert_eq!(
        operation.destination_pages().expect("destinations"),
        ["alternate", "terminal"]
    );
}

#[test]
fn malformed_page_sets_fail_during_lab_parse() {
    for value in [json!([]), json!(["terminal", "terminal"]), json!([""])] {
        assert!(
            serde_json::from_value::<NormalizedPageSet>(value).is_err(),
            "malformed page set must fail"
        );
    }
}
