// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, StreamInputRelayAction, current_unix_ms,
    parse_optional_duration_ms, parse_optional_usize, read_user_config,
    resolve_instance_id_for_flags, run_stream_input_relay, runtime_session_adapter,
    runtime_slice_cli, runtime_state_root, stream_check_requested, stream_contract_json,
    stream_events_json,
};
use actingcommand_contract::{
    CaptureSequenceSpec, EventActor, EventSource, ReadonlyObservation, RuntimeResult,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::time::Duration;

pub(super) fn run_stream(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_flags(&flags)?;
    if stream_check_requested(&flags) {
        return run_stream_check(global, &flags.without_first_positional());
    }
    if global.dry_run || flags.bool("--dry-run") {
        return run_dry_stream(global, &flags);
    }
    run_runtime_stream(global, &flags)
}

fn run_runtime_stream(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    if flags.bool("--require-fresh") {
        return Err(CliError::usage(
            "--require-fresh is not supported by the Runtime bounded capture sequence adapter",
        ));
    }
    let relay_actions = StreamInputRelayAction::parse_many(flags)?;
    let max_frames = parse_max_frames(flags)?;
    let interval = parse_optional_duration_ms(flags, "--interval-ms", 250)?;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let frame_count = u16::try_from(max_frames)
        .map_err(|_| CliError::usage("--max-frames must be between 1 and 60"))?;
    let interval_ms = u64::try_from(interval.as_millis())
        .map_err(|_| CliError::usage("--interval-ms is too large"))?;
    let spec = CaptureSequenceSpec::new(frame_count, interval_ms)
        .map_err(|error| CliError::usage(error.to_string()))?;
    let state_root = runtime_state_root()?;
    let requested_instance = flags
        .optional("--instance")
        .filter(|value| value != "true")
        .or_else(|| global.instance.clone());
    let instance = runtime_session_adapter::resolve_instance_alias(
        &state_root,
        requested_instance.as_deref(),
    )?;
    let input_relay = if relay_actions.is_empty() {
        disabled_input_relay()
    } else {
        let config = read_user_config()?;
        let mut runtime_global = global.clone();
        runtime_global.instance = Some(instance.clone());
        run_stream_input_relay(&runtime_global, &config, &relay_actions, false)?
    };
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        &state_root,
        EventActor::Lab,
        EventSource::Lab,
    ))
    .map_err(runtime_slice_cli::map_runtime_error)?;
    let output = client
        .capture_sequence(&instance, spec)
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let sequence = match output.receipt().result() {
        Some(RuntimeResult::CaptureSequenceCompleted { sequence }) => sequence,
        _ => {
            return Err(CliError::device(
                "Runtime returned an unexpected bounded capture sequence result",
            ));
        }
    };
    if sequence.spec() != spec {
        return Err(CliError::device(
            "Runtime returned a capture sequence with a mismatched specification",
        ));
    }
    let frames = project_frames(sequence.observations())?;
    Ok(build_stream_response(
        instance,
        max_frames,
        interval,
        fresh_delay,
        false,
        relay_actions.len(),
        input_relay,
        frames,
        false,
    ))
}

fn run_dry_stream(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let relay_actions = StreamInputRelayAction::parse_many(flags)?;
    let max_frames = parse_max_frames(flags)?;
    let interval = parse_optional_duration_ms(flags, "--interval-ms", 250)?;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let config = read_user_config()?;
    let instance = resolve_instance_id_for_flags(global, &config, flags)?;
    let input_relay = if relay_actions.is_empty() {
        disabled_input_relay()
    } else {
        run_stream_input_relay(global, &config, &relay_actions, true)?
    };
    Ok(build_stream_response(
        instance,
        max_frames,
        interval,
        fresh_delay,
        flags.bool("--require-fresh"),
        relay_actions.len(),
        input_relay,
        dry_run_frames(max_frames),
        true,
    ))
}

