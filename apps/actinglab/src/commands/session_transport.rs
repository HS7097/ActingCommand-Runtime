use crate::runtime_endpoint::{
    env_var_non_empty, runtime_endpoint_check, runtime_endpoint_policy,
    runtime_endpoint_policy_json,
};
use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, REQUIRE_SESSION_DAEMON_ENV,
    TRUSTED_REMOTE_CLIENT_CERT_ENV, TRUSTED_REMOTE_TOKEN_ENV, parse_optional_string_value,
    reject_legacy_session_routing,
};
use serde_json::{Value, json};

fn session_transport_contract() -> Value {
    json!({
        "schema_version": "session.transport.v0.1",
        "purpose": "machine-readable transport boundary for Session Layer clients",
        "channels": {
            "local_cli": {
                "status": "available",
                "transport": "process_stdio",
                "command": "actinglab",
                "encryption_required": false,
                "authentication_required": false,
                "intended_clients": ["local_operator", "local_agent"]
            },
            "daemon_file_ipc": {
                "status": "available",
                "transport": "session_state_directory_file_queue",
                "submit_command": "session request <command>",
                "request_dir": "requests/",
                "response_dir": "responses/",
                "journal": "request-journal.jsonl",
                "serialized_by_daemon": true,
                "read_only_requests_require_lease": false,
                "control_requests_require_matching_lease": true
            },
            "trusted_remote": {
                "status": "reserved",
                "network_listener_implemented": false,
                "plan_command": "session transport plan [--endpoint <url>]",
                "plan_gate_field": "trusted_remote_gate",
                "plan_gate_schema_version": "session.trusted_remote_gate.v0.1",
                "preflight_command": "session transport check --endpoint <url>",
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
            },
            "interactive_stream": {
                "status": "partial",
                "preflight_command": "stream check",
                "daemon_preflight_command": "session request stream check",
                "preflight_schema_version": "session.stream_check.v0.1",
                "implemented_surfaces": {
                    "bounded_local_cli_stream": {
                        "status": "available",
                        "command": "stream --max-frames <N>",
                        "schema_version": "session.stream.v0.1",
                        "frame_delivery": "json_array",
                        "frame_event_schema": "session.stream.event.v0.1",
                        "max_frames_per_request": 60
                    },
                    "daemon_bounded_stream_request": {
                        "status": "available",
                        "command": "session request stream",
                        "read_only_without_input_relay_requires_lease": false,
                        "input_relay_requires_matching_lease": true
                    },
                    "per_request_input_relay": {
                        "status": "available",
                        "actions": ["tap", "swipe", "long-tap", "key", "text"],
                        "max_events_per_request": 16,
                        "long_lived_session": false
                    }
                },
                "trusted_remote_long_lived_stream": {
                    "status": "reserved",
                    "future_transport": "trusted bidirectional channel",
                    "network_listener_implemented": false,
                    "encryption_required": true,
                    "authentication_required": true
                }
            }
        },
        "safety": {
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "session_daemon_required",
            "clients_must_not_directly_touch_adb_or_devices": true,
            "remote_transport_must_not_start_without_authentication": true,
            "remote_transport_must_not_start_without_encryption": true,
            "control_requests_are_lease_gated": true,
            "requests_are_serialized_by_resident_daemon": true
        },
        "out_of_scope": [
            "network listener",
            "TLS implementation",
            "token issuance",
            "trusted remote long-lived stream transport",
            "scheduler runtime"
        ]
    })
}

fn session_connect_plan_next_action(
    priority: u8,
    action: &str,
    reason: &str,
    command: &str,
    read_only: bool,
) -> Value {
    json!({
        "priority": priority,
        "action": action,
        "reason": reason,
        "command": command,
        "read_only": read_only
    })
}

pub(crate) fn run_session_transport(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let _ = global;
    reject_legacy_session_routing(&flags)?;
    session_transport_payload(&flags)
}

