// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{ResourceDeclarationIssue, ResourceDeclarationReason};

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
    // #286 NAVIGATION-DECLARATION-ONLY-v1: generated clicks share the declaration
    // grammar in both navigation edges and action metadata.
    for click in [
        json!({"kind":"point","x":12,"y":34}),
        json!({"kind":"rect","x":1,"y":2,"width":3,"height":4}),
        json!({"kind":"drag","from":{"x":1,"y":2,"width":3,"height":4},"to":{"x":5,"y":6,"width":7,"height":8},"duration_ms":900}),
        json!({"kind":"offset","target_id":"page/home","offset":{"x":1,"y":2,"width":3,"height":4}}),
        json!({"kind":"long_press","x":12,"y":34,"duration_ms":700}),
        json!({"kind":"target","target_id":"page/home"}),
        json!({"kind":"target_center","target_id":"page/home"}),
    ] {
        let click = click_to_navigation(&click).expect("generated navigation click");
        let navigation = json!({
            "navigation": [{"id":"home_to_next","from_page":"home","to_page":"next","click":click}],
            "page_operations": [{"id":"action","page":"home","click":click}],
            "destructive_actions": [{"id":"action","page":"home","click":click}]
        });
        validate_navigation_declarations(
            Path::new("navigation/fixture.navigation.json"),
            &navigation,
        )
        .expect("generated navigation declaration");
    }
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
        let operation = json!({"id": "drag", "from": "home", "click": click});
        let mut errors = Vec::new();
        let bundle = Bundle {
            task_id: "fixture".to_string(),
            dir: PathBuf::from("operations/fixture"),
            data: json!({
                "schema_version": "0.6",
                "task_id": "fixture",
                "game": "fixture",
                "server_scope": ["test"],
                "coordinate_space": {"width": 1280, "height": 720},
                "operations": [operation.clone()]
            }),
        };

        validate_click_shape(&bundle, &operation, &mut errors);

        assert!(
            errors
                .iter()
                .any(|error| error.contains("source drag click must use from/to")),
            "{errors:?}"
        );
        let rejection = declaration_file_requests(&[bundle]).expect_err(
            "source declaration rejects canonical endpoint spelling before reading dependencies",
        );
        assert_eq!(rejection.code, "resource_declaration_invalid");
        let issue: ResourceDeclarationIssue =
            serde_json::from_value(rejection.details.unwrap()).unwrap();
        assert_eq!(issue.declaration_file, "operations/fixture/task.json");
        assert_eq!(issue.field_path, "/operations/0/click/from_rect");
        assert_eq!(issue.reason, ResourceDeclarationReason::UnknownField);
    }
}

#[test]
fn converted_offset_click_rejects_color_probe_guard() {
    let pack = json!({
        "game": "fixture",
        "targets": [{
            "type": "color",
            "id": "target/button"
        }]
    });
    let pages = json!({
        "pages": [{
            "id": "fixture/home"
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
                "page_id": "fixture/home",
                "target_id": "target/button",
                "expected_rect": {"x": 10, "y": 20, "width": 30, "height": 40},
                "color_probe": "target/button"
            }
        }]
    });

    let err = validate_parsed_guard_references(&pack, &pages, &primitives)
        .expect_err("offset click must require template matched_rect source");

    assert!(err.message.contains("requires a template guard"));
    assert!(err.message.contains("must be a template target"));
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

/// Workflow #308 list selector slice L1: a task that declares a `repeated_anchor` layout.
fn anchored_bundle(layout: Value) -> Bundle {
    Bundle {
        task_id: "fixture".to_string(),
        dir: PathBuf::from("operations/fixture"),
        data: json!({
            "schema_version": "0.6",
            "task_id": "fixture",
            "game": "fixture",
            "server_scope": ["test"],
            "coordinate_space": {"width": 96, "height": 72},
            "entry_page": "list_page",
            "operations": [],
            "candidate_layouts": [layout]
        }),
    }
}

