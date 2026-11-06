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
//!
//! Workflow #407 GPU-0b adds the template mode (`--mode template`): whole-frame template search
//! with the CPU exact score map, today's direct enumeration and a generated ONNX `Conv` graph on
//! the process's execution provider, with the float path's error and recall. The graph encoder
//! (`onnx.rs`) and the float score and recall logic (`template.rs`) are platform neutral.
//!
//! Workflow #407 GPU-0c makes the GPU variants measurable: no failure drops ONNX Runtime's
//! status or a library load's Win32 code (`error.rs`); `--probe-dlls` names the dependency a
//! runtime DLL fails to load on (`probe.rs`, with the import-table reader `pe.rs`); DirectML may
//! allow CPU fallback (`--directml-cpu-fallback on`), and every GPU run records where ONNX
//! Runtime placed each model's nodes, read from its verbose session log (`report.rs`
//! `ModelPlacement`).
//!
//! Workflow #407 GPU-0e: for CUDA the engine makes the runtime folder part of the process's
//! default DLL search before ONNX Runtime is initialised (cuDNN 9 loads its sub-libraries by bare
//! name), the bench checks after the first session, after the first inference and at the end that
//! every cuDNN library loaded comes from that folder, and CUDA may allow CPU fallback
//! (`--cuda-cpu-fallback on`), recorded with its node placement as DirectML's is.
//!
//! Workflow #407 GPU-0f: the private-bytes cap is `--private-bytes-cap-gib <n>` (default 3), and
//! the report and the watcher's stop report record the cap in force and the peak private bytes.

pub mod cli;
pub mod consistency;
pub mod error;
pub mod onnx;
pub mod pe;
pub mod report;
pub mod safety;
pub mod stats;
pub mod template;
pub mod workload;

#[cfg(windows)]
pub mod probe;
#[cfg(windows)]
pub mod run;
#[cfg(windows)]
pub mod template_run;
#[cfg(windows)]
pub mod watch;
#[cfg(windows)]
pub mod win;
