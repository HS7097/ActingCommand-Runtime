use crate::{
    CliOutcome, FlagArgs, GlobalOptions, REQUIRE_SESSION_DAEMON_ENV,
    TRUSTED_REMOTE_CLIENT_CERT_ENV, TRUSTED_REMOTE_TOKEN_ENV, current_unix_ms,
    reject_legacy_session_routing,
};
use serde_json::{Value, json};

pub(crate) const SESSION_LEASE_STALE_MS: u64 = 30_000;
pub(crate) const SESSION_DAEMON_REQUEST_TIMEOUT_MS: u64 = 10_000;

fn session_throat_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.throat_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable unique Session Layer control throat policy",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "session_layer": {
            "resident_daemon": true,
            "only_control_throat": true,
            "clients_must_not_directly_touch_adb_or_devices": true,
            "ui_must_not_directly_touch_adb_or_device": true,
            "scheduler_must_use_session_layer_for_device_control": true,
            "agents_must_use_session_layer_for_device_control": true
        },
        "strict_session_throat": {
            "flag": "--require-session",
            "env": REQUIRE_SESSION_DAEMON_ENV,
            "failure_code": "session_daemon_required",
            "failure_is_visible": true
        },
        "route_policy": {
            "local_read_only_queries": {
                "may_run_local_when_no_resident_daemon": true,
                "prefer_resident_daemon_when_alive": true,
                "local_override_flag": "--local"
            },
            "control_requests": {
                "must_use_resident_daemon_when_available_or_strict": true,
                "requires_matching_lease": true,
                "blocked_without_matching_lease_code": "lab_lease_required"
            },
            "daemon_internal_execution": {
                "forces_local_execution": true,
                "reason": "avoid recursive request requeue inside the resident daemon"
            },
            "trusted_remote": {
                "status": "reserved",
                "requires_encryption": true,
                "requires_authentication": true,
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "lease_gate": {
            "required_for_control": true,
            "matching_fields": ["holder", "lease_id"],
            "preflight": "session command-check <command...>",
            "submit_plan": "session submit-plan <command...>"
        },
        "allowed_offline_evidence": [
            "session command-check",
            "session submit-plan",
            "session api",
            "session contract",
            "session bootstrap",
            "session validation-plan",
            "session throat-policy",
            "session self-heal-policy",
            "session self-heal-plan"
        ],
        "deferred_live_acceptance": {
            "status": "deferred",
            "deferred_code": "requires-live-device",
            "must_not_mark_live_pass_from_offline_checks": true
        },
        "failure_policy": {
            "severe_errors_fail_loud": true,
            "silent_failure_allowed": false,
            "transient_fallback_requires_full_logging": true
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

pub(crate) fn session_capture_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.capture_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable fresh-frame and stale-capture policy for Session Layer clients",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "fresh_frame_policy": {
            "require_fresh_flag": "--require-fresh",
            "diagnostic_command": "capture diagnose --require-fresh",
            "session_diagnostic_command": "session capture diagnose --require-fresh",
            "stale_frame_must_be_visible": true,
            "stale_frame_must_not_be_treated_as_success": true
        },
        "backend_policy": {
            "preferred_order": ["nemu_ipc", "droidcast_raw", "adb_screencap"],
            "adb_screencap_is_last_resort": true,
            "fallback_allowed_for_transient_capture_failures": true,
            "fallback_requires_full_logging": true,
            "fallback_log_context": [
                "trigger_reason",
                "source_backend",
                "fallback_backend",
                "instance",
                "game",
                "server",
                "user_visible_impact"
            ]
        },
        "stale_classification": {
            "must_not_classify_as_game_freeze_from_adb_screencap_alone": true,
            "must_compare_or_diagnose_before_freeze_conclusion": true,
            "stale_capture_status": "capture_stale_suspected",
            "game_freeze_status": "unverified_without_fresh_backend_evidence",
            "ak_known_stale_md5": "202752fa3e5cab706774819168639b6c",
            "finding": "FINDING-AK-game-freeze-2026-06-27"
        },
        "freeze_classification_gate": {
            "schema_version": "session.capture_freeze_classification_gate.v0.1",
            "status": "blocked_without_fresh_backend_evidence",
            "safe_to_classify_game_frozen": false,
            "must_not_classify_as_game_freeze_from_adb_screencap_alone": true,
            "finding": "FINDING-AK-game-freeze-2026-06-27",
            "insufficient_evidence": [
                "adb_screencap_same_md5_alone",
                "adb_disconnect_reconnect_same_md5_alone",
                "input_command_returned_ok_without_fresh_frame",
                "high_cpu_without_anr_or_fresh_backend_evidence",
                "page_detector_result_from_stale_frame"
            ],
            "required_before_game_freeze_label": [
                "run capture diagnose --require-fresh",
                "record backend name, frame hash, and timestamp or sequence evidence",
                "compare at least two frames or prove stale status through capture diagnose",
                "try a lighter non-adb_screencap backend when available",
                "record operator/live evidence before accepting a live game-freeze conclusion"
            ],
            "recommended_order": [
                "session capture-policy",
                "capture diagnose --require-fresh",
                "session recover --stale-capture",
                "session self-heal-plan --trigger capture_stale_suspected",
                "operator live validation"
            ],
            "live_validation": {
                "status": "deferred",
                "deferred_code": "requires-live-device",
                "must_not_mark_live_pass_from_offline_checks": true
            }
        },
        "recovery_policy": {
            "read_only_plan": "session recover --stale-capture",
            "diagnosis_first": true,
            "try_lighter_capture_backend_recovery_before_app_restart": true,
            "app_restart_is_heavy_recovery": true,
            "maintenance_recovery_requires_matching_lease_when_it_executes_control": true,
            "does_not_mark_recovery_live_pass_without_operator_observation": true
        },
        "client_guidance": {
            "ui_should_show_degraded_capture_state": true,
            "scheduler_should_not_submit_navigation_on_stale_frame": true,
            "agents_should_recheck_with_capture_policy_before_declaring_game_frozen": true,
            "operator_live_acceptance_deferred_code": "requires-live-device"
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

fn session_record_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.record_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable active recording authorization policy for Session Layer clients",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "authorization_model": {
            "active_authorization_required": true,
            "passive_full_recording_allowed": false,
            "navigation_is_not_recorded_by_default": true,
            "operator_selects_step_kind": true,
            "recording_session_required": true,
            "record_start_command": "session record start --task-id <id>",
            "record_step_command": "session record step --kind <kind>",
            "record_amend_command": "session record amend",
            "record_build_command": "session record build-task",
            "record_promote_command": "session record promote"
        },
        "allowed_step_kinds": [
            {
                "kind": "anchor",
                "purpose": "materialize a reviewed page or UI anchor from an authorized frame",
                "requires_explicit_frame_source": true,
                "can_materialize_template": true
            },
            {
                "kind": "operation",
                "purpose": "record reviewed operation metadata and click-bound references",
                "requires_explicit_click_reference": true,
                "can_execute_click": false
            },
            {
                "kind": "color-probe",
                "purpose": "sample a reviewed frame region into color-probe resource metadata",
                "requires_explicit_frame_source": true,
                "can_materialize_color_data": true
            },
            {
                "kind": "verify-template",
                "purpose": "materialize a reviewed verification template from an authorized frame",
                "requires_explicit_frame_source": true,
                "can_materialize_template": true
            }
        ],
        "frame_source_policy": {
            "local_png_allowed": true,
            "current_frame_allowed": true,
            "current_frame_requires_explicit_flag": "--capture or --current-frame",
            "current_frame_uses_existing_capture_backend": true,
            "current_frame_live_validation": "deferred",
            "deferred_code": "requires-live-device",
            "must_store_provenance": true,
            "must_store_hash": true,
            "must_store_freshness_metadata_when_available": true,
            "must_not_read_resource_repositories": true,
            "policy_command_captures": false
        },
        "resource_write_policy": {
            "build_task_writes_local_draft": true,
            "promote_requires_explicit_command": "session record promote",
            "policy_command_writes_resources": false,
            "policy_command_promotes_resources": false,
            "overwrite_requires_opt_in": true,
            "resource_repository_write_requires_explicit_repo": true,
            "promotion_must_preserve_provenance": true
        },
        "safety_policy": {
            "destructive_operation_requires_explicit_flag": true,
            "game_progress_actions_allowed": false,
            "premium_or_paid_resource_use_allowed": false,
            "blind_confirmation_allowed": false,
            "requires_session_layer_for_device_frame_capture": true,
            "requires_matching_lease_for_future_device_control": true,
            "severe_errors_fail_loud": true,
            "silent_failure_allowed": false
        },
        "client_guidance": {
            "ui_should_show_authorization_prompt": true,
            "ui_should_show_step_kind_picker": true,
            "ui_should_show_frame_source_picker": true,
            "ui_should_show_resource_write_warning_before_promote": true,
            "agents_should_call_record_policy_before_record_step": true,
            "operator_can_amend_before_build": true,
            "operator_can_review_candidates_before_build": true,
            "record_policy_query": "session record-policy",
            "daemon_record_policy_query": "session request record-policy"
        },
        "live_validation": {
            "status": "deferred",
            "deferred_code": "requires-live-device",
            "must_not_mark_live_pass_from_offline_checks": true
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true,
            "does_not_write_resource_repositories": true,
            "does_not_start_listener": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true
        }
    }))
}

