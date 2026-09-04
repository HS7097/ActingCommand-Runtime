// SPDX-License-Identifier: AGPL-3.0-only

//! Durable artifact storage and evidence export for the ActingCommand Runtime.
//!
//! This crate owns artifact bytes, hashes, retention metadata, frame buffering, and archive
//! generation. It emits typed event drafts through an injected sink and never owns the global
//! ledger writer, scheduler, Runtime lifecycle, or device backend.

#![forbid(unsafe_code)]

mod error;
#[cfg(feature = "evidence-archive")]
mod evidence_archive;
#[cfg(feature = "capture")]
mod exporter;
#[cfg(feature = "capture")]
mod frame_store;
#[cfg(feature = "capture")]
mod naming;
#[cfg(feature = "capture")]
mod pipeline;
#[cfg(feature = "capture")]
mod portable_archive;
mod store;

pub use error::*;
#[cfg(feature = "evidence-archive")]
pub use evidence_archive::*;
#[cfg(feature = "capture")]
pub use exporter::*;
#[cfg(feature = "capture")]
pub use frame_store::*;
#[cfg(feature = "capture")]
pub use naming::*;
#[cfg(feature = "capture")]
pub use pipeline::*;
#[cfg(feature = "capture")]
pub use portable_archive::*;
pub use store::*;
