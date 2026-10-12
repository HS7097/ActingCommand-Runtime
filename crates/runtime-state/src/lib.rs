// SPDX-License-Identifier: AGPL-3.0-only

//! SQLite-backed authoritative Runtime state and immutable release generations.

#![forbid(unsafe_code)]

mod codes;
mod error;
mod store;

pub use codes::{StateCode, StateLocation};
pub use error::*;
pub use store::*;
