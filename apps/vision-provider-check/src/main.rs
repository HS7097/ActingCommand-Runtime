// SPDX-License-Identifier: AGPL-3.0-only

//! Mechanical checks of vision inputs that never load ONNX Runtime or run a model: the model
//! folders of a vision root (Workflow #360) and the provider startup facts in a Runtime ledger.

use actingcommand_vision_ffi::{
    NnModelSpec, OcrModelSpec, VisionFfiError, VisionFfiResult, list_vision_models,
    ppocr_model_set_sha256, sha256_file_hex,
};
mod ledger;
use serde::Serialize;
use std::env;
use std::path::{Path, PathBuf};

const MODULE: &str = "vision-provider-check";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckOptions {
    models_root: PathBuf,
    hash: bool,
}

#[derive(Debug, Serialize)]
struct ModelFoldersReport {
    ok: bool,
    models_root: String,
    hashed: bool,
    ocr_models: Vec<OcrModelReport>,
    nn_models: Vec<NnModelReport>,
    invalid_models: Vec<InvalidModelReport>,
}

#[derive(Debug, Serialize)]
struct OcrModelReport {
    model_ref: String,
    layout: &'static str,
    detector: bool,
    description_sha256: String,
    languages: Vec<String>,
    /// The content identity a target names as `model_sha256`; only with `--hash`.
    #[serde(skip_serializing_if = "Option::is_none")]
    model_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
struct NnModelReport {
    model_ref: String,
    description_sha256: String,
    languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_sha256: Option<String>,
}

#[derive(Debug, Serialize)]
struct InvalidModelReport {
    name: String,
    path: String,
    reason: String,
}

fn main() {
    if let Err(error) = actingcommand_contract::process_installation() {
        eprintln!("FATAL vision-provider-check: {error}");
        std::process::exit(1);
    }
    if let Err(err) = run(env::args().skip(1)) {
        eprintln!("FATAL: {err}");
        std::process::exit(1);
    }
}

fn run<I>(args: I) -> VisionFfiResult<()>
where
    I: IntoIterator<Item = String>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--state-root") {
        return ledger::run(args);
    }
    let options = parse_args(args)?;
    let report = model_folders_report(&options)?;
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "observation": "model_folders",
        "report": report,
    }))
    .map_err(|err| {
        VisionFfiError::fatal(
            MODULE,
            format!("failed to serialize model folder report: {err}"),
        )
    })?;
    println!("{json}");
    if !report.ok {
        return Err(VisionFfiError::fatal(
            MODULE,
            format!(
                "{} model folder(s) break the folder rule",
                report.invalid_models.len()
            ),
        ));
    }
    Ok(())
}

fn parse_args<I>(args: I) -> VisionFfiResult<CheckOptions>
where
    I: IntoIterator<Item = String>,
{
    let mut models_root = None;
    let mut hash = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--models-root" => {
                models_root = Some(PathBuf::from(argument_value(&mut args, &arg)?));
            }
            "--hash" => hash = true,
            _ => {
                return Err(VisionFfiError::fatal(
                    MODULE,
                    format!("unknown argument: {arg}\n{}", usage()),
                ));
            }
        }
    }
    let models_root = models_root.ok_or_else(|| {
        VisionFfiError::fatal(
            MODULE,
            format!("--models-root or --state-root is required\n{}", usage()),
        )
    })?;
    Ok(CheckOptions { models_root, hash })
}

fn argument_value(args: &mut impl Iterator<Item = String>, arg: &str) -> VisionFfiResult<String> {
    args.next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| VisionFfiError::fatal(MODULE, format!("{arg} requires a value")))
}

