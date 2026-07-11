// SPDX-License-Identifier: AGPL-3.0-only

//! Typed local client for the resident ActingCommand Runtime.
//!
//! Clients discover and command the Runtime through local IPC. They never construct or own
//! production device backends, and dropping a UI or CLI client does not stop the Runtime.

#![forbid(unsafe_code)]

mod client;
mod error;
mod input;
mod ipc;

pub use client::*;
pub use error::*;
pub use input::*;

#[cfg(test)]
mod tests;
