// SPDX-License-Identifier: AGPL-3.0-only

//! The execution kernel's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/execution-kernel.json`; the outcome guard (G1) checks both
//! directions. Until slice A2d converts this crate, a code travels as its spelling through
//! `ContainedTaskError`, with only `key=value` tokens in its detail.

// ---- Workflow #386 SR-0 (scheduling rules): registered ahead of their slices; begin ----
actingcommand_contract::outcome_codes! {
    /// Codes the scheduling rules (Workflow #386) register in the documentation slice SR-0,
    /// ahead of the slices that emit them in their registered form (named on each entry).
    /// Contracts: `contracts/scheduling-blackout.md`, `contracts/policy-suspension.md`,
    /// `contracts/cost-commit.md`.
    pub(crate) enum SchedulingRuleKernelCode {
        /// Error. A recognition observation did not finish within its window after every
        /// observe-only retry (keys `attempts`, `timeout_ms`, `stage`; SR-1).
        RecognitionSampleDeadlineExceeded => "recognition_sample_deadline_exceeded": error,
        /// Error. A running task had less time left than one page recognition needs (keys
        /// `required_ms`, `remaining_ms`; SR-1).
        RecognitionSampleBudgetInsufficient => "recognition_sample_budget_insufficient": error,
        /// Error. Admission refused a task whose declared sampling exceeds the smaller of its
        /// step and task timeouts (keys `required_ms`, `limit_ms`, and `pack_operation_id`
        /// when one operation exceeds it; SR-1).
        RecognitionSampleBudgetInvalid => "recognition_sample_budget_invalid": error,
        /// Error. A running selection step had less time left than its candidate sampling
        /// needs (keys `required_ms`, `remaining_ms`; SR-1).
        CandidateSampleBudgetInsufficient => "candidate_sample_budget_insufficient": error,
        /// Error. A candidate sampling of a selection step did not finish within its window
        /// after every observe-only retry (keys `attempts`, `timeout_ms`; SR-1).
        CandidateSampleDeadlineExceeded => "candidate_sample_deadline_exceeded": error,
        /// Warning. A run recognised a page its task declares as the sign of a locked feature
        /// and stopped there, with no input (keys `fact_key`, `page_id`; SR-5b).
        ContainedTaskFeatureLocked => "contained_task_feature_locked": warning,
        /// Warning. Before the input of its declared entry-cost step, the run had too little
        /// time left and stopped with nothing spent (keys `required_ms`, `remaining_ms`,
        /// `step_id`; SR-10).
        CostCommitTimeInsufficient => "cost_commit_time_insufficient": warning,
        /// Error. A task package's `cost_commit` declaration breaks a rule (keys `package_id`,
        /// `field`; SR-10).
        CostCommitDeclarationInvalid => "cost_commit_declaration_invalid": error,
    }
}
// ---- Workflow #386 SR-0 (scheduling rules): end ----
