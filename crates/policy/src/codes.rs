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
        FactRefreshUnbound => "fact_refresh_unbound": warning,
        /// Warning. A candidate's selection document read an input that no reading can
        /// supply (an instruction key, pack data, a fixed field or a lookup that covers no
        /// value) and found it unknown: the candidate is deferred with no wake, until the facts
        /// or the catalog change (keys `instance_id`, `rule_id`, `unknown_reason`).
        SelectionInputUnknown => "selection_input_unknown": warning,
        /// Warning. A scheduling-layer selection document declares a `drop_candidate` or a
        /// substitute clause, which the scheduling layer does not apply (keys `rule_id`,
        /// `unknown_clause`).
        SelectionUnknownClauseIgnored => "selection_unknown_clause_ignored": warning,
    }
}

// ---- Workflow #386 SR-0 (scheduling rules): registered ahead of their slices; begin ----
actingcommand_contract::outcome_codes! {
    /// Codes the scheduling rules (Workflow #386) register in the documentation slice SR-0,
    /// ahead of the slices that emit them (named on each entry). Contracts:
    /// `contracts/scheduling/README.md`, `contracts/instance-fact-store.md`.
    pub(crate) enum SchedulingRulePolicyCode {
        /// Info. A candidate is deferred because a `session.feature.*` record of its instance
        /// says a feature the task needs is locked (keys `fact_key`, and
        /// `next_eligible_unix_ms` when the record expires; SR-5a).
        FeatureLocked => "feature_locked": info,
        /// Error. A decision that is not selected ended without a reason: a programming error.
        /// The candidate is still deferred and the round goes on (keys `task_id`,
        /// `instance_id`; SR-6a).
        DecisionReasonMissing => "decision_reason_missing": error,
        /// Info. A candidate's task package is not schedulable at this time by the instance's
        /// pack schedule (keys `package_id`, and `next_eligible_unix_ms` when the covering row
        /// ends; SR-11).
        PackScheduleClosed => "pack_schedule_closed": info,
        /// Info. A pack schedule row names a package no task of the active catalog uses;
        /// reported once per cycle, not an error (keys `package_id`; SR-11).
        PackScheduleUnknownPackage => "pack_schedule_unknown_package": info,
    }
}
// ---- Workflow #386 SR-0 (scheduling rules): end ----
