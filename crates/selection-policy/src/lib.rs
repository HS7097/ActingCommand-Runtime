// SPDX-License-Identifier: AGPL-3.0-only

//! Pure selection-policy contracts: one declared document, one deterministic evaluator.
//!
//! The crate answers exactly one question: given a bounded candidate set observed on one
//! screen, a declared selection-policy document, and an explicit fact snapshot, which
//! candidate identifiers are chosen and why. It owns the decision half only. It never
//! observes, never acts, never reads a clock, a ledger, a resource pack, or the filesystem;
//! every input is passed in and hashable, so the same input always yields the same output.

#![forbid(unsafe_code)]
