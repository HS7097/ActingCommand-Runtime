// SPDX-License-Identifier: AGPL-3.0-only

//! The command line (design §1.5 item 1):
//!
//! ```text
//! ac-vision-ep-bench [--mode ocr] --vision-root <dir> --ep cpu|cuda|directml [--device <n>]
//!     [--cpu-threads <n>] [--cuda-conv-search exhaustive|heuristic] [--cuda-tf32 on|off]
//!     [--cuda-cpu-fallback on|off] [--directml-cpu-fallback on|off]
//!     --workload <json> [--warmup 3] [--reps 20] --out <json>
//!     [--models-dir <dir>] [--label <name>] [--baseline <json>] [--private-bytes-cap-gib 3]
//! ac-vision-ep-bench --mode template --vision-root <dir> --ep cpu|cuda|directml [--device <n>]
//!     [--cpu-threads <n>] [--cuda-conv-search exhaustive|heuristic] [--cuda-tf32 on|off]
//!     --workload <json> [--warmup 3] [--reps 1] [--direct-enumeration on|off] --out <json>
//!     [--label <name>] [--private-bytes-cap-gib 3]
//! ac-vision-ep-bench --list-adapters
//! ac-vision-ep-bench --probe-dlls <ort dir>
//! ```
//!
//! `--device` is the CUDA ordinal for `cuda` and the DXGI adapter index for `directml`; `cpu`
//! takes none. The CUDA knobs belong to `cuda` only. `--baseline` names the `cpu-1t` report the
//! consistency section compares with.
//!
//! Workflow #407 GPU-0c: `--directml-cpu-fallback on|off` (default `off`, `directml` only) lets
//! ONNX Runtime place the nodes DirectML cannot run on its CPU execution provider. `--probe-dlls`
//! loads every DLL of one runtime folder with the engine's `LoadLibraryExW` flags and names the
//! dependency a failing one misses (`probe.rs`); it stands alone, as `--list-adapters` does.
//!
//! Workflow #407 GPU-0e: `--cuda-cpu-fallback on|off` (default `off`, `cuda` only) mirrors it for
//! CUDA: ONNX Runtime may place the nodes CUDA cannot run on its CPU execution provider.
//!
//! Workflow #407 GPU-0b: `--mode template` searches the workload's templates over every whole
//! frame (`template_run.rs`); there `--reps` counts the repetitions per frame, and
//! `--direct-enumeration off` skips today's direct enumeration, which does not depend on the
//! execution provider and takes up to 5 s per search.
//!
//! Workflow #407 GPU-0f: `--private-bytes-cap-gib <n>` (both modes, any EP; 1..=16, default 3)
//! sets the bench's own private-bytes cap; the host floors stay as they are.

use crate::error::{BenchCode, BenchError};
use crate::safety::{DEFAULT_PRIVATE_BYTES_CAP_GIB, PRIVATE_BYTES_CAP_GIB_RANGE, SafetyLimits};
use actingcommand_vision_ffi::{CudaConvAlgorithmSearch, OnnxExecutionProvider, OrtSessionTuning};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The most intra-op threads the bench accepts.
pub const MAX_CPU_THREADS: u16 = 64;

/// What one process measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchMode {
    /// GPU-0a: the OCR model, raw sessions and the production engine.
    Ocr,
    /// GPU-0b: whole-frame template search, CPU exact map against the generated Conv graph.
    Template,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print the DXGI adapters (and nothing else) as JSON on stdout.
    ListAdapters,
    /// Workflow #407 GPU-0c: load every DLL of the folder and print the probe as JSON on stdout.
    ProbeDlls(PathBuf),
    /// Boxed: the options are far larger than the data-less `ListAdapters`.
    Run(Box<RunOptions>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub mode: BenchMode,
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
    /// Template mode: whether today's direct enumeration runs.
    pub direct_enumeration: bool,
    /// GPU-0f: the memory limits in force, the private-bytes cap from `--private-bytes-cap-gib`.
    pub safety_limits: SafetyLimits,
}

