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
        /// Warning (Workflow #464 P3, `contracts/standard-pack-schedule.md` "Readiness"): a pack of
        /// a game root is not dispatched yet for one readiness item (keys `package_id`,
        /// `readiness_item`, `readiness_problem`, and the item's values; the check's own outcome
        /// as its cause where one exists). `standard-pack ready` answers one per failing item.
        PackageDispatchWaiting => "package_dispatch_waiting": warning / none,
        /// Info (Workflow #464 P3): a readiness item of a pack that was not judged, from what the
        /// readiness was given, or (item `unrecorded_screen_code`) an information line that never
        /// stops a pack (keys `package_id`, `readiness_item`, `readiness_unjudged`).
        PackageReadinessUnjudged => "package_readiness_unjudged": info / none,
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
        /// I2: the installation's `package` range holds the bundle's declared maximum.
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
