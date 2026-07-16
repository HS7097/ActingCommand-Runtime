// SPDX-License-Identifier: AGPL-3.0-only

//! Pure scheduling-policy contracts shared by the catalog compiler and evaluator.

#![forbid(unsafe_code)]

mod canonical;
mod compiler;
mod evaluator;
mod schema;
mod source;
mod validation;

pub use compiler::*;
pub use evaluator::*;
pub use schema::*;
pub use source::{CatalogDocumentSource, CatalogSources};
