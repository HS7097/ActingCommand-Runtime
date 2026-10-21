// SPDX-License-Identifier: AGPL-3.0-only

//! The execution kernel's outcome codes and locations (Workflow #378 A2d). Every entry is listed
//! in the fragment `contracts/outcome-codes/execution-kernel.json`; the outcome guard (G1) checks
//! both directions. Spellings several owners use with one meaning are the contract's
//! (`actingcommand_contract::codes::SharedCode`, `ContractLocation`, ...), and the kernel's error
//! types carry those as well. A recovery node's failure, an environment refusal and a guard
//! target left unevaluated carry registered codes (Workflow #378 A2-P); a node that
//! succeeded and a guard target that was evaluated carry none.

actingcommand_contract::outcome_codes! {
    /// The kernel's codes: execution sessions, contained tasks, offline simulation, online
    /// observation, run and drive decisions.
    pub enum ExecutionKernelCode {
        // Execution sessions and the kernel (`ExecutionKernelError`).
        ExecutionInstanceUnknown => "execution_instance_unknown": error / none,
        ExecutionInstanceIdentityMismatch => "execution_instance_identity_mismatch": error / none,
        ExecutionKernelClosed => "execution_kernel_closed": error / none,
        ExecutionKernelStatePoisoned => "execution_kernel_state_poisoned": error / none,
        ExecutionSessionSpawnFailed => "execution_session_spawn_failed": error / none,
        ExecutionSessionPanicked => "execution_session_panicked": error / none,
        ExecutionSessionStatePoisoned => "execution_session_state_poisoned": error / none,
        ExecutionSessionClosed => "execution_session_closed": error / none,
        ExecutionSessionUnavailable => "execution_session_unavailable": error / none,
        ExecutionSessionResponseLost => "execution_session_response_lost": error / none,
        ExecutionSessionClosePending => "execution_session_close_pending": error / none,
        ExecutionSessionCloseInProgress => "execution_session_close_in_progress": error / none,
        ExecutionSessionGenerationExhausted => "execution_session_generation_exhausted": error / none,
        ExecutionResourceCloseIncomplete => "execution_resource_close_incomplete": error / none,
        ExecutionEndpointRebindSessionOpen => "execution_endpoint_rebind_session_open": error / none,
        ExecutionEndpointRebindFailed => "execution_endpoint_rebind_failed": error / none,
        PreparationDeadlineExceeded => "preparation_deadline_exceeded": error / budget_or_permission,
        // Device backends of a session.
        // Workflow #387 D1c-F: a validated action whose plan fails to prepare (Runtime-internal).
        InputPlanPreparationFailed => "input_plan_preparation_failed": error / none,
        InputBackendMissing => "input_backend_missing": error / none,
        InputBackendCloseFailed => "input_backend_close_failed": error / stability_warning,
        InputFrameCaptureMismatch => "input_frame_capture_mismatch": error / none,
        InputFrameNotCommitted => "input_frame_not_committed": error / none,
        InputFrameSessionMissing => "input_frame_session_missing": error / none,
        CaptureBackendMissing => "capture_backend_missing": error / none,
        CaptureBackendCloseFailed => "capture_backend_close_failed": error / stability_warning,
        NemuInputFrameSourceMissing => "nemu_input_frame_source_missing": error / none,
        NemuInputFencingRequired => "nemu_input_fencing_required": error / none,
        NemuSessionCloseFailed => "nemu_session_close_failed": error / stability_warning,
        ApplicationFrameInvalidationFailed => "application_frame_invalidation_failed": error / none,
        ApplicationResourceCloseRequired => "application_resource_close_required": error / none,
        ApplicationAdbBaselineFailed => "application_adb_baseline_failed": error / environment_fault,
        // Capture geometry reads.
        CaptureGeometryKernelBusy => "capture_geometry_kernel_busy": error / none,
        CaptureGeometryKernelClosed => "capture_geometry_kernel_closed": error / none,
        CaptureGeometrySessionMissing => "capture_geometry_session_missing": error / none,
        CaptureGeometrySessionChanged => "capture_geometry_session_changed": error / none,
        CaptureGeometrySessionBusy => "capture_geometry_session_busy": error / none,
        CaptureGeometrySessionClosed => "capture_geometry_session_closed": error / none,
        CaptureGeometryQueueFull => "capture_geometry_queue_full": error / none,
        CaptureGeometryDeadlineElapsed => "capture_geometry_deadline_elapsed": error / environment_fault,
        CaptureGeometryReadFailed => "capture_geometry_read_failed": error / environment_fault,
        // Monitor observations.
        MonitorObservationInvalid => "monitor_observation_invalid": error / none,
        MonitorObservationUnavailable => "monitor_observation_unavailable": error / none,
        // Contained task admission (`ContainedTaskError`).
        ContainedTaskAdmissionFailed => "contained_task_admission_failed": error / declaration_error,
        ContainedTaskVisionProviderMissing => "contained_task_vision_provider_missing": error / declaration_error,
        ContainedTaskVisionProviderFailed => "contained_task_vision_provider_failed": error / environment_fault,
        ResourceDeclarationDiagnosticInvalid => "resource_declaration_diagnostic_invalid": error / none,
        BundleMaintenanceInvalid => "bundle_maintenance_invalid": error / none,
        ContainedTaskControlMissing => "contained_task_control_missing": error / declaration_error,
        ContainedTaskControlInvalid => "contained_task_control_invalid": error / declaration_error,
        ContainedTaskResolutionInvalid => "contained_task_resolution_invalid": error / declaration_error,
        ContainedTaskProgramInvalid => "contained_task_program_invalid": error / declaration_error,
        ContainedTaskPhasesInvalid => "contained_task_phases_invalid": error / declaration_error,
        ContainedTaskPageSetMissing => "contained_task_page_set_missing": error / declaration_error,
        ContainedTaskPageSetInvalid => "contained_task_page_set_invalid": error / declaration_error,
        ContainedTaskRecognitionPackMissing => "contained_task_recognition_pack_missing": error / declaration_error,
        ContainedTaskRecognitionInvalid => "contained_task_recognition_invalid": error / declaration_error,
        ContainedTaskRecognitionTargetRegionInvalid => "contained_task_recognition_target_region_invalid": error / declaration_error,
        ContainedTaskRecognitionTargetsUnavailable => "contained_task_recognition_targets_unavailable": error / none,
        ContainedTaskOperationMissing => "contained_task_operation_missing": error / declaration_error,
        ContainedTaskOperationInvalid => "contained_task_operation_invalid": error / declaration_error,
        ContainedTaskDestinationConflict => "contained_task_destination_conflict": error / declaration_error,
        ContainedTaskPrimitiveUnsupported => "contained_task_primitive_unsupported": error / declaration_error,
        ContainedTaskGuardMissing => "contained_task_guard_missing": error / declaration_error,
        ContainedTaskGuardInvalid => "contained_task_guard_invalid": error / declaration_error,
        ContainedTaskGuardTargetMissing => "contained_task_guard_target_missing": error / declaration_error,
        ContainedTaskGuardTargetInvalid => "contained_task_guard_target_invalid": error / declaration_error,
        ContainedTaskRecoveryMissing => "contained_task_recovery_missing": error / declaration_error,
        ContainedTaskRecoveryInvalid => "contained_task_recovery_invalid": error / declaration_error,
        ContainedTaskSamplingInvalid => "contained_task_sampling_invalid": error / declaration_error,
        ContainedTaskOutcomeDeclarationInvalid => "contained_task_outcome_declaration_invalid": error / declaration_error,
        ContainedTaskOutcomeDeclarationIncomplete => "contained_task_outcome_declaration_incomplete": error / declaration_error,
        // Workflow #390 OUT-2: a designated effect's cap names an instance fact that gave no cap
        // (keys `pack_operation_id`, `fact_key`; `unknown_reason`, or `count` and `limit_count`).
        ContainedTaskDesignatedEffectCapUnresolved => "contained_task_designated_effect_cap_unresolved": error / declaration_error,
        ContainedTaskResourceReadingInvalid => "contained_task_resource_reading_invalid": error / declaration_error,
        ContainedTaskLinearInvalid => "contained_task_linear_invalid": error / declaration_error,
        ContainedTaskSelectInvalid => "contained_task_select_invalid": error / declaration_error,
        ContainedTaskSelectPolicyMissing => "contained_task_select_policy_missing": error / declaration_error,
        ContainedTaskSelectPolicyInvalid => "contained_task_select_policy_invalid": error / declaration_error,
        ContainedTaskSelectPolicyMismatch => "contained_task_select_policy_mismatch": error / declaration_error,
        ContainedTaskRequiresScheduler => "contained_task_requires_scheduler": error / none,
        ContainedTaskHomeEntryNotRequired => "contained_task_home_entry_not_required": error / none,
        // Workflow #373 NAV-3b: a task's entry page set is refused (key `entry_problem`).
        ContainedTaskEntryPagesInvalid => "contained_task_entry_pages_invalid": error / declaration_error,
        // Contained task prerequisites and entry.
        ContainedTaskPrerequisiteUnbound => "contained_task_prerequisite_unbound": error / declaration_error,
        ContainedTaskPrerequisiteCycle => "contained_task_prerequisite_cycle": error / declaration_error,
        ContainedTaskPrerequisiteDepthExceeded => "contained_task_prerequisite_depth_exceeded": error / declaration_error,
        ContainedTaskPrerequisiteMismatch => "contained_task_prerequisite_mismatch": error / declaration_error,
        ContainedTaskPrerequisiteEntryUnmatched => "contained_task_prerequisite_entry_unmatched": error / pack_logic_mismatch,
        ContainedTaskReturnHomeEntryUnmatched => "contained_task_return_home_entry_unmatched": error / pack_logic_mismatch,
        ContainedTaskLinearEntryUnmatched => "contained_task_linear_entry_unmatched": error / pack_logic_mismatch,
        // Contained task execution.
        ContainedTaskTimeout => "contained_task_timeout": error / budget_or_permission,
        ContainedTaskRecordBoundary => "contained_task_record_boundary": error / none,
        ContainedTaskFrameInvalid => "contained_task_frame_invalid": error / environment_fault,
        ContainedTaskFrameResolutionMismatch => "contained_task_frame_resolution_mismatch": error / declaration_error,
        ContainedTaskInputOutOfBounds => "contained_task_input_out_of_bounds": error / declaration_error,
        ContainedTaskRecognitionConflict => "contained_task_recognition_conflict": error / pack_logic_mismatch,
        ContainedTaskGuardRefused => "contained_task_guard_refused": error / pack_logic_mismatch,
        // Workflow #390 EXEC-1: every candidate operation of the page was passed over (keys
        // `page_id`, `count`).
        ContainedTaskOperationCandidatesExhausted => "contained_task_operation_candidates_exhausted": error / pack_logic_mismatch,
        ContainedTaskStepUnconfirmed => "contained_task_step_unconfirmed": error / pack_logic_mismatch,
        ContainedTaskErrorPageReached => "contained_task_error_page_reached": error / pack_logic_mismatch,
        ContainedTaskLinearIntermediateUnobserved => "contained_task_linear_intermediate_unobserved": error / pack_logic_mismatch,
        ContainedTaskLinearApplicationUnconfirmed => "contained_task_linear_application_unconfirmed": error / pack_logic_mismatch,
        // Workflow #390 EXEC-2: a declared stability wait ended before its frames held (keys
        // `pack_operation_id`, `timeout_ms`).
        ContainedTaskInputStabilityUnconfirmed => "contained_task_input_stability_unconfirmed": error / pack_logic_mismatch,
        ContainedTaskObservationStabilityUnconfirmed => "contained_task_observation_stability_unconfirmed": error / pack_logic_mismatch,
        PageConfirmationFailed => "page_confirmation_failed": error / pack_logic_mismatch,
        /// Warning (Workflow #384 D2-S2): a reading mark gave no reading; the run goes on (keys
        /// `reading_id`, `fact_key`, `reading_problem`).
        ContainedTaskResourceReadingUnresolved => "contained_task_resource_reading_unresolved": warning / none,
        ContainedTaskStabilityComparisonFailed => "contained_task_stability_comparison_failed": error / environment_fault,
        ContainedTaskStabilityCounterInvalid => "contained_task_stability_counter_invalid": error / none,
        ContainedTaskStabilityCounterOverflow => "contained_task_stability_counter_overflow": error / none,
        // Recognition and candidate samples.
        RecognitionSamplesUnavailable => "recognition_samples_unavailable": error / none,
        RecognitionSamplingRuntimeUnsupported => "recognition_sampling_runtime_unsupported": error / none,
        RecognitionSampleBudgetInsufficient => "recognition_sample_budget_insufficient": error / budget_or_permission,
        RecognitionSampleDeadlineExceeded => "recognition_sample_deadline_exceeded": error / budget_or_permission,
        RecognitionSampleGeometryChanged => "recognition_sample_geometry_changed": error / environment_fault,
        CandidateSamplingRuntimeUnsupported => "candidate_sampling_runtime_unsupported": error / none,
        CandidateSampleBudgetInsufficient => "candidate_sample_budget_insufficient": error / budget_or_permission,
        CandidateSampleDeadlineExceeded => "candidate_sample_deadline_exceeded": error / budget_or_permission,
        CandidateSamplePageOrGeometryChanged => "candidate_sample_page_or_geometry_changed": error / pack_logic_mismatch,
        // Select steps (Workflow #308).
        SelectionStateUnavailable => "selection_state_unavailable": error / none,
        SelectionFactContextMismatch => "selection_fact_context_mismatch": error / none,
        SelectionEvaluationFailed => "selection_evaluation_failed": error / none,
        SelectionNotSelected => "selection_not_selected": error / none,
        SelectionPageChanged => "selection_page_changed": error / pack_logic_mismatch,
        SelectionProjectionMismatch => "selection_projection_mismatch": error / none,
        SelectionConfirmationCaptureFailed => "selection_confirmation_capture_failed": error / environment_fault,
        SelectPageMismatch => "select_page_mismatch": error / pack_logic_mismatch,
        // Workflow #308 SEL-L3: the scan session of a select step (frozen selector model S0 v2.1
        // §2.7; keys `stage`, `viewport_index` and each code's own values).
        SelectionScanStartUnconfirmed => "selection_scan_start_unconfirmed": error / pack_logic_mismatch,
        SelectionScanSettleIncomplete => "selection_scan_settle_incomplete": error / pack_logic_mismatch,
        SelectionScanAlignmentRejected => "selection_scan_alignment_rejected": error / pack_logic_mismatch,
        SelectionScanGapDetected => "selection_scan_gap_detected": error / pack_logic_mismatch,
        SelectionScanInstanceMissing => "selection_scan_instance_missing": error / pack_logic_mismatch,
        SelectionScanIdentityMismatch => "selection_scan_identity_mismatch": error / pack_logic_mismatch,
        SelectionScanEndUnconfirmed => "selection_scan_end_unconfirmed": error / pack_logic_mismatch,
        SelectionScanPositionUnread => "selection_scan_position_unread": error / pack_logic_mismatch,
        SelectionScanBoundsExceeded => "selection_scan_bounds_exceeded": error / budget_or_permission,
        // Workflow #308 SEL-L3: a scan dry run needed a frame its saved sequence does not hold.
        SelectionScanReplayFrameMissing => "selection_scan_replay_frame_missing": error / none,
        // Workflow #308 SEL-L4b: a decision unknown instances could change, a scan's confirmation
        // of the chosen member, and an `expect_identity` (frozen selector model S0 v2.1 §2.7).
        SelectionResultBlocked => "selection_result_blocked": error / pack_logic_mismatch,
        SelectionIdentityUnconfirmed => "selection_identity_unconfirmed": error / pack_logic_mismatch,
        SelectionIdentityMismatch => "selection_identity_mismatch": error / pack_logic_mismatch,
        // Post-admission OCR.
        OcrFieldsMissing => "ocr_fields_missing": error / declaration_error,
        OcrFieldsDictionaryMissing => "ocr_fields_dictionary_missing": error / declaration_error,
        OcrFieldsDictionaryInvalid => "ocr_fields_dictionary_invalid": error / declaration_error,
        ContainedTaskPostAdmissionOcrInvalid => "contained_task_post_admission_ocr_invalid": error / declaration_error,
        ContainedTaskPostAdmissionOcrOutcomeMissing => "contained_task_post_admission_ocr_outcome_missing": error / declaration_error,
        ContainedTaskPostAdmissionOcrOutcomeInvalid => "contained_task_post_admission_ocr_outcome_invalid": error / declaration_error,
        ContainedTaskPostAdmissionOcrPageGateMissing => "contained_task_post_admission_ocr_page_gate_missing": error / declaration_error,
        ContainedTaskPostAdmissionOcrPageGateInvalid => "contained_task_post_admission_ocr_page_gate_invalid": error / declaration_error,
        ContainedTaskPostAdmissionOcrTargetInvalid => "contained_task_post_admission_ocr_target_invalid": error / declaration_error,
        // Workflow #396 REC-6 retired `contained_task_post_admission_ocr_target_in_page_gate`:
        // a page gate may name the targets its page collects.
        ContainedTaskPostAdmissionOcrTargetOutOfBounds => "contained_task_post_admission_ocr_target_out_of_bounds": error / declaration_error,
        ContainedTaskPostAdmissionOcrTruthMissing => "contained_task_post_admission_ocr_truth_missing": error / declaration_error,
        ContainedTaskPostAdmissionOcrTruthInvalid => "contained_task_post_admission_ocr_truth_invalid": error / declaration_error,
        ContainedTaskPostAdmissionOcrTruthMismatch => "contained_task_post_admission_ocr_truth_mismatch": error / declaration_error,
        ContainedTaskPostAdmissionOcrLimitExceeded => "contained_task_post_admission_ocr_limit_exceeded": error / budget_or_permission,
        ContainedTaskPostAdmissionOcrCandidateWorkExceeded => "contained_task_post_admission_ocr_candidate_work_exceeded": error / budget_or_permission,
        ContainedTaskPostAdmissionOcrObservationMissing => "contained_task_post_admission_ocr_observation_missing": error / none,
        ContainedTaskPostAdmissionOcrEvidenceMismatch => "contained_task_post_admission_ocr_evidence_mismatch": error / none,
        // Offline simulation (`OfflineSimulationError`, `OfflineDecision::Refused`).
        OfflineFixtureExhausted => "offline_fixture_exhausted": error / none,
        OfflineDecisionFingerprintFailed => "offline_decision_fingerprint_failed": error / none,
        OfflineSimulationExecutedStepInvariant => "offline_simulation_executed_step_invariant": error / none,
        OfflineSimulationEffectIntentMissing => "offline_simulation_effect_intent_missing": error / none,
        OfflineSimulationConfirmationFrameMissing => "offline_simulation_confirmation_frame_missing": error / none,
        OfflineSimulationFrameCopyFailed => "offline_simulation_frame_copy_failed": error / none,
        OfflineSimulationActionMismatch => "offline_simulation_action_mismatch": error / none,
        OfflineSimulationMultipleEffects => "offline_simulation_multiple_effects": error / none,
        // Online page observation (`OnlineObservationError`) and its evidence rows.
        ObservationContainmentFailed => "observation_containment_failed": error / none,
        ObservationResourcesInvalid => "observation_resources_invalid": error / none,
        ObservationFrameInvalid => "observation_frame_invalid": error / none,
        ObservationFactFailed => "observation_fact_failed": error / none,
        ObservationProjectionFailed => "observation_projection_failed": error / none,
        EarlierTargetFailed => "earlier_target_failed": info / none,
        // Run and drive decisions (`RunDecisionError`, `DriveDecisionError`).
        RunDecisionInvalid => "run_decision_invalid": error / none,
        RunTransitionInvalid => "run_transition_invalid": error / none,
        DrivePlanInvalid => "drive_plan_invalid": error / none,
        DrivePackageInvalid => "drive_package_invalid": error / declaration_error,
        // Recovery graph node outcomes.
        DetectFailed => "detect_failed": error / pack_logic_mismatch,
        ActionFailed => "action_failed": error / environment_fault,
        // Failure frame comparison (Workflow #336 L6).
        PngInvalid => "png_invalid": error / none,
        SizeChanged => "size_changed": info / none,
        DigestCellsChanged => "digest_cells_changed": info / none,
        // Environment state refusals (`EnvironmentStateErrorKind::code`).
        SchemaMismatch => "schema_mismatch": error / none,
        ScopeMismatch => "scope_mismatch": error / none,
        DetectorMismatch => "detector_mismatch": error / none,
        ResourceHashChanged => "resource_hash_changed": error / none,
        MissingKey => "missing_key": error / none,
        UnsafeValue => "unsafe_value": error / none,
        UnallowedValue => "unallowed_value": error / none,
        InvalidPointer => "invalid_pointer": error / none,
        UndeclaredKey => "undeclared_key": error / none,
        // Workflow #386 SR-0 (scheduling rules): registered ahead of the slices that emit them
        // (`contracts/scheduling-blackout.md`, `contracts/policy-suspension.md`,
        // `contracts/cost-commit.md`). The fragment also carries SR-0's texts and values for
        // the four sampling codes above, which SR-1 emits in their registered form (SR-1 retired
        // `recognition_sample_deadline_or_permission_lost`, replaced by
        // `recognition_sample_deadline_exceeded`). Begin.
        /// Error. Admission refused a task whose declared sampling exceeds the smaller of its
        /// step and task timeouts (keys `required_ms`, `limit_ms`, and `pack_operation_id`
        /// when one operation exceeds it; SR-1 emits it in place of the running
        /// `recognition_sample_budget_insufficient`).
        RecognitionSampleBudgetInvalid => "recognition_sample_budget_invalid": error / budget_or_permission,
        /// Warning. A run recognised a page its task declares as the sign of a locked feature
        /// and stopped there, with no input (keys `fact_key`, `page_id`; SR-5b).
        ContainedTaskFeatureLocked => "contained_task_feature_locked": warning / planned_stop,
        /// Warning. Before the input of its declared entry-cost step, the run had too little
        /// time left and stopped with nothing spent (keys `required_ms`, `remaining_ms`,
        /// `step_id`; SR-10).
        CostCommitTimeInsufficient => "cost_commit_time_insufficient": warning / budget_or_permission,
        /// Error. A task package's `cost_commit` declaration breaks a rule (keys `package_id`,
        /// `field`; SR-10).
        CostCommitDeclarationInvalid => "cost_commit_declaration_invalid": error / declaration_error,
        // Workflow #386 SR-0. End.
        /// Info. The operation declares `unguarded_trusted_coordinate`, so its guard target is
        /// not evaluated: the task-diagnostic stream's unexecuted guard row (Workflow #378 A2-P).
        ContainedTaskGuardSkipped => "contained_task_guard_skipped": info / none,
        /// Warning. The observed page is not the guard's page, so the guard target is not
        /// evaluated: the task-diagnostic stream's unexecuted guard row; the run itself fails
        /// `contained_task_guard_refused` (Workflow #378 A2-P).
        ContainedTaskGuardPageMismatch => "contained_task_guard_page_mismatch": warning / none,
        /// Error. An environment detection value's confidence is below the declared minimum
        /// (`EnvironmentStateErrorKind::LowConfidence`; Workflow #378 A2-P).
        EnvironmentValueLowConfidence => "environment_value_low_confidence": error / none,
        /// Error. An environment detection value is older than its declared lifetime
        /// (`EnvironmentStateErrorKind::Expired`; Workflow #378 A2-P).
        EnvironmentValueExpired => "environment_value_expired": error / none,
    }
}