fn anchored_layout() -> Value {
    json!({
        "id": "layout/rows", "page_id": "list_page", "kind": "repeated_anchor",
        "anchor": "ui/row_marker", "max_instances": 4, "order": "top_to_bottom",
        "suppress_iou_milli": 0,
        "instance_rect": {"x": -2, "y": -2, "width": 40, "height": 12},
        "click": {"x": 10, "y": 0, "width": 20, "height": 6},
        "readable_band": {"x": 0, "y": 0, "width": 96, "height": 40},
        "features": [{"name": "open", "value": "passed", "target": "state/row_open",
                      "offset": {"x": 30, "y": 1}}]
    })
}

fn declaration_issue(error: actingcommand_contract::LabError) -> ResourceDeclarationIssue {
    serde_json::from_value(error.details.expect("declaration issue")).expect("issue shape")
}

#[test]
fn repeated_anchor_layouts_pass_the_gate_and_derive_with_their_own_fields() {
    declaration_file_requests(&[anchored_bundle(anchored_layout())])
        .expect("the declaration gate admits a repeated_anchor layout");
    let targets = HashMap::from([
        (
            "ui/row_marker".to_string(),
            json!({"type": "template", "id": "ui/row_marker"}),
        ),
        (
            "state/row_open".to_string(),
            json!({"type": "color", "id": "state/row_open"}),
        ),
        (
            "model/row_kind".to_string(),
            json!({"type": "nn", "id": "model/row_kind"}),
        ),
        (
            "check/row_ready".to_string(),
            json!({"type": "composite", "id": "check/row_ready"}),
        ),
    ]);
    let derived =
        derive_candidate_layouts(&targets, &[anchored_bundle(anchored_layout())], "fixture")
            .expect("derived layout");
    assert_eq!(
        serde_json::to_string(&derived).expect("derived JSON"),
        concat!(
            r#"[{"id":"layout/rows","page_id":"fixture/list_page","kind":"repeated_anchor","#,
            r#""anchor":"ui/row_marker","max_instances":4,"order":"top_to_bottom","#,
            r#""suppress_iou_milli":0,"instance_rect":{"x":-2,"y":-2,"width":40,"height":12},"#,
            r#""click":{"x":10,"y":0,"width":20,"height":6},"#,
            r#""readable_band":{"x":0,"y":0,"width":96,"height":40},"#,
            r#""features":[{"name":"open","value":"passed","target":"state/row_open","#,
            r#""offset":{"x":30,"y":1}}]}]"#
        )
    );

    // An nn or composite feature cannot be read at an offset, and the anchor is a template.
    for (field, target, pointer) in [
        (
            "/features/0/target",
            "model/row_kind",
            "/candidate_layouts/0/features/0/target",
        ),
        (
            "/features/0/target",
            "check/row_ready",
            "/candidate_layouts/0/features/0/target",
        ),
        ("/anchor", "state/row_open", "/candidate_layouts/0/anchor"),
    ] {
        let mut layout = anchored_layout();
        *layout.pointer_mut(field).expect("declared field") = json!(target);
        let error = derive_candidate_layouts(&targets, &[anchored_bundle(layout)], "fixture")
            .expect_err(pointer);
        let issue = declaration_issue(error);
        assert_eq!(issue.field_path, pointer);
        assert_eq!(issue.reason, ResourceDeclarationReason::InvalidValue);
    }
}

