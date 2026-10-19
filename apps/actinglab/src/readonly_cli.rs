// SPDX-License-Identifier: AGPL-3.0-only

use super::lab2_cli::refuse_repeated_options;
use super::{
    CliError, CliOutcome, FlagArgs, GlobalOptions, RecognitionEvaluator, Scene,
    finish_semantic_result_with_ledger, parse_optional_duration_ms, read_user_config,
    record_env_needs_detection, record_env_resolved, reject_legacy_session_routing,
    resolve_instance_id, semantic_ledger_context, target_argument,
};
use actingcommand_lab::{
    CurrentPageRequest, DetectPageOutput, DetectPageRequest, DetectPageResponse, IsVisibleRequest,
    ReadonlyRecognitionInput, RecognizeRequest,
};
use actingcommand_ledger::IdKind;
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn run_recognize_artifact(args: &[String]) -> CliOutcome<Value> {
    use actingcommand_contract::SavedArtifactOcrRequest;
    use actingcommand_runtime_client::RuntimeClient;
    use std::io::Read;
    let flags = FlagArgs::parse(args)?;
    let path = flags.required_path("--request")?;
    let file = std::fs::File::open(&path)
        .map_err(|error| CliError::usage(format!("cannot read recognition request: {error}")))?;
    let mut bytes = Vec::new();
    file.take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|error| CliError::usage(format!("cannot read recognition request: {error}")))?;
    if bytes.len() > 65_536 {
        return Err(CliError::usage("recognition request exceeds 65536 bytes"));
    }
    let request: SavedArtifactOcrRequest = serde_json::from_slice(&bytes)
        .map_err(|error| CliError::usage(format!("invalid recognition request: {error}")))?;
    request
        .validate()
        .map_err(|error| CliError::usage(error.to_string()))?;
    let client = RuntimeClient::connect(crate::lab_client_config(super::runtime_state_root()?))
        .map_err(|error| CliError::device(error.to_string()))?;
    serialize_response(
        client
            .recognize_artifact(request)
            .map_err(|error| CliError::device(error.to_string()))?,
    )
}

pub(super) fn run_recognize(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    refuse_repeated_options(&flags, "recognize", &FRAME_SET_REPEATS)?;
    let target = flags.required("--target")?;
    let prepared = prepare_frame_input(global, &flags, false)?;
    require_ocr_target(&prepared.input, &target)?;
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
    refuse_repeated_options(&flags, "detect-page", &FRAME_SET_REPEATS)?;
    let mut ledger = semantic_ledger_context("detect-page", global, args);
    let result = (|| -> CliOutcome<Value> {
        let prepared = prepare_frame_input(global, &flags, true)?;
        let check_pages = flags.bool("--check-pages");
        // `--check-pages` validates the page set and evaluates nothing.
        if !check_pages {
            super::offline_ocr::require_pages(&prepared.input.resources)?;
        }
        let mut lab = super::env_detection::build_readonly_lab_for_capture(
            prepared.capture_instance_alias.as_deref(),
        )?;
        let request = DetectPageRequest {
            input: prepared.input,
            check_pages,
        };
        let output = lab.detect_page(request)?;
        record_detect_page_output(&mut ledger, output)
    })();
    finish_semantic_result_with_ledger(global, ledger, result)
}

pub(super) fn run_current_page(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse(args)?;
    reject_legacy_session_routing(&flags)?;
    refuse_repeated_options(&flags, "current-page", &FRAME_SET_REPEATS)?;
    let prepared = prepare_frame_input(global, &flags, true)?;
    super::offline_ocr::require_pages(&prepared.input.resources)?;
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
    refuse_repeated_options(&flags, "is-visible", &FRAME_SET_REPEATS)?;
    let prepared = prepare_frame_input(global, &flags, false)?;
    let target = target_argument(&flags, "is-visible")?;
    require_ocr_target(&prepared.input, &target)?;
    let mut lab = super::env_detection::build_readonly_lab_for_capture(
        prepared.capture_instance_alias.as_deref(),
    )?;
    let request = IsVisibleRequest {
        input: prepared.input,
        target,
    };
    serialize_response(lab.is_visible(request)?)
}

