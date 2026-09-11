// SPDX-License-Identifier: AGPL-3.0-only

fn test_control() -> LabControl {
    LabControl {
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
    }
}

fn test_operation(to: Option<&str>, verify_template: Option<&str>) -> Operation {
    Operation {
        id: "open_terminal".to_string(),
        _purpose: "test".to_string(),
        from: "home".to_string(),
        to: to.map(|page| NormalizedPageSet(vec![page.to_string()])),
        click: OperationClick {
            kind: "point".to_string(),
            x: Some(100),
            y: Some(100),
            width: None,
            height: None,
            from_rect: None,
            to_rect: None,
            duration_ms: None,
            offset: None,
            target_id: None,
            extra: BTreeMap::new(),
        },
        verify_template: verify_template.map(str::to_string),
        expect_after: None,
        timeout_ms: None,
        max_attempts: None,
        retry_interval_ms: None,
        _pre_delay_ms: None,
        _post_delay_ms: None,
        _pre_wait_freezes_ms: None,
        _post_wait_freezes_ms: None,
        _retryable: None,
        effect: None,
        on_error: None,
        guard: None,
        unguarded_trusted_coordinate: true,
        _consumes: Vec::new(),
        _produces: Vec::new(),
        _verified_live: None,
        _provenance: None,
    }
}

fn test_operation_bundle(operation: Operation) -> OperationBundle {
    OperationBundle {
        schema_version: "0.3".to_string(),
        task_id: "task".to_string(),
        game: "arknights".to_string(),
        server_scope: vec!["cn".to_string()],
        _goal: "test".to_string(),
        coordinate_space: Resolution {
            width: 1280,
            height: 720,
        },
        defaults: OperationDefaults::default(),
        anchors: Vec::new(),
        _entry_page: Some("home".to_string()),
        target_page: Some(NormalizedPageSet(vec!["terminal".to_string()])),
        _error_pages: Vec::new(),
        recovery: None,
        max_task_retries: None,
        on_exhausted: None,
        _page_rules: BTreeMap::new(),
        operations: vec![operation],
    }
}

fn test_color_guard() -> OperationGuard {
    OperationGuard {
        page_id: "home".to_string(),
        target_id: "target/button".to_string(),
        expected_rect: PackRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        verify_template: None,
        color_probe: Some("target/button".to_string()),
    }
}

fn color_target_evaluation(id: &str, mean: [u8; 3], passed: bool) -> TargetEvaluation {
    TargetEvaluation {
        id: id.to_string(),
        kind: TargetKind::Color,
        passed,
        template: None,
        color: Some(actingcommand_recognition_pack::ColorEvaluation {
            distance: 0.0,
            max_distance: 20.0,
            mean,
            expected: mean,
            region: None,
        }),
        ocr: None,
        nn: None,
        message: if passed {
            "color passed".to_string()
        } else {
            "color failed".to_string()
        },
    }
}

fn one_pixel_png() -> &'static [u8] {
    &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0, 5, 0, 1,
        13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}

fn write_test_zip(path: &Path, files: &[(&str, &[u8])]) {
    let file = File::create(path).expect("zip file");
    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in files {
        zip.start_file(*name, options).expect("start file");
        zip.write_all(content).expect("write file");
    }
    zip.finish().expect("finish");
}