#[test]
fn candidate_layout_gate_refuses_each_repeated_anchor_rule_at_its_pointer() {
    let edit = |change: fn(&mut Value)| {
        let mut layout = anchored_layout();
        change(&mut layout);
        layout
    };
    let slots_layout = |feature: Value| {
        json!({
            "id": "layout/slots", "page_id": "list_page", "kind": "fixed_slots",
            "features": [feature],
            "slots": [{"rect": {"x": 0, "y": 0, "width": 8, "height": 8},
                       "click": {"x": 0, "y": 0, "width": 8, "height": 8},
                       "targets": {"open": "state/row_open"}}]
        })
    };
    let mut anchored_slots = slots_layout(json!({"name": "open", "value": "passed"}));
    anchored_slots["anchor"] = json!("ui/row_marker");
    for (layout, pointer, reason) in [
        (
            edit(|layout| layout["kind"] = json!("grid")),
            "/candidate_layouts/0/kind",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            edit(|layout| layout["slots"] = json!([])),
            "/candidate_layouts/0/slots",
            ResourceDeclarationReason::UnknownField,
        ),
        (
            edit(|layout| {
                layout.as_object_mut().expect("layout").remove("anchor");
            }),
            "/candidate_layouts/0/anchor",
            ResourceDeclarationReason::MissingField,
        ),
        (
            edit(|layout| layout["max_instances"] = json!(65)),
            "/candidate_layouts/0/max_instances",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            edit(|layout| layout["order"] = json!("row_major")),
            "/candidate_layouts/0/order",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            edit(|layout| layout["suppress_iou_milli"] = json!(1000)),
            "/candidate_layouts/0/suppress_iou_milli",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            edit(|layout| layout["instance_rect"]["width"] = json!(0)),
            "/candidate_layouts/0/instance_rect/width",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            edit(|layout| layout["readable_band"]["height"] = json!(80)),
            "/candidate_layouts/0/readable_band",
            ResourceDeclarationReason::InvalidValue,
        ),
        (
            edit(|layout| {
                layout["features"][0]
                    .as_object_mut()
                    .expect("feature")
                    .remove("offset");
            }),
            "/candidate_layouts/0/features/0/offset",
            ResourceDeclarationReason::MissingField,
        ),
        (
            anchored_slots,
            "/candidate_layouts/0/anchor",
            ResourceDeclarationReason::UnknownField,
        ),
        (
            slots_layout(json!({"name": "open", "value": "passed",
                                "target": "state/row_open", "offset": {"x": 0, "y": 0}})),
            "/candidate_layouts/0/slots/0/targets/open",
            ResourceDeclarationReason::InvalidValue,
        ),
    ] {
        let error = declaration_file_requests(&[anchored_bundle(layout)]).expect_err(pointer);
        assert_eq!(error.code, "resource_declaration_invalid");
        let issue = declaration_issue(error);
        assert_eq!(issue.declaration_file, "operations/fixture/task.json");
        assert_eq!((issue.field_path.as_str(), issue.reason), (pointer, reason));
    }
}

// Workflow #390 lpbox (exec key 76; Alice 10-09: pack taps use click boxes): a source long press
// declares its click box with both `width` and `height`, or its point as the contract already
// allowed; the navigation projection and a guard synthesized from the click keep the box, and
// half a box is refused at the member it lacks.
#[test]
fn lpbox_a_source_long_press_keeps_its_click_box_in_the_navigation_and_the_guard() {
    let boxed = json!({"kind":"long_press","x":12,"y":34,"width":8,"height":6,"duration_ms":700});
    let point = json!({"kind":"long_press","x":12,"y":34,"duration_ms":700});
    assert_eq!(click_to_navigation(&boxed).unwrap(), boxed);
    assert_eq!(
        click_to_guard_rect(&boxed).unwrap(),
        json!({"x":12,"y":34,"width":8,"height":6})
    );
    assert_eq!(click_to_navigation(&point).unwrap(), point);
    assert_eq!(
        click_to_guard_rect(&point).unwrap(),
        json!({"x":12,"y":34,"width":1,"height":1})
    );
    for click in [&boxed, &point] {
        let click = click_to_navigation(click).expect("generated navigation click");
        validate_navigation_declarations(
            Path::new("navigation/fixture.navigation.json"),
            &json!({"navigation": [
                {"id":"hold","from_page":"home","to_page":"next","click":click}
            ]}),
        )
        .expect("a long press navigation click");
    }

    for (missing, click) in [
        (
            "height",
            json!({"kind":"long_press","x":12,"y":34,"width":8,"duration_ms":700}),
        ),
        (
            "width",
            json!({"kind":"long_tap","x":12,"y":34,"height":6,"duration_ms":700}),
        ),
    ] {
        let operation = json!({"id": "hold", "from": "home", "click": click});
        let bundle = Bundle {
            task_id: "fixture".to_string(),
            dir: PathBuf::from("operations/fixture"),
            data: json!({
                "schema_version": "0.6",
                "task_id": "fixture",
                "game": "fixture",
                "server_scope": ["test"],
                "coordinate_space": {"width": 1280, "height": 720},
                "operations": [operation.clone()]
            }),
        };
        let mut errors = Vec::new();
        validate_click_shape(&bundle, &operation, &mut errors);
        assert!(
            errors
                .iter()
                .any(|error| error.contains(&format!("click missing \"{missing}\""))),
            "{missing}: {errors:?}"
        );
        let rejection = declaration_file_requests(&[bundle]).expect_err("half a click box");
        assert_eq!(rejection.code, "resource_declaration_invalid");
        let issue: ResourceDeclarationIssue =
            serde_json::from_value(rejection.details.unwrap()).unwrap();
        assert_eq!(issue.field_path, format!("/operations/0/click/{missing}"));
        assert_eq!(issue.reason, ResourceDeclarationReason::MissingField);
    }
}

