use super::*;

#[test]
fn schema_0_7_post_admission_ocr_validates_hash_bound_truth_and_closed_algorithms() {
    let root = tempfile::tempdir().expect("temp dir");
    let task_dir = root.path().join("operations/fixture_task");
    fs::create_dir_all(&task_dir).expect("task dir");
    let truth = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.ocr-truth-set.v1",
        "items": ["Alpha", "Beta"]
    }))
    .expect("truth bytes");
    fs::write(task_dir.join("truth.json"), &truth).expect("truth file");
    let truth_sha256 = format!("{:x}", Sha256::digest(&truth));
    let task = |schema_version: &str, normalization: &str, sha256: &str| Bundle {
        task_id: "fixture_task".to_string(),
        dir: task_dir.clone(),
        data: json!({
            "schema_version": schema_version,
            "task_id": "fixture_task",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 1280, "height": 720},
            "ocr_targets": [valid_ocr_declaration("fixture/ocr")],
            "scheduling_outcome": {
                "mappings": [{
                    "outcome_key": "comparison_recorded",
                    "effect": "no_designated_effect",
                    "terminal_pages": ["terminal"]
                }]
            },
            "post_admission_ocr": {
                "page_id": "admitted",
                "target_id": "fixture/ocr",
                "truth_set": {"path": "truth.json", "sha256": sha256},
                "normalization": normalization,
                "comparison": "exact_set_v1",
                "limits": {
                    "max_frames": 2,
                    "max_items": 16,
                    "max_string_bytes": 64,
                    "max_total_bytes": 4096,
                    "max_truth_entries": 16
                },
                "outcome_key": "comparison_recorded"
            },
            "operations": []
        }),
    };

    validate_post_admission_ocr_bundle(&task("0.7", "trim_lowercase_v1", &truth_sha256))
        .expect("schema 0.7 declaration");
    let mut sixteen_targets = task("0.7", "trim_lowercase_v1", &truth_sha256);
    let ordered_target_ids = (0..16)
        .map(|index| format!("fixture/ocr-{index:02}"))
        .collect::<Vec<_>>();
    sixteen_targets.data["ocr_targets"] = Value::Array(
        ordered_target_ids
            .iter()
            .map(|target_id| valid_ocr_declaration(target_id))
            .collect(),
    );
    sixteen_targets
        .data
        .get_mut("post_admission_ocr")
        .and_then(Value::as_object_mut)
        .expect("post-admission OCR object")
        .remove("target_id");
    sixteen_targets.data["post_admission_ocr"]["target_ids"] = json!(ordered_target_ids);
    validate_post_admission_ocr_bundle(&sixteen_targets)
        .expect("schema 0.7 ordered sixteen-target declaration");

    let mut two_pages = sixteen_targets.clone();
    two_pages
        .data
        .get_mut("post_admission_ocr")
        .and_then(Value::as_object_mut)
        .expect("post-admission OCR object")
        .remove("page_id");
    two_pages.data["post_admission_ocr"]["page_ids"] = json!(["operator", "operator_end"]);
    validate_post_admission_ocr_bundle(&two_pages)
        .expect("schema 0.7 exact two-page OCR declaration");

    let mut both_page_forms = two_pages.clone();
    both_page_forms.data["post_admission_ocr"]["page_id"] = json!("operator");
    assert!(
        validate_post_admission_ocr_bundle(&both_page_forms)
            .expect_err("both page forms must fail closed")
            .message
            .contains("exactly one")
    );
    let mut neither_page_form = two_pages.clone();
    neither_page_form
        .data
        .get_mut("post_admission_ocr")
        .and_then(Value::as_object_mut)
        .expect("post-admission OCR object")
        .remove("page_ids");
    assert!(
        validate_post_admission_ocr_bundle(&neither_page_form)
            .expect_err("missing page form must fail closed")
            .message
            .contains("exactly one")
    );
    for invalid_page_ids in [
        json!([]),
        json!(["operator"]),
        json!(["operator", "operator"]),
        json!(["operator", "operator_end", "other"]),
        json!(["operator", ""]),
        json!(["operator", 7]),
        Value::Null,
    ] {
        let mut invalid = two_pages.clone();
        invalid.data["post_admission_ocr"]["page_ids"] = invalid_page_ids;
        validate_post_admission_ocr_bundle(&invalid)
            .expect_err("invalid exact two-page form must fail closed");
    }

    let mut both_forms = sixteen_targets.clone();
    both_forms.data["post_admission_ocr"]["target_id"] = json!("fixture/ocr-00");
    assert!(
        validate_post_admission_ocr_bundle(&both_forms)
            .expect_err("both target forms must fail closed")
            .message
            .contains("exactly one")
    );

    let mut duplicate = sixteen_targets.clone();
    duplicate.data["post_admission_ocr"]["target_ids"][1] = json!("fixture/ocr-00");
    assert!(
        validate_post_admission_ocr_bundle(&duplicate)
            .expect_err("duplicate target IDs must fail closed")
            .message
            .contains("duplicate")
    );

    let mut off_page = sixteen_targets.clone();
    off_page.data["ocr_targets"][0]["region"]["rect"]["width"] = json!(1281);
    assert!(
        validate_post_admission_ocr_bundle(&off_page)
            .expect_err("out-of-bounds target must fail closed")
            .message
            .contains("page bounds")
    );
    assert!(
        validate_post_admission_ocr_bundle(&task("0.6", "trim_lowercase_v1", &truth_sha256))
            .expect_err("old schema cannot silently accept declaration")
            .message
            .contains("requires schema_version '0.7'")
    );
    assert!(
        validate_post_admission_ocr_bundle(&task("0.7", "unknown", &truth_sha256))
            .expect_err("unknown normalization")
            .message
            .contains("unsupported normalization")
    );
    assert!(
        validate_post_admission_ocr_bundle(&task("0.7", "trim_lowercase_v1", &"0".repeat(64)))
            .expect_err("truth hash mismatch")
            .message
            .contains("SHA-256")
    );

    let truth_v2_without_aliases = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.ocr-truth-set.v2",
        "items": ["缄默德克萨斯"]
    }))
    .expect("v2 truth bytes");
    fs::write(task_dir.join("truth.json"), &truth_v2_without_aliases).expect("v2 truth file");
    let truth_v2_sha256 = format!("{:x}", Sha256::digest(&truth_v2_without_aliases));
    validate_post_admission_ocr_bundle(&task("0.7", "trim_lowercase_v1", &truth_v2_sha256))
        .expect("schema v2 accepts a canonical dictionary without aliases");
}

#[test]
fn schema_0_7_task_timeout_is_optional_bounded_and_non_mutating() {
    let task = |schema_version: &str, timeout_ms: Option<Value>| {
        let mut data = json!({
            "schema_version": schema_version,
            "task_id": "fixture_task"
        });
        if let Some(timeout_ms) = timeout_ms {
            data["timeout_ms"] = timeout_ms;
        }
        Bundle {
            task_id: "fixture_task".to_string(),
            dir: PathBuf::from("operations/fixture_task"),
            data,
        }
    };

    let absent = task("0.7", None);
    let absent_bytes = serde_json::to_vec(&absent.data).expect("absent bytes");
    assert_eq!(validate_task_timeout_bundle(&absent).unwrap(), None);
    assert_eq!(
        serde_json::to_vec(&absent.data).expect("unchanged absent bytes"),
        absent_bytes
    );

    for timeout_ms in [1_u64, 300_000, 600_000] {
        let valid = task("0.7", Some(json!(timeout_ms)));
        let original = serde_json::to_vec(&valid.data).expect("valid bytes");
        assert_eq!(
            validate_task_timeout_bundle(&valid).unwrap(),
            Some(timeout_ms)
        );
        assert_eq!(
            serde_json::to_vec(&valid.data).expect("unchanged valid bytes"),
            original
        );
    }

    for invalid in [
        json!(0),
        json!(600_001),
        json!(-1),
        json!(1.5),
        json!("300000"),
        Value::Null,
    ] {
        validate_task_timeout_bundle(&task("0.7", Some(invalid)))
            .expect_err("invalid task timeout must fail closed");
    }
    validate_task_timeout_bundle(&task("0.6", Some(json!(300_000))))
        .expect_err("task timeout requires schema 0.7");
}

