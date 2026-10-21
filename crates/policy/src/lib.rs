// SPDX-License-Identifier: AGPL-3.0-only

//! Pure scheduling-policy contracts shared by the catalog compiler and evaluator.

#![forbid(unsafe_code)]

mod canonical;
mod codes;
mod compiler;
mod evaluator;
mod forward;
mod remedy;
mod resource_targets;
mod schema;
mod source;
mod strategy;
mod validation;

pub use codes::{PolicyCode, RefreshLimit};
pub use compiler::*;
pub use evaluator::*;
pub use forward::*;
pub use remedy::*;
pub use resource_targets::*;
pub use schema::*;
pub use source::{CatalogDocumentSource, CatalogSources, validate_catalog_declaration};
pub use strategy::*;

/// Gate tests that need the crate's private items (test plan §2.1), and their shared fixtures.
#[cfg(test)]
mod tests {
    // Workflow #361 R7, test row R7-T7: the S-08 day replay with and without `missed`.
    mod gate_361r7;
    mod gate_390_pool1;
    // Workflow #405 W1: the catalog predicate `fact_time_reached`.
    mod gate_405_w1;
    // Workflow #411 A26: a dispatch decision record says why each listed candidate did not run.
    mod gate_411_a26;
    // Workflow #411 B15 R2: the evaluator's plan of declared remedies.
    mod gate_411_b15e;
    // Workflow #411 B8 (policy key 58): a dispatch's schedule values for a task that opts in.
    mod gate_411_b8;
    mod gate_d2_s1;
    mod gate_d2_s4;
    // Workflow #386 SR-6b, MCP-19a (policy key 56): each decision's own wake, never serialized.
    mod gate_mcp19a;
    mod gate_p1;
    mod gate_p2a;
    mod gate_p2b;
    mod selection_catalog;
}
