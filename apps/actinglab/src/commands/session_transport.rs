use crate::runtime_endpoint::runtime_endpoint_check;
use crate::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, REQUIRE_SESSION_DAEMON_ENV,
    parse_optional_string_value, reject_legacy_session_routing,
};
use serde_json::{Value, json};

fn session_transport_contract() -> Value {
    json!({
        "schema_version": "session.transport.v0.1",
        "purpose": "machine-readable transport boundary for Session Layer clients",
        "channels": {
            "local_cli": {
                "status": "available",
                "available": true,
                "reason_code": "offline_handler_ready",
                "transport": "process_stdio",
                "command": "actinglab",
                "encryption_required": false,
                "authentication_required": false,
                "intended_clients": ["local_operator", "local_agent"]
            },
            "daemon_file_ipc": {
                "status": "retired",
                "available": false,
                "reason_code": "legacy_session_authority_retired",
                "transport": "session_state_directory_file_queue",
                "submit_command": "session request <command>",
                "request_dir": "requests/",
                "response_dir": "responses/",
                "journal": "request-journal.jsonl",
                "serialized_by_daemon": false,
                "read_only_requests_require_lease": false,
                "control_requests_require_matching_lease": true
            },
            "trusted_remote": {
                "status": "retired",
                "available": false,
                "reason_code": "trusted_remote_transport_retired"
            },
            "interactive_stream": {
                "status": "unverified",
                "available": false,
                "reason_code": "runtime_dependency_unverified",
                "preflight_command": "stream check",
                "daemon_preflight_command": "session request stream check",
                "preflight_schema_version": "session.stream_check.v0.1",
                "implemented_surfaces": {
                    "bounded_local_cli_stream": {
                        "status": "unverified",
                        "available": false,
                        "reason_code": "runtime_dependency_unverified",
                        "command": "stream --max-frames <N>",
                        "schema_version": "session.stream.v0.1",
                        "frame_delivery": "json_array",
                        "frame_event_schema": "session.stream.event.v0.1",
                        "max_frames_per_request": 60
                    },
                    "daemon_bounded_stream_request": {
                        "status": "retired",
                        "available": false,
                        "reason_code": "legacy_session_authority_retired",
                        "command": "session request stream",
                        "read_only_without_input_relay_requires_lease": false,
                        "input_relay_requires_matching_lease": true
                    },
                    "per_request_input_relay": {
                        "status": "unverified",
                        "available": false,
                        "reason_code": "runtime_dependency_unverified",
                        "actions": ["tap", "swipe", "long-tap", "key", "text"],
                        "max_events_per_request": 16,
                        "long_lived_session": false
                    }
                },
                "trusted_remote_long_lived_stream": {
                    "status": "retired",
                    "available": false,
                    "reason_code": "trusted_remote_transport_retired"
                }
            }
        },
        "safety": {
            "strict_session_throat_status": "retired",
            "strict_session_throat_flag": "--require-session",
            "strict_session_throat_env": REQUIRE_SESSION_DAEMON_ENV,
            "strict_session_throat_failure_code": "validation_failed",
            "clients_must_not_directly_touch_adb_or_devices": true,
            "control_requests_are_lease_gated": true,
            "requests_are_serialized_by_resident_daemon": false,
            "execution_authority": "runtime"
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
    parse_optional_string_value(flags, "--endpoint")?;
    // Workflow #355 D4: the trusted-remote transport plan was retired with the remote endpoints.
    Err(CliError::not_implemented(
        "trusted_remote_transport_retired",
        "trusted remote Runtime endpoints were retired; ActingLab connects only to a local Runtime, use session transport check --endpoint 127.0.0.1:<port>",
    ))
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