/// #411 Lab B-2 R7 (coordinator decision Q9 of 2026-10-19): the only option `recognize`,
/// `detect-page`, `current-page` and `is-visible` take more than once is `--scene`, their frame set
/// (R1, up to five frames); every other option of theirs given twice is refused as `repeated`
/// before anything is read, never last-wins.
const FRAME_SET_REPEATS: [&str; 1] = ["--scene"];

struct PreparedReadonlyInput {
    input: ReadonlyRecognitionInput,
    capture_instance_alias: Option<String>,
}

/// Workflow #396 LAB-3: an OCR target (or a composite with an OCR member) is refused with
/// `lab_ocr_unavailable` when actinglab has no OCR model, before anything is evaluated.
fn require_ocr_target(input: &ReadonlyRecognitionInput, target: &str) -> CliOutcome<()> {
    match input.resources.loaded_bundle().evaluator() {
        Some(evaluator) => super::offline_ocr::require_target(evaluator, target),
        None => Ok(()),
    }
}

fn prepare_recognition_input(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
) -> CliOutcome<PreparedReadonlyInput> {
    let config = read_user_config()?;
    prepare_recognition_input_with_config(global, flags, require_pages, &config)
}

/// #411 Lab B-2 (R1): the input of `recognize`, `detect-page`, `current-page` and `is-visible`:
/// one to five `--scene` frames, oldest first, the last the current frame. Counts and sizes are
/// refused before anything is evaluated (R3). One frame is read by the Lab as before; a frame set
/// is read here and handed over as the current frame and its earlier frames.
fn prepare_frame_input(
    global: &GlobalOptions,
    flags: &FlagArgs,
    require_pages: bool,
) -> CliOutcome<PreparedReadonlyInput> {
    let paths = scene_paths(flags)?;
    let mut prepared = prepare_recognition_input(global, flags, require_pages)?;
    if paths.is_empty() {
        return Ok(prepared);
    }
    if let Some(evaluator) = prepared.input.resources.loaded_bundle().evaluator() {
        require_scene_frames(evaluator, paths.len())?;
    }
    if paths.len() > 1 {
        let mut frames = read_scene_frames(&paths)?
            .into_iter()
            .map(|(_, scene)| scene)
            .collect::<Vec<_>>();
        prepared.input.scene = frames.pop();
        prepared.input.scene_path = None;
        prepared.input.sample_scenes = frames;
    }
    Ok(prepared)
}

/// #411 Lab B-2: an offline frame set holds at most five frames, as the recognition pack's
/// aggregation owner takes them (`scene_context_with_samples`).
pub(super) const MAX_SCENE_FRAMES: usize = 5;

/// #411 Lab B-2 (R3): the refusal of the `--scene` frames: `validation_failed` (exit 2) with
/// details `{argument: "--scene", argument_problem}` and the members of `extra`.
pub(super) fn scene_refusal(problem: &str, message: String, extra: Value) -> CliError {
    let mut details = json!({"argument": "--scene", "argument_problem": problem});
    if let (Some(details), Value::Object(extra)) = (details.as_object_mut(), extra) {
        details.extend(extra);
    }
    CliError::usage(message).with_details(details)
}

