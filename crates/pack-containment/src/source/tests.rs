// SPDX-License-Identifier: AGPL-3.0-only

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
fn color_check_region_is_flattened() {
    let input = json!({
        "region":{"mode":"rect","rect":{"x":1,"y":2,"width":3,"height":4}},
        "expected":[10,20,30]
    });
    assert_eq!(
        color_check_to_pack(Some(&input), "template")
            .unwrap()
            .unwrap(),
        json!({"region":{"x":1,"y":2,"width":3,"height":4},"expected":[10,20,30]})
    );

    // #279 TEMPLATE-MATCH-COLOR-v1 declaration-boundary scenario, using the
    // existing converter and alias propagation rather than a generated fixture.
    let relative = json!({"region":{"mode":"template_relative","anchor_target_id":"page/marker","offset":{"x":-2,"y":3},"width":4,"height":2},"expected":[10,20,30]});
    assert_eq!(
        color_check_to_pack(Some(&relative), "page/marker").unwrap(),
        Some(relative.clone())
    );
    assert!(color_check_to_pack(Some(&relative), "page/other").is_err());
    for (pointer, bad) in [
        ("/region/width", json!(0)),
        ("/region/offset/x", json!(2147483648_u64)),
        ("/expected", json!([256, 0, 0])),
    ] {
        let mut invalid = relative.clone();
        *invalid.pointer_mut(pointer).unwrap() = bad;
        assert!(
            color_check_to_pack(Some(&invalid), "page/marker").is_err(),
            "accepted {pointer}"
        );
    }
    let mut invalid = relative.clone();
    invalid["region"]["offset"]["extra"] = json!(1);
    assert!(color_check_to_pack(Some(&invalid), "page/marker").is_err());
    let mut targets = HashMap::from([
        (
            "page/marker".to_string(),
            json!({"id":"page/marker","template_path":"templates/marker.png","color_check":relative}),
        ),
        (
            "template/marker".to_string(),
            json!({"id":"template/marker","template_path":"templates/marker.png"}),
        ),
    ]);
    propagate_color_checks(
        &mut targets,
        &["page/marker".to_string(), "template/marker".to_string()],
    );
    let propagated = &targets["template/marker"]["color_check"];
    assert_eq!(propagated["region"]["anchor_target_id"], "template/marker");
    assert_eq!(propagated["region"]["offset"], json!({"x":-2,"y":3}));
    let check: actingcommand_recognition_pack::ColorCheck =
        serde_json::from_value(propagated.clone()).unwrap();
    check.validate_for_template("template/marker").unwrap();
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
        json!(1_800_001),
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
