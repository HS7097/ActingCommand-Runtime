// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, SESSION_STATE_ENV, StreamInputRelayAction,
    current_unix_ms, parse_optional_duration_ms, parse_optional_usize, read_user_config,
    run_stream_input_relay, run_stream_legacy, runtime_session_adapter, runtime_slice_cli,
    runtime_state_root, stream_check_requested, stream_contract_json, stream_events_json,
};
use actingcommand_contract::{
    CaptureSequenceSpec, EventActor, EventSource, ReadonlyObservation, RuntimeResult,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::env;

pub(super) fn run_stream(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    let dry_run = global.dry_run || flags.bool("--dry-run");
    if stream_check_requested(&flags)
        || dry_run
        || global.inside_session_daemon
        || explicit_legacy_stream_requested(&flags)
    {
        return run_stream_legacy(global, args);
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
    let max_frames = parse_optional_usize(flags, "--max-frames", 1)?;
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
        json!({
            "status": "disabled",
            "reason": "no input relay action requested"
        })
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
    let contract = stream_contract_json(
        max_frames,
        interval,
        fresh_delay,
        false,
        relay_actions.len(),
        false,
    );
    let stream_id = format!("stream-{}-{}", current_unix_ms(), std::process::id());
    let events = stream_events_json(&stream_id, &frames, &input_relay);
    Ok(json!({
        "stream_id": stream_id,
        "mode": "bounded_stream",
        "instance": instance,
        "transport": "local_cli",
        "max_frames": max_frames,
        "interval_ms": interval.as_millis(),
        "capture": {
            "require_fresh": false,
            "dry_run": false
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
    }))
}

fn explicit_legacy_stream_requested(flags: &FlagArgs) -> bool {
    flags.bool("--local")
        || flags.bool("--via-daemon")
        || flags.optional("--state-dir").is_some()
        || env::var_os(SESSION_STATE_ENV).is_some()
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
