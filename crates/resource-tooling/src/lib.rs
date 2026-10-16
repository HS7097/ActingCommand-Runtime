// SPDX-License-Identifier: AGPL-3.0-only

//! Deterministic resource compiler and package validation for the optional Lab toolchain.
//!
//! This crate has no live device, scheduler, Runtime, or Lab state authority.

#![forbid(unsafe_code)]
#![allow(clippy::result_large_err)]

mod api;
mod authoring;
mod business_catalog;
mod codes;
mod declaration_exclusions;
// Workflow #336 PKG-S5 (exec key 66): package bundle writes bundle v4 by `package_id`.
#[cfg(test)]
mod gate_336_pkgs5;
// Workflow #373 NAV-T (exec key 80): the standard-pack cross-check of package bundle and
// standard-pack check, the root files laid beside bundle.json and the reserved package ids.
#[cfg(test)]
mod gate_373_navt;
// Test plan v2.2 E-1 G10 (exec key 33): package bundle and the business catalog.
#[cfg(test)]
mod gate_e1;
// Test plan v2.2 E-4 G4b (exec key 12): the bundle's offline maintenance verdict and the
// Runtime's admission.
#[cfg(test)]
mod gate_e4;
mod maa_task_graph;
mod package_directory;
mod package_publish;
mod package_validate;
mod resource_convert;
mod resource_restore;
mod standard_pack_check;

pub use api::*;
pub use authoring::*;
pub use business_catalog::*;
pub use codes::ResourceToolingCode;
pub use declaration_exclusions::{
    DECLARATION_EXCLUSIONS_FILE, DECLARATION_EXCLUSIONS_SCHEMA, DeclarationExclusions,
};
pub use maa_task_graph::{
    MaaFact, MaaFactOrigin, MaaFactSource, MaaFactValue, MaaTaskFacts, MaaTaskGraph,
    MaaTaskGraphStats, compile_maa_task_graph,
};
pub use package_directory::package_bundle;
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
pub use standard_pack_check::check_standard_pack_root;