/// Lists the model folders exactly as actingd does at startup and, with `--hash`, reads every
/// model file once to compute the identity a target must name.
fn model_folders_report(options: &CheckOptions) -> VisionFfiResult<ModelFoldersReport> {
    let listing = list_vision_models(&options.models_root)?;
    let ocr_models = listing
        .ocr
        .iter()
        .map(|spec| {
            Ok(OcrModelReport {
                model_ref: spec.model_ref.clone(),
                layout: spec.layout.as_str(),
                detector: spec.detector_path.is_some(),
                description_sha256: spec.description_sha256.clone(),
                languages: spec.description.languages.clone(),
                model_sha256: options.hash.then(|| ocr_model_sha256(spec)).transpose()?,
            })
        })
        .collect::<VisionFfiResult<Vec<_>>>()?;
    let nn_models = listing
        .nn
        .iter()
        .map(|spec| {
            Ok(NnModelReport {
                model_ref: spec.model_ref.clone(),
                description_sha256: spec.description_sha256.clone(),
                languages: spec.description.languages.clone(),
                model_sha256: options.hash.then(|| nn_model_sha256(spec)).transpose()?,
            })
        })
        .collect::<VisionFfiResult<Vec<_>>>()?;
    let invalid_models = listing
        .invalid
        .into_iter()
        .map(|folder| InvalidModelReport {
            name: folder.name,
            path: path_string(&folder.path),
            reason: folder.reason,
        })
        .collect::<Vec<_>>();
    Ok(ModelFoldersReport {
        ok: invalid_models.is_empty(),
        models_root: path_string(&options.models_root),
        hashed: options.hash,
        ocr_models,
        nn_models,
        invalid_models,
    })
}

fn ocr_model_sha256(spec: &OcrModelSpec) -> VisionFfiResult<String> {
    let detector = spec
        .detector_path
        .as_deref()
        .map(|path| sha256_file_hex(MODULE, path))
        .transpose()?;
    ppocr_model_set_sha256(
        detector.as_deref(),
        &sha256_file_hex(MODULE, &spec.recognizer_path)?,
        &sha256_file_hex(MODULE, &spec.dictionary_path)?,
        None,
    )
}

fn nn_model_sha256(spec: &NnModelSpec) -> VisionFfiResult<String> {
    sha256_file_hex(MODULE, &spec.model_path)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn usage() -> &'static str {
    "Usage: actingcommand-vision-provider-check --state-root <runtime-state> [--after <sequence>] [--through <sequence>] [--limit <1..1024>]\nModel folders: --models-root <vision root>\\models [--hash]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_manifest_arg_is_fatal() {
        let err = parse_args(Vec::<String>::new()).expect_err("missing mode rejected");

        assert_eq!(err.module(), "vision-provider-check");
        assert!(
            err.message()
                .contains("--models-root or --state-root is required")
        );
    }

    #[test]
    fn parses_ledger_cursor_and_bounded_limit() {
        let options = ledger::parse(
            [
                "--state-root",
                "runtime-state",
                "--after",
                "4",
                "--through",
                "9",
                "--limit",
                "2",
            ]
            .map(str::to_owned),
        )
        .expect("read-only options");
        assert_eq!(options.state_root, PathBuf::from("runtime-state"));
        assert_eq!(options.after, 4);
        assert_eq!(options.through, Some(9));
        assert_eq!(options.limit, 2);
    }
    #[test]
    fn rejects_invalid_ledger_cursor_and_missing_root() {
        for arguments in [
            vec![],
            vec!["--state-root"],
            vec!["--state-root", ""],
            vec!["--state-root", "state", "--after", "10", "--through", "9"],
            vec!["--state-root", "state", "--limit", "0"],
            vec!["--state-root", "state", "--limit", "1025"],
            vec!["--state-root", "state", "--through", "invalid"],
            vec!["--state-root", "state", "--manifest", "provider.json"],
        ] {
            assert!(ledger::parse(arguments.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn rejects_execution_arguments() {
        for arguments in [
            vec!["--ocr-frame", "frame.png"],
            vec!["--nn-frame", "frame.png"],
            vec!["--ocr-region", "0,0,1,1"],
            vec!["--nn-model-id", "neutral"],
            vec!["--abi-check"],
            vec!["--models-root", "models", "--ocr-frame", "frame.png"],
            vec!["--models-root", "models", "--abi-check"],
            vec!["--abi-check", "--ocr-frame", "frame.png"],
            vec!["--ocr-frame", "frame.png", "--nn-frame", "frame.png"],
            vec!["--hash", "--ocr-frame", "frame.png"],
        ] {
            let error = parse_args(arguments.into_iter().map(str::to_owned))
                .expect_err("file observation cannot execute a provider");
            assert!(error.to_string().contains("unknown argument"));
        }
    }
}
