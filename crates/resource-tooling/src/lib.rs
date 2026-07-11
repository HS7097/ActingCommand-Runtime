// SPDX-License-Identifier: AGPL-3.0-only

//! Deterministic resource compiler and package validation for the optional Lab toolchain.
//!
//! This crate has no live device, scheduler, Runtime, or Lab state authority.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod api;
mod authoring;
mod environment;
mod maa_task_graph;
mod package_build;
mod package_validate;
mod resource_convert;

pub use api::*;
pub use authoring::*;
pub use environment::AuthoringEnvironmentSnapshot;
pub use maa_task_graph::{MaaTaskGraph, MaaTaskGraphStats, compile_maa_task_graph};
pub use package_build::{
    PackageBuildCatalog, PreparedPackageBuildTask, prepare_package_build_task,
};
pub use package_validate::validate_package;
pub use resource_convert::{
    Bundle, ConvertOutputs, OperationConverter, ResolvedResourceRoot, canonical_game,
    resolve_resource_root, resource_convert,
};
