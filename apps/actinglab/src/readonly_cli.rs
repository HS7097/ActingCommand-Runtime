// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, device_config,
    finish_semantic_result_with_ledger, parse_optional_duration_ms, read_user_config,
    recognition_resources, record_env_needs_detection, record_env_resolved,
    semantic_ledger_context, should_route_readonly_via_session_daemon,
    submit_readonly_session_request, target_argument,
};
use actingcommand_lab::{
    CurrentPageRequest, DetectPageOutput, DetectPageRequest, DetectPageResponse,
    EnvMarkerResolutionRequest, IsVisibleRequest, ReadonlyRecognitionInput, RecognizeRequest,
};
use actingcommand_ledger::IdKind;
use serde::Serialize;
use serde_json::{Value, json};
use std::time::Duration;

pub(super) fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "recognize", args);
    }
    let target = flags.required("--target")?;
    let request = RecognizeRequest {
        input: recognition_input(global, &flags, false)?,
        target,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.recognize(request)?)
}

pub(super) fn run_detect_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "detect_page", args);
    }
    let mut ledger = semantic_ledger_context("detect-page", global, args);
    let result = (|| -> CliOutcome<Value> {
        let request = DetectPageRequest {
            input: recognition_input(global, &flags, true)?,
            check_pages: flags.bool("--check-pages"),
        };
        let mut lab = super::env_detection::build_readonly_lab()?;
        let output = lab.detect_page(request)?;
        record_detect_page_output(&mut ledger, output)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_current_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "current_page", args);
    }
    let request = CurrentPageRequest {
        input: recognition_input(global, &flags, true)?,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.current_page(request)?)
}

pub(super) fn run_is_visible(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    if should_route_readonly_via_session_daemon(global, &flags)? {
        return submit_readonly_session_request(global, &flags, "is_visible", args);
    }
    let request = IsVisibleRequest {
        input: recognition_input(global, &flags, false)?,
        target: target_argument(&flags, "is-visible")?,
    };
    let mut lab = super::env_detection::build_readonly_lab()?;
    serialize_response(lab.is_visible(request)?)
}

pub(super) fn recognition_input(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
) -> CliOutcome<ReadonlyRecognitionInput> {
    let config = read_user_config()?;
    recognition_input_with_config(global, flags, require_pages, &config)
}

pub(super) fn recognition_input_with_config(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
    config: &super::UserConfig,
) -> CliOutcome<ReadonlyRecognitionInput> {
    let resources = recognition_resources(global, config, flags, require_pages)?;
    let capture_config = flags
        .bool("--capture")
        .then(|| device_config(global, config))
        .transpose()?
        .map(|device| device.capture_backend_config());
    let fresh_delay = if capture_config.is_some() {
        parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?
    } else {
        Duration::from_millis(160)
    };
    Ok(ReadonlyRecognitionInput {
        marker_request: EnvMarkerResolutionRequest {
            resource_root: resources.pack_root.clone(),
            instance: flags
                .optional("--instance")
                .or_else(|| global.instance.clone()),
            game: flags.optional("--game").or_else(|| global.game.clone()),
            server: flags.optional("--server").or_else(|| global.server.clone()),
            env_task: flags.optional("--env-task"),
        },
        pack_path: resources.pack_path,
        pack_root: resources.pack_root,
        pages_path: resources.pages_path,
        scene: None,
        scene_path: flags.optional_path("--scene"),
        capture_config,
        require_fresh: flags.bool("--require-fresh"),
        fresh_delay,
    })
}

fn record_detect_page_output(
    ledger: &mut actingcommand_lab::SemanticLedgerContext,
    output: DetectPageOutput,
) -> CliOutcome<Value> {
    record_env_resolved(ledger, "detect-page", &output.env_resolved)?;
    match output.response {
        DetectPageResponse::Check(response) => serialize_response(response),
        DetectPageResponse::Detection(mut response) => {
            let reco_id = ledger.issue(IdKind::Reco);
            response.req_id = Some(ledger.req_id.clone());
            response.reco_id = Some(reco_id.clone());
            if response.standby {
                record_env_needs_detection(
                    ledger,
                    "detect-page",
                    "current_page_unknown",
                    &response.page,
                    &output.env_resolved,
                )?;
            }
            ledger.record_drive(json!({
                "stage": "recognition",
                "command": "detect-page",
                "reco_id": reco_id,
                "page": response.page,
                "matched": response.matched,
                "standby": response.standby
            }))?;
            serialize_response(response)
        }
    }
}

fn serialize_response<T: Serialize>(response: T) -> CliOutcome<Value> {
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize Lab response: {error}")))
}
