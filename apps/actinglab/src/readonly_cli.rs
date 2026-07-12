// SPDX-License-Identifier: AGPL-3.0-only

use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, finish_semantic_result_with_ledger,
    parse_optional_duration_ms, read_user_config, record_env_needs_detection, record_env_resolved,
    reject_legacy_session_routing, resolve_instance_id, semantic_ledger_context, target_argument,
};
use actingcommand_lab::{
    CurrentPageRequest, DetectPageOutput, DetectPageRequest, DetectPageResponse, IsVisibleRequest,
    ReadonlyRecognitionInput, RecognizeRequest,
};
use actingcommand_ledger::IdKind;
use serde::Serialize;
use serde_json::{Value, json};
use std::time::Duration;

pub(super) fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let target = flags.required("--target")?;
    let prepared = prepare_recognition_input(global, &flags, false)?;
    let mut lab = super::env_detection::build_readonly_lab_for_capture(
        prepared.capture_instance_alias.as_deref(),
    )?;
    let request = RecognizeRequest {
        input: prepared.input,
        target,
    };
    serialize_response(lab.recognize(request)?)
}

pub(super) fn run_detect_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let mut ledger = semantic_ledger_context("detect-page", global, args);
    let result = (|| -> CliOutcome<Value> {
        let prepared = prepare_recognition_input(global, &flags, true)?;
        let mut lab = super::env_detection::build_readonly_lab_for_capture(
            prepared.capture_instance_alias.as_deref(),
        )?;
        let request = DetectPageRequest {
            input: prepared.input,
            check_pages: flags.bool("--check-pages"),
        };
        let output = lab.detect_page(request)?;
        record_detect_page_output(&mut ledger, output)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_current_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let prepared = prepare_recognition_input(global, &flags, true)?;
    let mut lab = super::env_detection::build_readonly_lab_for_capture(
        prepared.capture_instance_alias.as_deref(),
    )?;
    let request = CurrentPageRequest {
        input: prepared.input,
    };
    serialize_response(lab.current_page(request)?)
}

pub(super) fn run_is_visible(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    let prepared = prepare_recognition_input(global, &flags, false)?;
    let mut lab = super::env_detection::build_readonly_lab_for_capture(
        prepared.capture_instance_alias.as_deref(),
    )?;
    let request = IsVisibleRequest {
        input: prepared.input,
        target: target_argument(&flags, "is-visible")?,
    };
    serialize_response(lab.is_visible(request)?)
}

struct PreparedReadonlyInput {
    input: ReadonlyRecognitionInput,
    capture_instance_alias: Option<String>,
}

fn prepare_recognition_input(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
) -> CliOutcome<PreparedReadonlyInput> {
    let config = read_user_config()?;
    prepare_recognition_input_with_config(global, flags, require_pages, &config)
}

pub(super) fn recognition_input_with_config(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
    config: &super::UserConfig,
) -> CliOutcome<ReadonlyRecognitionInput> {
    Ok(prepare_recognition_input_with_config(global, flags, require_pages, config)?.input)
}

fn prepare_recognition_input_with_config(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
    config: &super::UserConfig,
) -> CliOutcome<PreparedReadonlyInput> {
    let resources = super::contained_resources::load(flags, "readonly")?;
    if require_pages {
        super::contained_resources::recognition_pipeline(&resources)?;
    }
    let (capture_config, capture_instance_alias) = if !flags.bool("--capture") {
        (None, None)
    } else {
        (
            Some(super::env_detection::runtime_capture_port_config()),
            Some(resolve_instance_id(global, config)?),
        )
    };
    let fresh_delay = if capture_config.is_some() {
        parse_optional_duration_ms(flags, "--fresh-delay-ms", 160)?
    } else {
        Duration::from_millis(160)
    };
    Ok(PreparedReadonlyInput {
        input: ReadonlyRecognitionInput {
            resources,
            scene: None,
            scene_path: flags.optional_path("--scene"),
            capture_config,
            require_fresh: flags.bool("--require-fresh"),
            fresh_delay,
        },
        capture_instance_alias,
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