fn write_minimal_lab_package(path: &Path) {
    write_test_zip(
            path,
            &[
                (
                    "control.json",
                    br#"{
                        "schema_version":"Lab-1y.control.v1",
                        "package_id":"fixture.task",
                        "execution_mode":"recognize_only",
                        "game":"arknights",
                        "server":"cn",
                        "resolution":{"width":1280,"height":720},
                        "entry_task_id":"task"
                    }"#,
                ),
                (
                    "resources/manifest.json",
                    br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
                ),
                (
                    "resources/operations/task/task.json",
                    br#"{
                        "schema_version":"0.3",
                        "task_id":"task",
                        "game":"arknights",
                        "server_scope":["cn"],
                        "goal":"fixture",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "anchors":[{"id":"home","template":"assets/PAGE_HOME.png"}],
                        "entry_page":"home",
                        "target_page":"home",
                        "operations":[
                            {
                                "id":"noop",
                                "purpose":"fixture",
                                "from":"home",
                                "to":null,
                                "click":{"kind":"point","x":1,"y":1},
                                "verify_template":null,
                                "unguarded_trusted_coordinate":true,
                                "consumes":[],
                                "produces":[]
                            }
                        ]
                    }"#,
                ),
                ("resources/operations/task/assets/PAGE_HOME.png", one_pixel_png()),
                (
                    "resources/recognition/arknights.cn.pack.json",
                    br#"{
                        "schema_version":"0.3",
                        "game":"arknights",
                        "server":"cn",
                        "locale":"zh-CN",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "targets":[
                            {
                                "type":"template",
                                "id":"page/home",
                                "template_path":"operations/task/assets/PAGE_HOME.png",
                                "region":{"x":0,"y":0,"width":1,"height":1},
                                "threshold":0.9
                            }
                        ]
                    }"#,
                ),
                (
                    "resources/recognition/arknights.cn.pages.json",
                    br#"{
                        "schema_version":"0.3",
                        "pages":[
                            {"id":"arknights/home","required":["page/home"],"optional":[],"forbidden":[]}
                        ]
                    }"#,
                ),
            ],
        );
}

fn write_lab_package_with_unsupported_recognition(path: &Path) {
    write_test_zip(
            path,
            &[
                (
                    "control.json",
                    br#"{
                        "schema_version":"Lab-1y.control.v1",
                        "package_id":"fixture.task",
                        "execution_mode":"recognize_only",
                        "game":"arknights",
                        "server":"cn",
                        "resolution":{"width":1280,"height":720},
                        "entry_task_id":"task"
                    }"#,
                ),
                (
                    "resources/manifest.json",
                    br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
                ),
                (
                    "resources/operations/task/task.json",
                    br#"{
                        "schema_version":"0.3",
                        "task_id":"task",
                        "game":"arknights",
                        "server_scope":["cn"],
                        "goal":"fixture",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "anchors":[{"id":"home","template":"assets/PAGE_HOME.png"}],
                        "entry_page":"home",
                        "target_page":"home",
                        "operations":[
                            {
                                "id":"noop",
                                "purpose":"fixture",
                                "from":"home",
                                "to":null,
                                "click":{"kind":"point","x":1,"y":1},
                                "verify_template":null,
                                "unguarded_trusted_coordinate":true,
                                "consumes":[],
                                "produces":[]
                            }
                        ]
                    }"#,
                ),
                ("resources/operations/task/assets/PAGE_HOME.png", one_pixel_png()),
                (
                    "resources/recognition/arknights.cn.pack.json",
                    br#"{
                        "schema_version":"0.5",
                        "game":"arknights",
                        "server":"cn",
                        "locale":"zh-CN",
                        "coordinate_space":{"width":1280,"height":720},
                        "defaults":{"template_threshold":0.9,"color_max_distance":20.0},
                        "targets":[
                            {
                                "type":"template",
                                "id":"page/home",
                                "template_path":"operations/task/assets/PAGE_HOME.png",
                                "region":{"x":0,"y":0,"width":1,"height":1},
                                "threshold":0.9,
                                "method":"rgb_count",
                                "mask":{"type":"range","lower":1,"upper":255}
                            }
                        ]
                    }"#,
                ),
                (
                    "resources/recognition/arknights.cn.pages.json",
                    br#"{
                        "schema_version":"0.3",
                        "pages":[
                            {"id":"arknights/home","required":["page/home"],"optional":[],"forbidden":[]}
                        ]
                    }"#,
                ),
            ],
        );
}
