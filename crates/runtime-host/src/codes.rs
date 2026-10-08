// SPDX-License-Identifier: AGPL-3.0-only

//! The Runtime host's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/runtime-host.json`; the outcome guard (G1) checks both directions.
//! Until slice A2 converts this crate, a code travels as its spelling (`HostCode::as_str`)
//! through the existing error types, with only `key=value` tokens in their detail fields.

actingcommand_contract::outcome_codes! {
    /// Codes the Runtime host registered ahead of the A2 sweep (Workflow #369).
    pub(crate) enum HostCode {
        /// Info. A policy candidate, or its admission, met an instance that a claim holds or
        /// that an eligible claim waits for: the candidate is deferred, or its intent is
        /// rejected at Info, and the driver wakes when a key returns (keys `instance_id`,
        /// `holder_kind`, `count`).
        DispatchInstanceHeld => "dispatch_instance_held": info,
        /// Info. A policy admission found the instance's admission guard taken by a keyless
        /// observe or probe for its whole short retry; the intent is rejected at Info and may
        /// run again from `next_eligible_unix_ms` (keys `instance_id`,
        /// `next_eligible_unix_ms`).
        DispatchInstanceContended => "dispatch_instance_contended": info,
    }
}
