// SPDX-License-Identifier: AGPL-3.0-only

//! Device-layer primitives for the Rust ActingCommand runtime mainline.
//!
//! This crate is intentionally narrow: touch input is selected through an
//! explicit backend chain so single-backend failures are visible and bounded.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod adb;
pub mod capture;
pub mod discovery;
pub mod emulator;
pub mod error;
pub mod input;
pub mod maatouch;
pub mod minitouch;
pub mod replay;
pub mod touch;
mod vendor_stdio;

pub use adb::*;
pub use capture::*;
pub use discovery::*;
pub use emulator::*;
pub use error::*;
pub use input::*;
pub use maatouch::*;
pub use minitouch::*;
pub use replay::*;
pub use touch::*;
pub use vendor_stdio::vendor_stdio_session_diagnostic;