#[test]
fn schema_0_7_max_steps_is_optional_bounded_and_non_mutating() {
    let task = |schema_version: &str, max_steps: Option<Value>| {
        let mut data = json!({
            "schema_version": schema_version,
            "task_id": "fixture_task"
        });
        if let Some(max_steps) = max_steps {
            data["max_steps"] = max_steps;
        }
        Bundle {
            task_id: "fixture_task".to_string(),
            dir: PathBuf::from("operations/fixture_task"),
            data,
        }
    };

    let absent = task("0.7", None);
    let absent_bytes = serde_json::to_vec(&absent.data).expect("absent bytes");
    assert_eq!(validate_task_max_steps_bundle(&absent).unwrap(), None);
    assert_eq!(
        serde_json::to_vec(&absent.data).expect("unchanged absent bytes"),
        absent_bytes
    );

    for max_steps in [1_u32, 61, 1_000] {
        let valid = task("0.7", Some(json!(max_steps)));
        let original = serde_json::to_vec(&valid.data).expect("valid bytes");
        assert_eq!(
            validate_task_max_steps_bundle(&valid).unwrap(),
            Some(max_steps)
        );
        assert_eq!(
            serde_json::to_vec(&valid.data).expect("unchanged valid bytes"),
            original
        );
    }

    for invalid in [
        json!(0),
        json!(1_001),
        json!(-1),
        json!(1.5),
        json!("61"),
        Value::Null,
    ] {
        validate_task_max_steps_bundle(&task("0.7", Some(invalid)))
            .expect_err("invalid max steps must fail closed");
    }
    validate_task_max_steps_bundle(&task("0.6", Some(json!(61))))
        .expect_err("max steps requires schema 0.7");

    let mut mismatch = task("0.7", Some(json!(61)));
    mismatch.data["stability_termination"] = json!({"max_steps": 62});
    assert!(
        validate_task_max_steps_bundle(&mismatch)
            .expect_err("root and stability max steps must match")
            .message
            .contains("must match")
    );
}

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
fn build_pages_derives_variant_any_of_only_without_positive_page_rule() {
    let mut converter = OperationConverter {
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

    converter.bundles[0].data["page_rules"] = json!({
        "operator": {"optional": ["page/operator_0"]}
    });
    let pages = converter.build_pages().unwrap();
    let operator = pages.pointer("/pages/0").unwrap();
    assert_eq!(operator.pointer("/required"), Some(&json!([])));
    assert_eq!(
        operator.pointer("/optional"),
        Some(&json!(["page/operator_0"]))
    );
    assert_eq!(operator.pointer("/any_of"), None);
}

#[test]
fn build_pages_matches_authoritative_mail_page_rule_output() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "arknights".to_string(),
        server: "cn".to_string(),
        locale: "zh-CN".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "mail-collect".to_string(),
            dir: PathBuf::from("operations/mail-collect"),
            data: json!({
                "schema_version": "0.5",
                "task_id": "mail-collect",
                "anchors": [
                    {"id":"mail_inbox_claimable","template":"assets/MAIL_RECEIVE_ALL.png","region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}},
                    {"id":"mail_inbox_cleared_black","template":"assets/MAIL_RETURN_BLACK.png","region":{"mode":"rect","rect":{"x":5,"y":6,"width":7,"height":8}}},
                    {"id":"mail_inbox_cleared_white","template":"assets/MAIL_RETURN_WHITE.png","region":{"mode":"rect","rect":{"x":9,"y":10,"width":11,"height":12}}},
                    {"id":"mail_post_claim_done","template":"assets/MAIL_RECEIVED_ALL.png","region":{"mode":"rect","rect":{"x":13,"y":14,"width":15,"height":16}}},
                    {"id":"mail_post_claim_reward","template":"assets/MAIL_REWARD_OVERLAY.png","region":{"mode":"rect","rect":{"x":17,"y":18,"width":19,"height":20}}},
                    {"id":"negative_mail_popup_confirm","template":"assets/NEGATIVE_POPUP_CONFIRM.png","region":{"mode":"rect","rect":{"x":21,"y":22,"width":23,"height":24}}},
                    {"id":"negative_mail_offline_confirm","template":"assets/NEGATIVE_OFFLINE_CONFIRM.png","region":{"mode":"rect","rect":{"x":25,"y":26,"width":27,"height":28}}}
                ],
                "entry_page": "mail_inbox_claimable",
                "target_page": ["mail_inbox_claimable", "mail_post_claim"],
                "error_pages": [
                    "negative_mail_popup_confirm",
                    "negative_mail_offline_confirm"
                ],
                "page_rules": {
                    "mail_inbox_claimable": {
                        "any_of": [[
                            "page/mail_inbox_cleared_black",
                            "page/mail_inbox_cleared_white"
                        ]],
                        "forbidden": [
                            "page/mail_post_claim_reward",
                            "page/negative_mail_popup_confirm",
                            "page/negative_mail_offline_confirm"
                        ]
                    },
                    "mail_post_claim": {
                        "required": ["page/mail_post_claim_reward"],
                        "forbidden": [
                            "page/negative_mail_popup_confirm",
                            "page/negative_mail_offline_confirm"
                        ]
                    }
                },
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pages = converter.build_pages().expect("build mail pages");
    let pages = pages["pages"].as_array().expect("pages array");
    let page = |id: &str| {
        pages
            .iter()
            .find(|page| page["id"] == id)
            .unwrap_or_else(|| panic!("missing generated page {id}"))
    };

    assert_eq!(
        page("arknights/mail_inbox_claimable"),
        &json!({
            "id": "arknights/mail_inbox_claimable",
            "required": ["page/mail_inbox_claimable"],
            "optional": [],
            "forbidden": [
                "page/mail_post_claim_reward",
                "page/negative_mail_popup_confirm",
                "page/negative_mail_offline_confirm"
            ],
            "any_of": [[
                "page/mail_inbox_cleared_black",
                "page/mail_inbox_cleared_white"
            ]]
        })
    );
    assert_eq!(
        page("arknights/mail_post_claim"),
        &json!({
            "id": "arknights/mail_post_claim",
            "required": ["page/mail_post_claim_reward"],
            "optional": [],
            "forbidden": [
                "page/negative_mail_popup_confirm",
                "page/negative_mail_offline_confirm"
            ]
        })
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
fn build_pages_materializes_page_referenced_only_by_error_pages() {
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "neutral".to_string(),
        server: "test".to_string(),
        locale: "en-US".to_string(),
        coordinate_space: json!({"width":1,"height":1}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "error-page-check".to_string(),
            dir: PathBuf::from("operations/error-page-check"),
            data: json!({
                "schema_version": "0.5",
                "task_id": "error-page-check",
                "anchors": [
                    {"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}},
                    {"id":"failure","template":"assets/FAILURE.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}}
                ],
                "entry_page": "home",
                "target_page": "home",
                "error_pages": ["failure"],
                "operations": []
            }),
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };

    let pages = converter.build_pages().expect("build pages");

    assert!(
        pages["pages"]
            .as_array()
            .expect("pages array")
            .iter()
            .any(|page| page["id"] == "neutral/failure"),
        "error-pages-only reference must be materialized"
    );
}

#[test]
fn build_pages_materializes_and_validates_scheduling_outcome_references() {
    let bundle = |outcome_key: &str| Bundle {
        task_id: "outcome-page-check".to_string(),
        dir: PathBuf::from("operations/outcome-page-check"),
        data: json!({
            "schema_version": "0.6",
            "task_id": "outcome-page-check",
            "anchors": [
                {"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}},
                {"id":"result","template":"assets/RESULT.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}}
            ],
            "entry_page": "home",
            "target_page": "home",
            "scheduling_outcome": {
                "mappings": [{
                    "outcome_key": outcome_key,
                    "effect": "no_designated_effect",
                    "terminal_pages": ["result"]
                }]
            },
            "operations": []
        }),
    };
    let converter = OperationConverter {
        root: PathBuf::from("."),
        game: "neutral".to_string(),
        server: "test".to_string(),
        locale: "en-US".to_string(),
        coordinate_space: json!({"width":1,"height":1}),
        defaults: json!({"template_threshold":0.9}),
        resource_ids: HashSet::new(),
        bundles: vec![bundle("opaque-result")],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    };
    let pages = converter.build_pages().expect("build outcome pages");
    assert!(
        pages["pages"]
            .as_array()
            .expect("pages array")
            .iter()
            .any(|page| page["id"] == "neutral/result")
    );

    let invalid = OperationConverter {
        bundles: vec![bundle("invalid outcome key")],
        ..converter
    };
    let error = invalid
        .build_pages()
        .expect_err("invalid scheduling outcome");
    assert!(error.message.contains("scheduling_outcome"));
}

fn one_pixel_png() -> &'static [u8] {
    &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0,
        3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}

fn write_error_page_fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let task_dir = root.path().join("operations/error-page-check");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    for asset in ["HOME.png", "SUCCESS.png", "ALTERNATE.png", "FAILURE.png"] {
        fs::write(task_dir.join("assets").join(asset), one_pixel_png()).unwrap();
    }
    fs::write(
        root.path().join("operations/resources.json"),
        serde_json::to_vec(&json!({"resources":[]})).unwrap(),
    )
    .unwrap();
    fs::write(
        task_dir.join("task.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.5",
            "task_id": "error-page-check",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "coordinate_space": {"width":1,"height":1},
            "defaults": {"template_threshold":0.9},
            "anchors": [
                {"id":"home","template":"assets/HOME.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}},
                {"id":"success","template":"assets/SUCCESS.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}},
                {"id":"alternate","template":"assets/ALTERNATE.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}},
                {"id":"failure","template":"assets/FAILURE.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}}
            ],
            "entry_page": "home",
            "target_page": "success",
            "error_pages": ["failure"],
            "page_rules": {
                "success": {"forbidden": ["page/failure"]}
            },
            "operations": []
        }))
        .unwrap(),
    )
    .unwrap();
    root
}

fn update_error_page_fixture_task(root: &Path, update: impl FnOnce(&mut Value)) {
    let task_path = root.join("operations/error-page-check/task.json");
    let mut task: Value = serde_json::from_slice(&fs::read(&task_path).unwrap()).unwrap();
    update(&mut task);
    fs::write(task_path, serde_json::to_vec_pretty(&task).unwrap()).unwrap();
}

#[test]
fn error_page_only_reference_is_consumed_by_formal_detector() {
    let root = write_error_page_fixture();
    let converter = OperationConverter::load(root.path(), None, None, None).unwrap();
    let outputs = converter.build_all().unwrap();
    let evaluator = actingcommand_recognition_pack::RecognitionEvaluator::new(
        root.path().to_path_buf(),
        serde_json::from_value(outputs.pack.clone()).unwrap(),
    )
    .unwrap();
    let page_set: actingcommand_page_detector::PageSet =
        serde_json::from_value(outputs.pages.clone()).unwrap();
    let detector = actingcommand_page_detector::PageDetector::new(page_set).unwrap();
    let scene = actingcommand_recognition::Scene::from_png(one_pixel_png()).unwrap();

    detector.validate(&evaluator).unwrap();
    assert!(detector.contains_page("neutral/failure"));
    let evaluation = detector
        .evaluate_page(&evaluator, &scene, "neutral/failure")
        .unwrap();
    assert_eq!(evaluation.page_id, "neutral/failure");
    assert!(evaluation.matched);
}

#[test]
fn error_page_conversion_is_byte_deterministic() {
    let root = write_error_page_fixture();
    let converter = OperationConverter::load(root.path(), None, None, None).unwrap();

    let first = converter.build_all().unwrap();
    let second = converter.build_all().unwrap();
    let serialize = |outputs: ConvertOutputs| {
        serde_json::to_vec(&json!({
            "pack": outputs.pack,
            "pages": outputs.pages,
            "navigation": outputs.navigation,
            "index": outputs.index,
            "primitives": outputs.primitives
        }))
        .unwrap()
    };

    assert_eq!(serialize(first), serialize(second));
}

#[test]
fn invalid_error_page_identifiers_fail_loud() {
    for (case, error_pages, expected) in [
        ("wrong-shape", json!("failure"), "must be an array"),
        ("empty", json!([""]), "non-empty"),
        ("duplicate", json!(["failure", "failure"]), "duplicate"),
        ("missing", json!(["missing"]), "no matching anchor"),
    ] {
        let root = write_error_page_fixture();
        update_error_page_fixture_task(root.path(), |task| {
            task["error_pages"] = error_pages;
        });

        let error = OperationConverter::load(root.path(), None, None, None).expect_err(case);

        assert_eq!(error.code, "package_invalid");
        assert!(
            error.message.contains(expected),
            "{case}: {}",
            error.message
        );
    }
}

fn finite_page_operation(id: &str, to: Value, expect_after: Option<Value>) -> Value {
    let mut operation = json!({
        "id": id,
        "purpose": "exercise finite postcondition declarations",
        "from": "home",
        "to": to,
        "click": {"kind":"point","x":0,"y":0},
        "unguarded_trusted_coordinate": true,
        "consumes": [],
        "produces": []
    });
    if let Some(expect_after) = expect_after {
        operation["expect_after"] = expect_after;
    }
    operation
}

#[test]
fn finite_page_sets_are_normalized_materialized_and_not_truncated() {
    let root = write_error_page_fixture();
    update_error_page_fixture_task(root.path(), |task| {
        task["target_page"] = json!(["success", "alternate"]);
        task["operations"] = json!([
            finite_page_operation("set_destination", json!(["success", "alternate"]), None,),
            finite_page_operation(
                "expect_after_destination",
                Value::Null,
                Some(json!({
                    "page_id": ["success", "alternate"],
                    "timeout_ms": 10
                })),
            ),
        ]);
    });

    let converter = OperationConverter::load(root.path(), None, None, None).expect("load");
    let outputs = converter.build_all().expect("convert finite sets");
    let selected = converter
        .build_selected(&["error-page-check".to_string()])
        .expect("selected finite sets");
    let canonical = converter
        .canonical_task("error-page-check")
        .expect("canonical task");

    let page_ids = outputs.pages["pages"]
        .as_array()
        .expect("pages")
        .iter()
        .filter_map(|page| page.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    assert!(page_ids.contains("neutral/success"));
    assert!(page_ids.contains("neutral/alternate"));
    assert_eq!(selected.pages, outputs.pages);
    assert_eq!(canonical["target_page"], json!(["alternate", "success"]));
    assert_eq!(
        canonical["operations"][0]["to"],
        json!(["alternate", "success"])
    );
    assert_eq!(
        canonical["operations"][1]["expect_after"]["page_id"],
        json!(["alternate", "success"])
    );
    assert_eq!(
        outputs.primitives["primitives"][0]["to"],
        json!(["alternate", "success"])
    );
    assert_eq!(
        outputs.primitives["primitives"][1]["expect_after"]["page_id"],
        json!(["alternate", "success"])
    );
    assert!(
        outputs.navigation["navigation"]
            .as_array()
            .expect("navigation")
            .is_empty(),
        "set-shaped destinations must not be truncated into a legacy singleton edge"
    );

    let repeated = converter.build_all().expect("repeat finite-set conversion");
    let serialize = |outputs: &ConvertOutputs| {
        serde_json::to_vec(&json!({
            "pack": &outputs.pack,
            "pages": &outputs.pages,
            "navigation": &outputs.navigation,
            "index": &outputs.index,
            "primitives": &outputs.primitives,
        }))
        .expect("serialize outputs")
    };
    assert_eq!(serialize(&outputs), serialize(&repeated));
}

#[test]
fn singleton_page_declarations_keep_legacy_scalar_shapes() {
    let root = write_error_page_fixture();
    update_error_page_fixture_task(root.path(), |task| {
        task["target_page"] = json!("success");
        task["operations"] = json!([finite_page_operation(
            "singleton_destination",
            json!("alternate"),
            None,
        )]);
    });

    let converter = OperationConverter::load(root.path(), None, None, None).expect("load");
    let outputs = converter.build_all().expect("singleton conversion");
    let canonical = converter
        .canonical_task("error-page-check")
        .expect("canonical task");

    assert_eq!(canonical["target_page"], json!("success"));
    assert_eq!(canonical["operations"][0]["to"], json!("alternate"));
    assert_eq!(
        outputs.primitives["primitives"][0]["to"],
        json!("alternate")
    );
    assert_eq!(
        outputs.navigation["navigation"][0]["to_page"],
        json!("neutral/alternate")
    );
}

#[test]
fn destructive_actions_keep_exact_pre_normalization_null_semantics() {
    let root = write_error_page_fixture();
    update_error_page_fixture_task(root.path(), |task| {
        task["operations"] = json!([
            finite_page_operation(
                "explicit_null",
                Value::Null,
                Some(json!({
                    "page_id": ["success", "alternate"],
                    "timeout_ms": 10
                })),
            ),
            {
                "id": "omitted_to",
                "purpose": "omitted destination remains non-destructive",
                "from": "home",
                "click": {"kind":"point","x":0,"y":0},
                "unguarded_trusted_coordinate": true,
                "consumes": [],
                "produces": []
            },
        ]);
    });

    let converter = OperationConverter::load(root.path(), None, None, None).expect("load");
    let navigation = converter.build_navigation().expect("navigation");
    let destructive = navigation["destructive_actions"]
        .as_array()
        .expect("destructive actions");

    assert_eq!(destructive.len(), 1);
    assert_eq!(destructive[0]["id"], "explicit_null");
    assert_eq!(
        serde_json::to_vec(&navigation["destructive_actions"]).expect("serialize"),
        br#"[{"task_id":"error-page-check","page":"neutral/home","id":"explicit_null","purpose":"exercise finite postcondition declarations","click":{"kind":"point","point":"0,0"},"expect_after":{"page_id":["success","alternate"],"timeout_ms":10},"verify_template":null,"consumes":[],"produces":[]}]"#
    );
}

#[test]
fn invalid_finite_page_declarations_fail_closed() {
    for (case, update, expected) in [
        (
            "empty-terminal-set",
            ("target_page", json!([]), None),
            "target_page",
        ),
        (
            "duplicate-terminal-set",
            ("target_page", json!(["success", "success"]), None),
            "duplicate",
        ),
        (
            "malformed-destination",
            ("to", json!(7), None),
            "destination",
        ),
        (
            "empty-destination-set",
            ("to", json!([]), None),
            "destination",
        ),
        (
            "duplicate-destination-set",
            ("to", json!(["success", "success"]), None),
            "duplicate",
        ),
        (
            "missing-destination",
            ("to", json!("missing"), None),
            "missing",
        ),
        (
            "conflicting-destination",
            ("to", json!("success"), Some(json!({"page_id":"alternate"}))),
            "conflicting",
        ),
    ] {
        let root = write_error_page_fixture();
        update_error_page_fixture_task(root.path(), |task| {
            if update.0 == "target_page" {
                task["target_page"] = update.1;
            } else {
                task["operations"] = json!([finite_page_operation("invalid", update.1, update.2)]);
            }
        });

        let error = OperationConverter::load(root.path(), None, None, None)
            .and_then(|converter| converter.build_all())
            .expect_err(case);
        assert!(
            error.message.contains(expected),
            "{case}: {}",
            error.message
        );
    }
}

#[test]
fn malformed_expect_after_declarations_fail_closed() {
    for (case, expect_after, expected) in [
        ("not-an-object", json!("success"), "must be an object"),
        (
            "missing-page-id",
            json!({"timeout_ms":10}),
            "missing page_id",
        ),
        ("empty-page-set", json!({"page_id":[]}), "non-empty"),
        (
            "duplicate-page-set",
            json!({"page_id":["success","success"]}),
            "duplicate",
        ),
        (
            "missing-page-reference",
            json!({"page_id":"missing"}),
            "missing",
        ),
    ] {
        let root = write_error_page_fixture();
        update_error_page_fixture_task(root.path(), |task| {
            task["operations"] = json!([finite_page_operation(
                "invalid_expectation",
                Value::Null,
                Some(expect_after),
            )]);
        });

        let error = OperationConverter::load(root.path(), None, None, None)
            .and_then(|converter| converter.build_all())
            .expect_err(case);
        assert!(
            error.message.contains(expected),
            "{case}: {}",
            error.message
        );
    }
}

#[test]
fn error_page_rule_and_asset_references_fail_loud() {
    let root = write_error_page_fixture();
    update_error_page_fixture_task(root.path(), |task| {
        task["page_rules"]["failure"] = json!({"required": ["page/missing"]});
    });
    let converter = OperationConverter::load(root.path(), None, None, None).unwrap();
    let error = converter.build_all().expect_err("missing page-rule target");
    assert!(error.message.contains("page/missing"));

    let root = write_error_page_fixture();
    fs::remove_file(
        root.path()
            .join("operations/error-page-check/assets/FAILURE.png"),
    )
    .unwrap();
    let error = OperationConverter::load(root.path(), None, None, None)
        .expect_err("missing error-page asset");
    assert!(error.message.contains("FAILURE.png"));
    assert!(error.message.contains("missing on disk"));
}

#[test]
fn selected_build_requires_error_page_definition_in_selected_closure() {
    let root = write_error_page_fixture();
    let task_dir = root.path().join("operations/selected-only");
    fs::create_dir_all(task_dir.join("assets")).unwrap();
    fs::write(task_dir.join("assets/SELECTED.png"), one_pixel_png()).unwrap();
    fs::write(
        task_dir.join("task.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "0.5",
            "task_id": "selected-only",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "coordinate_space": {"width":1,"height":1},
            "defaults": {"template_threshold":0.9},
            "anchors": [
                {"id":"selected","template":"assets/SELECTED.png","region":{"mode":"rect","rect":{"x":0,"y":0,"width":1,"height":1}}}
            ],
            "target_page": "selected",
            "error_pages": ["failure"],
            "page_rules": {
                "selected": {"forbidden": ["page/failure"]}
            },
            "operations": []
        }))
        .unwrap(),
    )
    .unwrap();
    let converter = OperationConverter::load(root.path(), None, None, None).unwrap();

    let error = converter
        .build_selected(&["selected-only".to_string()])
        .expect_err("selected error page definition must remain available");

    assert!(error.message.contains("error_pages"));
    assert!(error.message.contains("no matching anchor definition"));
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
                    "color_probes": [{
                        "id": "color/selected",
                        "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                        "expected": [10, 20, 30]
                    }],
                    "verify_templates": [{
                        "id": "template/selected",
                        "template": "assets/SELECTED.png",
                        "region": {"mode":"rect","rect":{"x":10,"y":20,"width":30,"height":40}},
                        "threshold": 0.97
                    }],
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
                        "home": {"required": ["page/home", "color/selected", "template/selected"]},
                        "depot": {"forbidden": ["page/home", "color/selected", "template/selected", "page/recruit", "color/nonresident", "template/nonresident"]},
                        "recruit": {"forbidden": ["page/home"]},
                        "quickswitch_dropdown": {"optional": ["page/depot", "color/selected", "template/selected", "page/friends", "color/nonresident", "template/nonresident"]}
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

    let bundles = converter
        .prune_page_rules_for_selected_build(converter.bundles.clone(), &[])
        .expect("prune selected page rules");
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
        rules.get("home").unwrap().get("required").unwrap(),
        &json!(["page/home", "color/selected", "template/selected"])
    );
    assert_eq!(
        rules
            .get("depot")
            .unwrap()
            .get("forbidden")
            .unwrap()
            .as_array()
            .unwrap(),
        &vec![
            json!("page/home"),
            json!("color/selected"),
            json!("template/selected"),
        ]
    );
    assert_eq!(
        rules
            .get("quickswitch_dropdown")
            .unwrap()
            .get("optional")
            .unwrap()
            .as_array()
            .unwrap(),
        &vec![
            json!("page/depot"),
            json!("color/selected"),
            json!("template/selected"),
        ]
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

fn valid_ocr_declaration(id: &str) -> Value {
    json!({
        "id": id,
        "region": {
            "mode": "rect",
            "rect": {"x": 10, "y": 20, "width": 300, "height": 40}
        },
        "languages": ["en", "zh"],
        "timeout_ms": 5_000,
        "match_mode": "contains",
        "expected": ["synthetic text"],
        "case_sensitive": false,
        "minimum_confidence": 0.8,
        "model_ref": "PP-OCRv6_medium",
        "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "click": {"x": 11, "y": 21, "width": 30, "height": 20}
    })
}

fn ocr_test_converter(schema_version: &str, ocr_targets: Option<Value>) -> OperationConverter {
    let mut data = json!({
        "schema_version": schema_version,
        "task_id": "ocr-check",
        "anchors": [],
        "operations": []
    });
    if let Some(ocr_targets) = ocr_targets {
        data["ocr_targets"] = ocr_targets;
    }
    OperationConverter {
        root: PathBuf::from("."),
        game: "neutral".to_string(),
        server: "test".to_string(),
        locale: "en-US".to_string(),
        coordinate_space: json!({"width":1280,"height":720}),
        defaults: json!({"template_threshold":0.9,"color_max_distance":20.0}),
        resource_ids: HashSet::new(),
        bundles: vec![Bundle {
            task_id: "ocr-check".to_string(),
            dir: PathBuf::from("operations/ocr-check"),
            data,
        }],
        existing_navigation: None,
        maa_task_overlays: HashMap::new(),
    }
}

#[test]
fn build_pack_maps_schema_06_ocr_target_to_canonical_output() {
    let pack = ocr_test_converter(
        "0.6",
        Some(json!([valid_ocr_declaration("ocr/synthetic-label")])),
    )
    .build_pack()
    .expect("valid OCR declaration");

    assert_eq!(
        pack.pointer("/targets/0"),
        Some(&json!({
            "type": "ocr",
            "id": "ocr/synthetic-label",
            "region": {"x":10,"y":20,"width":300,"height":40},
            "languages": ["en", "zh"],
            "timeout_ms": 5_000,
            "match_mode": "contains",
            "expected": ["synthetic text"],
            "case_sensitive": false,
            "minimum_confidence": 0.8,
            "model_ref": "PP-OCRv6_medium",
            "model_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "click": {"x":11,"y":21,"width":30,"height":20}
        }))
    );
}

#[test]
fn build_pack_without_ocr_targets_preserves_existing_output() {
    let pack = ocr_test_converter("0.6", None)
        .build_pack()
        .expect("existing no-OCR output");
    let serialized = serde_json::to_string_pretty(&pack).unwrap();

    assert_eq!(
        pack,
        json!({
            "schema_version": "0.6",
            "converter_schema_version": "0.5",
            "generated": true,
            "generated_by": "actinglab resource convert",
            "game": "neutral",
            "server": "test",
            "locale": "en-US",
            "coordinate_space": {"width":1280,"height":720},
            "defaults": {"template_threshold":0.9,"color_max_distance":20.0},
            "targets": []
        })
    );
    assert_eq!(
        serialized,
        concat!(
            "{\n",
            "  \"schema_version\": \"0.6\",\n",
            "  \"converter_schema_version\": \"0.5\",\n",
            "  \"generated\": true,\n",
            "  \"generated_by\": \"actinglab resource convert\",\n",
            "  \"game\": \"neutral\",\n",
            "  \"server\": \"test\",\n",
            "  \"locale\": \"en-US\",\n",
            "  \"coordinate_space\": {\n",
            "    \"width\": 1280,\n",
            "    \"height\": 720\n",
            "  },\n",
            "  \"defaults\": {\n",
            "    \"template_threshold\": 0.9,\n",
            "    \"color_max_distance\": 20.0\n",
            "  },\n",
            "  \"targets\": []\n",
            "}"
        )
    );
}

#[test]
fn ocr_targets_reject_wrong_schema_container_and_entry_shape() {
    for (label, converter) in [
        (
            "schema 0.5",
            ocr_test_converter("0.5", Some(json!([valid_ocr_declaration("ocr/name")]))),
        ),
        ("non-array", ocr_test_converter("0.6", Some(json!({})))),
        (
            "non-object",
            ocr_test_converter("0.6", Some(json!(["ocr/name"]))),
        ),
    ] {
        converter
            .build_pack()
            .expect_err(&format!("{label} must fail closed"));
    }

    let mut declaration = valid_ocr_declaration("ocr/name");
    declaration["unsupported"] = json!(true);
    let error = ocr_test_converter("0.6", Some(json!([declaration])))
        .build_pack()
        .expect_err("unknown OCR field must fail closed");
    assert!(error.message.contains("unsupported field 'unsupported'"));
}

#[test]
fn ocr_targets_reject_invalid_existing_contract_fields() {
    let mut cases = Vec::new();
    let mut invalid = valid_ocr_declaration("ocr/invalid-region");
    invalid["region"]["rect"]["width"] = json!(0);
    cases.push(("region", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-languages");
    invalid["languages"] = json!([]);
    cases.push(("languages", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-expected");
    invalid["expected"] = json!([]);
    cases.push(("expected", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-timeout");
    invalid["timeout_ms"] = json!(0);
    cases.push(("timeout", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-match-mode");
    invalid["match_mode"] = json!("regex");
    cases.push(("match_mode", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-confidence");
    invalid["minimum_confidence"] = json!(1.1);
    cases.push(("minimum_confidence", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-model");
    invalid["model_ref"] = json!("OtherModel");
    cases.push(("model_ref", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-hash");
    invalid["model_sha256"] = json!("A");
    cases.push(("model_sha256", invalid));
    let mut invalid = valid_ocr_declaration("ocr/invalid-click");
    invalid["click"]["height"] = json!(0);
    cases.push(("click", invalid));

    for (label, declaration) in cases {
        ocr_test_converter("0.6", Some(json!([declaration])))
            .build_pack()
            .expect_err(&format!("invalid {label} must fail closed"));
    }
}

#[test]
fn ocr_target_duplicates_coalesce_only_when_identical() {
    let declaration = valid_ocr_declaration("ocr/synthetic-label");
    let pack = ocr_test_converter(
        "0.6",
        Some(json!([declaration.clone(), declaration.clone()])),
    )
    .build_pack()
    .expect("identical duplicate follows first-target ownership");
    assert_eq!(pack["targets"].as_array().unwrap().len(), 1);

    let mut conflicting = declaration.clone();
    conflicting["timeout_ms"] = json!(6_000);
    let error = ocr_test_converter("0.6", Some(json!([declaration, conflicting])))
        .build_pack()
        .expect_err("incompatible duplicate must fail closed");
    assert!(
        error
            .message
            .contains("conflicts with an earlier recognition target")
    );

    let mut colliding = ocr_test_converter(
        "0.6",
        Some(json!([valid_ocr_declaration("ocr/synthetic-label")])),
    );
    colliding.bundles[0].data["color_probes"] = json!([{
        "id": "ocr/synthetic-label",
        "region": {"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}},
        "expected": [1, 2, 3]
    }]);
    colliding
        .build_pack()
        .expect_err("OCR ID collision with an existing target must fail closed");
}

#[test]
fn selected_build_retains_required_ocr_target_closure() {
    let root = tempfile::tempdir().expect("fixture root");
    fs::create_dir_all(root.path().join("operations/selected/assets")).unwrap();
    fs::create_dir_all(root.path().join("operations/unselected/assets")).unwrap();
    fs::create_dir_all(root.path().join("recognition")).unwrap();
    fs::write(
        root.path().join("operations/resources.json"),
        serde_json::to_vec(&json!({"resources":[]})).unwrap(),
    )
    .unwrap();
    for (task_id, page, ocr_id) in [
        ("selected", "selected-page", "ocr/selected"),
        ("unselected", "unselected-page", "ocr/unselected"),
    ] {
        let task_dir = root.path().join("operations").join(task_id);
        fs::write(task_dir.join("assets/PAGE.png"), b"fixture").unwrap();
        let mut page_rules = Map::new();
        page_rules.insert(
            page.to_string(),
            json!({
                "required": [format!("page/{page}")],
                "optional": [ocr_id]
            }),
        );
        fs::write(
            task_dir.join("task.json"),
            serde_json::to_vec(&json!({
                "schema_version": "0.6",
                "task_id": task_id,
                "game": "neutral",
                "server_scope": ["test"],
                "locale": "en-US",
                "coordinate_space": {"width":1280,"height":720},
                "defaults": {"template_threshold":0.9,"color_max_distance":20.0},
                "anchors": [{
                    "id": page,
                    "template": "assets/PAGE.png",
                    "region": {"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}}
                }],
                "ocr_targets": [valid_ocr_declaration(ocr_id)],
                "entry_page": page,
                "target_page": page,
                "page_rules": Value::Object(page_rules),
                "operations": []
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let converter = OperationConverter::load(root.path(), None, None, None).unwrap();

    let outputs = converter
        .build_selected(&["selected".to_string()])
        .expect("selected build with OCR closure");

    let target_ids = array_field(&outputs.pack, "targets")
        .iter()
        .filter_map(|target| target.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(target_ids.contains(&"ocr/selected"));
    assert!(!target_ids.contains(&"ocr/unselected"));
    assert_eq!(
        outputs.pages.pointer("/pages/0/optional"),
        Some(&json!(["ocr/selected"]))
    );
    let path = root.path().join("operations/selected/task.json");
    let mut source: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    source["ocr_targets"][0]["region"] = json!({"mode":"template_relative","anchor_target_id":"page/unselected-page","offset":{"x":-2,"y":3},"width":30,"height":10});
    source["page_rules"]["selected-page"]["optional"] =
        json!(["ocr/selected", "page/unselected-page"]);
    fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let selected = OperationConverter::load(root.path(), None, None, None)
        .unwrap()
        .build_selected(&["selected".into()])
        .unwrap();
    let targets = selected.pack["targets"].as_array().unwrap();
    assert!(
        targets
            .iter()
            .any(|target| target["id"] == "page/unselected-page" && target["type"] == "template")
    );
    assert!(
        !targets
            .iter()
            .any(|target| target["id"] == "ocr/unselected")
    );
    assert_eq!(
        targets
            .iter()
            .find(|target| target["id"] == "ocr/selected")
            .unwrap()["region"],
        source["ocr_targets"][0]["region"]
    );
    assert_eq!(
        selected.pages.pointer("/pages/0/optional"),
        Some(&json!(["ocr/selected", "page/unselected-page"]))
    );
    for anchor in ["missing", "ocr/selected"] {
        source["ocr_targets"][0]["region"]["anchor_target_id"] = json!(anchor);
        fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
        OperationConverter::load(root.path(), None, None, None)
            .unwrap()
            .build_selected(&["selected".into()])
            .expect_err("missing or non-template anchor rejected by canonical admission");
    }
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
                "method": "NCC",
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

    let facts_tasks = (0..64)
        .map(|index| {
            let task_id = format!("SyntheticTask{index:02}");
            json!({
                "task_id": {
                    "kind": "string",
                    "value": task_id,
                    "source_task_id": task_id,
                    "source_json_path": "synthetic/tasks.json",
                    "source_file_sha256": "0".repeat(64),
                    "origin": "declared"
                }
            })
        })
        .collect::<Vec<_>>();
    let facts = json!({
        "schema_version": "0.2",
        "cli_version": "0.1.0",
        "runtime_version": "0.1.0",
        "ok": true,
        "command": "resource compile-maa",
        "data": {
            "schema_version": "actingcommand.maa-task-facts-set.v1",
            "tasks": facts_tasks
        }
    });
    let facts_bytes = serde_json::to_vec_pretty(&facts).unwrap();
    let facts_sha256 = format!("{:x}", Sha256::digest(&facts_bytes));
    let facts_dir = root.path().join("upstream-sync");
    fs::create_dir_all(&facts_dir).unwrap();
    fs::write(facts_dir.join("maa.tasks.json"), facts_bytes).unwrap();

    let mappings = (0..64)
        .map(|index| {
            let (product_heading, page_id) = match index % 3 {
                0 => ("warehouse", "arknights/depot"),
                1 => ("home_sanity", "arknights/home"),
                _ => ("stage_proxy_settlement", "arknights/terminal"),
            };
            let role = match index % 5 {
                0 => "page_anchor",
                1 => "page_transition",
                2 => "page_operation",
                3 => "observation",
                _ => "topology",
            };
            json!({
                "source_task_id": format!("SyntheticTask{index:02}"),
                "product_heading": product_heading,
                "page_id": page_id,
                "role": role
            })
        })
        .collect::<Vec<_>>();
    let mapping_dir = root.path().join("tasks");
    fs::create_dir_all(&mapping_dir).unwrap();
    fs::write(
        mapping_dir.join("maa-semantic-mapping.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": "actingcommand.maa-semantic-mapping.v1",
            "facts_container": {
                "path": "ours/upstream-sync/maa.tasks.json",
                "sha256": facts_sha256,
                "data_schema_version": "actingcommand.maa-task-facts-set.v1",
                "task_count": 64
            },
            "mappings": mappings
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
        Some("ncc")
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
    for output in [
        &outputs.pack,
        &outputs.pages,
        &outputs.navigation,
        &outputs.index,
        &outputs.primitives,
    ] {
        assert_eq!(
            output.get("schema_version").and_then(Value::as_str),
            Some("0.6")
        );
    }
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
fn schema_0_6_converter_rejects_deprecated_template_primitives_with_migration_diagnostics() {
    let build = |source: Value| {
        pack_target(
            &source,
            "fixture/target",
            "operations/fixture/assets/TARGET.png",
            Value::String("full_frame".to_string()),
            json!(0.9),
            None,
            None,
        )
    };

    for (source, expected) in [
        (json!({"method":"RGBCount"}), "rgb_count"),
        (json!({"method":"HSVCount"}), "hsv_count"),
        (json!({"maskRange":[7,199]}), "template mask"),
    ] {
        let error = build(source).expect_err("deprecated primitive must not be emitted");
        assert!(
            error.message.contains(expected),
            "expected {expected:?} in {:?}",
            error.message
        );
        assert!(error.message.contains("schema 0.6"));
        assert!(error.message.contains("migrate"));
    }
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

// Task Contract: Workflow #269 / #269-A2B-MAPPING-ADMISSION-IMPLEMENT-v2
// (comment 5533851835). Test class: specification criterion.
#[test]
fn resource_convert_strictly_admits_maa_semantic_mapping_before_outputs() {
    for (task_count, use_overlay) in [(64, true), (1, false), (0, false), (0, true)] {
        let (root, maa_dir) = write_synthetic_maa_convert_fixture();
        let mapping_path = root.path().join("tasks/maa-semantic-mapping.json");
        let facts_path = root.path().join("upstream-sync/maa.tasks.json");
        if task_count == 0 {
            fs::remove_file(&mapping_path).expect("remove mapping");
            fs::remove_file(&facts_path).expect("remove facts");
        } else if task_count != 64 {
            let mut facts: Value =
                serde_json::from_slice(&fs::read(&facts_path).expect("facts bytes"))
                    .expect("facts JSON");
            facts["data"]["tasks"]
                .as_array_mut()
                .unwrap()
                .truncate(task_count);
            let facts_bytes = serde_json::to_vec_pretty(&facts).expect("serialize facts");
            fs::write(&facts_path, &facts_bytes).expect("write facts");
            let mut mapping: Value =
                serde_json::from_slice(&fs::read(&mapping_path).expect("mapping bytes"))
                    .expect("mapping JSON");
            mapping["facts_container"]["task_count"] = json!(task_count);
            mapping["facts_container"]["sha256"] =
                json!(format!("{:x}", Sha256::digest(&facts_bytes)));
            mapping["mappings"]
                .as_array_mut()
                .unwrap()
                .truncate(task_count);
            mapping["mappings"][0]["product_heading"] = json!("external_heading");
            fs::write(
                &mapping_path,
                serde_json::to_vec_pretty(&mapping).expect("serialize mapping"),
            )
            .expect("write mapping");
        }
        let summary = resource_convert(ResourceConvertRequest {
            repo: root.path().to_path_buf(),
            game: None,
            server: None,
            locale: None,
            maa_tasks_root: use_overlay.then_some(maa_dir),
            dry_run: task_count != 0,
        })
        .expect("valid resource conversion");
        assert_eq!(summary.maa_semantic_mappings, task_count);
        assert_eq!(
            serde_json::to_value(&summary)
                .expect("serialized response")
                .get("maa_semantic_mappings")
                .and_then(Value::as_u64),
            Some(task_count as u64)
        );
    }

    #[derive(Clone, Copy, Debug)]
    enum InvalidCase {
        MissingMapping,
        MissingContainer,
        HashMismatch,
        MappingSchemaMismatch,
        DataSchemaMismatch,
        TaskCountMismatch,
        ZeroTaskCount,
        ExcessTaskCount,
        ActualTaskCountMismatch,
        MissingTask,
        DuplicateTask,
        UnknownTask,
        OrderMismatch,
        MalformedHeading,
        UnknownRole,
        MalformedPageId,
        RowKeyExpansion,
    }

    let cases = [
        (InvalidCase::MissingMapping, "maa-semantic-mapping.json"),
        (InvalidCase::MissingContainer, "maa.tasks.json"),
        (InvalidCase::HashMismatch, "SHA-256"),
        (InvalidCase::MappingSchemaMismatch, "mapping schema"),
        (InvalidCase::DataSchemaMismatch, "facts data schema"),
        (InvalidCase::TaskCountMismatch, "task_count"),
        (InvalidCase::ZeroTaskCount, "task_count must be within"),
        (InvalidCase::ExcessTaskCount, "task_count must be within"),
        (InvalidCase::ActualTaskCountMismatch, "actual task count"),
        (InvalidCase::MissingTask, "mapping row count"),
        (InvalidCase::DuplicateTask, "duplicate source_task_id"),
        (InvalidCase::UnknownTask, "unknown source_task_id"),
        (InvalidCase::OrderMismatch, "ordinal order"),
        (InvalidCase::MalformedHeading, "invalid product_heading"),
        (InvalidCase::UnknownRole, "unknown role"),
        (InvalidCase::MalformedPageId, "invalid page_id"),
        (InvalidCase::RowKeyExpansion, "unknown field"),
    ];

    for (case, expected_message) in cases {
        let (root, _maa_dir) = write_synthetic_maa_convert_fixture();
        let mapping_path = root.path().join("tasks/maa-semantic-mapping.json");
        let facts_path = root.path().join("upstream-sync/maa.tasks.json");
        let mut mapping: Value =
            serde_json::from_slice(&fs::read(&mapping_path).expect("mapping bytes"))
                .expect("mapping JSON");
        let mut facts: Value = serde_json::from_slice(&fs::read(&facts_path).expect("facts bytes"))
            .expect("facts JSON");
        let mut write_mapping = true;
        let mut write_facts = true;

        match case {
            InvalidCase::MissingMapping => {
                fs::remove_file(&mapping_path).expect("remove mapping");
                write_mapping = false;
            }
            InvalidCase::MissingContainer => {
                fs::remove_file(&facts_path).expect("remove facts");
                write_facts = false;
            }
            InvalidCase::HashMismatch => {
                mapping["facts_container"]["sha256"] = json!("0".repeat(64));
            }
            InvalidCase::MappingSchemaMismatch => {
                mapping["schema_version"] = json!("actingcommand.maa-semantic-mapping.v2");
            }
            InvalidCase::DataSchemaMismatch => {
                facts["data"]["schema_version"] = json!("actingcommand.maa-task-facts-set.v2");
            }
            InvalidCase::TaskCountMismatch => {
                mapping["facts_container"]["task_count"] = json!(63);
            }
            InvalidCase::ZeroTaskCount => {
                mapping["facts_container"]["task_count"] = json!(0);
            }
            InvalidCase::ExcessTaskCount => {
                mapping["facts_container"]["task_count"] =
                    json!(maa_task_graph::MAX_MAA_TASK_FACT_SELECTIONS + 1);
            }
            InvalidCase::ActualTaskCountMismatch => {
                facts["data"]["tasks"].as_array_mut().unwrap().pop();
            }
            InvalidCase::MissingTask => {
                mapping["mappings"].as_array_mut().unwrap().pop();
            }
            InvalidCase::DuplicateTask => {
                mapping["mappings"][63]["source_task_id"] = json!("SyntheticTask00");
            }
            InvalidCase::UnknownTask => {
                mapping["mappings"][63]["source_task_id"] = json!("SyntheticTaskUnknown");
            }
            InvalidCase::OrderMismatch => {
                mapping["mappings"].as_array_mut().unwrap().swap(0, 1);
            }
            InvalidCase::MalformedHeading => {
                mapping["mappings"][0]["product_heading"] = json!("invalid/heading");
            }
            InvalidCase::UnknownRole => {
                mapping["mappings"][0]["role"] = json!("unknown");
            }
            InvalidCase::MalformedPageId => {
                mapping["mappings"][0]["page_id"] = json!("arknights");
            }
            InvalidCase::RowKeyExpansion => {
                mapping["mappings"][0]
                    .as_object_mut()
                    .unwrap()
                    .insert("extra".to_string(), json!(true));
            }
        }

        if write_facts {
            let facts_bytes = serde_json::to_vec_pretty(&facts).expect("serialize facts");
            fs::write(&facts_path, &facts_bytes).expect("write facts");
            if matches!(
                case,
                InvalidCase::DataSchemaMismatch | InvalidCase::ActualTaskCountMismatch
            ) {
                mapping["facts_container"]["sha256"] =
                    json!(format!("{:x}", Sha256::digest(&facts_bytes)));
            }
        }
        if write_mapping {
            fs::write(
                &mapping_path,
                serde_json::to_vec_pretty(&mapping).expect("serialize mapping"),
            )
            .expect("write mapping");
        }

        let error = resource_convert(ResourceConvertRequest {
            repo: root.path().to_path_buf(),
            game: None,
            server: None,
            locale: None,
            maa_tasks_root: Some(root.path().join("missing-maa-tasks")),
            dry_run: false,
        })
        .expect_err("invalid semantic mapping must fail");
        assert_eq!(error.code, "package_invalid", "case {case:?}");
        assert!(
            error.message.contains(expected_message),
            "case {case:?}: expected {expected_message:?} in {:?}",
            error.message
        );
        for output in [
            "recognition/arknights.cn.pack.json",
            "recognition/arknights.cn.pages.json",
            "navigation/arknights.cn.navigation.json",
            "operations/operations.index.json",
            "operations/operations.primitives.json",
        ] {
            assert!(
                !root.path().join(output).exists(),
                "case {case:?} wrote {output}"
            );
        }
    }
}

#[test]
fn resource_convert_rejects_missing_coordinate_space_before_writing_outputs() {
    let (root, _maa_dir) = write_synthetic_maa_convert_fixture();
    let task_path = root.path().join("operations/synthetic-maa/task.json");
    let mut task: Value =
        serde_json::from_slice(&fs::read(&task_path).expect("read task")).expect("parse task");
    let task = task.as_object_mut().expect("task object");
    task.insert("game".to_string(), json!("neutral"));
    task.insert("server_scope".to_string(), json!(["test"]));
    task.remove("coordinate_space");
    fs::write(
        &task_path,
        serde_json::to_vec_pretty(&task).expect("serialize task"),
    )
    .expect("write task");

    let err = resource_convert(ResourceConvertRequest {
        repo: root.path().to_path_buf(),
        game: None,
        server: None,
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })
    .expect_err("missing coordinate_space must fail before output");

    assert!(err.message.contains("missing coordinate_space"));
    for output in [
        "recognition/neutral.test.pack.json",
        "recognition/neutral.test.pages.json",
        "navigation/neutral.test.navigation.json",
        "operations/operations.index.json",
        "operations/operations.primitives.json",
    ] {
        assert!(!root.path().join(output).exists(), "wrote {output}");
    }
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

    let canonical_task = converter.canonical_task("open-menu-drag").unwrap();
    assert_eq!(
        canonical_task.pointer("/operations/0/click"),
        Some(&json!({
            "kind": "drag",
            "duration_ms": 500,
            "from_rect": {"x": 100, "y": 110, "width": 20, "height": 25},
            "to_rect": {"x": 500, "y": 110, "width": 20, "height": 25}
        }))
    );
    assert!(canonical_task.pointer("/operations/0/click/from").is_none());
    assert!(canonical_task.pointer("/operations/0/click/to").is_none());

    let outputs = converter.build_all().unwrap();
    let primitive = outputs.primitives.pointer("/primitives/0").unwrap();

    assert!(primitive.get("guard").is_some_and(Value::is_null));
    assert_eq!(
        primitive
            .get("unguarded_trusted_coordinate")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        primitive.get("click"),
        Some(&json!({
            "kind": "drag",
            "duration_ms": 500,
            "from_rect": {"x": 100, "y": 110, "width": 20, "height": 25},
            "to_rect": {"x": 500, "y": 110, "width": 20, "height": 25}
        }))
    );
    assert!(primitive.pointer("/click/from").is_none());
    assert!(primitive.pointer("/click/to").is_none());
}

#[test]
fn source_drag_rejects_canonical_or_mixed_endpoint_spelling() {
    for click in [
        json!({
            "kind": "drag",
            "from_rect": {"x": 1, "y": 2, "width": 3, "height": 4},
            "to_rect": {"x": 5, "y": 6, "width": 7, "height": 8},
            "duration_ms": 500
        }),
        json!({
            "kind": "drag",
            "from": {"x": 1, "y": 2, "width": 3, "height": 4},
            "to": {"x": 5, "y": 6, "width": 7, "height": 8},
            "from_rect": {"x": 1, "y": 2, "width": 3, "height": 4},
            "to_rect": {"x": 5, "y": 6, "width": 7, "height": 8},
            "duration_ms": 500
        }),
    ] {
        let operation = json!({"id": "drag", "click": click});
        let mut errors = Vec::new();
        let bundle = Bundle {
            task_id: "fixture".to_string(),
            dir: PathBuf::from("operations/fixture"),
            data: json!({
                "schema_version": "0.6",
                "coordinate_space": {"width": 1280, "height": 720}
            }),
        };

        validate_click_shape(&bundle, &operation, &mut errors);

        assert!(
            errors
                .iter()
                .any(|error| error.contains("source drag click must use from/to")),
            "{errors:?}"
        );
    }
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
