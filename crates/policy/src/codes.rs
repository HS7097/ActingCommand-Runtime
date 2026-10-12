// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduling policy's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/policy.json`; the outcome guard (G1) checks both directions.
//! Until slice A2 converts this crate, a code travels as its spelling (`PolicyCode::as_str`)
//! in a decision reason, with only `key=value` tokens in its detail.

actingcommand_contract::outcome_codes! {
    /// Codes the scheduling policy registered ahead of the A2 sweep (D2 S1, Workflow #384).
    pub(crate) enum PolicyCode {
        /// Warning. A candidate's selection document read a realistic instance fact key (one
        /// that recognition writes) whose value is unknown, and no reader can refresh it: the
        /// candidate is deferred with no wake, until the facts, the catalog or the
        /// configuration change (keys `instance_id`, `fact_key`, `unknown_reason`; optional
        /// `reader_problem`).
        FactRefreshUnbound => "fact_refresh_unbound": warning / none,
        /// Warning. A candidate's selection document read an input that no reading can
        /// supply (an instruction key, pack data, a fixed field or a lookup that covers no
        /// value) and found it unknown: the candidate is deferred with no wake, until the facts
        /// or the catalog change (keys `instance_id`, `rule_id`, `unknown_reason`).
        SelectionInputUnknown => "selection_input_unknown": warning / none,
        /// Warning. A scheduling-layer selection document declares a `drop_candidate` or a
        /// substitute clause, which the scheduling layer does not apply (keys `rule_id`,
        /// `unknown_clause`).
        SelectionUnknownClauseIgnored => "selection_unknown_clause_ignored": warning / declaration_error,
    }
}
