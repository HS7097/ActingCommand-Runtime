// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduling policy's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/policy.json`; the outcome guard (G1) checks both directions.
//! A decision reason still carries its code as a spelling (`PolicyCode::as_str`), with only
//! `key=value` tokens in its detail: its channel also carries spellings that cannot be
//! registered as they are (`eligible`, `ranked`, `scored`, `score_unknown:<term>`,
//! `candidate_not_selected:<task>`, `resource_target_unmapped:<id>`, ...), which A2-P renames or
//! splits. Spellings several owners use with one meaning are the contract's (`SharedCode`).

actingcommand_contract::outcome_codes! {
    /// The scheduling policy's codes: D2 S1 registered the first three ahead of the A2 sweep
    /// (Workflow #384); A2d registers the rest of the crate (Workflow #378).
    pub enum PolicyCode {
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
        // Catalog compile diagnostics (`CatalogDiagnosticCode`); the spellings other owners also
        // use are the contract's `SharedCode`s.
        CatalogTooLarge => "catalog_too_large": error / none,
        UnknownField => "unknown_field": error / none,
        DescriptorMismatch => "descriptor_mismatch": error / none,
        PredicateUnreachable => "predicate_unreachable": error / none,
        PredicateUncomputable => "predicate_uncomputable": error / none,
        LoopBudgetMissing => "loop_budget_missing": error / none,
        EffectIncompatible => "effect_incompatible": error / none,
        ApprovalMissing => "approval_missing": error / none,
        PrioritySelectionWithoutDocument => "priority_selection_without_document": error / none,
        PrioritySelectionPercentileInvalid => "priority_selection_percentile_invalid": error / none,
        // Policy evaluation failures (`PolicyEvaluationError`).
        PolicyEvaluationInputInvalid => "policy_evaluation_input_invalid": error / none,
        PolicyEvaluationFactTypeMismatch => "policy_evaluation_fact_type_mismatch": error / none,
        PolicyEvaluationNumericOverflow => "policy_evaluation_numeric_overflow": error / none,
        ScoreOverflow => "score_overflow": error / none,
        SelectionPolicyInvalid => "selection_policy_invalid": error / none,
        // Scheduling decision reasons.
        ScopeWithoutInstance => "scope_without_instance": info / none,
        FailureRetryImmediate => "failure_retry_immediate": info / none,
        TriggerFalse => "trigger_false": info / none,
        TriggerUnknown => "trigger_unknown": info / none,
        FeedbackStopTrue => "feedback_stop_true": info / none,
        FeedbackStopUnknown => "feedback_stop_unknown": info / none,
        TaskCooldownActive => "task_cooldown_active": info / none,
        InstanceAlreadySelected => "instance_already_selected": info / none,
        HeavySceneBudgetDeferred => "heavy_scene_budget_deferred": info / none,
        HostBudgetDeferred => "host_budget_deferred": info / none,
        PlacementSelected => "placement_selected": info / none,
        RankBreakdown => "rank_breakdown": info / none,
        DecisionRecord => "decision_record": info / none,
        InstanceUnavailable => "instance_unavailable": info / none,
        CapabilityMissing => "capability_missing": info / none,
        InstanceOverrideDisabled => "instance_override_disabled": info / none,
        ActivityProfileMissing => "activity_profile_missing": info / none,
        HeavySceneBudgetExhausted => "heavy_scene_budget_exhausted": info / none,
        HostCapacityInsufficient => "host_capacity_insufficient": info / none,
        ScoreDeferred => "score_deferred": info / none,
        ScorePromoted => "score_promoted": info / none,
        ScoreAgingCap => "score_aging_cap": info / none,
        TimelineUnavailable => "timeline_unavailable": info / none,
        TimelineActive => "timeline_active": info / none,
        // Resource target reasons and failures.
        ResourceTargets => "resource_targets": info / none,
        ResourceWeights => "resource_weights": info / none,
        ResourceTargetPolicyIgnored => "resource_target_policy_ignored": warning / none,
        ResourceTargetPolicyUnreadable => "resource_target_policy_unreadable": warning / none,
        ResourceTargetPolicyExpired => "resource_target_policy_expired": info / none,
        ResourceTargetTasksUnevaluable => "resource_target_tasks_unevaluable": warning / none,
        ResourceTargetsParserCodeUnexpected => "resource_targets_parser_code_unexpected": error / none,
        ResourceTargetsCanonicalEncodeFailed => "resource_targets_canonical_encode_failed": error / none,
        ResourceTargetsViewEncodeFailed => "resource_targets_view_encode_failed": error / none,
        MalformedRows => "malformed_rows": error / none,
        // Forward projection (`ForwardError`) and its evidence gaps.
        ForwardProjectionInvalid => "forward_projection_invalid": error / none,
        ForwardProjectionOverflow => "forward_projection_overflow": error / none,
        StepBudgetExhausted => "step_budget_exhausted": warning / none,
        EffectEvidenceInsufficient => "effect_evidence_insufficient": warning / none,
        // Strategic report projection (`StrategyError`) and its planning-lane reasons.
        StrategyReportInvalid => "strategy_report_invalid": error / none,
        StrategyCatalogMismatch => "strategy_catalog_mismatch": error / none,
        StrategyTemplateAmbiguous => "strategy_template_ambiguous": error / none,
        StrategyNumericOverflow => "strategy_numeric_overflow": error / none,
        ExecutionContinues => "execution_continues": info / none,
        NoConditionalTemplateMatched => "no_conditional_template_matched": info / none,
        PinnedObservationMissing => "pinned_observation_missing": info / none,
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
        FeatureLocked => "feature_locked": info / none,
        /// Error. A decision that is not selected ended without a reason: a programming error.
        /// The candidate is still deferred and the round goes on (keys `task_id`,
        /// `instance_id`; SR-6a).
        DecisionReasonMissing => "decision_reason_missing": error / none,
        /// Info. A candidate's task package is not schedulable at this time by the instance's
        /// pack schedule (keys `package_id`, and `next_eligible_unix_ms` when the covering row
        /// ends; SR-11).
        PackScheduleClosed => "pack_schedule_closed": info / none,
        /// Info. A pack schedule row names a package no task of the active catalog uses;
        /// reported once per cycle, not an error (keys `package_id`; SR-11).
        PackScheduleUnknownPackage => "pack_schedule_unknown_package": info / none,
    }
}
// ---- Workflow #386 SR-0 (scheduling rules): end ----
