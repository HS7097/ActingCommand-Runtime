// SPDX-License-Identifier: AGPL-3.0-only

    #[test]
    fn error_page_detection_matches_explicit_and_negative_pages() {
        let explicit = vec!["arknights/error_popup".to_string()];

        assert!(page_is_error_page(
            "arknights",
            Some("arknights/error_popup"),
            &explicit
        ));
        assert!(page_is_error_page(
            "arknights",
            Some("arknights/negative_connection"),
            &[]
        ));
        assert!(!page_is_error_page(
            "arknights",
            Some("arknights/home"),
            &explicit
        ));
    }

    #[test]
    fn error_page_detection_uses_passed_forbidden_targets() {
        let scene = CapturedScene {
            scene: Scene::from_png(one_pixel_png()).expect("scene"),
            matched_page: None,
            page_evaluations: vec![PageEvaluation {
                page_id: "arknights/home".to_string(),
                matched: false,
                required_passed: 0,
                required_total: 1,
                any_of_passed: 0,
                any_of_total: 0,
                optional_passed: 0,
                optional_total: 0,
                forbidden_passed: 1,
                forbidden_total: 1,
                target_results: vec![PageTargetEvaluation {
                    target_id: "page/negative_announcement".to_string(),
                    role: PageTargetRole::Forbidden,
                    passed: true,
                    message: "template passed".to_string(),
                }],
                message: "forbidden target passed".to_string(),
            }],
            verify_template_matched: false,
            width: 1,
            height: 1,
        };

        assert!(scene_hits_error_page("arknights", &scene, &[]));
    }

    #[test]
    fn explicit_error_pages_can_match_forbidden_page_targets() {
        let scene = CapturedScene {
            scene: Scene::from_png(one_pixel_png()).expect("scene"),
            matched_page: None,
            page_evaluations: vec![PageEvaluation {
                page_id: "arknights/depot".to_string(),
                matched: false,
                required_passed: 1,
                required_total: 1,
                any_of_passed: 0,
                any_of_total: 0,
                optional_passed: 0,
                optional_total: 0,
                forbidden_passed: 1,
                forbidden_total: 1,
                target_results: vec![PageTargetEvaluation {
                    target_id: "page/home".to_string(),
                    role: PageTargetRole::Forbidden,
                    passed: true,
                    message: "template passed".to_string(),
                }],
                message: "forbidden target passed".to_string(),
            }],
            verify_template_matched: false,
            width: 1,
            height: 1,
        };

        assert!(scene_hits_error_page(
            "arknights",
            &scene,
            &["home".to_string()]
        ));
        assert!(!scene_hits_error_page("arknights", &scene, &[]));
    }

    #[test]
    fn trusted_unguarded_point_and_long_press_use_original_coordinate() {
        let control = test_control();
        for kind in ["point", "long_press"] {
            let mut operation = test_operation(Some("terminal"), None);
            operation.click = OperationClick {
                kind: kind.to_string(),
                x: Some(123),
                y: Some(234),
                width: None,
                height: None,
                from_rect: None,
                to_rect: None,
                duration_ms: Some(800),
                offset: None,
                target_id: None,
            };

            operation
                .validate(&control)
                .expect("trusted unguarded coordinate valid");
            let action = operation
                .input_action(&control.resolution, 0, None)
                .expect("input action");

            match (kind, action) {
                ("long_press", LabInputAction::LongTap { point, duration_ms }) => {
                    assert_eq!(duration_ms, 800);
                    assert_eq!(point.x, 123);
                    assert_eq!(point.y, 234);
                }
                ("point", LabInputAction::Tap(point)) => {
                    assert_eq!(point.x, 123);
                    assert_eq!(point.y, 234);
                }
                _ => panic!("unexpected action for {kind}"),
            }
        }
    }

    #[test]
    fn pre_execution_guard_passes_when_page_and_target_match() {
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let scene = captured_rgb_scene(Some("arknights/home"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        match outcome {
            PreExecutionGuardOutcome::Passed {
                current_page,
                target,
            } => {
                assert_eq!(current_page, Some("home".to_string()));
                assert!(target.passed);
                assert_eq!(target.id, "target/button");
            }
            other => panic!("expected guard pass, got {other:?}"),
        }
    }

    #[test]
    fn pre_execution_guard_rejects_changed_execution_page() {
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let scene = captured_rgb_scene(Some("arknights/terminal"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        assert_eq!(
            outcome,
            PreExecutionGuardOutcome::Failed {
                reason: "page_guard_mismatch",
                current_page: Some("terminal".to_string()),
                diagnostics: json!({
                    "expected_page": "home",
                    "matched_page": "arknights/terminal",
                    "operation_from": "home"
                })
            }
        );
    }

    #[test]
    fn pre_execution_guard_allows_any_page_guard_when_target_matches() {
        let mut operation = test_operation(None, None);
        operation.from = "any".to_string();
        operation.unguarded_trusted_coordinate = false;
        let mut guard = test_color_guard();
        guard.page_id = "any".to_string();
        operation.guard = Some(guard);
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([0, 0, 0]);
        let scene = captured_rgb_scene(Some("arknights/terminal"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        match outcome {
            PreExecutionGuardOutcome::Passed {
                current_page,
                target,
            } => {
                assert_eq!(current_page, Some("terminal".to_string()));
                assert!(target.passed);
            }
            other => panic!("expected guard pass, got {other:?}"),
        }
    }

    #[test]
    fn pre_execution_guard_rejects_target_mismatch_on_same_page() {
        let mut operation = test_operation(None, None);
        operation.unguarded_trusted_coordinate = false;
        operation.guard = Some(test_color_guard());
        let guard = operation.guard.as_ref().expect("guard");
        let evaluator = one_pixel_color_evaluator([255, 255, 255]);
        let scene = captured_rgb_scene(Some("arknights/home"), [0, 0, 0]);

        let outcome =
            evaluate_pre_execution_guard("arknights", &operation, guard, &scene, &evaluator)
                .expect("guard evaluation");

        match outcome {
            PreExecutionGuardOutcome::TargetMismatch {
                current_page,
                target,
                diagnostics,
            } => {
                assert_eq!(current_page, Some("home".to_string()));
                assert!(!target.passed);
                assert_eq!(
                    diagnostics
                        .pointer("/target/passed")
                        .and_then(Value::as_bool),
                    Some(false)
                );
            }
            other => panic!("expected target mismatch, got {other:?}"),
        }
    }

    #[test]
    fn resource_drift_gate_detects_stable_target_mismatch() {
        let initial = color_target_evaluation("target/button", [9, 0, 0], false);
        let mut gate = ResourceDriftGate::new(2, initial).expect("gate");

        assert_eq!(
            gate.observe(color_target_evaluation("target/button", [9, 0, 0], false)),
            ResourceDriftObservation::Drift
        );
        assert_eq!(gate.stable_mismatch_frames, 2);
        assert_eq!(gate.observed_frames, 2);
    }

    #[test]
    fn resource_drift_gate_waits_on_moving_target_mismatch() {
        let initial = color_target_evaluation("target/button", [0, 0, 0], false);
        let mut gate = ResourceDriftGate::new(2, initial).expect("gate");

        for mean in [[3, 0, 0], [6, 0, 0], [9, 0, 0]] {
            assert_eq!(
                gate.observe(color_target_evaluation("target/button", mean, false)),
                ResourceDriftObservation::Waiting
            );
        }
        assert_eq!(gate.stable_mismatch_frames, 1);
    }

    #[test]
    fn resource_drift_gate_recovers_when_target_passes() {
        let initial = color_target_evaluation("target/button", [0, 0, 0], false);
        let mut gate = ResourceDriftGate::new(2, initial).expect("gate");

        assert_eq!(
            gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)),
            ResourceDriftObservation::Recovered
        );
    }

    #[test]
    fn resource_drift_gate_rejects_initial_passing_target() {
        let err =
            ResourceDriftGate::new(2, color_target_evaluation("target/button", [0, 0, 0], true))
                .expect_err("passing target is not drift");

        assert_eq!(err.code, "device_error");
        assert!(err.message.contains("initial target mismatch"));
    }

    #[test]
    fn resource_drift_diagnostics_include_recalibration_context() {
        let mut operation = test_operation(None, None);
        operation.provenance = Some(json!({"version": "pack-20260703"}));
        let guard = test_color_guard();
        let target = color_target_evaluation("target/button", [9, 0, 0], false);

        let diagnostics = resource_drift_diagnostics(&operation, &guard, &target, 2);

        assert_eq!(
            diagnostics.get("trigger").and_then(Value::as_str),
            Some("resource_drift")
        );
        assert_eq!(
            diagnostics.get("resource_status").and_then(Value::as_str),
            Some("needs_recalibration")
        );
        assert_eq!(
            diagnostics.get("target_id").and_then(Value::as_str),
            Some("target/button")
        );
        assert_eq!(
            diagnostics
                .pointer("/expected_rect/width")
                .and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(
            diagnostics
                .pointer("/measured/passed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            diagnostics
                .get("provenance_version")
                .and_then(Value::as_str),
            Some("pack-20260703")
        );
    }

    #[test]
    fn roi_stability_gate_waits_until_roi_becomes_stable() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        assert!(!gate.observe(color_target_evaluation("target/button", [8, 0, 0], true)));
        assert!(gate.observe(color_target_evaluation("target/button", [8, 0, 0], true)));
        assert_eq!(gate.stable_frames, 2);
        assert_eq!(gate.observed_frames, 3);
    }

    #[test]
    fn roi_stability_gate_passes_static_roi_on_first_followup_frame() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        assert!(gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)));
        assert_eq!(gate.observed_frames, 2);
    }

    #[test]
    fn roi_stability_gate_rejects_continuously_changing_roi() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        for mean in [[3, 0, 0], [6, 0, 0], [9, 0, 0]] {
            assert!(!gate.observe(color_target_evaluation("target/button", mean, true)));
        }
        assert_eq!(gate.stable_frames, 1);
    }

    #[test]
    fn roi_stability_gate_resets_when_target_fails() {
        let baseline = color_target_evaluation("target/button", [0, 0, 0], true);
        let mut gate = RoiStabilityGate::new(2, baseline).expect("gate");

        assert!(!gate.observe(color_target_evaluation("target/button", [0, 0, 0], false)));
        assert!(!gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)));
        assert!(gate.observe(color_target_evaluation("target/button", [0, 0, 0], true)));
        assert_eq!(gate.stable_frames, 2);
    }

    #[test]
    fn page_namespace_matches_operation_anchors_without_blind_split() {
        assert_eq!(canonical_page_anchor("arknights", "arknights/home"), "home");
        assert_eq!(
            canonical_page_anchor("arknights", "arknights/navigation/home_to_task"),
            "navigation/home_to_task"
        );
        assert_eq!(canonical_page_anchor("arknights", "home"), "home");
        assert!(page_anchor_matches("arknights", "arknights/home", "home"));
        assert!(page_anchor_matches("arknights", "home", "home"));
        assert!(page_anchor_matches(
            "arknights",
            "arknights/quickswitch_dropdown",
            "quickswitch_dropdown"
        ));
        assert!(!page_anchor_matches(
            "arknights",
            "bluearchive/home",
            "home"
        ));
    }

    #[test]
    fn operation_selection_prefers_specific_page_before_any_fallback() {
        let generic = Operation {
            id: "open_quickswitch".to_string(),
            from: "any".to_string(),
            to: Some("quickswitch_dropdown".to_string()),
            ..test_operation(Some("quickswitch_dropdown"), None)
        };
        let specific = Operation {
            id: "quickswitch_to_home".to_string(),
            from: "quickswitch_dropdown".to_string(),
            to: Some("home".to_string()),
            ..test_operation(Some("home"), None)
        };
        let operations = vec![generic, specific];

        let selected =
            select_operation_for_page("arknights", "arknights/quickswitch_dropdown", &operations)
                .unwrap();

        assert_eq!(selected.id, "quickswitch_to_home");
    }

    #[test]
    fn operation_verification_marks_to_null_without_template_unverified() {
        let operation = test_operation(None, None);
        let scene = captured_scene(Some("arknights/home"), false);

        let result = operation_verification_status("arknights", &operation, &scene);

        assert_eq!(result, OperationVerification::ExecutedUnverified);
        assert_eq!(result.result_label(), "executed_unverified");
    }

    #[test]
    fn operation_verification_requires_template_when_to_is_null_with_template() {
        let operation = test_operation(None, Some("terminal.png"));
        let failed = captured_scene(Some("arknights/home"), false);
        let passed = captured_scene(Some("arknights/home"), true);

        assert_eq!(
            operation_verification_status("arknights", &operation, &failed),
            OperationVerification::Failed
        );
        assert_eq!(
            operation_verification_status("arknights", &operation, &passed),
            OperationVerification::Verified
        );
    }

    #[test]
    fn operation_verification_accepts_namespaced_arrival_page() {
        let operation = test_operation(Some("terminal"), None);
        let scene = captured_scene(Some("arknights/terminal"), false);

        assert_eq!(
            operation_verification_status("arknights", &operation, &scene),
            OperationVerification::Verified
        );
    }

    #[test]
    fn operation_verification_uses_expect_after_page() {
        let mut operation = test_operation(None, None);
        operation.expect_after = Some(OperationExpectation {
            page_id: "terminal".to_string(),
            timeout_ms: Some(50),
            interval_ms: None,
        });
        let matched = captured_scene(Some("arknights/terminal"), false);
        let mismatched = captured_scene(Some("arknights/home"), false);

        assert_eq!(
            operation_verification_status("arknights", &operation, &matched),
            OperationVerification::Verified
        );
        assert_eq!(
            operation_verification_status("arknights", &operation, &mismatched),
            OperationVerification::Failed
        );
        assert_eq!(operation.expected_after_page(), Some("terminal"));
        assert_eq!(
            operation.after_timeout_ms(OperationDefaults::default(), 10_000),
            50
        );
    }
