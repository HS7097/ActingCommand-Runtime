// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, finish_semantic_result_with_ledger,
    parse_optional_duration_ms, read_user_config, reject_legacy_session_routing,
    resolve_instance_id, semantic_ledger_context, target_argument,
};
use actingcommand_lab::{NavigateRequest, TapTargetRequest};
use serde::Serialize;
use serde_json::Value;

pub(super) fn run_tap_target(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let target = target_argument(&flags, "tap-target")?;
    let mut ledger = semantic_ledger_context("tap-target", global, args);
    let result = (|| -> CliOutcome<Value> {
        let dry_run = global.dry_run || flags.bool("--dry-run");
        let config = read_user_config()?;
        let capture_requested = flags.bool("--capture");
        let instance_alias = capture_requested
            .then(|| resolve_instance_id(global, &config))
            .transpose()?;
        let input =
            super::readonly_cli::recognition_input_with_config(global, &flags, false, &config)?;
        let mut lab =
            super::env_detection::build_drive_lab(config, instance_alias.as_deref(), !dry_run)?;
        let request = TapTargetRequest {
            input,
            target,
            allow_destructive: flags.bool("--allow-destructive"),
            dry_run,
            capture_requested,
        };
        serialize_response(lab.tap_target(request, &mut ledger)?)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_navigate(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let to = flags.required("--to")?;
    let mut ledger = semantic_ledger_context("navigate", global, args);
    let result = (|| -> CliOutcome<Value> {
        let dry_run = global.dry_run || flags.bool("--dry-run");
        let config = read_user_config()?;
        let capture_requested = flags.bool("--capture");
        let instance_alias = capture_requested
            .then(|| resolve_instance_id(global, &config))
            .transpose()?;
        let input =
            super::readonly_cli::recognition_input_with_config(global, &flags, true, &config)?;
        let mut lab =
            super::env_detection::build_drive_lab(config, instance_alias.as_deref(), !dry_run)?;
        let request = NavigateRequest {
            input,
            to,
            allow_destructive: flags.bool("--allow-destructive"),
            dry_run,
            capture_requested,
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
