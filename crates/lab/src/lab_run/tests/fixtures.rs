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
            timeout_ms: None,
            step_timeout_ms: None,
            max_steps: None,
            stop_on_error: None,
            stop_on_confirmation: None,
            allow_placeholder_coords: None,
            output: None,
            capture_backend: None,
            frame_store: FrameStoreControl::default(),
            producer: None,
            trusted_execution: None,
        }
    }

    fn test_operation(to: Option<&str>, verify_template: Option<&str>) -> Operation {
        Operation {
            id: "open_terminal".to_string(),
            purpose: "test".to_string(),
            from: "home".to_string(),
            to: to.map(str::to_string),
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
            },
            verify_template: verify_template.map(str::to_string),
            expect_after: None,
            timeout_ms: None,
            max_attempts: None,
            retry_interval_ms: None,
            pre_delay_ms: None,
            post_delay_ms: None,
            pre_wait_freezes_ms: None,
            post_wait_freezes_ms: None,
            retryable: None,
            effect: None,
            on_error: None,
            guard: None,
            unguarded_trusted_coordinate: true,
            consumes: Vec::new(),
            produces: Vec::new(),
            verified_live: None,
            provenance: None,
        }
    }

    fn test_operation_bundle(operation: Operation) -> OperationBundle {
        OperationBundle {
            schema_version: "0.3".to_string(),
            task_id: "task".to_string(),
            game: "arknights".to_string(),
            server_scope: vec!["cn".to_string()],
            goal: "test".to_string(),
            coordinate_space: Resolution {
                width: 1280,
                height: 720,
            },
            defaults: OperationDefaults::default(),
            anchors: Vec::new(),
            entry_page: Some("home".to_string()),
            target_page: Some("terminal".to_string()),
            error_pages: Vec::new(),
            recovery: None,
            max_task_retries: None,
            on_exhausted: None,
            page_rules: BTreeMap::new(),
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

    fn captured_scene(page: Option<&str>, verify_template_matched: bool) -> CapturedScene {
        CapturedScene {
            scene: Scene::from_png(one_pixel_png()).expect("scene"),
            matched_page: page.map(str::to_string),
            page_evaluations: Vec::new(),
            verify_template_matched,
            width: 1,
            height: 1,
        }
    }

    fn captured_rgb_scene(page: Option<&str>, rgb: [u8; 3]) -> CapturedScene {
        CapturedScene {
            scene: Scene::from_pixels(1, 1, &rgb, ScenePixelFormat::Rgb8).expect("scene"),
            matched_page: page.map(str::to_string),
            page_evaluations: Vec::new(),
            verify_template_matched: false,
            width: 1,
            height: 1,
        }
    }

    fn one_pixel_color_evaluator(expected: [u8; 3]) -> RecognitionEvaluator {
        let pack = load_pack_from_json_str(&format!(
            r#"{{
                "schema_version":"0.3",
                "game":"arknights",
                "server":"cn",
                "coordinate_space":{{"width":1,"height":1}},
                "defaults":{{"color_max_distance":0.0}},
                "targets":[{{
                    "type":"color",
                    "id":"target/button",
                    "region":{{"x":0,"y":0,"width":1,"height":1}},
                    "expected":[{},{},{}]
                }}]
            }}"#,
            expected[0], expected[1], expected[2]
        ))
        .expect("pack");
        RecognitionEvaluator::new(PathBuf::from("."), pack).expect("evaluator")
    }

    struct StaticCapture {
        frame: Frame,
    }

    impl CaptureBackend for StaticCapture {
        fn capture(&mut self) -> actingcommand_device::DeviceResult<Frame> {
            Ok(self.frame.clone())
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
            }),
            message: if passed {
                "color passed".to_string()
            } else {
                "color failed".to_string()
            },
        }
    }

    fn template_target_evaluation(id: &str, rect: PackRect) -> TargetEvaluation {
        TargetEvaluation {
            id: id.to_string(),
            kind: TargetKind::Template,
            passed: true,
            template: Some(actingcommand_recognition_pack::TemplateEvaluation {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                raw_score: 1.0,
                score: 1.0,
                threshold: 0.9,
            }),
            color: None,
            message: "template passed".to_string(),
        }
    }

    fn one_pixel_png() -> &'static [u8] {
        &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 10, 73, 68, 65, 84, 120, 156, 99, 0, 1, 0, 0,
            5, 0, 1, 13, 10, 45, 180, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
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

    fn zip_text(archive: &mut ZipArchive<File>, name: &str) -> String {
        let mut entry = archive.by_name(name).expect("zip entry");
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("zip text");
        text
    }

    fn has_record_type(ledger: &actingcommand_ledger::LedgerRead, record_type: &str) -> bool {
        ledger.records.iter().any(|record| {
            record.payload.get("record_type").and_then(Value::as_str) == Some(record_type)
        })
    }

    fn has_event(ledger: &actingcommand_ledger::LedgerRead, event: &str) -> bool {
        ledger
            .events
            .iter()
            .any(|entry| entry.payload.get("event").and_then(Value::as_str) == Some(event))
    }

    fn assert_ordered(text: &str, needles: &[&str]) {
        let mut previous = 0;
        for needle in needles {
            let offset = text[previous..].find(needle).expect("needle order");
            previous += offset + needle.len();
        }
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
