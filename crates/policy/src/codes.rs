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
        // Catalog compile diagnostics (`CatalogDiagnosticCode`); the spellings other owners also
        // use are the contract's `SharedCode`s.
        CatalogTooLarge => "catalog_too_large": error,
        UnknownField => "unknown_field": error,
        DescriptorMismatch => "descriptor_mismatch": error,
        PredicateUnreachable => "predicate_unreachable": error,
        PredicateUncomputable => "predicate_uncomputable": error,
        LoopBudgetMissing => "loop_budget_missing": error,
        EffectIncompatible => "effect_incompatible": error,
        ApprovalMissing => "approval_missing": error,
        PrioritySelectionWithoutDocument => "priority_selection_without_document": error,
        PrioritySelectionPercentileInvalid => "priority_selection_percentile_invalid": error,
        // Policy evaluation failures (`PolicyEvaluationError`).
        PolicyEvaluationInputInvalid => "policy_evaluation_input_invalid": error,
        PolicyEvaluationFactTypeMismatch => "policy_evaluation_fact_type_mismatch": error,
        PolicyEvaluationNumericOverflow => "policy_evaluation_numeric_overflow": error,
        ScoreOverflow => "score_overflow": error,
        SelectionPolicyInvalid => "selection_policy_invalid": error,
        // Scheduling decision reasons.
        ScopeWithoutInstance => "scope_without_instance": info,
        FailureRetryImmediate => "failure_retry_immediate": info,
        TriggerFalse => "trigger_false": info,
        TriggerUnknown => "trigger_unknown": info,
        FeedbackStopTrue => "feedback_stop_true": info,
        FeedbackStopUnknown => "feedback_stop_unknown": info,
        TaskCooldownActive => "task_cooldown_active": info,
        InstanceAlreadySelected => "instance_already_selected": info,
        HeavySceneBudgetDeferred => "heavy_scene_budget_deferred": info,
        HostBudgetDeferred => "host_budget_deferred": info,
        PlacementSelected => "placement_selected": info,
        RankBreakdown => "rank_breakdown": info,
        DecisionRecord => "decision_record": info,
        InstanceUnavailable => "instance_unavailable": info,
        CapabilityMissing => "capability_missing": info,
        InstanceOverrideDisabled => "instance_override_disabled": info,
        ActivityProfileMissing => "activity_profile_missing": info,
        HeavySceneBudgetExhausted => "heavy_scene_budget_exhausted": info,
        HostCapacityInsufficient => "host_capacity_insufficient": info,
        ScoreDeferred => "score_deferred": info,
        ScorePromoted => "score_promoted": info,
        ScoreAgingCap => "score_aging_cap": info,
        TimelineUnavailable => "timeline_unavailable": info,
        TimelineActive => "timeline_active": info,
        // Resource target reasons and failures.
        ResourceTargets => "resource_targets": info,
        ResourceWeights => "resource_weights": info,
        ResourceTargetPolicyIgnored => "resource_target_policy_ignored": warning,
        ResourceTargetPolicyUnreadable => "resource_target_policy_unreadable": warning,
        ResourceTargetPolicyExpired => "resource_target_policy_expired": info,
        ResourceTargetTasksUnevaluable => "resource_target_tasks_unevaluable": warning,
        ResourceTargetsParserCodeUnexpected => "resource_targets_parser_code_unexpected": error,
        ResourceTargetsCanonicalEncodeFailed => "resource_targets_canonical_encode_failed": error,
        ResourceTargetsViewEncodeFailed => "resource_targets_view_encode_failed": error,
        MalformedRows => "malformed_rows": error,
        // Forward projection (`ForwardError`) and its evidence gaps.
        ForwardProjectionInvalid => "forward_projection_invalid": error,
        ForwardProjectionOverflow => "forward_projection_overflow": error,
        StepBudgetExhausted => "step_budget_exhausted": warning,
        EffectEvidenceInsufficient => "effect_evidence_insufficient": warning,
        // Strategic report projection (`StrategyError`) and its planning-lane reasons.
        StrategyReportInvalid => "strategy_report_invalid": error,
        StrategyCatalogMismatch => "strategy_catalog_mismatch": error,
        StrategyTemplateAmbiguous => "strategy_template_ambiguous": error,
        StrategyNumericOverflow => "strategy_numeric_overflow": error,
        ExecutionContinues => "execution_continues": info,
        NoConditionalTemplateMatched => "no_conditional_template_matched": info,
        PinnedObservationMissing => "pinned_observation_missing": info,
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