actingcommand_contract::outcome_locations! {
    /// The kernel's locations: the stages of an online page observation, the stage of the
    /// provider default's emulator control refusal (Workflow #378 A2-P), and the observation a
    /// sampling timeout ended (Workflow #386 SR-1).
    pub enum ExecutionKernelLocation {
        // Online page observation stages (`OnlineObservationError::stage`, evidence rows).
        LoadObservationResources => "load_observation_resources",
        DecodeObservationFrame => "decode_observation_frame",
        EncodeObservationFact => "encode_observation_fact",
        ProjectObservation => "project_observation",
        EvaluatePages => "evaluate_pages",
        EvaluatePage => "evaluate_page",
        EvaluatePageTarget => "evaluate_page_target",
        EvaluateRequestedTarget => "evaluate_requested_target",
        EmulatorControlSupport => "emulator_control_support",
        // Workflow #386 SR-1: the observation whose sampling timed out (the `stage` of
        // `recognition_sample_deadline_exceeded` and `candidate_sample_deadline_exceeded`).
        ObservationEntry => "observation_entry",
        ObservationBeforeInput => "observation_before_input",
        ObservationAfterInput => "observation_after_input",
        ObservationPageWait => "observation_page_wait",
        // Workflow #308 SEL-L3: the stages of a select step's scan session (the `stage` of the
        // `selection_scan_` codes, frozen selector model S0 v2.1 §2.7).
        SelectScanStart => "select_scan_start",
        SelectScanSettle => "select_scan_settle",
        SelectScanAlign => "select_scan_align",
        SelectScanObserve => "select_scan_observe",
        SelectScanEnd => "select_scan_end",
        SelectScanDecide => "select_scan_decide",
        // Workflow #308 SEL-L4b: a select step's decision, the return to its chosen member, the
        // member's confirmation and the destination's `expect_identity` (S0 v2.1 §2.7).
        SelectDecide => "select_decide",
        SelectReturn => "select_return",
        SelectConfirm => "select_confirm",
        SelectExpectIdentity => "select_expect_identity",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Which rule of a contained task's program refused it at admission (vocabulary
    /// `program_problem`, Workflow #411 A39): the value of `contained_task_program_invalid` where
    /// the refusal names its rule.
    pub enum ProgramProblem: "program_problem" {
        /// The task declares no operation, which only a task schema `0.8` zero-input reading
        /// task without recovery may (contracts/ocr-fields.md).
        OperationsEmpty => "operations_empty",
    }
}