fn session_transport_payload(flags: &FlagArgs) -> CliOutcome<Value> {
    match flags.positionals.first().map(String::as_str) {
        None => Ok(session_transport_contract()),
        Some("plan") => session_transport_plan_payload(&flags.without_first_positional()),
        Some("check") => session_transport_check_payload(&flags.without_first_positional()),
        Some(other) => Err(CliError::usage(format!(
            "unknown session transport command: {other}"
        ))),
    }
}

fn session_transport_plan_payload(flags: &FlagArgs) -> CliOutcome<Value> {
    flags.expect_positionals("session transport plan", 0)?;
    let endpoint = parse_optional_string_value(flags, "--endpoint")?;
    let endpoint_policy = session_transport_plan_endpoint_policy(endpoint.as_deref());
    let endpoint_policy_safe = endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool);
    let has_endpoint_policy_blocker = endpoint_policy_safe == Some(false);
    let blockers = session_transport_plan_blockers(&endpoint_policy);
    let trusted_remote_gate =
        session_transport_plan_trusted_remote_gate(&endpoint_policy, &blockers);
    let next_actions = session_transport_plan_next_actions(&endpoint_policy, &blockers);
    Ok(json!({
        "schema_version": "session.transport_plan.v0.1",
        "status": if has_endpoint_policy_blocker { "blocked" } else { "reserved" },
        "mode": "trusted_channel_startup_preflight",
        "local_cli": {
            "status": "available",
            "command": "actinglab",
            "encryption_required": false,
            "authentication_required": false
        },
        "daemon_file_ipc": {
            "status": "available",
            "command": "session request <command>",
            "serialized_by_daemon": true,
            "control_requests_require_matching_lease": true
        },
        "trusted_remote": {
            "status": "reserved",
            "network_listener_implemented": false,
            "safe_to_start_listener": false,
            "ready_to_accept_remote_clients": false,
            "requires_encryption": true,
            "requires_authentication": true,
            "token_configured": env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV),
            "client_certificate_configured": env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV),
            "token_env": TRUSTED_REMOTE_TOKEN_ENV,
            "client_certificate_env": TRUSTED_REMOTE_CLIENT_CERT_ENV,
            "endpoint_policy": endpoint_policy,
            "required_before_enable": [
                "reviewed network listener implementation",
                "TLS or mutually authenticated local IPC",
                "token or client certificate authentication",
                "request serialization through the resident Session Layer",
                "audit logging for accepted remote commands"
            ]
        },
        "trusted_remote_gate": trusted_remote_gate,
        "blockers": blockers,
        "next_actions": next_actions,
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_probe_tcp": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true,
            "does_not_read_resource_repositories": true
        }
    }))
}

fn session_transport_plan_endpoint_policy(endpoint: Option<&str>) -> Value {
    let Some(endpoint) = endpoint else {
        return json!({
            "checked": false,
            "safe_for_policy": null,
            "does_not_probe_tcp": true,
            "message": "No endpoint was provided; run with --endpoint <url> to classify local versus trusted remote policy."
        });
    };
    match runtime_endpoint_policy(endpoint) {
        Ok(policy) => json!({
            "checked": true,
            "endpoint": endpoint,
            "safe_for_policy": true,
            "policy": runtime_endpoint_policy_json(&policy),
            "does_not_probe_tcp": true
        }),
        Err(err) => json!({
            "checked": true,
            "endpoint": endpoint,
            "safe_for_policy": false,
            "error_code": err.code,
            "error": err.message,
            "blocked_by": err.blocked_by,
            "does_not_probe_tcp": true
        }),
    }
}

