// SPDX-License-Identifier: AGPL-3.0-only

//! Durable artifact storage and evidence export for the ActingCommand Runtime.
//!
//! This crate owns artifact bytes, hashes, retention metadata, frame buffering, and archive
//! generation. It emits typed event drafts through an injected sink and never owns the global
//! ledger writer, scheduler, Runtime lifecycle, or device backend.

#![forbid(unsafe_code)]

mod error;
mod frame_store;
mod naming;
mod pipeline;
mod store;

pub use error::*;
pub use frame_store::*;
pub use naming::*;
pub use pipeline::*;
pub use store::*;
