// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduling policy's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/policy.json`; the outcome guard (G1) checks both directions.
//! A decision reason carries its code as an outcome, with its values; its detail is
//! developer text only (Workflow #378 A2-P). Spellings several owners use with one meaning
//! are the contract's (`SharedCode`).

actingcommand_contract::outcome_codes! {
    /// The scheduling policy's codes: D2 S1 registered the first three ahead of the A2 sweep
    /// (Workflow #384); A2d registers the rest of the crate (Workflow #378); Workflow #411 B15
    /// R0 registers the remedy reasons at the end, ahead of the slices that emit them.
    pub enum PolicyCode {
        /// Warning. A candidate's selection document read a realistic instance fact key (one
        /// that recognition writes) whose value is unknown, and no reader can refresh it: the
        /// candidate is deferred with no wake, until the facts, the catalog or the
        /// configuration change (keys `instance_id`, `fact_key`, `unknown_reason`; optional
        /// `reader_problem`).
        FactRefreshUnbound => "fact_refresh_unbound": warning / none,
        /// Info (Workflow #384 D2-S4). The instance's recognition maintenance package is
        /// requested for a realistic key a candidate on the instance read as unknown: a reason
        /// of the package's own decision and dispatch chain, one per key (keys `instance_id`,
        /// `fact_key`, `unknown_reason`; optional `rule_id`).
        FactRefreshRequested => "fact_refresh_requested": info / none,
        /// Info (Workflow #384 D2-S4). A candidate waits for the instance's recognition
        /// maintenance package to read a key it reads: the package is selected this round, or
        /// is held back for now (keys `instance_id`, `fact_key`, `unknown_reason`, `package_id`;
        /// optional `gate_code`, `failure_code`, `next_eligible_unix_ms`).
        FactRefreshPending => "fact_refresh_pending": info / none,
        /// Warning (Workflow #384 D2-S4). The package's latest dispatch since the Runtime loaded
        /// the instance facts succeeded and still left the key unknown: it gave no reading of
        /// the key, or one recognition could not use (keys `instance_id`, `fact_key`,
        /// `unknown_reason`, `package_id`; optional `next_eligible_unix_ms`).
        FactRefreshUnresolved => "fact_refresh_unresolved": warning / none,
        /// Warning (Workflow #384 D2-S4). A limit holds the package back (keys `instance_id`,
        /// `fact_key`, `package_id`, `refresh_limit`; optional `gate_code`,
        /// `next_eligible_unix_ms`).
        FactRefreshExhausted => "fact_refresh_exhausted": warning / none,
        /// Info (Workflow #384 D2-S5). The candidates of an instance no longer wait for a key
        /// they waited for: only a transition of the fact refresh recorder, never a decision
        /// reason (keys `instance_id`, `fact_key`; optional `package_id`).
        FactRefreshResolved => "fact_refresh_resolved": info / none,
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
        /// Info (Workflow #361 R7). A clock predicate with `missed: skip_after` is not due: its
        /// latest occurrence was not served before its grace ran out, so the clock waits for
        /// its next occurrence (keys `occurrence_unix_ms`, `grace_ms`).
        ClockOccurrenceExpired => "clock_occurrence_expired": info / none,
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
        /// Info. Workflow #378 A2-P.
        TaskEligible => "task_eligible": info / none,
        /// Info. Workflow #378 A2-P.
        RankAssigned => "rank_assigned": info / none,
        /// Info. Workflow #378 A2-P.
        ScoreComputed => "score_computed": info / none,
        /// Info. Workflow #378 A2-P (keys `rule_id`).
        ScoreUnknown => "score_unknown": info / none,
        /// Info. Workflow #378 A2-P.
        ScoreCostUnknown => "score_cost_unknown": info / none,
        /// Info. Workflow #378 A2-P (keys `task_id`).
        DecisionCandidateNotSelected => "decision_candidate_not_selected": info / none,
        /// Info. Workflow #378 A2-P (keys `resource_target_id`).
        ResourceTargetUnmapped => "resource_target_unmapped": info / none,
        /// Info. Workflow #378 A2-P (keys `resource_target_id`).
        ResourceTargetOverride => "resource_target_override": info / none,
        /// Info. Workflow #378 A2-P (keys `resource_target_id`).
        ResourceTargetPending => "resource_target_pending": info / none,
        /// Info. Workflow #378 A2-P (keys `resource_target_id`).
        ResourceTargetSatisfied => "resource_target_satisfied": info / none,
        /// Info. Workflow #378 A2-P (keys `resource_target_id`).
        ResourceTargetApplied => "resource_target_applied": info / none,
        // ---- Workflow #411 B15 R0 (declared remedies, `contracts/remedies.md`): registered
        // ahead of the slices that emit them (R2 the evaluator's plan, R4 the host's signals);
        // begin ----
        /// Info. A blocked task is held until the remedy its result declares is served (keys
        /// `task_id`, `instance_id`, `outcome_key`, `need`, `decision_id`; optional `gate_code`,
        /// `next_eligible_unix_ms`; R2).
        PolicyRemedyWaiting => "policy_remedy_waiting": info / none,
        /// Info. A provider of an allowed capability is treated as triggered and promoted for a
        /// blocked task (keys `task_id`, `instance_id`, `decision_id`, `outcome_key`, `need`,
        /// `capability`, `capabilities_allowed`, `capabilities_declared`, `remedy_source`,
        /// `remedy_depth`, `count`, `limit_count`, `irreversible`; R2).
        PolicyRemedyRequested => "policy_remedy_requested": info / none,
        /// Info. A blocked task is retried once after its remedy ran (keys `task_id`,
        /// `instance_id`, `decision_id`, `outcome_key`, `capability`, `remedy_depth`, `count`,
        /// `limit_count`; R2).
        PolicyRemedyRetry => "policy_remedy_retry": info / none,
        /// Warning. A blocked task stopped for a person: no source allows a usable remedy (keys
        /// `task_id`, `instance_id`, `decision_id`, `outcome_key`, `need`, `remedy_problem`,
        /// `remedy_inputs_digest`; optional `remedy_source`, `capability`; R2, R4).
        PolicyRemedyUnchosen => "policy_remedy_unchosen": warning / none,
        /// Error. A blocked task stopped for a person: an allowed remedy has no task that can
        /// serve it (keys as `PolicyRemedyUnchosen`; R2, R4).
        PolicyRemedyUnavailable => "policy_remedy_unavailable": error / none,
        /// Warning. A blocked task stopped for a person: its remedies ran as often as the round
        /// allows, or the chain reached three layers (keys as `PolicyRemedyUnchosen`, and
        /// `count`, `limit_count`, `remedy_depth`; R2, R4).
        PolicyRemedyExhausted => "policy_remedy_exhausted": warning / none,
        /// Info. A stopped blocked task was released: its remedy inputs changed or a person
        /// lifted it (keys `task_id`, `instance_id`, `decision_id`, `remedy_inputs_digest`; R2,
        /// R4).
        PolicyRemedyLifted => "policy_remedy_lifted": info / none,
        /// Warning. A remedy round ended with its activity window before the remedy was served
        /// (keys `task_id`, `instance_id`, `decision_id`, `outcome_key`, `need`,
        /// `window_end_unix_ms`; optional `gate_code`; R2, R4).
        PolicyRemedyExpired => "policy_remedy_expired": warning / none,
        /// Info. The read-only parameters a policy dispatch hands to its run (keys
        /// `run_parameters`; R2; the channel Workflow #411 B8 shares).
        PolicyDispatchParameters => "policy_dispatch_parameters": info / none,
        // ---- Workflow #411 B15 R0: end ----
        // ---- Workflow #405 W1 (fact-time wake, `contracts/scheduling/README.md`, "Fact-time
        // wake"): begin ----
        /// Info. A `fact_time_reached` predicate is false because the time its fact holds plus
        /// its `offset_ms` has not come yet; the evaluation wakes at that instant (keys
        /// `fact_key`, `next_eligible_unix_ms`).
        FactTimePending => "fact_time_pending": info / none,
        /// Info. A `fact_time_reached` predicate is false because its fact gives no time; no
        /// fact refresh is requested (keys `fact_key`, `fact_time_problem`).
        FactTimeUnavailable => "fact_time_unavailable": info / none,
        // ---- Workflow #405 W1: end ----
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

// ---- Workflow #384 D2-S4: begin ----
actingcommand_contract::outcome_vocabulary! {
    /// Which limit holds an instance's recognition maintenance package back (the vocabulary
    /// `refresh_limit`, D2 model §3.7 row 4, §7.3).
    pub enum RefreshLimit: "refresh_limit" {
        /// The package's dispatches today reached `scheduling.fact_refresh.daily_limit`.
        DailyLimit => "daily_limit",
        /// Its failed runs in a row reached `scheduling.fact_refresh.max_consecutive_failures`
        /// today.
        FailurePause => "failure_pause",
        /// The instance's schedulable-pack state does not include the package now.
        SchedulableState => "schedulable_state",
        /// The instance's activity window is closed or its activity budget spent.
        ActivityWindow => "activity_window",
        /// The instance cannot place the package.
        Placement => "placement",
    }
}
// ---- Workflow #384 D2-S4: end ----

// ---- Workflow #405 W1: begin ----
actingcommand_contract::outcome_vocabulary! {
    /// Why a `fact_time_reached` predicate's fact gives no time (the vocabulary
    /// `fact_time_problem`).
    pub enum FactTimeProblem: "fact_time_problem" {
        /// No fact with this key exists in the predicate's scope.
        Missing => "missing",
        /// The fact's confidence is 0.
        LowConfidence => "low_confidence",
        /// The fact was observed longer ago than the predicate's `max_age_ms`.
        Stale => "stale",
        /// The fact is past its own expiry.
        Expired => "expired",
        /// The fact's value is not a `timestamp_ms` value.
        NotATime => "not_a_time",
    }
}
// ---- Workflow #405 W1: end ----
