// SPDX-License-Identifier: AGPL-3.0-only

//! Deterministic resource compiler and package validation for the optional Lab toolchain.
//!
//! This crate has no live device, scheduler, Runtime, or Lab state authority.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod api;
mod authoring;
mod business_catalog;
mod maa_task_graph;
mod package_directory;
mod package_publish;
mod package_validate;
mod resource_convert;
mod resource_restore;

pub use api::*;
pub use authoring::*;
pub use business_catalog::*;
pub use maa_task_graph::{
    MaaFact, MaaFactOrigin, MaaFactSource, MaaFactValue, MaaTaskFacts, MaaTaskGraph,
    MaaTaskGraphStats, compile_maa_task_graph,
};
pub use package_directory::{package_bundle, package_digest};
pub use package_publish::{
    PackagePublicationCommit, PackagePublicationTransaction, PublishedPackageReader,
    open_published_package,
};
pub use package_validate::validate_package;
pub use resource_convert::{
    Bundle, OperationParser, ParseOutputs, ResolvedResourceRoot, canonical_game, canonical_locale,
    canonical_server, resolve_resource_root, validate_maa_semantic_declarations,
};
pub use resource_restore::*;
