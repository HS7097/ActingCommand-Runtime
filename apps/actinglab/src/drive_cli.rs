// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, device_config,
    finish_semantic_result_with_ledger, navigation_path, parse_optional_duration_ms,
    read_user_config, semantic_ledger_context, should_route_control_via_session_daemon,
    submit_control_session_request, target_argument,
};
use actingcommand_lab::{NavigateRequest, TapTargetRequest};
use serde::Serialize;
use serde_json::Value;

pub(super) fn run_tap_target(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_control_via_session_daemon(global, &flags)? {
        return submit_control_session_request(global, &flags, "tap_target", args);
    }
    let target = target_argument(&flags, "tap-target")?;
    let mut ledger = semantic_ledger_context("tap-target", global, args);
    let result = (|| -> CliOutcome<Value> {
        let dry_run = global.dry_run || flags.bool("--dry-run");
        let config = read_user_config()?;
        let input =
            super::readonly_cli::recognition_input_with_config(global, &flags, false, &config)?;
        let device = (!dry_run).then(|| device_config(global, &config));
        let mut lab = super::env_detection::build_control_lab(
            config,
            device.as_ref().and_then(|result| result.as_ref().ok()),
        )?;
        let request = TapTargetRequest {
            input,
            target,
            allow_destructive: flags.bool("--allow-destructive"),
            dry_run,
            capture_requested: flags.bool("--capture"),
            touch_config: device.map(|result| result.map(|device| device.touch_backend_config())),
        };
        serialize_response(lab.tap_target(request, &mut ledger)?)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_navigate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_control_via_session_daemon(global, &flags)? {
        return submit_control_session_request(global, &flags, "navigate", args);
    }
    let to = flags.required("--to")?;
    let mut ledger = semantic_ledger_context("navigate", global, args);
    let result = (|| -> CliOutcome<Value> {
        let dry_run = global.dry_run || flags.bool("--dry-run");
        let config = read_user_config()?;
        let input =
            super::readonly_cli::recognition_input_with_config(global, &flags, true, &config)?;
        let navigation_path = navigation_path(global, &config, &flags);
        let device = (!dry_run).then(|| device_config(global, &config));
        let mut lab = super::env_detection::build_control_lab(
            config,
            device.as_ref().and_then(|result| result.as_ref().ok()),
        )?;
        let request = NavigateRequest {
            input,
            navigation_path,
            to,
            allow_destructive: flags.bool("--allow-destructive"),
            dry_run,
            capture_requested: flags.bool("--capture"),
            touch_config: device.map(|result| result.map(|device| device.touch_backend_config())),
            step_timeout: (!dry_run)
                .then(|| parse_optional_duration_ms(&flags, "--step-timeout-ms", 5_000)),
            poll: (!dry_run).then(|| parse_optional_duration_ms(&flags, "--poll-ms", 500)),
        };
        serialize_response(lab.navigate(request, &mut ledger)?)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