/// #411 Lab B-2 (R1, R3): the `--scene` frames as given, oldest first, the last the current
/// frame, before anything is read: more than five are `out_of_range` (1..5); in a set of two or
/// more, a `--scene` without its value is `value_missing` and a path given twice is `repeated`.
/// One `--scene` without a value names no frame, as before.
pub(super) fn scene_paths(flags: &FlagArgs) -> CliOutcome<Vec<PathBuf>> {
    let given = flags.count("--scene");
    if given > MAX_SCENE_FRAMES {
        return Err(scene_refusal(
            "out_of_range",
            format!("--scene may be given 1 to {MAX_SCENE_FRAMES} times, got {given}"),
            json!({"range_min": 1, "range_max": MAX_SCENE_FRAMES}),
        ));
    }
    if given < 2 {
        return Ok(flags.optional_path("--scene").into_iter().collect());
    }
    let values = flags.values("--scene");
    if values.iter().any(|value| value == "true") {
        return Err(scene_refusal(
            "value_missing",
            "every --scene of a frame set names its <png>".to_string(),
            json!({}),
        ));
    }
    let paths = values.iter().map(PathBuf::from).collect::<Vec<_>>();
    if let Some(index) = (1..paths.len()).find(|&index| paths[..index].contains(&paths[index])) {
        return Err(scene_refusal(
            "repeated",
            format!(
                "--scene names {} twice; each frame of a set is its own capture",
                paths[index].display()
            ),
            json!({}),
        ));
    }
    Ok(paths)
}

/// #411 Lab B-2 (R3): the package's rule for `given` (1 to 5) frames, before anything is
/// evaluated: more than one frame for a package that declares no agreement over several frames
/// (target consensus and every candidate layout over one frame) is `repeated`; fewer frames than
/// its target consensus requires is `out_of_range` with `range_min` the required frames,
/// `range_max` 5, `required_frames` and `provided_frames`.
pub(super) fn require_scene_frames(
    evaluator: &RecognitionEvaluator,
    given: usize,
) -> CliOutcome<()> {
    let required = evaluator.target_sample_frames();
    let agreement = evaluator
        .pack()
        .candidate_layouts
        .iter()
        .map(|layout| layout.required_frames())
        .fold(required, usize::max);
    if given > MAX_SCENE_FRAMES && agreement == 1 {
        return Err(scene_refusal(
            "repeated",
            format!(
                "--scene was given {given} times, but the package declares no agreement over several frames and reads one"
            ),
            json!({}),
        ));
    }
    if given < required {
        return Err(scene_refusal(
            "out_of_range",
            format!(
                "the package's target consensus requires {required} frames; --scene gave {given}"
            ),
            json!({
                "range_min": required,
                "range_max": MAX_SCENE_FRAMES,
                "required_frames": required,
                "provided_frames": given,
            }),
        ));
    }
    Ok(())
}

/// #411 Lab B-2 (R3): the frames `paths` name, oldest first, each with its PNG; a frame whose
/// size differs from the current (last) frame's is refused before anything is evaluated as
/// `unusable` with its `index` (0 the oldest).
pub(super) fn read_scene_frames(paths: &[PathBuf]) -> CliOutcome<Vec<(Vec<u8>, Scene)>> {
    let frames = paths
        .iter()
        .map(|path| {
            let png = fs::read(path).map_err(|err| {
                CliError::device(format!("failed to read {}: {err}", path.display()))
            })?;
            let scene = Scene::from_png(&png).map_err(|err| CliError::device(err.to_string()))?;
            Ok((png, scene))
        })
        .collect::<CliOutcome<Vec<_>>>()?;
    if let Some((_, current)) = frames.last()
        && let Some(index) = frames.iter().position(|(_, scene)| {
            scene.width() != current.width() || scene.height() != current.height()
        })
    {
        let scene = &frames[index].1;
        return Err(scene_refusal(
            "unusable",
            format!(
                "--scene frame {index} is {}x{} and the current frame {}x{}; the frames of a set show one screen",
                scene.width(),
                scene.height(),
                current.width(),
                current.height()
            ),
            json!({"index": index}),
        ));
    }
    Ok(frames)
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
    if flags.bool("--capture") {
        return Err(CliError::usage(
            "legacy semantic --capture is retired; use observe/do/ensure/wait so one Runtime debug correlation owns capture, input, and ledger facts",
        ));
    }
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
            sample_scenes: Vec::new(),
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
