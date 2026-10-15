// SPDX-License-Identifier: AGPL-3.0-only

#[test]
fn lab_validate_accepts_minimal_self_contained_package() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);

    let response = validate_lab_package_zip(&zip).expect("valid package");

    assert_eq!(response.status, "valid");
    assert_eq!(response.control.entry_task_id, "task");
    assert_eq!(response.resources.operation_count, 1);
}

// Workflow #336 PKG-S4 (option 丙, M-PKG v1.1 §4.5): Lab validation admits the package
// structurally and names no digest of it.
#[test]
fn lab_validate_names_no_digest_of_the_package() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_minimal_lab_package(&zip);

    let response = validate_lab_package_zip(&zip).expect("valid package");
    let encoded = serde_json::to_value(&response).expect("encode the response");

    for member in ["input_sha256", "hash_source", "externally_verified"] {
        assert!(encoded.get(member).is_none(), "{member}: {encoded}");
    }
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

    let contained = load_lab_package_for_validation(&zip, "fixture").expect("admitted bundle");

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
    let contained = load_lab_package_for_validation(&zip, "fixture").expect("admitted bundle");
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
        check: None,
    });
    operation.click = Some(OperationClick {
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
    });

    let err = operation
        .validate(&control)
        .expect_err("color guard cannot drive offset");
    assert!(err.message.contains("color-probe guards cannot produce"));
}

#[test]
fn target_click_rejects_color_probe_guard() {
    let control = test_control();
    let mut operation = test_operation(Some("terminal"), None);
    operation.unguarded_trusted_coordinate = false;
    operation.guard = Some(test_color_guard());
    operation.click = Some(OperationClick {
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
    });

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
fn malformed_page_sets_fail_during_lab_parse() {
    for value in [json!([]), json!(["terminal", "terminal"]), json!([""])] {
        assert!(
            serde_json::from_value::<NormalizedPageSet>(value).is_err(),
            "malformed page set must fail"
        );
    }
}