pub(crate) fn session_self_heal_policy_payload(
    global: &GlobalOptions,
    flags: &FlagArgs,
    command_name: &str,
) -> CliOutcome<Value> {
    flags.expect_positionals(command_name, 0)?;
    Ok(json!({
        "schema_version": "session.self_heal_policy.v0.1",
        "status": "offline_policy",
        "purpose": "machine-readable Phase C maintenance self-heal policy for Session Layer clients",
        "generated_at_unix_ms": current_unix_ms(),
        "scope": {
            "instance": global.instance.clone(),
            "game": global.game.clone(),
            "server": global.server.clone()
        },
        "phase_c": {
            "name": "self-heal",
            "goal": "return a session to a known-good state without executing game-progress actions",
            "target_state": "home_or_known_good_page",
            "live_acceptance_status": "deferred",
            "deferred_code": "requires-live-device"
        },
        "flow": [
            {
                "stage": "observe",
                "allowed_commands": ["monitor --once", "session status --diagnostics"],
                "device_control_allowed": false
            },
            {
                "stage": "diagnose",
                "allowed_commands": ["capture diagnose --require-fresh", "session capture diagnose --require-fresh", "current-page", "is-visible"],
                "must_distinguish_capture_stale_from_game_freeze": true
            },
            {
                "stage": "plan",
                "allowed_commands": ["session recover --stale-capture", "session recover --to <page> --dry-run", "session submit-plan <command...>"],
                "must_be_inspectable_before_execution": true
            },
            {
                "stage": "execute",
                "allowed_commands": ["session request recover", "session request app restart", "session monitor-policy set --recover"],
                "requires_matching_lease": true,
                "must_run_through_session_layer": true
            }
        ],
        "trigger_policy": {
            "supported_triggers": [
                "stale_frame",
                "hang",
                "resource_drift",
                "session_expired",
                "standby",
                "modal_popup",
                "off_route_page",
                "unstable_page"
            ],
            "legacy_trigger_aliases": [
                {"alias": "capture_stale_suspected", "canonical": "stale_frame"},
                {"alias": "capture_backend_unavailable", "canonical": "stale_frame"},
                {"alias": "startup_login_required", "canonical": "session_expired"},
                {"alias": "unexpected_page", "canonical": "off_route_page"}
            ],
            "priority_order": [
                ["stale_frame", "hang"],
                ["resource_drift"],
                ["session_expired", "standby"],
                ["modal_popup"],
                ["off_route_page"],
                ["unstable_page"]
            ],
            "stale_adb_screencap_alone_is_not_game_freeze": true,
            "must_diagnose_before_restart": true,
            "must_not_treat_missing_evidence_as_success": true
        },
        "recovery_order": [
            {
                "order": 1,
                "kind": "read_only_diagnosis",
                "examples": ["monitor --once", "capture diagnose --require-fresh"]
            },
            {
                "order": 2,
                "kind": "capture_backend_recovery",
                "examples": ["try nemu_ipc", "try droidcast_raw", "use adb_screencap only as last resort"]
            },
            {
                "order": 3,
                "kind": "maintenance_navigation",
                "examples": ["standby wake", "modal close", "safe route to home"]
            },
            {
                "order": 4,
                "kind": "startup_login_loop",
                "examples": ["session recover --startup-login --dry-run", "bounded popup close loop"]
            },
            {
                "order": 5,
                "kind": "app_lifecycle_restart",
                "examples": ["session app restart"],
                "heavy_recovery": true
            }
        ],
        "maintenance_boundary": {
            "allowed_outcome": "known_good_state_only",
            "game_progress_actions_allowed": false,
            "destructive_actions_allowed": false,
            "premium_or_paid_resource_use_allowed": false,
            "pvp_or_exercise_allowed": false,
            "blind_confirmation_allowed": false,
            "navigation_only_default": true
        },
        "lease_and_scheduler_policy": {
            "scheduler_owns_arbitration": true,
            "session_layer_owns_device_mechanism": true,
            "control_execution_requires_matching_lease": true,
            "monitor_policy_recovery_without_matching_lease": "deferred_by_lease",
            "ui_must_not_bypass_session_layer": true
        },
        "client_guidance": {
            "ui_should_show_degraded_state": true,
            "scheduler_should_pause_task_submission_until_policy_allows_execution": true,
            "agents_should_request_plan_before_execution": true,
            "interactive_stream_should_report_recovery_state_but_not_execute_without_lease": true,
            "operator_live_acceptance_deferred_code": "requires-live-device"
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_start_apps": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

pub(crate) fn session_access_contract() -> Value {
    json!({
        "schema_version": "session.access.v0.1",
        "purpose": "machine-readable access boundary for Session Layer clients",
        "session_layer": {
            "resident_daemon": true,
            "only_control_throat": true,
            "ui_direct_device_access_allowed": false,
            "direct_adb_access_allowed_for_clients": false
        },
        "entrypoints": {
            "local_cli": {
                "status": "available",
                "encryption_required": false,
                "authentication_required": false,
                "command": "actinglab"
            },
            "trusted_remote": {
                "status": "reserved",
                "encryption_required": true,
                "authentication_required": true,
                "minimum_transport": "TLS or mutually authenticated local IPC",
                "token_or_certificate_required": true,
                "plan_command": "session transport plan [--endpoint <url>]",
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "daemon_queries": {
            "bootstrap": "session request bootstrap",
            "throat_policy": "session request throat-policy",
            "capture_policy": "session request capture-policy",
            "record_policy": "session request record-policy",
            "self_heal_policy": "session request self-heal-policy",
            "self_heal_plan": "session request self-heal-plan [--trigger <kind>] [--to <page>]",
            "phase_c_plan": "session request phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
            "contract": "session request contract",
            "api": "session request api",
            "transport": "session request transport",
            "transport_plan": "session request transport plan [--endpoint <url>]",
            "transport_check": "session request transport check --endpoint <url>",
            "capabilities": "session request capabilities",
            "readiness": "session request readiness",
            "connect_plan": "session request connect-plan",
            "stream_plan": "session request stream-plan",
            "command_check": "session request command-check <command...>",
            "submit_plan": "session request submit-plan <command...>",
            "validation_plan": "session request validation-plan",
            "status": "session request status --diagnostics",
            "queue": "session request queue",
            "journal": "session request journal",
            "events": "session request events",
            "instance_registry": "session request instance registry",
            "monitor_policy": "session request monitor-policy status"
        },
        "daemon_controls": {
            "app_lifecycle": "session request app <launch|stop|force-stop|restart>",
            "instance_app_lifecycle": "session request instance app <launch|stop|force-stop|restart>"
        },
        "request_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": [
                    "status",
                    "bootstrap",
                    "throat-policy",
                    "capture-policy",
                    "record-policy",
                    "self-heal-policy",
                    "self-heal-plan",
                    "phase-c-plan",
                    "queue",
                    "journal",
                    "readiness",
                    "stream-plan",
                    "command-check",
                    "submit-plan",
                    "validation-plan",
                    "contract",
                    "transport plan",
                    "transport check",
                    "capabilities",
                    "devices",
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session recover --stale-capture",
                    "session record step --capture",
                    "session record step --current-frame",
                    "session monitor-policy status",
                    "session instance registry",
                    "monitor-once"
                ],
                "device_affecting_examples": [
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session record step --capture",
                    "session record step --current-frame"
                ]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": [
                    "session record start",
                    "session record status",
                    "session record stop",
                    "session record step --frame <png>",
                    "session record candidates",
                    "session record amend",
                    "session record build-task",
                    "session record promote",
                    "session monitor-policy set",
                    "session monitor-policy clear"
                ]
            },
            "control": {
                "requires_lease": true,
                "examples": [
                    "lease",
                    "session app launch",
                    "session app stop",
                    "session app force-stop",
                    "session app restart",
                    "session instance app launch",
                    "session instance app stop",
                    "session instance app force-stop",
                    "session instance app restart",
                    "lab-run",
                    "package-run",
                    "operation-run",
                    "tap",
                    "swipe",
                    "long-tap",
                    "key",
                    "text",
                    "stream --input-relay",
                    "stream --input-event <action,args>",
                    "stream --relay-event <action,args>",
                    "tap-target",
                    "navigate",
                    "recover except --stale-capture"
                ]
            }
        },
        "safety": {
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "session_daemon_required",
            "clients_must_not_directly_touch_adb_or_devices": true,
            "ui_must_not_directly_touch_adb_or_device": true,
            "control_requests_require_matching_lease": true,
            "requests_are_serialized_by_resident_daemon": true,
            "severe_errors_fail_loud": true,
            "transient_recovery_path_must_be_logged": true
        },
        "local_reliability_threat_model": {
            "schema_version": "session.local_reliability_threat_model.v0.1",
            "scope": "local automation reliability",
            "state_dir_and_endpoint_writable_by_same_user_are_trusted_environment": true,
            "same_user_forged_state_or_endpoint_is_accepted_risk": true,
            "current_readiness_is_not_same_user_authentication": true,
            "must_fail_fast_when_daemon_does_not_ack_request": true,
            "authentication_key_material_and_memory_protection_deferred_to_trusted_channel_scheduler_ui": true,
            "trusted_channel_phase": "P3/#10"
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
            "same-user state_dir/endpoint forgery authentication",
            "secret challenge proof",
            "memory encryption",
            "UI transport",
            "scheduler runtime"
        ]
    })
}

pub(crate) fn session_api_contract() -> Value {
    let mut contract = json!({
        "schema_version": "session.api.v0.1",
        "purpose": "machine-readable command and envelope contract for Session Layer clients",
        "session_layer": {
            "resident_daemon": true,
            "only_control_throat": true,
            "clients_must_not_directly_touch_adb_or_devices": true,
            "requests_are_serialized_by_resident_daemon": true
        },
        "access_channels": {
            "local_cli": {
                "status": "available",
                "command": "actinglab",
                "encryption_required": false,
                "authentication_required": false
            },
            "trusted_remote": {
                "status": "reserved",
                "network_listener_implemented": false,
                "encryption_required": true,
                "authentication_required": true,
                "minimum_transport": "TLS or mutually authenticated local IPC",
                "token_or_certificate_required": true,
                "auth_env": {
                    "token": TRUSTED_REMOTE_TOKEN_ENV,
                    "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
                },
                "blocked_without_auth_code": "trusted_remote_auth_required",
                "blocked_without_encryption_code": "trusted_remote_transport_blocked"
            }
        },
        "daemon_request_queue": {
            "status": "available",
            "submit_command": "session request <command>",
            "request_dir": "requests/",
            "response_dir": "responses/",
            "journal": "request-journal.jsonl",
            "request_fields": [
                "request_id",
                "command",
                "global",
                "args",
                "lease",
                "created_at_unix_ms"
            ],
            "submit_modes": {
                "sync_wait": {
                    "default": true,
                    "waits_for_response": true,
                    "consumes_response_on_success": true,
                    "timeout_flag": "--request-timeout-ms"
                },
                "no_wait": {
                    "flag": "--no-wait",
                    "waits_for_acknowledgement": true,
                    "ack_timeout_flag": "--request-ack-timeout-ms",
                    "waits_for_response": false,
                    "response_query": "session response get <request-id>",
                    "consume_query": "session response get <request-id> --consume"
                }
            },
            "cancel_query": "session request cancel <request-id> [--reason text] [--dry-run]",
            "cancel_error_code": "request_cancelled",
            "cancel_records_journal": true,
            "cancel_dry_run_preserves_queue": true,
            "admission_gate": {
                "queue_health_field": "diagnostics.queues.health",
                "blocks_status": "needs_attention",
                "error_code": "request_queue_needs_attention",
                "preflight_command": "session command-check <command...>"
            },
            "response_fields": [
                "request_id",
                "command",
                "ok",
                "data",
                "error",
                "started_at_unix_ms",
                "completed_at_unix_ms"
            ]
        },
        "envelopes": {
            "cli": {
                "schema_version": "0.2",
                "success_fields": ["ok", "command", "data"],
                "error_fields": ["ok", "command", "error"]
            },
            "transport_view": {
                "query": "session transport",
                "daemon_query": "session request transport",
                "schema_version": "session.transport.v0.1",
                "plan_query": "session transport plan [--endpoint <url>]",
                "daemon_plan_query": "session request transport plan [--endpoint <url>]",
                "plan_schema_version": "session.transport_plan.v0.1",
                "plan_next_actions_field": "next_actions",
                "plan_trusted_remote_gate_field": "trusted_remote_gate",
                "plan_trusted_remote_gate_schema_version": "session.trusted_remote_gate.v0.1",
                "check_query": "session transport check --endpoint <url>",
                "daemon_check_query": "session request transport check --endpoint <url>",
                "check_schema_version": "session.transport_check.v0.1"
            },
            "status_view": {
                "query": "session status --diagnostics",
                "daemon_query": "session request status --diagnostics",
                "liveness_field": "diagnostics.liveness",
                "instance_registry_field": "diagnostics.instances",
                "lease_field": "diagnostics.leases",
                "queue_field": "diagnostics.queues",
                "queue_health_field": "diagnostics.queues.health",
                "pending_request_preview_field": "diagnostics.queues.pending_request_preview",
                "pending_response_preview_field": "diagnostics.queues.pending_response_preview",
                "journal_field": "diagnostics.journal",
                "recommended_actions_field": "diagnostics.recommended_actions",
                "capture_freshness_summary_field": "diagnostics.capture_freshness",
                "self_heal_summary_field": "diagnostics.self_heal",
                "interaction_flow_summary_field": "diagnostics.interaction_flow",
                "trusted_channel_summary_field": "diagnostics.trusted_channel",
                "phase_c_summary_field": "diagnostics.phase_c",
                "validation_summary_field": "diagnostics.validation",
                "monitor_policy_lease_actions": [
                    "monitor_policy_inspect_lease",
                    "monitor_policy_acquire_lease",
                    "monitor_policy_preempt_lease"
                ],
                "lease_freshness_actions": [
                    "stale_lease_inspect"
                ],
                "capture_health_actions": [
                    "stale_capture_recover",
                    "capture_backend_health_check"
                ],
                "self_heal_escalation_actions": [
                    "self_heal_escalation_review"
                ],
                "interaction_channel_actions": [
                    "interactive_stream_preflight_review",
                    "trusted_channel_preflight_review"
                ],
                "phase_c_plan_actions": [
                    "phase_c_plan_review"
                ],
                "validation_plan_actions": [
                    "validation_plan_review"
                ],
                "queue_health_actions": [
                    "blocked_request_inspect",
                    "blocked_request_cancel_dry_run",
                    "blocked_request_cancel",
                    "blocked_request_cancel_requires_lease",
                    "blocked_running_request_inspect",
                    "unclaimed_response_read"
                ],
                "journal_error_actions": [
                    "failed_request_inspect"
                ]
            },
            "readiness_view": {
                "query": "session readiness [--endpoint <url>]",
                "daemon_query": "session request readiness [--endpoint <url>]",
                "schema_version": "session.readiness.v0.1",
                "ready_field": "ready",
                "status_field": "status",
                "daemon_ready_field": "daemon.can_accept_requests",
                "queues_field": "queues",
                "queue_health_field": "queues.health",
                "instances_field": "instances",
                "instance_status_field": "instances.status",
                "selected_instance_status_field": "instances.selected_status",
                "selected_instance_missing_required_field": "instances.selected_missing_required",
                "transport_ready_field": "transport.safe_to_connect",
                "policy_summary_field": "policy_summary",
                "policy_summary_schema_version": "session.readiness_policy_summary.v0.1",
                "diagnostics_summary_field": "diagnostics_summary",
                "diagnostics_summary_schema_version": "session.readiness_diagnostics_summary.v0.1",
                "phase_c_summary_field": "diagnostics_summary.phase_c",
                "phase_c_acceptance_gates_schema_version_field": "diagnostics_summary.phase_c.acceptance_gates_schema_version",
                "phase_c_acceptance_gate_lane_count_field": "diagnostics_summary.phase_c.acceptance_gate_lane_count",
                "recommended_actions_field": "recommended_actions",
                "blockers_field": "blockers"
            },
            "queue_view": {
                "query": "session queue",
                "daemon_query": "session request queue",
                "schema_version": "session.queue.v0.1",
                "health_field": "health",
                "counts_field": "counts",
                "previews_field": "previews",
                "recommended_actions_field": "recommended_actions",
                "admission_field": "admission",
                "local_query_inspects_blocked_queue": true,
                "does_not_enqueue": true,
                "does_not_touch_device": true
            },
            "command_check_view": {
                "query": "session command-check <command...>",
                "daemon_query": "session request command-check <command...>",
                "schema_version": "session.command_check.v0.1",
                "safe_to_submit_field": "safe_to_submit",
                "command_class_field": "command_class",
                "lease_gate_field": "lease_gate",
                "queue_gate_field": "queue_gate",
                "instance_gate_field": "instance_gate",
                "throat_gate_field": "throat_gate",
                "phase_c_scope_field": "phase_c_scope",
                "phase_c_scope_schema_version": "session.command_phase_c_scope.v0.1",
                "routing_field": "routing",
                "does_not_enqueue": true,
                "does_not_touch_device": true
            },
            "submit_plan_view": {
                "query": "session submit-plan <command...>",
                "daemon_query": "session request submit-plan <command...>",
                "schema_version": "session.submit_plan.v0.1",
                "ready_to_submit_field": "ready_to_submit",
                "preflight_summary_field": "preflight_summary",
                "phase_c_execution_preflight_field": "phase_c_execution_preflight",
                "phase_c_execution_preflight_schema_version": "session.submit_phase_c_execution_preflight.v0.1",
                "readiness_field": "readiness",
                "command_check_field": "command_check",
                "queue_field": "queue",
                "blockers_field": "blockers",
                "does_not_enqueue": true,
                "does_not_touch_device": true
            },
            "validation_plan_view": {
                "query": "session validation-plan",
                "daemon_query": "session request validation-plan",
                "schema_version": "session.validation_plan.v0.1",
                "live_validation_status_field": "live_validation_status",
                "deferred_code_field": "deferred_code",
                "deferred_live_tasks_field": "deferred_live_tasks",
                "pending_live_acceptance_field": "pending_live_acceptance",
                "phase_acceptance_matrix_field": "phase_acceptance_matrix",
                "next_actions_field": "next_actions",
                "offline_verification_allowed_field": "offline_verification_allowed",
                "does_not_enqueue": true,
                "does_not_touch_device": true,
                "does_not_capture": true,
                "does_not_start_maatouch": true
            },
            "lease_view": {
                "query": "session lease list|status|touch|wait|acquire|release|preempt",
                "daemon_query": "session request lease list|status|touch|wait|acquire|release|preempt",
                "list_schema_version": "session.lease_list.v0.1",
                "list_query": "session lease list [--holder <id>] [--lease-id <id>]",
                "daemon_list_query": "session request lease list [--holder <id>] [--lease-id <id>]",
                "list_filters": ["--holder", "--lease-holder", "--lease-id"],
                "freshness_field": "freshness",
                "freshness_statuses": ["fresh", "stale"],
                "freshness_stale_after_ms": SESSION_LEASE_STALE_MS,
                "status_schema_version": "session.lease_status.v0.1",
                "touch_schema_version": "session.lease_touch.v0.1",
                "touch_query": "session lease touch [--holder <id>] [--lease-id <id>]",
                "daemon_touch_query": "session request lease touch [--holder <id>] [--lease-id <id>]",
                "touch_updates": "updated_at_unix_ms",
                "touch_requires_matching_holder": true,
                "wait_schema_version": "session.lease_wait.v0.1",
                "wait_query": "session lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]",
                "daemon_wait_query": "session request lease wait [--status free|held] [--holder <id>] [--lease-id <id>] [--timeout-ms N] [--poll-ms N]",
                "wait_default_status": "free",
                "wait_statuses": ["free", "held"],
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "wait_timeout_returns_current_state": true
            },
            "journal_view": {
                "query": "session journal",
                "daemon_query": "session request journal",
                "filters": ["--limit", "--command", "--data-summary-kind", "--status", "--lease-holder"],
                "global_filters": ["--instance", "--game", "--server"],
                "command_filter_repeats": true,
                "data_summary_kind_filter_repeats": true,
                "status_filter_values": ["completed", "failed"],
                "status_filter_repeats": true,
                "lease_holder_filter_repeats": true,
                "entry_selector_field": "entries[].global"
            },
            "response_view": {
                "query": "session response get <request-id> [--consume]",
                "daemon_query": "session request response get <request-id> [--consume]",
                "wait_query": "session response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]",
                "daemon_wait_query": "session request response wait <request-id> [--timeout-ms N] [--poll-ms N] [--consume]",
                "schema_version": "session.response.v0.1",
                "consume_flag": "--consume",
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "delete_after_successful_parse": true,
                "missing_response_code": "runtime_not_running"
            },
            "request_state_view": {
                "query": "session request-state get <request-id>",
                "daemon_query": "session request request-state get <request-id>",
                "wait_query": "session request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]",
                "daemon_wait_query": "session request request-state wait <request-id> [--status <state>] [--timeout-ms N] [--poll-ms N]",
                "schema_version": "session.request_state.v0.1",
                "list_query": "session request-state list [--limit N] [--status <state>] [--lease-holder <id>]",
                "daemon_list_query": "session request request-state list [--limit N] [--status <state>] [--lease-holder <id>]",
                "list_schema_version": "session.request_state_list.v0.1",
                "list_filters": ["--limit", "--status", "--lease-holder"],
                "list_global_filters": ["--instance", "--game", "--server"],
                "lease_holder_filter_repeats": true,
                "statuses": ["queued", "running", "response_available", "completed", "failed", "unknown"],
                "state_sources": ["requests", "running", "responses", "request-journal"],
                "wait_default_statuses": ["response_available", "completed", "failed"],
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "wait_timeout_returns_current_state": true
            },
            "event_view": {
                "query": "session events",
                "daemon_query": "session request events",
                "wait_query": "session events wait [--timeout-ms N] [--poll-ms N]",
                "daemon_wait_query": "session request events wait [--timeout-ms N] [--poll-ms N]",
                "schema_version": "session.events.v0.1",
                "filters": ["--limit", "--after-unix-ms", "--after-request-id", "--command", "--data-summary-kind", "--status", "--lease-holder"],
                "global_filters": ["--instance", "--game", "--server"],
                "wait_timeout_default_ms": SESSION_DAEMON_REQUEST_TIMEOUT_MS,
                "wait_poll_default_ms": 100,
                "wait_timeout_returns_empty_events": true,
                "command_filter_repeats": true,
                "data_summary_field": "events[].data_summary",
                "stream_data_summary_kind": "stream",
                "data_summary_kinds": ["stream", "queue", "bootstrap", "readiness", "throat_policy", "command_check", "submit_plan", "capture_policy", "record_policy", "self_heal_policy", "self_heal_plan", "phase_c_plan", "connect_plan", "stream_plan", "transport_plan", "validation_plan", "capture_diagnose", "stale_capture_recovery"],
                "data_summary_kind_filter_repeats": true,
                "status_filter_values": ["completed", "failed"],
                "status_filter_repeats": true,
                "lease_holder_filter_repeats": true,
                "cursor_fields": [
                    "latest_timestamp_unix_ms",
                    "next_after_unix_ms",
                    "latest_request_id",
                    "next_after_request_id"
                ],
                "cursor_error": "event_cursor_not_found"
            },
            "monitor_policy_view": {
                "query": "session monitor-policy status",
                "daemon_query": "session request monitor-policy status",
                "schema_version": "session.monitor_policy_status.v0.1",
                "state_field": "state",
                "policy_field": "policy",
                "execution_model": "daemon_owned_monitor_once",
                "default_read_only": true,
                "recovery_requires_matching_lease": true,
                "recovery_without_matching_lease_status": "deferred_by_lease"
            },
            "instance_registry_view": {
                "query": "session instance registry",
                "daemon_query": "session request instance registry",
                "schema_version": "session.instance_registry.v0.1",
                "ready_field": "instances[].validation.ready_for_device_control"
            },
            "app_lifecycle_view": {
                "query": "session app <launch|stop|force-stop|restart>",
                "daemon_query": "session request app <launch|stop|force-stop|restart>",
                "aliases": ["session instance app <launch|stop|force-stop|restart>", "session request instance app <launch|stop|force-stop|restart>"],
                "requires_lease": true,
                "actions": ["launch", "stop", "force-stop", "restart"],
                "action_field": "action",
                "package_field": "package"
            },
            "stream_view": null,
            "stale_capture_recovery_view": {
                "query": "session recover --stale-capture [--capture|--diagnose]",
                "daemon_query": "session request recover --stale-capture [--capture|--diagnose]",
                "read_only": true,
                "requires_lease": false,
                "executes_input": false,
                "executes_app_restart": false,
                "diagnosis_statuses": ["planned", "diagnosed_fresh", "diagnosed_stale", "diagnosis_unavailable"],
                "recovery_gate": "diagnose_capture_backend_before_restart"
            }
        },
        "command_classes": {
            "read_only": {
                "requires_lease": false,
                "examples": [
                    "status",
                    "bootstrap",
                    "readiness",
                    "connect-plan",
                    "stream-plan",
                    "throat-policy",
                    "capture-policy",
                    "record-policy",
                    "self-heal-policy",
                    "self-heal-plan",
                    "command-check",
                    "submit-plan",
                    "validation-plan",
                    "journal",
                    "events",
                    "response",
                    "request-state",
                    "contract",
                    "api",
                    "capabilities",
                    "devices",
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session recover --stale-capture",
                    "session record step --capture",
                    "session record step --current-frame",
                    "session monitor-policy status",
                    "session instance registry",
                    "monitor-once"
                ],
                "device_affecting_examples": [
                    "capture",
                    "capture-diagnose",
                    "stream",
                    "recognize",
                    "detect-page",
                    "current-page",
                    "is-visible",
                    "locate",
                    "session record step --capture",
                    "session record step --current-frame"
                ]
            },
            "control": {
                "requires_lease": true,
                "examples": [
                    "lease",
                    "session app launch",
                    "session app stop",
                    "session app force-stop",
                    "session app restart",
                    "session instance app launch",
                    "session instance app stop",
                    "session instance app force-stop",
                    "session instance app restart",
                    "lab-run",
                    "package-run",
                    "operation-run",
                    "tap",
                    "swipe",
                    "long-tap",
                    "key",
                    "text",
                    "stream --input-relay",
                    "stream --input-event <action,args>",
                    "stream --relay-event <action,args>",
                    "tap-target",
                    "navigate",
                    "recover except --stale-capture"
                ]
            },
            "daemon_state": {
                "requires_lease": false,
                "recovery_policy_requires_matching_lease": true,
                "recovery_policy_defers_without_matching_lease": true,
                "examples": [
                    "session record start",
                    "session record status",
                    "session record stop",
                    "session record step --frame <png>",
                    "session record candidates",
                    "session record amend",
                    "session record build-task",
                    "session record promote",
                    "session monitor-policy set",
                    "session monitor-policy clear"
                ]
            }
        },
        "failure_contract": {
            "missing_or_stale_daemon_code": "runtime_not_running",
            "strict_session_throat_failure_code": "session_daemon_required",
            "control_without_matching_lease_code": "lab_lease_required",
            "untrusted_remote_endpoint_code": "trusted_remote_transport_blocked",
            "missing_trusted_remote_auth_code": "trusted_remote_auth_required",
            "severe_errors_fail_loud": true
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
            "UI transport",
            "scheduler runtime"
        ]
    });
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "bootstrap_view".to_string(),
            session_bootstrap_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "connect_plan_view".to_string(),
            session_connect_plan_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert("stream_view".to_string(), session_stream_view_contract());
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "stream_plan_view".to_string(),
            session_stream_plan_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "throat_policy_view".to_string(),
            session_throat_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "capture_policy_view".to_string(),
            session_capture_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "record_policy_view".to_string(),
            session_record_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "self_heal_policy_view".to_string(),
            session_self_heal_policy_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "self_heal_plan_view".to_string(),
            session_self_heal_plan_view_contract(),
        );
    contract
        .pointer_mut("/envelopes")
        .and_then(Value::as_object_mut)
        .expect("session api contract envelopes must be an object")
        .insert(
            "phase_c_plan_view".to_string(),
            session_phase_c_plan_view_contract(),
        );
    contract
}

fn session_connect_plan_view_contract() -> Value {
    json!({
        "query": "session connect-plan [--endpoint <url>] [stream check flags]",
        "daemon_query": "session request connect-plan [--endpoint <url>] [stream check flags]",
        "schema_version": "session.connect_plan.v0.1",
        "readiness_field": "readiness",
        "transport_field": "transport",
        "stream_preflight_field": "stream_preflight",
        "phase_c_preflight_field": "phase_c_preflight",
        "phase_c_preflight_schema_version": "session.connect_phase_c_preflight.v0.1",
        "next_actions_field": "next_actions",
        "safe_to_start_client_field": "safe_to_start_client",
        "blocked_reason_field": "blockers",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true
    })
}

fn session_stream_view_contract() -> Value {
    json!({
        "query": "stream --max-frames <N>",
        "daemon_query": "session request stream",
        "check_query": "stream check",
        "daemon_check_query": "session request stream check",
        "plan_query": "session stream-plan",
        "daemon_plan_query": "session request stream-plan",
        "schema_version": "session.stream.v0.1",
        "check_schema_version": "session.stream_check.v0.1",
        "plan_schema_version": "session.stream_plan.v0.1",
        "event_schema_version": "session.stream.event.v0.1",
        "bounded_local_cli_status": "available",
        "read_only_without_input_relay_requires_lease": false,
        "input_relay_requires_lease": true,
        "safe_to_start_field": "safe_to_start",
        "input_relay_actions": ["tap", "swipe", "long-tap", "key", "text"],
        "input_relay_event_flags": ["--input-relay", "--input-event", "--relay-event"],
        "input_relay_preflight_command": "session command-check stream --input-event <action,args>",
        "trusted_remote_long_lived_stream_status": "reserved"
    })
}

fn session_stream_plan_view_contract() -> Value {
    json!({
        "query": "session stream-plan [--endpoint <url>] [stream check flags]",
        "daemon_query": "session request stream-plan [--endpoint <url>] [stream check flags]",
        "schema_version": "session.stream_plan.v0.1",
        "connect_plan_field": "connect_plan",
        "stream_preflight_field": "stream_preflight",
        "stream_modes_field": "stream_modes",
        "next_actions_field": "next_actions",
        "trusted_remote_long_lived_status_field": "stream_modes.trusted_remote_long_lived.status",
        "safe_to_open_stream_field": "safe_to_open_stream",
        "blocked_reason_field": "blockers",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true
    })
}

fn session_throat_policy_view_contract() -> Value {
    json!({
        "query": "session throat-policy",
        "daemon_query": "session request throat-policy",
        "schema_version": "session.throat_policy.v0.1",
        "only_control_throat_field": "session_layer.only_control_throat",
        "strict_session_throat_field": "strict_session_throat",
        "route_policy_field": "route_policy",
        "lease_gate_field": "lease_gate",
        "deferred_live_acceptance_field": "deferred_live_acceptance",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_capture_policy_view_contract() -> Value {
    json!({
        "query": "session capture-policy",
        "daemon_query": "session request capture-policy",
        "schema_version": "session.capture_policy.v0.1",
        "fresh_frame_policy_field": "fresh_frame_policy",
        "backend_policy_field": "backend_policy",
        "stale_classification_field": "stale_classification",
        "freeze_classification_gate_field": "freeze_classification_gate",
        "freeze_classification_gate_schema_version": "session.capture_freeze_classification_gate.v0.1",
        "recovery_policy_field": "recovery_policy",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_record_policy_view_contract() -> Value {
    json!({
        "query": "session record-policy",
        "daemon_query": "session request record-policy",
        "schema_version": "session.record_policy.v0.1",
        "authorization_model_field": "authorization_model",
        "allowed_step_kinds_field": "allowed_step_kinds",
        "frame_source_policy_field": "frame_source_policy",
        "resource_write_policy_field": "resource_write_policy",
        "safety_policy_field": "safety_policy",
        "client_guidance_field": "client_guidance",
        "live_validation_field": "live_validation",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_read_resource_repositories": true,
        "does_not_write_resource_repositories": true
    })
}

fn session_self_heal_policy_view_contract() -> Value {
    json!({
        "query": "session self-heal-policy",
        "daemon_query": "session request self-heal-policy",
        "schema_version": "session.self_heal_policy.v0.1",
        "phase_c_field": "phase_c",
        "flow_field": "flow",
        "trigger_policy_field": "trigger_policy",
        "recovery_order_field": "recovery_order",
        "maintenance_boundary_field": "maintenance_boundary",
        "lease_and_scheduler_policy_field": "lease_and_scheduler_policy",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_self_heal_plan_view_contract() -> Value {
    json!({
        "query": "session self-heal-plan [--trigger <kind>] [--to <page>]",
        "daemon_query": "session request self-heal-plan [--trigger <kind>] [--to <page>]",
        "schema_version": "session.self_heal_plan.v0.1",
        "status_field": "status",
        "trigger_field": "trigger",
        "recovery_field": "recovery",
        "escalation_field": "escalation",
        "readiness_field": "readiness",
        "queue_field": "queue",
        "lease_gate_field": "lease_gate",
        "execution_gate_field": "execution_gate",
        "execution_gate_schema_version": "session.self_heal_execution_gate.v0.1",
        "blockers_field": "blockers",
        "ready_to_execute_field": "ready_to_execute_maintenance",
        "next_actions_field": "next_actions",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

fn session_phase_c_plan_view_contract() -> Value {
    json!({
        "query": "session phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
        "daemon_query": "session request phase-c-plan [--endpoint <url>] [--trigger <kind>] [--to <page>]",
        "schema_version": "session.phase_c_plan.v0.1",
        "self_heal_field": "self_heal",
        "interaction_flow_field": "interaction_flow",
        "interaction_plan_schema_version": "session.phase_c_interaction_plan.v0.2",
        "interaction_stream_plan_contract_field": "interaction_flow.contract",
        "trusted_channel_field": "trusted_channel",
        "implementation_plan_field": "implementation_plan",
        "implementation_plan_schema_version": "session.phase_c_implementation_plan.v0.1",
        "acceptance_gates_field": "acceptance_gates",
        "acceptance_gates_schema_version": "session.phase_c_acceptance_gates.v0.1",
        "live_validation_field": "live_validation",
        "next_actions_field": "next_actions",
        "milestones_field": "milestones",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true,
        "does_not_issue_tokens": true,
        "does_not_start_tls": true
    })
}

fn session_bootstrap_view_contract() -> Value {
    json!({
        "query": "session bootstrap",
        "daemon_query": "session request bootstrap",
        "schema_version": "session.bootstrap.v0.1",
        "status_diagnostics_field": "status_diagnostics",
        "status_diagnostics_capture_freshness_field": "status_diagnostics.capture_freshness",
        "status_diagnostics_self_heal_field": "status_diagnostics.self_heal",
        "status_diagnostics_interaction_flow_field": "status_diagnostics.interaction_flow",
        "status_diagnostics_trusted_channel_field": "status_diagnostics.trusted_channel",
        "status_diagnostics_phase_c_field": "status_diagnostics.phase_c",
        "status_diagnostics_validation_field": "status_diagnostics.validation",
        "readiness_field": "readiness",
        "queue_field": "queue",
        "throat_policy_field": "throat_policy",
        "capture_policy_field": "capture_policy",
        "self_heal_policy_field": "self_heal_policy",
        "validation_plan_field": "validation_plan",
        "phase_c_plan_field": "phase_c_plan",
        "api_contract_field": "api_contract",
        "access_contract_field": "access_contract",
        "does_not_enqueue": true,
        "does_not_touch_device": true,
        "does_not_capture": true,
        "does_not_start_maatouch": true
    })
}

pub(crate) fn run_session_contract(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_session_routing(&flags)?;
    flags.expect_positionals("session contract", 0)?;
    Ok(session_access_contract())
}

pub(crate) fn run_session_api(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_session_routing(&flags)?;
    flags.expect_positionals("session api", 0)?;
    Ok(session_api_contract())
}

pub(crate) fn run_session_throat_policy(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_throat_policy_payload(global, &flags, "session throat-policy")
}

pub(crate) fn run_session_capture_policy(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_capture_policy_payload(global, &flags, "session capture-policy")
}

pub(crate) fn run_session_record_policy(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_record_policy_payload(global, &flags, "session record-policy")
}

pub(crate) fn run_session_self_heal_policy(
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    session_self_heal_policy_payload(global, &flags, "session self-heal-policy")
}
