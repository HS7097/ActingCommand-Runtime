// SPDX-License-Identifier: AGPL-3.0-only

//! The command line (design §1.5 item 1):
//!
//! ```text
//! ac-vision-ep-bench --vision-root <dir> --ep cpu|cuda|directml [--device <n>]
//!     [--cpu-threads <n>] [--cuda-conv-search exhaustive|heuristic] [--cuda-tf32 on|off]
//!     --workload <json> [--warmup 3] [--reps 20] --out <json>
//!     [--models-dir <dir>] [--label <name>] [--baseline <json>]
//! ac-vision-ep-bench --list-adapters
//! ```
//!
//! `--device` is the CUDA ordinal for `cuda` and the DXGI adapter index for `directml`; `cpu`
//! takes none. The CUDA knobs belong to `cuda` only. `--baseline` names the `cpu-1t` report the
//! consistency section compares with.

use crate::error::{BenchCode, BenchError};
use actingcommand_vision_ffi::{CudaConvAlgorithmSearch, OnnxExecutionProvider, OrtSessionTuning};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The most intra-op threads the bench accepts.
pub const MAX_CPU_THREADS: u16 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print the DXGI adapters (and nothing else) as JSON on stdout.
    ListAdapters,
    /// Boxed: the options are far larger than the data-less `ListAdapters`.
    Run(Box<RunOptions>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub vision_root: PathBuf,
    pub models_dir: PathBuf,
    pub execution_provider: OnnxExecutionProvider,
    pub device: Option<u32>,
    pub tuning: OrtSessionTuning,
    pub workload: PathBuf,
    pub warmup: u32,
    pub reps: u32,
    pub out: PathBuf,
    pub label: String,
    pub baseline: Option<PathBuf>,
}

const VALUE_FLAGS: &[&str] = &[
    "--vision-root",
    "--models-dir",
    "--ep",
    "--device",
    "--cpu-threads",
    "--cuda-conv-search",
    "--cuda-tf32",
    "--workload",
    "--warmup",
    "--reps",
    "--out",
    "--label",
    "--baseline",
];

/// Parses the arguments after the program name.
pub fn parse_args(args: &[String]) -> Result<Command, BenchError> {
    if args.len() == 1 && args[0] == "--list-adapters" {
        return Ok(Command::ListAdapters);
    }
    let mut values = BTreeMap::<&str, &str>::new();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let Some(known) = VALUE_FLAGS.iter().find(|known| **known == flag) else {
            return Err(cli(format!(
                "unknown argument {flag:?} (--list-adapters stands alone)"
            )));
        };
        let Some(value) = args.get(index + 1) else {
            return Err(cli(format!("{flag} needs a value")));
        };
        if values.insert(*known, value.as_str()).is_some() {
            return Err(cli(format!("{flag} is given twice")));
        }
        index += 2;
    }
    let required = |flag: &str| {
        values
            .get(flag)
            .copied()
            .ok_or_else(|| cli(format!("{flag} is required")))
    };
    let vision_root = PathBuf::from(required("--vision-root")?);
    let execution_provider = match required("--ep")? {
        "cpu" => OnnxExecutionProvider::Cpu,
        "cuda" => OnnxExecutionProvider::Cuda,
        "directml" => OnnxExecutionProvider::Directml,
        other => return Err(cli(format!("--ep {other:?} is not cpu, cuda or directml"))),
    };
    let device = values
        .get("--device")
        .map(|value| parse_number::<u32>("--device", value))
        .transpose()?;
    match (execution_provider, device) {
        (OnnxExecutionProvider::Cpu, Some(_)) => {
            return Err(cli("--ep cpu takes no --device"));
        }
        (OnnxExecutionProvider::Cuda | OnnxExecutionProvider::Directml, None) => {
            return Err(cli(
                "--ep cuda needs --device <CUDA ordinal>; --ep directml needs --device <DXGI adapter index>",
            ));
        }
        _ => {}
    }
    let mut tuning = OrtSessionTuning::default();
    if let Some(value) = values.get("--cpu-threads") {
        let threads = parse_number::<u16>("--cpu-threads", value)?;
        if threads == 0 || threads > MAX_CPU_THREADS {
            return Err(cli(format!(
                "--cpu-threads must be in 1..={MAX_CPU_THREADS}"
            )));
        }
        tuning.intra_threads = threads;
    }
    let cuda_knob = values.contains_key("--cuda-conv-search") || values.contains_key("--cuda-tf32");
    if cuda_knob && execution_provider != OnnxExecutionProvider::Cuda {
        return Err(cli(
            "--cuda-conv-search and --cuda-tf32 belong to --ep cuda only",
        ));
    }
    if let Some(value) = values.get("--cuda-conv-search") {
        tuning.cuda_conv_algorithm_search = match *value {
            "exhaustive" => CudaConvAlgorithmSearch::Exhaustive,
            "heuristic" => CudaConvAlgorithmSearch::Heuristic,
            other => {
                return Err(cli(format!(
                    "--cuda-conv-search {other:?} is not exhaustive or heuristic"
                )));
            }
        };
    }
    if let Some(value) = values.get("--cuda-tf32") {
        tuning.cuda_tf32 = match *value {
            "on" => true,
            "off" => false,
            other => return Err(cli(format!("--cuda-tf32 {other:?} is not on or off"))),
        };
    }
    let warmup = values
        .get("--warmup")
        .map(|value| parse_number::<u32>("--warmup", value))
        .transpose()?
        .unwrap_or(3);
    let reps = values
        .get("--reps")
        .map(|value| parse_number::<u32>("--reps", value))
        .transpose()?
        .unwrap_or(20);
    if reps == 0 {
        return Err(cli("--reps must be at least 1"));
    }
    let models_dir = values
        .get("--models-dir")
        .map_or_else(|| vision_root.join("models"), |dir| PathBuf::from(*dir));
    let label = match values.get("--label") {
        Some(label) if label.trim().is_empty() => return Err(cli("--label is empty")),
        Some(label) => (*label).to_owned(),
        None => default_label(execution_provider, device, &tuning),
    };
    Ok(Command::Run(Box::new(RunOptions {
        vision_root,
        models_dir,
        execution_provider,
        device,
        tuning,
        workload: PathBuf::from(required("--workload")?),
        warmup,
        reps,
        out: PathBuf::from(required("--out")?),
        label,
        baseline: values.get("--baseline").map(|path| PathBuf::from(*path)),
    })))
}

/// The variant name of design §1.2 when no `--label` is given, e.g. `cpu-1t`, `cuda0-default`,
/// `cuda0-tuned`, `dml1`.
pub fn default_label(
    execution_provider: OnnxExecutionProvider,
    device: Option<u32>,
    tuning: &OrtSessionTuning,
) -> String {
    let device = device.map(|device| device.to_string()).unwrap_or_default();
    match execution_provider {
        OnnxExecutionProvider::Cpu => format!("cpu-{}t", tuning.intra_threads),
        OnnxExecutionProvider::Cuda if tuning.cuda_is_default() => {
            format!("cuda{device}-default")
        }
        OnnxExecutionProvider::Cuda => format!("cuda{device}-tuned"),
        OnnxExecutionProvider::Directml => format!("dml{device}"),
    }
}

fn parse_number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, BenchError> {
    value
        .parse::<T>()
        .map_err(|_| cli(format!("{flag} {value:?} is not a non-negative integer")))
}

fn cli(message: impl Into<String>) -> BenchError {
    BenchError::new(BenchCode::CliInvalid, message)
}
