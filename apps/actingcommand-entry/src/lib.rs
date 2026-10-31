// SPDX-License-Identifier: AGPL-3.0-only

//! The fixed entries of an A/B root (Workflow #429 R-15b; `contracts/stable-entry.md`).
//!
//! An entry `<root>\runtime\<name>.exe` or `<root>\ui\<name>.exe` forwards to the program of
//! the same name in the selected slot's member, after it read `install\active.json`, checked
//! the program against its member's manifest and took the slot's shared lock. R-15b freezes the
//! contract and registers the entries' outcome codes ([`codes`], owner `actingcommand-entry`);
//! R-15c builds the two programs, so this build ships neither.

pub mod codes;

pub use codes::EntryCode;