fn run_stream_check(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let relay_actions = StreamInputRelayAction::parse_many(flags)?;
    if relay_actions.is_empty() {
        flags.expect_positionals("stream check", 0)?;
    }
    let max_frames = parse_max_frames(flags)?;
    let interval = parse_optional_duration_ms(flags, "--interval-ms", 250)?;
    let fresh_delay = parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?;
    let config = read_user_config()?;
    let instance = resolve_instance_id_for_flags(global, &config, flags)?;
    let require_fresh = flags.bool("--require-fresh");
    Ok(json!({
        "schema_version": "session.stream_check.v0.1",
        "instance": instance,
        "safe_to_start": !require_fresh,
        "does_not_capture": true,
        "does_not_start_maatouch": true,
        "does_not_start_listener": true,
        "mode": "bounded_stream_preflight",
        "routing": {
            "authority": "runtime",
            "legacy_session_retired": true,
            "would_route_via_daemon": false,
            "daemon_alive": Value::Null,
            "daemon_required_satisfied": true,
            "local_override": false,
            "explicit_daemon": false
        },
        "capture": {
            "require_fresh": require_fresh,
            "dry_run": global.dry_run || flags.bool("--dry-run"),
            "interval_ms": interval.as_millis(),
            "fresh_delay_ms": fresh_delay.as_millis(),
            "requested_max_frames": max_frames,
            "max_frames_per_request": 60
        },
        "input_relay": {
            "requested": !relay_actions.is_empty(),
            "action_count": relay_actions.len(),
            "actions": relay_actions.iter().map(StreamInputRelayAction::to_json).collect::<Vec<_>>(),
            "runtime_scheduler_fencing": true,
            "lease_gate": {
                "ok": true,
                "status": "runtime_managed",
                "reason": "Runtime input proxy acquires and fences its scheduler lease"
            }
        },
        "trusted_channel": {
            "status": "reserved",
            "long_lived_stream_implemented": false
        },
        "blockers": if require_fresh {
            vec![json!({
                "code": "runtime_freshness_contract_unavailable",
                "message": "Runtime bounded capture sequences do not yet expose a cross-frame freshness contract"
            })]
        } else {
            Vec::<Value>::new()
        }
    }))
}

fn parse_max_frames(flags: &FlagArgs) -> CliOutcome<usize> {
    let max_frames = parse_optional_usize(flags, "--max-frames", 1)?;
    if max_frames == 0 || max_frames > 60 {
        return Err(CliError::usage("--max-frames must be between 1 and 60"));
    }
    Ok(max_frames)
}

#[allow(clippy::too_many_arguments)]
fn build_stream_response(
    instance: String,
    max_frames: usize,
    interval: Duration,
    fresh_delay: Duration,
    require_fresh: bool,
    input_event_count: usize,
    input_relay: Value,
    frames: Vec<Value>,
    dry_run: bool,
) -> Value {
    let contract = stream_contract_json(
        max_frames,
        interval,
        fresh_delay,
        require_fresh,
        input_event_count,
        dry_run,
    );
    let stream_id = format!("stream-{}-{}", current_unix_ms(), std::process::id());
    let events = stream_events_json(&stream_id, &frames, &input_relay);
    json!({
        "stream_id": stream_id,
        "mode": "bounded_stream",
        "instance": instance,
        "transport": "local_cli",
        "max_frames": max_frames,
        "interval_ms": interval.as_millis(),
        "capture": {
            "require_fresh": require_fresh,
            "dry_run": dry_run
        },
        "trusted_channel": {
            "status": "reserved",
            "long_lived_stream_implemented": false,
            "reason": "trusted remote long-lived stream transport is not implemented; this command is a bounded local CLI stream"
        },
        "contract": contract,
        "input_relay": input_relay,
        "events": events,
        "frames": frames
    })
}

fn disabled_input_relay() -> Value {
    json!({
        "status": "disabled",
        "reason": "no input relay action requested"
    })
}

fn reject_legacy_flags(flags: &FlagArgs) -> CliOutcome<()> {
    if flags.bool("--via-daemon")
        || flags.bool("--local")
        || flags.optional("--state-dir").is_some()
    {
        return Err(CliError::not_implemented(
            "legacy_session_authority_retired",
            "legacy Session daemon and file-state routing were retired; use the resident Runtime",
        ));
    }
    Ok(())
}

fn project_frames(observations: &[ReadonlyObservation]) -> CliOutcome<Vec<Value>> {
    observations
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            let artifact = observation.artifact();
            if artifact.object_key().is_none() {
                return Err(CliError::device(
                    "Runtime capture sequence omitted its stored artifact object key",
                ));
            }
            let backend = serde_json::to_value(observation.capture_backend()).map_err(|error| {
                CliError::device(format!(
                    "failed to serialize Runtime capture backend: {error}"
                ))
            })?;
            let artifact_value = serde_json::to_value(artifact).map_err(|error| {
                CliError::device(format!(
                    "failed to serialize Runtime capture artifact: {error}"
                ))
            })?;
            Ok(json!({
                "index": index,
                "captured": true,
                "captured_at_unix_ms": artifact.created_at_unix_ms,
                "frame": {
                    "width": observation.width(),
                    "height": observation.height(),
                    "backend": backend,
                    "digest": artifact.sha256
                },
                "freshness": {
                    "status": "runtime_artifact_verified",
                    "require_fresh": false
                },
                "adb_source": "runtime_owned",
                "adb_warning": Value::Null,
                "capture_backend_attempts": [],
                "artifact": artifact_value
            }))
        })
        .collect()
}

pub(super) fn dry_run_frames(max_frames: usize) -> Vec<Value> {
    (0..max_frames)
        .map(|index| {
            json!({
                "index": index,
                "captured": false,
                "mode": "dry_run"
            })
        })
        .collect()
}