const VALUE_FLAGS: &[&str] = &[
    "--private-bytes-cap-gib",
    "--mode",
    "--direct-enumeration",
    "--vision-root",
    "--models-dir",
    "--ep",
    "--device",
    "--cpu-threads",
    "--cuda-conv-search",
    "--cuda-tf32",
    "--directml-cpu-fallback",
    "--cuda-cpu-fallback",
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
    if args.first().map(String::as_str) == Some("--probe-dlls") {
        return match args {
            [_, folder] if !folder.trim().is_empty() => {
                Ok(Command::ProbeDlls(PathBuf::from(folder)))
            }
            _ => Err(cli(
                "--probe-dlls takes exactly one folder and stands alone",
            )),
        };
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
    let mode = match values.get("--mode").copied() {
        None | Some("ocr") => BenchMode::Ocr,
        Some("template") => BenchMode::Template,
        Some(other) => return Err(cli(format!("--mode {other:?} is not ocr or template"))),
    };
    let mode_flags: &[&str] = match mode {
        BenchMode::Ocr => &["--direct-enumeration"],
        BenchMode::Template => &["--models-dir", "--baseline"],
    };
    if let Some(flag) = mode_flags.iter().find(|flag| values.contains_key(**flag)) {
        return Err(cli(format!(
            "{flag} does not belong to --mode {}",
            match mode {
                BenchMode::Ocr => "ocr",
                BenchMode::Template => "template",
            }
        )));
    }
    let direct_enumeration = match values.get("--direct-enumeration").copied() {
        None | Some("on") => true,
        Some("off") => false,
        Some(other) => {
            return Err(cli(format!(
                "--direct-enumeration {other:?} is not on or off"
            )));
        }
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
    if let Some(value) = values.get("--directml-cpu-fallback") {
        if execution_provider != OnnxExecutionProvider::Directml {
            return Err(cli("--directml-cpu-fallback belongs to --ep directml only"));
        }
        tuning.directml_cpu_fallback = match *value {
            "on" => true,
            "off" => false,
            other => {
                return Err(cli(format!(
                    "--directml-cpu-fallback {other:?} is not on or off"
                )));
            }
        };
    }
    if let Some(value) = values.get("--cuda-cpu-fallback") {
        if execution_provider != OnnxExecutionProvider::Cuda {
            return Err(cli("--cuda-cpu-fallback belongs to --ep cuda only"));
        }
        tuning.cuda_cpu_fallback = match *value {
            "on" => true,
            "off" => false,
            other => {
                return Err(cli(format!(
                    "--cuda-cpu-fallback {other:?} is not on or off"
                )));
            }
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
        .unwrap_or(match mode {
            BenchMode::Ocr => 20,
            BenchMode::Template => 1,
        });
    if reps == 0 {
        return Err(cli("--reps must be at least 1"));
    }
    let private_bytes_cap_gib = values
        .get("--private-bytes-cap-gib")
        .map(|value| parse_number::<u64>("--private-bytes-cap-gib", value))
        .transpose()?
        .unwrap_or(DEFAULT_PRIVATE_BYTES_CAP_GIB);
    if !PRIVATE_BYTES_CAP_GIB_RANGE.contains(&private_bytes_cap_gib) {
        return Err(cli(format!(
            "--private-bytes-cap-gib must be in {}..={}",
            PRIVATE_BYTES_CAP_GIB_RANGE.start(),
            PRIVATE_BYTES_CAP_GIB_RANGE.end()
        )));
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
        mode,
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
        direct_enumeration,
        safety_limits: SafetyLimits::with_private_bytes_cap_gib(private_bytes_cap_gib),
    })))
}

/// The variant name of design §1.2 when no `--label` is given, e.g. `cpu-1t`, `cuda0-default`,
/// `cuda0-tuned`, `cuda0-default-fallback`, `dml1`, `dml1-fallback`.
pub fn default_label(
    execution_provider: OnnxExecutionProvider,
    device: Option<u32>,
    tuning: &OrtSessionTuning,
) -> String {
    let device = device.map(|device| device.to_string()).unwrap_or_default();
    match execution_provider {
        OnnxExecutionProvider::Cpu => format!("cpu-{}t", tuning.intra_threads),
        OnnxExecutionProvider::Cuda => format!(
            "cuda{device}-{}{}",
            if tuning.cuda_is_default() {
                "default"
            } else {
                "tuned"
            },
            if tuning.cuda_cpu_fallback {
                "-fallback"
            } else {
                ""
            }
        ),
        OnnxExecutionProvider::Directml if tuning.directml_cpu_fallback => {
            format!("dml{device}-fallback")
        }
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
