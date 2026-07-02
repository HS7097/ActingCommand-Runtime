// SPDX-License-Identifier: AGPL-3.0-only

//! Device-layer primitives for the Rust ActingCommand runtime mainline.
//!
//! This crate is intentionally narrow: touch input is selected through an
//! explicit backend chain so single-backend failures are visible and bounded.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod adb;
pub mod capture;
pub mod error;
pub mod input;
pub mod maatouch;
pub mod touch;

pub use adb::*;
pub use capture::*;
pub use error::*;
pub use input::*;
pub use maatouch::*;
pub use touch::*;
