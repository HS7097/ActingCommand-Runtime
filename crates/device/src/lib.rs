// SPDX-License-Identifier: AGPL-3.0-only

//! Device-layer primitives for the Rust ActingCommand runtime mainline.
//!
//! This crate is intentionally narrow: MaaTouch is the only touch path here.
//! MaaTouch failures must surface as fatal device-layer errors during this phase.

#![forbid(unsafe_code)]

pub mod adb;
pub mod capture;
pub mod error;
pub mod input;
pub mod maatouch;

pub use adb::*;
pub use capture::*;
pub use error::*;
pub use input::*;
pub use maatouch::*;
