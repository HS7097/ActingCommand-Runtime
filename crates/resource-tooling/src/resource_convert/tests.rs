use super::*;

#[test]
fn derives_target_ids_like_python_converter() {
    assert_eq!(anchor_target_id("home"), "page/home");
    assert_eq!(
        template_target_id("assets/BUTTON_ALL_COLLECT.png"),
        "button/all_collect"
    );
    assert_eq!(
        template_target_id("assets/POPUP_MOMOTALK.png"),
        "popup/momotalk"
    );
    assert_eq!(template_target_id("assets/PAGE_HOME.png"), "page/home");
    assert_eq!(
        template_target_id("assets/DOCK_CHECK.png"),
        "template/dock_check"
    );
}

#[test]
fn converts_region_and_click_shapes() {
    let rect = json!({"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}});
    assert_eq!(
        region_to_pack(&rect).unwrap(),
        json!({"x":1,"y":2,"width":3,"height":4})
    );
    assert_eq!(
        region_to_pack(&json!({"mode":"full_frame"})).unwrap(),
        Value::String("full_frame".to_string())
    );
    assert_eq!(
        click_to_navigation(&json!({"kind":"point","x":12,"y":34})).unwrap(),
        json!({"kind":"point","point":"12,34"})
    );
    assert_eq!(
        click_to_navigation(&json!({"kind":"rect","x":1,"y":2,"width":3,"height":4})).unwrap(),
        json!({"kind":"rect","x":1,"y":2,"width":3,"height":4})
    );
    assert_eq!(
            click_to_navigation(&json!({"kind":"drag","from":{"x":1,"y":2,"width":3,"height":4},"to":{"x":5,"y":6,"width":7,"height":8},"duration_ms":900})).unwrap(),
            json!({"kind":"drag","from":{"x":1,"y":2,"width":3,"height":4},"to":{"x":5,"y":6,"width":7,"height":8},"duration_ms":900})
        );
    assert_eq!(
            click_to_navigation(&json!({"kind":"offset","target_id":"page/home","offset":{"x":1,"y":2,"width":3,"height":4}})).unwrap(),
            json!({"kind":"offset","target_id":"page/home","offset":{"x":1,"y":2,"width":3,"height":4}})
        );
    assert_eq!(
        click_to_navigation(&json!({"kind":"long_press","x":12,"y":34,"duration_ms":700})).unwrap(),
        json!({"kind":"long_press","x":12,"y":34,"duration_ms":700})
    );
}

#[test]
fn resolves_page_anchor_variants_as_any_of_group() {
    let ids = BTreeSet::from([
        "home".to_string(),
        "operator_0".to_string(),
        "operator_1".to_string(),
    ]);
    assert_eq!(
        resolve_page_requirements("home", &ids),
        PageRequirements {
            required: vec!["page/home".to_string()],
            any_of: Vec::new()
        }
    );
    assert_eq!(
        resolve_page_requirements("operator", &ids),
        PageRequirements {
            required: Vec::new(),
            any_of: vec![vec![
                "page/operator_0".to_string(),
                "page/operator_1".to_string()
            ]]
        }
    );
}

#[test]
fn build_pages_emits_any_of_for_anchor_variants() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "operator-check".to_string(),
            dir: PathBuf::from("operations/operator-check"),
            data: json!({
                "schema_version": "0.5",
                "task_id": "operator-check",
                "anchors": [
                    {"id":"operator_0","template":"assets/OPERATOR_0.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}},
                    {"id":"operator_1","template":"assets/OPERATOR_1.png","region":{"mode":"rect","rect":{"x":5,"y":6,"width":7,"height":8}}}
                ],
                "entry_page": "operator",
                "target_page": "operator",
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pages = converter.build_pages().unwrap();
    let operator = pages.pointer("/pages/0").unwrap();
    assert_eq!(operator.pointer("/required"), Some(&json!([])));
    assert_eq!(
        operator.pointer("/any_of"),
        Some(&json!([["page/operator_0", "page/operator_1"]]))
    );
}

#[test]
fn build_pages_applies_page_rules() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "home-check".to_string(),
            dir: PathBuf::from("operations/home-check"),
            data: json!({
                "schema_version": "0.5",
                "task_id": "home-check",
                "anchors": [
                    {"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}},
                    {"id":"mission_result_negative","template":"assets/MISSION_RESULT.png","region":{"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}}
                ],
                "entry_page": "home",
                "target_page": "home",
                "page_rules": {
                    "home": {
                        "optional": ["page/extra_context"],
                        "forbidden": ["page/mission_result_negative"]
                    }
                },
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pages = converter.build_pages().unwrap();
    let home = pages.pointer("/pages/0").unwrap();
    assert_eq!(
        home.pointer("/optional/0").and_then(Value::as_str),
        Some("page/extra_context")
    );
    assert_eq!(
        home.pointer("/forbidden/0").and_then(Value::as_str),
        Some("page/mission_result_negative")
    );
}

#[test]
fn build_pages_rejects_unknown_page_rule() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "home-check".to_string(),
            dir: PathBuf::from("operations/home-check"),
            data: json!({
                "schema_version": "0.5",
                "task_id": "home-check",
                "anchors": [{"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}}],
                "entry_page": "home",
                "target_page": "home",
                "page_rules": {"missing": {"forbidden": ["page/home"]}},
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let err = converter.build_pages().expect_err("unknown page rule");
    assert!(err.message.contains("unknown page"));
}

#[test]
fn validate_page_rule_targets_rejects_missing_targets() {
    let pack = json!({"targets":[{"id":"page/home"}]});
    let bundles = vec![Bundle {
        task_id: "home-check".to_string(),
        dir: PathBuf::from("operations/home-check"),
        data: json!({
            "page_rules": {
                "home": {
                    "required": ["page/home"],
                    "forbidden": ["page/missing"]
                }
            }
        }),
    }];

    let err = validate_page_rule_targets(&pack, &bundles).expect_err("missing target");
    assert!(err.message.contains("page/missing"));
}

#[test]
fn selected_build_prunes_nonresident_page_rules_and_soft_targets() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![
            Bundle {
                task_id: "open_depot".to_string(),
                dir: PathBuf::from("operations/open_depot"),
                data: json!({
                    "schema_version": "0.5",
                    "task_id": "open_depot",
                    "anchors": [
                        {"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}},
                        {"id":"depot","template":"assets/DEPOT.png","region":{"mode":"rect","rect":{"x":5,"y":6,"width":7,"height":8}}}
                    ],
                    "entry_page": "home",
                    "target_page": "depot",
                    "operations": [
                        {"id":"home_to_depot","from":"home","to":"depot"}
                    ]
                }),
            },
            Bundle {
                task_id: "return_home".to_string(),
                dir: PathBuf::from("operations/return_home"),
                data: json!({
                    "schema_version": "0.5",
                    "task_id": "return_home",
                    "anchors": [
                        {"id":"quickswitch_dropdown","template":"assets/QUICKSWITCH.png","region":{"mode":"rect","rect":{"x":9,"y":10,"width":11,"height":12}}}
                    ],
                    "entry_page": "any",
                    "target_page": "home",
                    "page_rules": {
                        "depot": {"forbidden": ["page/home", "page/recruit"]},
                        "recruit": {"forbidden": ["page/home"]},
                        "quickswitch_dropdown": {"optional": ["page/depot", "page/friends"]}
                    },
                    "operations": [
                        {"id":"open_quickswitch","from":"any","to":"quickswitch_dropdown"},
                        {"id":"quickswitch_to_home","from":"quickswitch_dropdown","to":"home"}
                    ]
                }),
            },
        ],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let bundles = converter.prune_page_rules_for_selected_build(converter.bundles.clone());
    let recovery = bundles
        .iter()
        .find(|bundle| bundle.task_id == "return_home")
        .unwrap();
    let rules = recovery
        .data
        .get("page_rules")
        .unwrap()
        .as_object()
        .unwrap();

    assert!(rules.get("recruit").is_none());
    assert_eq!(
        rules
            .get("depot")
            .unwrap()
            .get("forbidden")
            .unwrap()
            .as_array()
            .unwrap(),
        &vec![json!("page/home")]
    );
    assert_eq!(
        rules
            .get("quickswitch_dropdown")
            .unwrap()
            .get("optional")
            .unwrap()
            .as_array()
            .unwrap(),
        &vec![json!("page/depot")]
    );
}

#[test]
fn color_check_region_is_flattened() {
    let input = json!({
        "region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}},
        "expected":[10,20,30]
    });
    assert_eq!(
        color_check_to_pack(Some(&input)).unwrap().unwrap(),
        json!({"region":{"x":1,"y":2,"width":3,"height":4},"expected":[10,20,30]})
    );
}

#[test]
fn build_pack_includes_color_probe_targets() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "daily-check".to_string(),
            dir: PathBuf::from("operations/daily-check"),
            data: json!({
                "schema_version": "0.3",
                "task_id": "daily-check",
                "anchors": [],
                "color_probes": [{
                    "id": "color/home-status",
                    "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                    "expected": [10, 20, 30]
                }],
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pack = converter.build_pack().unwrap();
    let target_value = pack.pointer("/targets/0").expect("color target value");
    let target = target_value.as_object().expect("color target");
    assert_eq!(target.get("type").and_then(Value::as_str), Some("color"));
    assert_eq!(
        target.get("id").and_then(Value::as_str),
        Some("color/home-status")
    );
    assert_eq!(
        target_value.pointer("/region/x").and_then(Value::as_i64),
        Some(10)
    );
    assert_eq!(
        target_value.pointer("/expected/2").and_then(Value::as_u64),
        Some(30)
    );
}

#[test]
fn build_pack_includes_verify_template_targets() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let converter = OperationConverter {
        root: root.clone(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "daily-check".to_string(),
            dir: root.join("operations/daily-check"),
            data: json!({
                "schema_version": "0.3",
                "task_id": "daily-check",
                "anchors": [],
                "verify_templates": [{
                    "id": "template/mail-ready",
                    "template": "assets/VERIFY_MAIL_READY.png",
                    "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                    "threshold": 0.97
                }],
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pack = converter.build_pack().unwrap();
    let target_value = pack
        .pointer("/targets/0")
        .expect("verify-template target value");
    let target = target_value.as_object().expect("verify-template target");
    assert_eq!(target.get("type").and_then(Value::as_str), Some("template"));
    assert_eq!(
        target.get("id").and_then(Value::as_str),
        Some("template/mail-ready")
    );
    assert_eq!(
        target.get("template_path").and_then(Value::as_str),
        Some("operations/daily-check/assets/VERIFY_MAIL_READY.png")
    );
    assert_eq!(
        target_value.pointer("/region/y").and_then(Value::as_i64),
        Some(20)
    );
    assert_eq!(
        target_value.pointer("/threshold").and_then(Value::as_f64),
        Some(0.97)
    );
}

fn write_synthetic_maa_convert_fixture() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/synthetic-maa");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/HOME.png"), b"synthetic").unwrap();
    fs::write(task_dir.join("assets/TERMINAL.png"), b"synthetic").unwrap();
    fs::write(
        root.path().join("operations/resources.json"),
        serde_json::to_vec_pretty(&json!({"resources":[]})).unwrap(),
    )
    .unwrap();
    fs::write(
        task_dir.join("task.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.5",
            "task_id": "synthetic-maa",
            "game": "arknights",
            "server_scope": ["cn"],
            "locale": "zh-CN",
            "coordinate_space": {"width":1280,"height":720},
            "defaults": {"template_threshold":0.5},
            "anchors": [{
                "id": "home",
                "maa_task": "Check@Base",
                "template": "assets/HOME.png",
                "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}
            }, {
                "id": "terminal",
                "template": "assets/TERMINAL.png",
                "region": {"mode":"rect","rect":{"x":50,"y":60,"width":30,"height":40}}
            }],
            "operations": [{
                "id": "tap_home",
                "purpose": "synthetic rectMove",
                "from": "home",
                "to": "terminal",
                "click": {"kind":"point","x":100,"y":100},
                "expect_after": {"page_id":"terminal","timeout_ms":500},
                "consumes": [],
                "produces": []
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    let maa_dir = root.path().join("maa-tasks");
    fs::create_dir_all(&maa_dir).unwrap();
    fs::write(
        maa_dir.join("tasks.json"),
        serde_json::to_vec_pretty(&json!({
            "Base": {
                "template": "BASE.png",
                "templThreshold": 0.67,
                "method": "RGBCount",
                "maskRange": [7, 199],
                "rectMove": [1, 2, 3, 4],
                "next": ["Helper"]
            },
            "Helper": {
                "template": "HELPER.png",
                "next": ["Stop"]
            },
            "Check@Base": {
                "templThreshold": 0.91,
                "rectMove": [11, 22, 33, 44],
                "next": ["Base#next"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    (root, maa_dir)
}

#[test]
fn maa_tasks_mode_feeds_expanded_template_fields_into_pack_targets() {
    let (root, maa_dir) = write_synthetic_maa_convert_fixture();

    let mut converter = OperationConverter::load(root.path(), None, None, None).unwrap();
    converter.load_maa_task_overlays(&maa_dir).unwrap();
    let outputs = converter.build_all().unwrap();
    let target = outputs.pack.pointer("/targets/0").unwrap();

    assert_eq!(
        target.pointer("/id").and_then(Value::as_str),
        Some("page/home")
    );
    assert_eq!(
        target.pointer("/threshold").and_then(Value::as_f64),
        Some(0.91)
    );
    assert_eq!(
        target.pointer("/method").and_then(Value::as_str),
        Some("rgb_count")
    );
    assert_eq!(
        target.pointer("/mask/type").and_then(Value::as_str),
        Some("range")
    );
    assert_eq!(
        target.pointer("/mask/lower").and_then(Value::as_u64),
        Some(7)
    );
    assert_eq!(
        target.pointer("/mask/upper").and_then(Value::as_u64),
        Some(199)
    );
    assert_eq!(
        target.pointer("/rect_move"),
        Some(&json!({"x":11,"y":22,"width":33,"height":44}))
    );
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();
    assert_eq!(
        primitive.pointer("/click/kind").and_then(Value::as_str),
        Some("offset")
    );
    assert_eq!(
        primitive
            .pointer("/click/target_id")
            .and_then(Value::as_str),
        Some("page/home")
    );
    assert_eq!(
        primitive.pointer("/click/offset"),
        Some(&json!({"x":11,"y":22,"width":33,"height":44}))
    );
    assert_eq!(
        primitive
            .pointer("/expect_after/page_id")
            .and_then(Value::as_str),
        Some("terminal")
    );
}

#[test]
fn resource_convert_accepts_explicit_maa_tasks_mode() {
    let (root, maa_dir) = write_synthetic_maa_convert_fixture();
    let summary = resource_convert(ResourceConvertRequest {
        repo: root.path().to_path_buf(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: Some(maa_dir),
        dry_run: true,
    })
    .unwrap();

    assert_eq!(summary.source_mode.as_deref(), Some("maa_tasks"));
    assert_eq!(summary.maa_compiled_tasks, Some(3));
    assert_eq!(summary.targets, 2);
}

#[test]
fn default_operation_bundle_mode_does_not_apply_maa_overlay_fields() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let converter = OperationConverter {
        root: root.clone(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.5}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "synthetic-maa".to_string(),
            dir: root.join("operations/synthetic-maa"),
            data: json!({
                "schema_version": "0.5",
                "task_id": "synthetic-maa",
                "anchors": [{
                    "id": "home",
                    "maa_task": "Check@Base",
                    "template": "assets/HOME.png",
                    "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}
                }],
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pack = converter.build_pack().unwrap();
    assert_eq!(
        pack.pointer("/targets/0"),
        Some(&json!({
            "type": "template",
            "id": "page/home",
            "template_path": "operations/synthetic-maa/assets/HOME.png",
            "region": {"x":10,"y":20,"width":30,"height":40},
            "threshold": 0.5
        }))
    );
}

#[test]
fn build_primitives_synthesizes_guard_from_operation_verify_template() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/daily-check");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/VERIFY_READY.png"), b"png").unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "daily-check".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "daily-check",
                "anchors": [],
                "verify_templates": [{
                    "id": "template/verify_ready",
                    "template": "assets/VERIFY_READY.png",
                    "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                    "threshold": 0.97
                }],
                "operations": [{
                    "id": "home_to_target",
                    "purpose": "go target",
                    "from": "home",
                    "to": "target",
                    "click": {"kind":"rect","x":100,"y":110,"width":20,"height":25},
                    "verify_template": "assets/VERIFY_READY.png"
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs
        .primitives
        .pointer("/primitives/0")
        .expect("primitive");

    assert_eq!(
        primitive.pointer("/guard/page_id").and_then(Value::as_str),
        Some("arknights/home")
    );
    assert_eq!(
        primitive
            .pointer("/guard/target_id")
            .and_then(Value::as_str),
        Some("template/verify_ready")
    );
    assert_eq!(
        primitive.pointer("/guard/expected_rect"),
        Some(&json!({"x":10,"y":20,"width":30,"height":40}))
    );
    assert_eq!(
        outputs
            .primitives
            .get("converter_schema_version")
            .and_then(Value::as_str),
        Some(CONVERTER_SCHEMA_VERSION)
    );
}

#[test]
fn build_primitives_synthesizes_guard_from_source_anchor_without_operation_verify_template() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/open-terminal");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/HOME.png"), b"png").unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "open-terminal".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "open-terminal",
                "anchors": [{
                    "id": "home",
                    "template": "assets/HOME.png",
                    "region": {"mode":"rect","rect":{"x":200,"y":300,"width":40,"height":50}},
                    "threshold": 0.8
                }],
                "operations": [{
                    "id": "home_to_terminal",
                    "purpose": "go terminal",
                    "from": "home",
                    "to": "terminal",
                    "click": {"kind":"rect","x":100,"y":110,"width":20,"height":25},
                    "verify_template": null
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert_eq!(
        primitive.pointer("/guard/page_id").and_then(Value::as_str),
        Some("arknights/home")
    );
    assert_eq!(
        primitive
            .pointer("/guard/target_id")
            .and_then(Value::as_str),
        Some("page/home")
    );
    assert_eq!(
        primitive.pointer("/guard/expected_rect"),
        Some(&json!({"x":200,"y":300,"width":40,"height":50}))
    );
    assert_eq!(
        primitive
            .pointer("/guard/verify_template")
            .and_then(Value::as_str),
        Some("assets/HOME.png")
    );
}

#[test]
fn build_primitives_synthesizes_any_page_guard_from_matching_anchor_template() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/return-home");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/HOME_BUTTON.png"), b"png").unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "azurlane".to_string(),
        server: "jp".to_string(),
        locale: "ja-JP".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "return-home".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "return-home",
                "anchors": [{
                    "id": "home",
                    "template": "assets/HOME_BUTTON.png",
                    "region": {"mode":"rect","rect":{"x":1100,"y":20,"width":60,"height":40}},
                    "threshold": 0.9
                }],
                "operations": [{
                    "id": "goto_home",
                    "purpose": "return home",
                    "from": "any",
                    "to": "home",
                    "click": {"kind":"rect","x":1100,"y":20,"width":60,"height":40},
                    "verify_template": "assets/HOME_BUTTON.png"
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert_eq!(
        primitive.pointer("/guard/page_id").and_then(Value::as_str),
        Some("any")
    );
    assert_eq!(
        primitive
            .pointer("/guard/target_id")
            .and_then(Value::as_str),
        Some("page/home")
    );
}

#[test]
fn build_primitives_synthesizes_guard_from_source_anchor_without_verify_template() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/open-menu");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/HOME.png"), b"png").unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "open-menu".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "open-menu",
                "anchors": [{
                    "id": "home",
                    "template": "assets/HOME.png",
                    "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}}
                }],
                "operations": [{
                    "id": "open_menu",
                    "purpose": "open menu",
                    "from": "home",
                    "to": "menu",
                    "click": {"kind":"specific_rect","x":100,"y":110,"width":20,"height":25}
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert_eq!(
        primitive.pointer("/guard/page_id").and_then(Value::as_str),
        Some("arknights/home")
    );
    assert_eq!(
        primitive
            .pointer("/guard/target_id")
            .and_then(Value::as_str),
        Some("page/home")
    );
    assert_eq!(
        primitive.pointer("/guard/expected_rect"),
        Some(&json!({"x":10,"y":20,"width":30,"height":40}))
    );
}

#[test]
fn build_primitives_rejects_rect_and_specific_rect_without_guard_source() {
    for kind in ["rect", "specific_rect"] {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join(format!("operations/open-menu-{kind}"));
        fs::create_dir_all(&task_dir).unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: format!("open-menu-{kind}"),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": format!("open-menu-{kind}"),
                    "anchors": [],
                    "operations": [{
                        "id": "open_menu",
                        "purpose": "open menu",
                        "from": "home",
                        "to": "menu",
                        "click": {"kind": kind, "x":100,"y":110,"width":20,"height":25}
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let err = converter
            .build_all()
            .expect_err("coordinate operation without guard source must fail");
        assert!(err.message.contains("cannot synthesize guard"));
        assert!(err.message.contains("unguarded_trusted_coordinate"));
    }
}

#[test]
fn build_primitives_rejects_drag_without_guard_source() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/open-menu-drag");
    fs::create_dir_all(&task_dir).unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "open-menu-drag".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "open-menu-drag",
                "anchors": [],
                "operations": [{
                    "id": "drag_menu",
                    "purpose": "drag menu",
                    "from": "home",
                    "to": "menu",
                    "click": {
                        "kind": "drag",
                        "from": {"x": 100, "y": 110, "width": 20, "height": 25},
                        "to": {"x": 500, "y": 110, "width": 20, "height": 25},
                        "duration_ms": 500
                    }
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let err = converter
        .build_all()
        .expect_err("drag without guard source must fail");
    assert!(err.message.contains("cannot synthesize guard"));
    assert!(err.message.contains("unguarded_trusted_coordinate"));
}

#[test]
fn build_primitives_rejects_point_and_long_press_without_guard_source() {
    for (kind, click) in [
        ("point", json!({"kind":"point","x":100,"y":110})),
        (
            "long_press",
            json!({"kind":"long_press","x":100,"y":110,"duration_ms":700}),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let task_dir = root.path().join(format!("operations/open-menu-{kind}"));
        fs::create_dir_all(&task_dir).unwrap();
        let converter = OperationConverter {
            root: root.path().to_path_buf(),
            game: "arknights".to_string(),
            server: "cn".to_string(),
            locale: "zh-CN".to_string(),
            coordinate_space: json!({"width":1280,"height":720}),
            defaults: json!({"template_threshold":0.95}),
            resource_ids: HashSet::new(),
            bundles: vec![Bundle {
                task_id: format!("open-menu-{kind}"),
                dir: task_dir,
                data: json!({
                    "schema_version": "0.3",
                    "task_id": format!("open-menu-{kind}"),
                    "anchors": [],
                    "operations": [{
                        "id": "open_menu",
                        "purpose": "open menu",
                        "from": "home",
                        "to": "menu",
                        "click": click
                    }]
                }),
            }],
            existing_navigation: None,
            maa_task_overlays: HashMap::new(),
        };

        let err = converter
            .build_all()
            .expect_err("point-like operation without guard source must fail");
        assert!(err.message.contains("cannot synthesize guard"));
        assert!(err.message.contains("unguarded_trusted_coordinate"));
    }
}

#[test]
fn build_primitives_allows_explicit_trusted_unguarded_long_press() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/open-menu-long-press");
    fs::create_dir_all(&task_dir).unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "open-menu-long-press".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "open-menu-long-press",
                "anchors": [],
                "operations": [{
                    "id": "hold_menu",
                    "purpose": "hold menu",
                    "from": "home",
                    "to": "menu",
                    "click": {"kind":"long_press","x":100,"y":110,"duration_ms":700},
                    "unguarded_trusted_coordinate": true
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert!(primitive.get("guard").is_some_and(Value::is_null));
    assert_eq!(
        primitive
            .get("unguarded_trusted_coordinate")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn build_primitives_allows_explicit_trusted_unguarded_drag() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/open-menu-drag");
    fs::create_dir_all(&task_dir).unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "open-menu-drag".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "open-menu-drag",
                "anchors": [],
                "operations": [{
                    "id": "drag_menu",
                    "purpose": "drag menu",
                    "from": "home",
                    "to": "menu",
                    "click": {
                        "kind": "drag",
                        "from": {"x": 100, "y": 110, "width": 20, "height": 25},
                        "to": {"x": 500, "y": 110, "width": 20, "height": 25},
                        "duration_ms": 500
                    },
                    "unguarded_trusted_coordinate": true
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert!(primitive.get("guard").is_some_and(Value::is_null));
    assert_eq!(
        primitive
            .get("unguarded_trusted_coordinate")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn build_primitives_synthesizes_guard_from_operation_verify_template_click_rect() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/return-home");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/HOME_ICON.png"), b"png").unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "bluearchive".to_string(),
        server: "jp".to_string(),
        locale: "ja-JP".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "return-home".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "return-home",
                "anchors": [],
                "operations": [{
                    "id": "tap_home",
                    "purpose": "tap home",
                    "from": "any",
                    "to": "home",
                    "click": {"kind":"point","x":1236,"y":25},
                    "verify_template": "assets/HOME_ICON.png"
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert_eq!(
        primitive.pointer("/guard/page_id").and_then(Value::as_str),
        Some("any")
    );
    assert_eq!(
        primitive
            .pointer("/guard/target_id")
            .and_then(Value::as_str),
        Some("template/home_icon")
    );
    assert_eq!(
        primitive.pointer("/guard/expected_rect"),
        Some(&json!({"x":1236,"y":25,"width":1,"height":1}))
    );
}

#[test]
fn build_primitives_rejects_unmatched_verify_template_without_rect_guard_source() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/daily-check");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/VERIFY_READY.png"), b"png").unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "daily-check".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "daily-check",
                "anchors": [],
                "operations": [{
                    "id": "home_to_target",
                    "purpose": "go target",
                    "from": "home",
                    "to": "target",
                    "click": {"kind":"offset","target_id":"target/button","offset":{"x":1,"y":2,"width":3,"height":4}},
                    "verify_template": "assets/VERIFY_READY.png"
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let err = converter
        .build_all()
        .expect_err("guard synthesis should fail");

    assert!(
        err.message
            .contains("cannot synthesize guard expected_rect from click kind")
    );
}

#[test]
fn converted_offset_click_rejects_color_probe_guard() {
    let pack = json!({
        "game": "arknights",
        "targets": [{
            "type": "color",
            "id": "target/button"
        }]
    });
    let pages = json!({
        "pages": [{
            "id": "arknights/home"
        }]
    });
    let primitives = json!({
        "primitives": [{
            "id": "tap_offset",
            "from": "home",
            "click": {
                "kind": "offset",
                "target_id": "target/button",
                "offset": {"x": 1, "y": 2, "width": 3, "height": 4}
            },
            "guard": {
                "page_id": "arknights/home",
                "target_id": "target/button",
                "expected_rect": {"x": 10, "y": 20, "width": 30, "height": 40},
                "color_probe": "target/button"
            }
        }]
    });

    let err = validate_converted_guard_references(&pack, &pages, &primitives)
        .expect_err("offset click must require template matched_rect source");

    assert!(err.message.contains("requires a template guard"));
    assert!(err.message.contains("must be a template target"));
}

#[test]
fn build_primitives_allows_explicit_trusted_unguarded_coordinate() {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/daily-check");
    fs::create_dir_all(&task_dir).unwrap();
    let converter = OperationConverter {
        root: root.path().to_path_buf(),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.95}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "daily-check".to_string(),
            dir: task_dir,
            data: json!({
                "schema_version": "0.3",
                "task_id": "daily-check",
                "anchors": [],
                "operations": [{
                    "id": "home_to_target",
                    "purpose": "go target",
                    "from": "home",
                    "to": "target",
                    "click": {"kind":"rect","x":100,"y":110,"width":20,"height":25},
                    "verify_template": null,
                    "unguarded_trusted_coordinate": true
                }]
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert!(primitive.get("guard").is_some_and(Value::is_null));
    assert_eq!(
        primitive
            .get("unguarded_trusted_coordinate")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn resource_selectors_are_generic_and_path_safe() {
    assert_eq!(
        canonical_game(" Fixture-Game.4 ").unwrap(),
        "fixture-game.4"
    );
    assert_eq!(canonical_server(" Test_Shard ").unwrap(), "test_shard");
    assert!(canonical_game("fixture/game").is_err());
    assert!(canonical_server(" ").is_err());
}
