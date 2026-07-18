// SPDX-License-Identifier: AGPL-3.0-only

//! SQLite-backed authoritative Runtime state and immutable release generations.

#![forbid(unsafe_code)]

mod error;
mod store;

pub use error::*;
pub use store::*;