fn session_transport_plan_blockers(endpoint_policy: &Value) -> Vec<Value> {
    let mut blockers = vec![json!({
        "kind": "trusted_remote_listener",
        "code": "trusted_remote_listener_reserved",
        "message": "Trusted remote listener is reserved and is not implemented in this offline milestone."
    })];
    if endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool)
        == Some(false)
    {
        blockers.push(json!({
            "kind": "trusted_remote_endpoint_policy",
            "code": endpoint_policy.get("error_code"),
            "message": endpoint_policy.get("error"),
            "blocked_by": endpoint_policy.get("blocked_by"),
            "endpoint": endpoint_policy.get("endpoint")
        }));
    }
    blockers
}

fn session_transport_plan_trusted_remote_gate(
    endpoint_policy: &Value,
    blockers: &[Value],
) -> Value {
    let endpoint_checked = endpoint_policy
        .get("checked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let endpoint_safe = endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool);
    let endpoint_channel = endpoint_policy
        .pointer("/policy/channel")
        .and_then(Value::as_str);
    let trusted_remote_requested = endpoint_channel == Some("trusted_remote");
    let token_configured = env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV);
    let client_certificate_configured = env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV);
    let auth_material_configured = token_configured || client_certificate_configured;
    let mut blocked_reasons = blockers
        .iter()
        .map(|blocker| {
            json!({
                "kind": blocker.get("kind").cloned().unwrap_or(Value::Null),
                "code": blocker.get("code").cloned().unwrap_or(Value::Null),
                "message": blocker.get("message").cloned().unwrap_or(Value::Null)
            })
        })
        .collect::<Vec<_>>();

    if !endpoint_checked {
        blocked_reasons.push(json!({
            "kind": "trusted_remote_endpoint_policy",
            "code": "trusted_remote_endpoint_not_checked",
            "message": "Run session transport check --endpoint <url> before enabling trusted remote access."
        }));
    }
    if !auth_material_configured {
        blocked_reasons.push(json!({
            "kind": "trusted_remote_authentication",
            "code": "trusted_remote_auth_required",
            "message": "Configure a token or client certificate before trusted remote clients can authenticate."
        }));
    }

    let status = if endpoint_safe == Some(false) {
        "blocked"
    } else if endpoint_channel == Some("local_direct") {
        "not_applicable_local_direct"
    } else {
        "reserved"
    };

    json!({
        "schema_version": "session.trusted_remote_gate.v0.1",
        "status": status,
        "trusted_remote_requested": trusted_remote_requested,
        "endpoint_policy_checked": endpoint_checked,
        "endpoint_policy_safe": endpoint_safe,
        "endpoint": endpoint_policy.get("endpoint").cloned().unwrap_or(Value::Null),
        "endpoint_channel": endpoint_channel,
        "requires_encryption": true,
        "requires_authentication": true,
        "token_configured": token_configured,
        "client_certificate_configured": client_certificate_configured,
        "auth_material_configured": auth_material_configured,
        "network_listener_implemented": false,
        "tls_implemented": false,
        "token_issuer_implemented": false,
        "request_serialization_required": true,
        "audit_logging_required": true,
        "safe_to_start_listener": false,
        "safe_to_accept_remote_clients": false,
        "blocked_reason_count": blocked_reasons.len(),
        "blocked_reasons": blocked_reasons,
        "live_validation": {
            "status": "deferred",
            "deferred_code": "requires-live-device"
        },
        "guarantees": {
            "does_not_enqueue": true,
            "does_not_touch_device": true,
            "does_not_capture": true,
            "does_not_start_maatouch": true,
            "does_not_start_listener": true,
            "does_not_probe_tcp": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true,
            "does_not_read_resource_repositories": true,
            "does_not_mark_live_validation_passed": true
        }
    })
}