// Workflow #308 SEL-L4b (frozen selector model S0 §1.4 (c)): a phased route whose select step
// declares `on_empty` may end on the step's own page before any designated effect, so that state
// is a terminal too. The source parser refuses the bundle `task_phases_outcome_incomplete` when
// no mapping lists the page under that effect condition, and admits it when the one mapping that
// does carries the `on_empty` key.
#[test]
fn l4b_a_phased_on_empty_select_step_needs_a_mapping_for_its_own_page() {
    let mapping = |key: &str, effect: &str, page: &str| json!({"outcome_key": key, "effect": effect, "terminal_pages": [page]});
    let bundle = |mappings: Vec<Value>| Bundle {
        task_id: "fixture".to_string(),
        dir: PathBuf::from("operations/fixture"),
        data: json!({
            "schema_version": "0.9",
            "task_id": "fixture",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 1280, "height": 720},
            "entry_page": "home",
            "target_page": "home",
            "phases": [
                {"id": "depart", "operations": ["open_list"], "target_pages": ["list"]},
                {"id": "business", "operations": ["choose"], "target_pages": ["detail"]},
                {"id": "return", "operations": ["back_home"], "target_pages": ["home"]}
            ],
            "scheduling_outcome": {"designated_operation": "choose", "mappings": mappings},
            "operations": [
                {"id": "open_list", "from": "home", "to": "list",
                 "click": {"kind": "rect", "x": 40, "y": 600, "width": 120, "height": 60}},
                {"id": "choose", "from": "list",
                 "select": {"layout_id": "layout/cards",
                            "policy": {"path": "policies/cards.json"},
                            "on_empty": {"outcome_key": "nobody_listed"}},
                 "guard": {"page_id": "list", "target_id": "check/list_ready",
                           "expected_rect": {"x": 0, "y": 0, "width": 1280, "height": 120}},
                 "expect_after": {"page_id": "detail", "timeout_ms": 3000, "interval_ms": 200}},
                {"id": "back_home", "from": "detail", "to": "home",
                 "click": {"kind": "rect", "x": 20, "y": 20, "width": 60, "height": 60}}
            ]
        }),
    };
    let returned = || mapping("returned", "designated_effect_completed", "home");
    validate_phases_bundle(&bundle(vec![
        returned(),
        mapping("nobody_listed", "no_designated_effect", "list"),
    ]))
    .expect("the on_empty key's mapping lists the step's page under no designated effect");
    for mappings in [
        vec![returned()],
        vec![
            returned(),
            mapping("nobody_listed", "designated_effect_completed", "list"),
        ],
    ] {
        let error = validate_phases_bundle(&bundle(mappings))
            .expect_err("no mapping lists the step's page under no designated effect");
        assert_eq!(error.message, "task_phases_outcome_incomplete");
    }
}

