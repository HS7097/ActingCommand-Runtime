// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 GPU-0a: the vision execution-provider bench (experiment branch
//! `exp/407-gpu0-bench` only, never merged; design `model-gpu0-404.md` §1.5).
//!
//! One process measures one ONNX Runtime execution provider (ONNX Runtime is initialised once
//! per process from one `onnxruntime.dll`): a raw `ort` session layer and the production engine
//! layer, cold and warm, with memory, CPU time, GPU counters, the loaded modules and, against a
//! `cpu-1t` baseline report, the consistency section. `run-variants.ps1` beside this crate runs
//! the variants of design §1.2 one process each.
//!
//! The logic the tests pin (percentiles, the workload schema, the memory stops) is platform
//! neutral; the samplers and the run are Windows only.

pub mod cli;
pub mod consistency;
pub mod error;
pub mod report;
pub mod safety;
pub mod stats;
pub mod workload;

#[cfg(windows)]
pub mod run;
#[cfg(windows)]
pub mod watch;
#[cfg(windows)]
pub mod win;