fn session_transport_plan_next_actions(endpoint_policy: &Value, blockers: &[Value]) -> Value {
    let endpoint_checked = endpoint_policy
        .get("checked")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let endpoint_safe = endpoint_policy
        .get("safe_for_policy")
        .and_then(Value::as_bool);
    let token_configured = env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV);
    let client_certificate_configured = env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV);
    let auth_material_configured = token_configured || client_certificate_configured;
    let mut ordered = Vec::new();
    let mut priority = 1;

    if !endpoint_checked {
        ordered.push(session_connect_plan_next_action(
            priority,
            "classify_endpoint_policy",
            "Classify the intended trusted remote endpoint before any listener or client transport work.",
            "session transport check --endpoint <url>",
            true,
        ));
        priority += 1;
    }

    if endpoint_safe == Some(false) {
        ordered.push(session_connect_plan_next_action(
            priority,
            "review_endpoint_policy_blocker",
            "Fix the trusted remote endpoint policy before any remote channel can be enabled.",
            "session transport check --endpoint <url>",
            true,
        ));
        priority += 1;
    }

    if !auth_material_configured {
        ordered.push(session_connect_plan_next_action(
            priority,
            "prepare_remote_auth_material",
            "Configure a token or client certificate before remote clients can authenticate.",
            "configure ACTINGLAB_TRUSTED_REMOTE_TOKEN or ACTINGLAB_TRUSTED_REMOTE_CLIENT_CERT",
            false,
        ));
        priority += 1;
    }

    ordered.push(session_connect_plan_next_action(
        priority,
        "review_listener_and_tls_design",
        "Review the network listener, TLS boundary, and authentication model before implementation.",
        "session transport plan [--endpoint <url>]",
        true,
    ));
    priority += 1;

    ordered.push(session_connect_plan_next_action(
        priority,
        "review_request_serialization_and_audit",
        "Remote commands must serialize through the resident Session Layer and leave an audit trail.",
        "session api",
        true,
    ));
    priority += 1;

    ordered.push(session_connect_plan_next_action(
        priority,
        "review_live_acceptance_checklist",
        "Trusted remote transport still requires live listener, TLS, auth, and operator validation later.",
        "session validation-plan",
        true,
    ));

    json!({
        "schema_version": "session.transport_next_actions.v0.1",
        "status": if endpoint_safe == Some(false) { "blocked" } else { "reserved" },
        "ordered": ordered,
        "trusted_remote": {
            "status": "reserved",
            "network_listener_implemented": false,
            "ready_to_accept_remote_clients": false,
            "endpoint_policy_checked": endpoint_checked,
            "endpoint_policy_safe": endpoint_safe,
            "endpoint": endpoint_policy.get("endpoint").cloned().unwrap_or(Value::Null),
            "token_configured": token_configured,
            "client_certificate_configured": client_certificate_configured,
            "auth_material_configured": auth_material_configured,
            "blocker_count": blockers.len()
        },
        "required_before_enable": [
            "reviewed network listener implementation",
            "TLS or mutually authenticated local IPC",
            "token or client certificate authentication",
            "request serialization through the resident Session Layer",
            "audit logging for accepted remote commands"
        ],
        "local_cli": {
            "status": "available",
            "encryption_required": false,
            "authentication_required": false
        },
        "daemon_file_ipc": {
            "status": "available",
            "serialized_by_daemon": true,
            "control_requests_require_matching_lease": true
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
            "does_not_start_listener": true,
            "does_not_probe_tcp": true,
            "does_not_issue_tokens": true,
            "does_not_start_tls": true,
            "does_not_read_resource_repositories": true,
            "does_not_mark_live_validation_passed": true
        }
    })
}

fn session_transport_check_payload(flags: &FlagArgs) -> CliOutcome<Value> {
    flags.expect_positionals("session transport check", 0)?;
    let endpoint = flags.required("--endpoint")?;
    let check = runtime_endpoint_check(&endpoint);
    Ok(json!({
        "schema_version": "session.transport_check.v0.1",
        "endpoint": endpoint,
        "check": check,
        "safe_to_connect": check.get("ok").and_then(Value::as_bool).unwrap_or(false),
        "does_not_start_listener": true
    }))
}