// #411 A17 (requirement 4): the source build refuses a stage start of an entry page set with the
// detail of the Runtime's admission refusal: the `entry_problem`, the page and, when operations
// of two stages leave from that page, those stages in declaration order.
#[test]
fn a17_a_source_entry_set_refusal_names_the_page_and_its_stages() {
    let click = json!({"kind": "rect", "x": 40, "y": 600, "width": 120, "height": 60});
    let bundle = |entry_page: Value, panel_back: bool| {
        let mut return_operations = vec![json!("back_home")];
        let mut operations = vec![
            json!({"id": "open_panel", "from": "home", "to": "panel", "click": click}),
            json!({"id": "submit", "from": "panel", "to": "result", "click": click}),
            json!({"id": "back_home", "from": "result", "to": "home", "click": click}),
        ];
        if panel_back {
            return_operations.push(json!("panel_back"));
            operations
                .push(json!({"id": "panel_back", "from": "panel", "to": "home", "click": click}));
        }
        Bundle {
            task_id: "fixture".to_string(),
            dir: PathBuf::from("operations/fixture"),
            data: json!({
                "schema_version": "0.9",
                "task_id": "fixture",
                "game": "neutral",
                "server_scope": ["test"],
                "coordinate_space": {"width": 1280, "height": 720},
                "entry_page": entry_page,
                "target_page": "home",
                "phases": [
                    {"id": "depart", "operations": ["open_panel"], "target_pages": ["panel"]},
                    {"id": "business", "operations": ["submit"], "target_pages": ["result"]},
                    {"id": "return", "operations": return_operations, "target_pages": ["home"]}
                ],
                "scheduling_outcome": {"designated_operation": "submit", "mappings": [
                    {"outcome_key": "returned", "effect": "designated_effect_completed",
                     "terminal_pages": ["home"]}
                ]},
                "operations": operations
            }),
        }
    };
    for (entry_page, panel_back, message) in [
        (
            json!(["home", "other"]),
            false,
            "entry_problem=phase_start_missing page_id=other",
        ),
        (
            json!(["home", "panel"]),
            true,
            "entry_problem=phase_start_ambiguous page_id=panel phase_ids=business,return",
        ),
    ] {
        let error = validate_phases_bundle(&bundle(entry_page, panel_back)).expect_err(message);
        assert_eq!(error.message, message);
    }
}

// #411 A20 (requirements 3 and 5): the source build refuses a phased route whose second stage
// reaches a page from which only another stage has an operation with the detail of the
// Runtime's admission refusal: the code, the rule, the stage, the page it cannot leave, the
// operation that led there and the start.
#[test]
fn a20_the_source_build_names_the_stage_page_and_operation_of_an_unreachable_route() {
    let click = json!({"kind": "rect", "x": 40, "y": 600, "width": 120, "height": 60});
    let bundle = Bundle {
        task_id: "fixture".to_string(),
        dir: PathBuf::from("operations/fixture"),
        data: json!({
            "schema_version": "0.9",
            "task_id": "fixture",
            "game": "neutral",
            "server_scope": ["test"],
            "coordinate_space": {"width": 1280, "height": 720},
            "entry_page": ["home", "other"],
            "target_page": "home",
            "phases": [
                {"id": "depart", "operations": ["open_panel"], "target_pages": ["panel"]},
                {"id": "business", "operations": ["submit", "stray"], "target_pages": ["result"]},
                {"id": "return", "operations": ["back_home"], "target_pages": ["home"]}
            ],
            "scheduling_outcome": {"designated_operation": "submit", "mappings": [
                {"outcome_key": "returned", "effect": "designated_effect_completed",
                 "terminal_pages": ["home"]}
            ]},
            "operations": [
                {"id": "open_panel", "from": "home", "to": "panel", "click": click},
                {"id": "submit", "from": "panel", "to": "result", "click": click},
                {"id": "back_home", "from": "result", "to": "home", "click": click},
                {"id": "stray", "from": "other", "to": "home", "click": click}
            ]
        }),
    };
    let error = validate_phases_bundle(&bundle).expect_err("a start from which a stage is stuck");
    assert_eq!(
        error.message,
        format!(
            "{}: phase_problem=no_operation json_pointer=/phases/1 phase_id=business \
             page_id=home pack_operation_id=stray entry_page=other",
            actingcommand_contract::codes::TaskflowCode::TaskPhasesUnreachable.as_str()
        )
    );
}
