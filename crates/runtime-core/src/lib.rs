// SPDX-License-Identifier: AGPL-3.0-only

//! Runtime-core utilities for the Rust ActingCommand mainline.
//!
//! P2.1 intentionally keeps this crate narrow: it only persists captured frames
//! and creates contract-level `CaptureRef` values.

#![forbid(unsafe_code)]

pub mod actinglab;
pub mod capture_store;

pub use actinglab::*;
pub use capture_store::*;
