// SPDX-License-Identifier: AGPL-3.0-only

//! Resource tooling's outcome codes (Workflow #378 A2d). Every entry is listed in the fragment
//! `contracts/outcome-codes/resource-tooling.json`; the outcome guard (G1) checks both
//! directions. A refusal still travels as a Lab error (`LabError`), whose code field stays a
//! spelling (`ResourceToolingCode::as_str`) until B4 converts the Lab sites.

actingcommand_contract::outcome_codes! {
    /// Why resource authoring, a package bundle or a resource conversion refused.
    pub enum ResourceToolingCode {
        // Resource authoring transactions (`authoring`).
        AuthoringBackupRenameFailed => "authoring_backup_rename_failed": error / none,
        AuthoringCandidateRenameFailed => "authoring_candidate_rename_failed": error / none,
        AuthoringCleanupFailed => "authoring_cleanup_failed": error / none,
        AuthoringCommitCleanupFailed => "authoring_commit_cleanup_failed": error / none,
        AuthoringCommittedTargetMissing => "authoring_committed_target_missing": error / none,
        AuthoringCreateDirectoryFailed => "authoring_create_directory_failed": error / none,
        AuthoringDestinationHasNoParent => "authoring_destination_has_no_parent": error / none,
        AuthoringDraftEmpty => "authoring_draft_empty": error / none,
        AuthoringFailureNotDurable => "authoring_failure_not_durable": error / none,
        AuthoringFieldEmpty => "authoring_field_empty": error / none,
        AuthoringFileCreateFailed => "authoring_file_create_failed": error / none,
        AuthoringFileSyncFailed => "authoring_file_sync_failed": error / none,
        AuthoringFileWriteFailed => "authoring_file_write_failed": error / none,
        AuthoringHashMetadataFailed => "authoring_hash_metadata_failed": error / none,
        AuthoringHashReadFailed => "authoring_hash_read_failed": error / none,
        AuthoringHashScopeError => "authoring_hash_scope_error": error / none,
        AuthoringInjectedRenameFailure => "authoring_injected_rename_failure": error / none,
        AuthoringJournalCorrupt => "authoring_journal_corrupt": error / none,
        AuthoringJournalIdentityInvalid => "authoring_journal_identity_invalid": error / none,
        AuthoringJournalIdentityMissing => "authoring_journal_identity_missing": error / none,
        AuthoringJournalOpenFailed => "authoring_journal_open_failed": error / none,
        AuthoringJournalPathInvalid => "authoring_journal_path_invalid": error / none,
        AuthoringJournalReadFailed => "authoring_journal_read_failed": error / none,
        AuthoringJournalRemoveFailed => "authoring_journal_remove_failed": error / none,
        AuthoringJournalSchemaUnsupported => "authoring_journal_schema_unsupported": error / none,
        AuthoringJournalSerializeFailed => "authoring_journal_serialize_failed": error / none,
        AuthoringJournalSyncFailed => "authoring_journal_sync_failed": error / none,
        AuthoringJournalTransitionInvalid => "authoring_journal_transition_invalid": error / none,
        AuthoringJournalTypeMismatch => "authoring_journal_type_mismatch": error / none,
        AuthoringJournalWriteFailed => "authoring_journal_write_failed": error / none,
        AuthoringMetadataFailed => "authoring_metadata_failed": error / none,
        AuthoringOutcomeNotDurable => "authoring_outcome_not_durable": error / none,
        AuthoringPathConflict => "authoring_path_conflict": error / none,
        AuthoringPathInvalid => "authoring_path_invalid": error / none,
        AuthoringRecoveryAmbiguous => "authoring_recovery_ambiguous": error / none,
        AuthoringRecoveryCandidateMismatch => "authoring_recovery_candidate_mismatch": error / none,
        AuthoringRecoveryCandidateMissing => "authoring_recovery_candidate_missing": error / none,
        AuthoringRecoveryContextInvalid => "authoring_recovery_context_invalid": error / none,
        AuthoringRecoveryOracleRequired => "authoring_recovery_oracle_required": error / none,
        AuthoringRecoveryRestoreFailed => "authoring_recovery_restore_failed": error / none,
        AuthoringRecoveryTargetCanonicalizeFailed => "authoring_recovery_target_canonicalize_failed": error / none,
        AuthoringRecoveryTargetMismatch => "authoring_recovery_target_mismatch": error / none,
        AuthoringRollbackBackupMissing => "authoring_rollback_backup_missing": error / none,
        AuthoringRollbackFailed => "authoring_rollback_failed": error / none,
        AuthoringRollbackRestoreFailed => "authoring_rollback_restore_failed": error / none,
        AuthoringScopeEscape => "authoring_scope_escape": error / none,
        AuthoringSourceMetadataFailed => "authoring_source_metadata_failed": error / none,
        AuthoringSourceNotRegularFile => "authoring_source_not_regular_file": error / none,
        AuthoringSourceReadFailed => "authoring_source_read_failed": error / none,
        AuthoringSpecialFileRejected => "authoring_special_file_rejected": error / none,
        AuthoringStageCreateFailed => "authoring_stage_create_failed": error / none,
        AuthoringSymlinkRejected => "authoring_symlink_rejected": error / none,
        AuthoringTargetCreateFailed => "authoring_target_create_failed": error / none,
        AuthoringTargetHasNoName => "authoring_target_has_no_name": error / none,
        AuthoringTargetHasNoParent => "authoring_target_has_no_parent": error / none,
        AuthoringTargetNotDirectory => "authoring_target_not_directory": error / none,
        AuthoringTargetParentCreateFailed => "authoring_target_parent_create_failed": error / none,
        AuthoringTargetSymlinkRejected => "authoring_target_symlink_rejected": error / none,
        AuthoringTargetUnavailable => "authoring_target_unavailable": error / none,
        AuthoringTransactionConflict => "authoring_transaction_conflict": error / none,
        AuthoringTreeReadFailed => "authoring_tree_read_failed": error / none,
        AuthoringTreeRemoveFailed => "authoring_tree_remove_failed": error / none,
        AuthoringTreeTypeMismatch => "authoring_tree_type_mismatch": error / none,
        AuthoringValidationEmpty => "authoring_validation_empty": error / none,
        AuthoringValidationFailed => "authoring_validation_failed": error / none,
        RecordPromoteTargetExists => "record_promote_target_exists": error / none,
        // Package bundle directories (`package_directory`).
        PackageBundleOutExists => "package_bundle_out_exists": error / none,
        PackageBundleApplicationsInvalid => "package_bundle_applications_invalid": error / none,
        PackageBundlePackMismatch => "package_bundle_pack_mismatch": error / none,
        PackageBundleDefaultPackageMissing => "package_bundle_default_package_missing": error / none,
        PackageBundleMaintenanceInvalid => "package_bundle_maintenance_invalid": error / none,
        PackageBundleIndexInvalid => "package_bundle_index_invalid": error / none,
        PackageBundlePacksRootInvalid => "package_bundle_packs_root_invalid": error / none,
        /// Workflow #411 A40: the pack table holds no pack: `package bundle`'s `--packs-root` holds
        /// no pack directory, or the `bundle.json` `actinglab standard-pack check` reads lists no
        /// pack (key `path`, where it looked, relative to the standard package; `json_pointer`
        /// inside `bundle.json`).
        PackageBundlePacksEmpty => "package_bundle_packs_empty": error / none,
        PackageBundleWriteFailed => "package_bundle_write_failed": error / none,
        /// Workflow #373 NAV-T (M-373 v1.1 §3.9): a package a standard-pack root file names
        /// cannot play its part, as the Runtime would refuse it (keys `package_id`,
        /// `navigation_role`, `navigation_problem`; the loader's refusal as the cause of
        /// `admission_failed`). `package bundle` and `actinglab standard-pack check` report it.
        PackageBundleNavigationPackageInvalid => "package_bundle_navigation_package_invalid": error / none,
        /// Warning (Workflow #373 NAV-T, M-373 §4.4, §10.1 NAV-4): a pack's entry pages are
        /// not all pages of `pages.json` (keys `package_id`, `page_ids`), so the Runtime does
        /// not navigate to it; the report the resource CI reads.
        PackageBundleEntryPageUnlisted => "package_bundle_entry_page_unlisted": warning / none,
        /// Warning (Workflow #411 B15, `contracts/remedies.md`; registered by R0 ahead of R1,
        /// which reports it): a package's remedy cannot be served inside its game root: no
        /// package of the root provides the capability, the packages form a cycle, or two
        /// providers of one capability declare different parameters (keys `package_id`,
        /// `remedy_problem`; optional `need`, `capability`, `outcome_key`). `package bundle` and
        /// `actinglab standard-pack check` list one per finding in `warnings`.
        PackageBundleRemedyInvalid => "package_bundle_remedy_invalid": warning / none,
        /// Warning (Workflow #411 B35): a package declares a page that `interrupts.json` lists as
        /// a shared interrupt page (keys `package_id`, `page_id`); the page is the package's own,
        /// so that interrupt does not act for its runs. `package bundle` and `actinglab
        /// standard-pack check` list one per package and page in `warnings`; a feature package
        /// and the page's handlers are not listed.
        PackageBundleInterruptPageShadowed => "package_bundle_interrupt_page_shadowed": warning / none,
        /// Warning (STORE-FG-1, design-store-fg §8.1): a store step of a package that declares
        /// `store_update` taps a fixed place (click kind `point`, `rect` or `specific_rect`)
        /// rather than a target located on the frame (keys `package_id`, `pack_operation_id`).
        /// `package bundle` and `actinglab standard-pack check` list one per package and such
        /// step in `warnings`; nothing is refused.
        PackageBundleStoreTapUnlocated => "package_bundle_store_tap_unlocated": warning / none,
        /// Warning (Workflow #428 CFL-2, design CF v2 §5.6): a slot's or feature's OCR target lies
        /// in a layout's shared read with its model and languages, but declares another
        /// `preprocess` chain (colors in another order too), so it reads on its own (keys
        /// `package_id`, `target_id`, and `json_pointer`, the shared read in the package's
        /// recognition pack). `package bundle` and `actinglab standard-pack check` list one per
        /// package, target and shared read in `warnings`; nothing is refused.
        PackageBundlePreprocessDivergent => "package_bundle_preprocess_divergent": warning / none,
        /// Warning (Workflow #384 D2-S2, transitional): a pack of a bundle with maintenance
        /// declarations only navigates and reads, but the declarations do not name it for
        /// `recognition` (key `package_id`). The resource repositories make it an error once
        /// their recognition packages are declared (D2-S6).
        RecognitionPackUndeclared => "recognition_pack_undeclared": warning / none,
        // Resource conversion source reads (`resource_convert`).
        SelectionPolicyLimitExceeded => "selection_policy_limit_exceeded": error / none,
        // Workflow #464 P1: `actinglab standard-pack adopt` (`adopt`).
        /// Workflow #464 P1 (contracts/standard-pack-authoring.md, "The checks"): a check of the
        /// adoption refused before anything was written in the repository (key `adopt_problem`;
        /// optional `path`, `entry`, `json_pointer`, `package_id`, `member`, `count`; the refusal
        /// of the loader, the reader or the composer as its cause).
        AdoptRefused => "adopt_refused": error / none,
        /// Workflow #464 P1 (contracts/standard-pack-authoring.md, "What it writes"): a write of
        /// the adoption failed, or stopped because a root file changed after the call read it
        /// (key `path`, where it stopped; optional `paths`, what it had written or moved before;
        /// the operating system's refusal as its cause when there is one); nothing is removed in
        /// the checkout.
        AdoptWriteFailed => "adopt_write_failed": error / none,
        /// Warning (Workflow #464 P3, `contracts/standard-pack-schedule.md` "Readiness"): a pack of
        /// a game root is not dispatched yet for one readiness item (keys `package_id`,
        /// `readiness_item`, `readiness_problem`, and the item's values; the check's own outcome
        /// as its cause where one exists). `standard-pack ready` answers one per failing item.
        PackageDispatchWaiting => "package_dispatch_waiting": warning / none,
        /// Info (Workflow #464 P3): a readiness item of a pack that was not judged, from what the
        /// readiness was given, or (item `unrecorded_screen_code`) an information line that never
        /// stops a pack (keys `package_id`, `readiness_item`, `readiness_unjudged`).
        PackageReadinessUnjudged => "package_readiness_unjudged": info / none,
        // Workflow #463 S3-10: `actinglab standard-pack trial`, its plan and its record
        // (`standard_pack_trial`, contracts/standard-pack-trial.md). Its malformed arguments are
        // `validation_failed`, as the command line's are (fix round 2).
        /// Workflow #463 S3-10: the state directory is, or lies inside, a repository or an
        /// installed root (keys `path`, `trial_state_place`).
        TrialStateDirRefused => "trial_state_dir_refused": error / none,
        /// Workflow #463 S3-10 fix round 2: a trial record of that name, or the `.part` of one,
        /// is there already and is never replaced (key `path`, the file that is there). Workflow
        /// #463 S3-11 fix round 2: or the run being recorded has its record already, whatever
        /// content that record names (`path` that record): a run has one record.
        TrialRecordExists => "trial_record_exists": error / none,
        /// Workflow #463 S3-10: the operating system refused a step of writing a trial record,
        /// and no record was written (key `path`; its refusal as the cause; fix round 3: a file
        /// the write created and could not remove after it, named by a `cleanup` link).
        TrialRecordWriteFailed => "trial_record_write_failed": error / none,
        /// Workflow #463 S3-10: a file of a trial record's name does not read as that record (keys
        /// `path`, `trial_record_problem`; the refusal of the read or of the decoder as its
        /// cause).
        TrialRecordUnreadable => "trial_record_unreadable": error / none,
        /// Warning (Workflow #463 S3-10 fix round 3): a trial record is written and in place, but
        /// the `.part` its write created is left beside it, its removal refused (key `path`, the
        /// `.part`; the refusal as its cause). The writer answers success with it in `warnings`.
        TrialRecordPartLeft => "trial_record_part_left": warning / none,
        // Workflow #463 S3-11: the run of `actinglab standard-pack trial`
        // (contracts/standard-pack-trial.md, "The run"). The first three are the refusals only the
        // run needs, before anything is sent or written (`check_trial_run`).
        /// Workflow #463 S3-11: a trial's run was asked for without `--authorization` (key
        /// `package_id`); a plan needs none.
        TrialAuthorizationMissing => "trial_authorization_missing": error / none,
        /// Workflow #463 S3-11: the irreversible mark flags the package over the root given, and
        /// the run was asked for without `--irreversible` (key `package_id`).
        TrialIrreversibleUnacknowledged => "trial_irreversible_unacknowledged": error / none,
        /// Workflow #463 S3-11: the run was asked for with a `--variable`, and a direct run carries
        /// no variable value yet (keys `package_id`, `variable_name`, the first given).
        TrialVariablesNotCarried => "trial_variables_not_carried": error / none,
        /// Warning (Workflow #463 S3-11): the trial's run ended normally on another end than the
        /// one `--expect` named (key `package_id`; optional `outcome_key`, the end's key). The
        /// command succeeds with `matched` false.
        TrialEndUnexpected => "trial_end_unexpected": warning / none,
        /// Workflow #463 S3-11: the read of the trial's run shows that it has not ended yet (keys
        /// `request_id`, `run_state`: `not_found`, `admitted` or `running`), so no record was
        /// written; it is read again later (fix round 2: its one cause; an interrupted run and a
        /// terminal whose record was not found have their own codes).
        TrialEndUnread => "trial_end_unread": error / none,
        /// Workflow #463 S3-11: `--collect` names a run that is not a trial's run of the trial's
        /// package on its instance (keys `request_id`, `trial_collect_problem`).
        TrialCollectMismatch => "trial_collect_mismatch": error / none,
        /// Workflow #463 S3-11 fix round 2: the trial's run was interrupted: the Runtime started or
        /// took over after its first fact and it has no terminal (`interrupted_unterminated`), so it
        /// has no end to record (key `request_id`); no record was written.
        TrialRunInterrupted => "trial_run_interrupted": error / none,
        /// Workflow #463 S3-11 fix round 2: the read of the trial's run names its terminal (key
        /// `run_state`, the end it names), but the ledger's record of that terminal was not found
        /// by its request at its sequence (key `request_id`); no record was written.
        TrialTerminalUnrecorded => "trial_terminal_unrecorded": error / none,
        /// Workflow #463 S3-11 fix round 2: the trial's run ended failed or cancelled (keys
        /// `request_id`, `run_state`) and its terminal names no failure code; its record is written
        /// and the command fails as for a failed run.
        TrialEndUncoded => "trial_end_uncoded": error / none,
        // Workflow #463 S3-12: what `actinglab standard-pack adopt` says before a package is
        // enabled (contracts/standard-pack-authoring.md, "The trial and the two warnings"). Both
        // ride in the adoption's `warnings` and refuse nothing.
        /// Warning (Workflow #463 S3-12): the decision adopts the package in a class that runs
        /// (business or maintenance), lifting its hold where it had one, so the placement that
        /// enables it asks for `--authorization` and for `--enables` naming it (key `package_id`).
        EnablingNeedsAuthorization => "enabling_needs_authorization": warning / none,
        /// Warning (Workflow #463 S3-12): no passed trial of the content the adoption writes is on
        /// record in its state directory (key `package_id`; the review's `trial` says what is).
        NotTried => "not_tried": warning / none,
        // Workflow #463 S3-15a: the refusals of `actinglab standard-pack status --installation`
        // (contracts/standard-pack-schedule.md, "Where a pack stands"), each before the game
        // root, a journal or the suspension report is read.
        /// Workflow #463 S3-15a: `--root` is not the installed game root of its game in the
        /// installation actinglab runs from, `<standard_packs.root>\<game>` (key `path`, the root
        /// given).
        PackageStatusRootNotInstalled => "package_status_root_not_installed": error / none,
        /// Workflow #463 S3-15a: this build of actinglab holds no rescue module (it is built for
        /// Windows only), so it reads no installation for `--installation`.
        PackageStatusInstallationUnbuilt => "package_status_installation_unbuilt": error / none,
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-12: the latest trial of the content an adoption writes, as the trial
    /// records keep it (vocabulary `trial_state`, the member `review.trial.state` of `standard-pack
    /// adopt`). The train of S3-12 and S3-14 (rulings R1, R2): the one vocabulary of a content's
    /// trials, the `state` of the trial module's `TrialSummary`, which `standard-pack status`
    /// answers as `trial.state` too; each record judged by `PackTrialRecord::passed`.
    pub enum TrialState: "trial_state" {
        /// The latest record of this content passed (`PackTrialRecord::passed`): its run ended on
        /// an end the package declares and, when the trial named the end it expected, on that end.
        Passed => "passed",
        /// The latest record of this content is of a run that failed or was cancelled, or that
        /// ended normally on another end than the one the trial expected.
        Failed => "failed",
        /// No record of this content is on record.
        NoRecord => "none",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-11: why the run `--collect` names is not a trial's run (vocabulary
    /// `trial_collect_problem`, the value `trial_collect_problem` of `trial_collect_mismatch`).
    pub enum TrialCollectProblem: "trial_collect_problem" {
        /// The run's facts do not name the trial's package as the package it admitted (another
        /// one, or none).
        OtherPackage => "other_package",
        /// The run's facts do not name the instance the trial is for (another one, or none).
        OtherInstance => "other_instance",
        /// Fix round 2: the run is the scheduler's (`dispatch` `scheduled`); a trial's run is sent
        /// by hand.
        Scheduled => "scheduled",
        /// Fix round 2: the run's facts hold no record of its request's origin (`dispatch`
        /// `unknown`), so whether it was sent by hand is not known.
        OriginUnknown => "origin_unknown",
        /// Fix round 2: the run was sent by hand on another channel than the Lab's (`origin` `cli`
        /// or `ui`); a trial's run is sent on the Lab's channel.
        OtherChannel => "other_channel",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-10: where a state directory refused for trial records lies (vocabulary
    /// `trial_state_place`, the value `trial_state_place` of `trial_state_dir_refused`).
    pub enum TrialStatePlace: "trial_state_place" {
        /// It is, or lies inside, a Git work tree: a directory along its path holds `.git`.
        Repository => "repository",
        /// It is, or lies inside, an installed root: a directory along its path holds the
        /// installer's own state file `install\state.json` (fix round 2).
        InstalledRoot => "installed_root",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-10 fix round 2: why a file of a trial record's name is not read as that
    /// record (vocabulary `trial_record_problem`, the value `trial_record_problem` of
    /// `trial_record_unreadable`).
    pub enum TrialRecordProblem: "trial_record_problem" {
        /// The operating system refused to open or read the file, or to list its directory (its
        /// refusal as the cause).
        ReadFailed => "read_failed",
        /// The file is larger than a record is read up to (1 MiB).
        TooLarge => "too_large",
        /// The file does not decode as `actingcommand.pack-trial.v1` (the decoder's refusal as
        /// the cause).
        Undecodable => "undecodable",
        /// The file decodes, but is not the record of the package, the content and the time its
        /// name says.
        NameMismatch => "name_mismatch",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #464 P1: which check of `actinglab standard-pack adopt` refused (vocabulary
    /// `adopt_problem`, the value `adopt_problem` of `adopt_refused`).
    pub enum AdoptProblem: "adopt_problem" {
        RepositoryInvalid => "repository_invalid",
        RecordingNotCurrent => "recording_not_current",
        PackageInvalid => "package_invalid",
        PackageMismatch => "package_mismatch",
        PackExists => "pack_exists",
        PackDirty => "pack_dirty",
        RecordingUnreviewed => "recording_unreviewed",
        LocalPath => "local_path",
        DecisionInvalid => "decision_invalid",
        /// The decision's `value` names the origin `model`, which only the model's export writes
        /// (the coordinator's ruling on #464).
        ValueOriginModel => "value_origin_model",
        RuleRefused => "rule_refused",
        RootFileNotCanonical => "root_file_not_canonical",
        /// With `--replace`, the replaced directory's kept place under the state directory is on
        /// another volume than the checkout, where it cannot be moved.
        ReplaceAcrossVolumes => "replace_across_volumes",
        /// The place of the checkout's check tree holds no marker of the tool's own, so it is not
        /// touched (the coordinator's ruling on the check tree).
        CheckTreeUnmarked => "check_tree_unmarked",
        /// Another adoption of the checkout with the same state directory holds its check tree.
        CheckTreeBusy => "check_tree_busy",
        /// The place of the checkout's check tree is a link (a symbolic link or a junction), so
        /// nothing in it is cleared (fix round 2); or so is a directory beside it named as one a
        /// call makes the tree whole in (fix round 3).
        CheckTreeLink => "check_tree_link",
        /// A maintenance decision by its uses alone meets the package's business rule (a rule the
        /// composer judges by its value, fix round 3), which adopt does not turn into a
        /// maintenance rule (fix round 2).
        BusinessRuleInTheWay => "business_rule_in_the_way",
    }
}

// ---- Workflow #464 P3 (`contracts/standard-pack-schedule.md`, "Readiness"): begin ----
actingcommand_contract::outcome_vocabulary! {
    /// The thirteen readiness items of a pack (the vocabulary `readiness_item`): the seven a game
    /// root tells (S1 to S7) and the six an installation tells (I1 to I6).
    pub enum ReadinessItem: "readiness_item" {
        /// S1: the Runtime's loader admits the pack.
        Admitted => "admitted",
        /// S2: a rule names the pack and it is not held, or the root files give it a maintenance
        /// role.
        Classified => "classified",
        /// S3: every rule of the pack is in the maintenance tier or declares its value.
        Valued => "valued",
        /// S4: the composer accepts the game's `schedule.json` with one instance per server.
        Composed => "composed",
        /// S5: the pack's entry is reached: by an application entry, a registered prerequisite,
        /// navigation that lists it or its server's return-home package (for a page-graph pack's
        /// home entry, one the kernel accepts as the Runtime's entry asks, fix round 2); and its
        /// prerequisite chain passes the kernel's own predicate (fix round 1).
        EntryReachable => "entry_reachable",
        /// S6: the models the pack's recognition targets need are named.
        ModelsNamed => "models_named",
        /// S7 (fix round 1): `standard-pack check` accepts the game root, beyond what S1 and S4
        /// tell (a refusal of their kind is answered under them).
        RootChecked => "root_checked",
        /// I1: an instance of the installation is bound to the pack's game and server.
        InstanceBound => "instance_bound",
        /// I2: the installation's `package` range holds the bundle's declared maximum, and that
        /// maximum is the revision of what the game root carries at least, by the rule of `place`
        /// at the edge.
        PackageRevision => "package_revision",
        /// I3: whether the live Runtime has the code for an unrecorded screen; an information line.
        UnrecordedScreenCode => "unrecorded_screen_code",
        /// I4: the live build names every `requires` word of the game's rules.
        RequiresKnown => "requires_known",
        /// I5: no pair of the pack is suspended.
        PairUnsuspended => "pair_unsuspended",
        /// I6: the installation holds the models of S6.
        ModelsInstalled => "models_installed",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why a readiness item keeps a pack waiting (the vocabulary `readiness_problem`), the value
    /// `readiness_problem` of `package_dispatch_waiting`.
    pub enum ReadinessProblem: "readiness_problem" {
        /// S1: the loader refused the pack (its refusal is the cause).
        AdmissionRefused => "admission_refused",
        /// S2: the game root holds no `schedule.json` and no root file gives the pack a role.
        ScheduleMissing => "schedule_missing",
        /// S2: no rule names the pack and no root file gives it a role.
        Unclassified => "unclassified",
        /// S2: `schedule.json` holds the pack (`held`).
        Held => "held",
        /// S3: a rule of the pack lacks a value that is still the pending marker (`task_id`).
        ValuePending => "value_pending",
        /// S3: a rule of the pack declares no value (`task_id`, `pool_id` where a pool lacks one).
        ValueUndeclared => "value_undeclared",
        /// S3 (fix round 1): every rule of the pack rides with another rule's chain, and the pack
        /// of each head waits (`task_id` the follower, `blocking_package_id` the head's pack).
        HeadWaiting => "head_waiting",
        /// S4: the composer refuses the game's `schedule.json`, or `standard-pack check` refuses
        /// it (its refusal is the cause).
        CompositionRefused => "composition_refused",
        /// S5: the prerequisite package the pack declares is no package of the game root (the
        /// kernel's `contained_task_prerequisite_unbound` is the cause, `blocking_package_id` the
        /// layer).
        PrerequisiteUnregistered => "prerequisite_unregistered",
        /// S5 (fix round 1): the kernel's prerequisite chain refuses the pack's chain (another
        /// game, server or resolution, a layer that cannot be one, a cycle, too deep, too many
        /// steps), or a layer's admission fails; the kernel's code or the layer's refusal is the
        /// cause, `blocking_package_id` the layer.
        PrerequisiteRefused => "prerequisite_refused",
        /// S5 (fix round 2): the return-home package the home entry of a page-graph pack takes is
        /// one the Runtime's entry refuses (the kernel's `require_configured_package_id` and
        /// `prerequisite_incompatibility_with` for a configured return-home package: another
        /// game, server or resolution, one that cannot be a layer, one that declares a
        /// prerequisite package of its own), or one the loader refuses; the kernel's code or the
        /// package's refusal is the cause, `blocking_package_id` that package.
        ReturnHomeRefused => "return_home_refused",
        /// S5: nothing reaches the pack's entry pages (`server`, `page_ids`).
        EntryUnreachable => "entry_unreachable",
        /// S7 (fix round 1): `standard-pack check` refuses the game root at this pack, or at no
        /// pack (every pack carries it); its refusal is the cause.
        CheckRefused => "check_refused",
        /// S7 (fix round 1): `standard-pack check` refuses the game root at another pack
        /// (`blocking_package_id`, its refusal the cause) and judges nothing after it.
        RootRefused => "root_refused",
        /// I1: no instance of the installation is bound to the pack's game (`game`).
        InstanceUnbound => "instance_unbound",
        /// I1: the instances bound to the game are bound to other servers (`game`, `server`).
        ServerUnbound => "server_unbound",
        /// I2: the bundle's declared maximum is outside the installation's range
        /// (`required_revision`, `range_min`, `range_max`).
        RevisionOutside => "revision_outside",
        /// I2 (the second merge of main, after Workflow #463 V1): the game root carries what a
        /// later `package` revision brought (the notices directory, `goals.json`, a pack whose
        /// control declaration declares variables) and the bundle declares a lower maximum, so
        /// `place` refuses the bundle (`required_revision` that revision; `place`'s refusal,
        /// `standard_pack_root_invalid` with `notice_undeclared`, `goal_undeclared` or
        /// `variable_undeclared`, is the cause).
        RevisionUndeclared => "revision_undeclared",
        /// I4: a `requires` word of the game's rules is one the live build does not name.
        RequiresUnknown => "requires_unknown",
        /// I5: a pair of the pack is suspended (`task_id`, `instance_id` where told).
        PairSuspended => "pair_suspended",
        /// I6: a model the pack needs is not installed with its identity (`model_name`).
        ModelMissing => "model_missing",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why a readiness item was not judged (the vocabulary `readiness_unjudged`), the value
    /// `readiness_unjudged` of `package_readiness_unjudged`.
    pub enum ReadinessUnjudged: "readiness_unjudged" {
        /// No installation was read: only the seven items of the game root are judged; the
        /// installation's are judged where one is read.
        InstallationNotRead => "installation_not_read",
        /// The installation was read, but not the part this item needs.
        PartNotRead => "part_not_read",
        /// The loader did not admit the pack, so what its content tells is unknown.
        NotAdmitted => "not_admitted",
        /// The composer refused the game's `schedule.json`, so no rule is classified or valued.
        CompositionRefused => "composition_refused",
        /// No rule of `schedule.json` names the pack, so it has no value to judge.
        NoRule => "no_rule",
        /// The installation's reading binds instances to the game without telling their servers.
        ServerNotTold => "server_not_told",
        /// The bundle's declared `package` range was not given (an installed game root keeps no
        /// `interfaces.json`).
        PackageUndeclared => "package_undeclared",
        /// No interface revision tells whether the live Runtime has the code for an unrecorded
        /// screen (the coordinator's ruling 3 on board #464): an information line.
        NoRevisionTells => "no_revision_tells",
        /// S7 (fix round 1): `standard-pack check` stopped at this pack's refusal, answered under
        /// S1 or S4, and judged nothing after it.
        CheckStopped => "check_stopped",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Fix round 1: a pack's answer (the vocabulary `readiness_state`, the member `readiness`).
    pub enum ReadinessState: "readiness_state" {
        /// An item fails, every failing item listed with its problem (an item of the game root
        /// that was not judged comes only with one that fails, and never counts as holding).
        Waiting => "waiting",
        /// Every item of the game root holds and no item of an installation fails, but not every
        /// one was judged (no installation was read, or not all of it): what the game root tells
        /// is complete, and the installation's items are judged where it is placed.
        ReadyToPlace => "ready_to_place",
        /// Every item of the game root and of the installation holds (I3 is information).
        Dispatchable => "dispatchable",
        /// The pack plays a supporting part (`roles`) that no rule dispatches: other packs' runs
        /// use it.
        NotScheduled => "not_scheduled",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Fix round 1: the supporting part a root file gives a pack (the vocabulary
    /// `readiness_role`, the member `roles`), the parts of
    /// `standard_pack_maintenance_packages`.
    pub enum ReadinessRole: "readiness_role" {
        /// A `bundle.json` `maintenance[]` entry's use `startup`.
        Startup => "startup",
        /// A `bundle.json` `maintenance[]` entry's use `return_home`.
        ReturnHome => "return_home",
        /// A `bundle.json` `maintenance[]` entry's use `recognition`.
        Recognition => "recognition",
        /// A `pages.json` feature package.
        Features => "features",
        /// A `navigation.json` edge's package.
        Edge => "edge",
        /// A step of `navigation.json`'s recovery chain.
        Chain => "chain",
        /// An `interrupts.json` handler.
        Handler => "handler",
        /// A package an `interrupts.json` `then.run` names.
        ThenRun => "then_run",
    }
}
// ---- Workflow #464 P3: end ----

// ---- Workflow #463 S3-14 (`contracts/standard-pack-schedule.md`, "Where a pack stands"): begin ----
actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-14: where a pack stands in stage three of package making (the vocabulary
    /// `stage_state`, the member `stage_state` of `standard-pack status`). The train of S3-12 and
    /// S3-14 (ruling R5): it says only what the row knows.
    pub enum StageState: "stage_state" {
        /// Its readiness item S1 does not hold: the loader, or `standard-pack check`, refuses it.
        NotAdmitted => "not_admitted",
        /// The train (R5): admitted, and the composer refuses the root's `schedule.json`
        /// (`enabled.state` `not_judged`): the Runtime keeps the last schedule it accepted, which
        /// the root does not tell.
        NotJudged => "not_judged",
        /// Admitted, and nothing of the root's accepted `schedule.json` enables it: it is held
        /// (`enabled.state` `held`), or no rule names it (`not_placed`).
        CanBeTried => "can_be_tried",
        /// The train (R5): admitted, and it plays a supporting part a root file gives it, used by
        /// other packs' runs, and no rule names it (`enabled.state` `not_scheduled`).
        Supporting => "supporting",
        /// Admitted, and a rule of the root's `schedule.json` enables it (`enabled.state`
        /// `enabled`).
        Enabled => "enabled",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-14: whether the root's `schedule.json` enables a pack, judged from the
    /// classification its readiness item S2 reads (the vocabulary `enabled_state`, the member
    /// `enabled.state` of `standard-pack status`).
    pub enum EnabledState: "enabled_state" {
        /// A rule names it and it is not held: the scheduler dispatches it by that rule.
        Enabled => "enabled",
        /// `schedule.json` holds it, and no root file gives it a supporting part.
        Held => "held",
        /// No rule names it, it is not held and no root file gives it a supporting part, or the
        /// root holds no `schedule.json`: nothing places it into scheduling.
        NotPlaced => "not_placed",
        /// It plays a supporting part a root file gives it and no rule dispatches it (the
        /// readiness's `not_scheduled`).
        NotScheduled => "not_scheduled",
        /// The composer refuses the root's `schedule.json`, so no rule is classified (S2
        /// `composition_refused`); the Runtime keeps the last schedule it accepted, which this root
        /// does not tell.
        NotJudged => "not_judged",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-14: what `standard-pack status` says of the authorization a pack was
    /// enabled under (the vocabulary `pack_authorization`, the member `enabled.authorization`).
    pub enum PackAuthorization: "pack_authorization" {
        /// No placement journal read names the pack: the tool never infers an authorization.
        NotRecorded => "not_recorded",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-14: why no authorization is recorded for a pack (the vocabulary
    /// `authorization_unrecorded`, the member `enabled.authorization_unrecorded`). Workflow #463
    /// S3-15a: one word per cause.
    pub enum AuthorizationUnrecorded: "authorization_unrecorded" {
        /// The placement journals were not read: the row was not asked to read the installation
        /// (`--installation`).
        JournalsNotRead => "journals_not_read",
        /// Workflow #463 S3-15a: every journal of the installation was read and no placement that
        /// counts names the pack in its `enabled` (none that finished with its items placed and
        /// was neither undone nor rolled back).
        NoJournalNamesIt => "no_journal_names_it",
        /// Workflow #463 S3-15a: no journal that was read names the pack, and some journals could
        /// not be read (the root's `installation.operations_unread`): one of them may.
        JournalsUnreadable => "journals_unreadable",
    }
}
// ---- Workflow #463 S3-14: end ----

// ---- Workflow #463 S3-15a (`contracts/standard-pack-schedule.md`, "Where a pack stands"): begin ----
actingcommand_contract::outcome_vocabulary! {
    /// Workflow #463 S3-15a: why `standard-pack status --installation` answers no pairs (the
    /// vocabulary `pairs_unread`: each pack's `pairs` in place of its list, and the root's
    /// `installation.pairs_unread` beside the outcome that says why).
    pub enum PairsUnread: "pairs_unread" {
        /// The installation's `actingd suspended` gave no report: it could not be started, it
        /// failed or printed no report, it read another state root, or it was ended at its
        /// deadline (the outcome a diagnosis gives the same child).
        SuspensionReportNotRead => "suspension_report_not_read",
        /// The report was read, but the composed bindings that name the package each catalog task
        /// runs could not be read (`actingctl packs`' reading of the installation's configuration).
        BindingsNotRead => "bindings_not_read",
    }
}
// ---- Workflow #463 S3-15a: end ----
