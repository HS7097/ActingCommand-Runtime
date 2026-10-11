// SPDX-License-Identifier: AGPL-3.0-only

//! Pure scheduling-policy contracts shared by the catalog compiler and evaluator.

#![forbid(unsafe_code)]

mod canonical;
mod codes;
mod compiler;
mod evaluator;
mod forward;
mod resource_targets;
mod schema;
mod source;
mod strategy;
mod validation;

pub use compiler::*;
pub use evaluator::*;
pub use forward::*;
pub use resource_targets::*;
pub use schema::*;
pub use source::{CatalogDocumentSource, CatalogSources, validate_catalog_declaration};
pub use strategy::*;

/// Gate tests that need the crate's private items (test plan §2.1), and their shared fixtures.
#[cfg(test)]
mod tests {
    mod gate_d2_s1;
    mod gate_p1;
    mod selection_catalog;
}
