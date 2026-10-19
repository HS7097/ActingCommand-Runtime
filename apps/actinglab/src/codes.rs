// SPDX-License-Identifier: AGPL-3.0-only

//! actinglab's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/actinglab.json`; the outcome guard (G1) checks both directions.

actingcommand_contract::outcome_codes! {
    /// actinglab's registered codes; the #389 U-0 block is registered ahead of the slice that
    /// emits it.
    // ---- #389 U-0 registrations (pending), emitted from U-3
    // (contracts/runtime-interface.md, "Outcome codes"). Nothing emits them yet.
    #[allow(dead_code)]
    pub(crate) enum ActinglabCode {
        /// Error. An offline package-tool command while Lab option one (authoring) is not
        /// installed (keys `operation`, `lab_option`).
        LabCommandOptionDisabled => "lab_command_option_disabled": error / none,
        // ---- end of #389 U-0 registrations
        // ---- #391 MCP-391b (pending)
        /// Error. `observe`'s Min output still exceeds its hard byte limit with every fact row
        /// and every projection entry omitted, so only the identity is left and it does not
        /// fit (keys `actual_bytes`, `limit_bytes`). An unknown page's near-miss candidates are
        /// cut first (marked `candidates_truncated`), so it means the identity itself does not
        /// fit (#411 A9).
        ObserveIdentityLimitExceeded => "observe_identity_limit_exceeded": error / none,
        // ---- end of #391 MCP-391b
        // ---- Workflow #308 SEL-L6a (pending): `actinglab replay --frames`.
        /// Error. The replayed run did not complete: it ended with the outcome the details carry
        /// as `end.outcome` (`offline_fixture_exhausted` when it needed a frame after the last).
        ReplayRunRefused => "replay_run_refused": error / none,
        /// Error. The `--frames` directory or one of its entries could not be listed.
        ReplayFramesUnreadable => "replay_frames_unreadable": error / none,
        /// Error. The `--frames` directory holds an entry that is not a PNG file.
        ReplayFramesInvalid => "replay_frames_invalid": error / none,
        /// Error. The `--facts` file could not be opened or read.
        ReplayFactsUnreadable => "replay_facts_unreadable": error / none,
        /// Error. The `--facts` file is not one complete instance fact answer of a Runtime.
        ReplayFactsInvalid => "replay_facts_invalid": error / none,
        // ---- end of SEL-L6a
        // ---- Workflow #308 SEL-L6b (pending): `actinglab replay --frames --select --scan`.
        /// Error. The replayed scan did not cover its list: the evaluator ended it before its
        /// coverage (the details carry the scan with its decision), or it failed with the
        /// outcome the details carry as `end.outcome` (`selection_scan_replay_frame_missing`
        /// when it needed a frame the sequence does not hold).
        ReplayScanRefused => "replay_scan_refused": error / none,
        /// Error. The `--scan` file could not be opened or read.
        ReplayScanUnreadable => "replay_scan_unreadable": error / none,
        /// Error. The `--scan` file is not one scan declaration of a select step.
        ReplayScanInvalid => "replay_scan_invalid": error / none,
        // ---- end of SEL-L6b
        // ---- Workflow #331 RSU-1 (pending): `actinglab recognition-regression`.
        /// Error. An `--evidence` file could not be opened or read.
        RecognitionRegressionEvidenceUnreadable => "recognition_regression_evidence_unreadable": error / none,
        /// Error. An `--evidence` file is not one page of a run's evidence (`request_id`, and
        /// `steps` with their frame references), or its pages are of different runs, or it names
        /// no frame.
        RecognitionRegressionEvidenceInvalid => "recognition_regression_evidence_invalid": error / none,
        /// Error. The ledger of `--state-root` could not be opened, is incomplete, or the run's
        /// events or an artifact could not be read.
        RecognitionRegressionLedgerUnreadable => "recognition_regression_ledger_unreadable": error / none,
        /// Error. A frame reference of the evidence names no verified capture frame of the run in
        /// the ledger of `--state-root`.
        RecognitionRegressionFrameUnknown => "recognition_regression_frame_unknown": error / none,
        /// Error. A referenced frame's material is not a PNG image, does not read or does not
        /// decode.
        RecognitionRegressionFrameInvalid => "recognition_regression_frame_invalid": error / none,
        // ---- end of RSU-1
        // ---- Workflow #390 TOOL-1 (pending): `actinglab package fmt` and `package edit`.
        /// Error. `package fmt --check` found JSON files of the content directory that are not
        /// in the canonical serialization; nothing was written.
        PackageFmtCheckFailed => "package_fmt_check_failed": error / none,
        /// Error. `package fmt` could not write a reformatted file over the old one.
        PackageFmtWriteFailed => "package_fmt_write_failed": error / none,
        /// Error. The edits of `package edit` are not an edit list, or an edit names a file the
        /// package does not hold or that is not JSON, or its path is not a JSON pointer.
        PackageEditPatchInvalid => "package_edit_patch_invalid": error / none,
        /// Error. An edit's JSON pointer does not name a place its operation can take.
        PackageEditPointerUnresolved => "package_edit_pointer_unresolved": error / none,
        /// Error. The edited package fails structural admission; nothing was written.
        PackageEditAdmissionRefused => "package_edit_admission_refused": error / none,
        /// Error. `package edit` could not write an edited file over the old one.
        PackageEditWriteFailed => "package_edit_write_failed": error / none,
        // ---- end of TOOL-1
        // ---- Workflow #410 item 1 (pending): observe and do say what their frame is.
        /// Warning. `observe --preview` or a performed `do` could not export a frame: reading the
        /// Runtime's verified frame, encoding its JPEG preview or writing the frame or the
        /// preview into the Lab frame directory failed. The answer still succeeds and carries,
        /// in place of the part not written, `failure {code, path?, raw_text?}` (keys `path`,
        /// `raw_text`).
        LabFrameExportFailed => "lab_frame_export_failed": warning / none,
        // ---- end of #410 item 1
        // ---- Workflow #390 nine6 (pending): `resource validate` reads a pools document.
        /// Warning. A `ledger_fact` pool of a validated pools document that nothing in the
        /// repository publishes: no reading mark (its `fact_key` or `capacity_fact_key`) and no
        /// adjustment of an operation writes the pool's key, or the pool is not of instance
        /// scope, which is the only scope a reading or an adjustment writes. The validation
        /// still succeeds (keys `path`, `json_pointer`, `fact_key`).
        ResourcePoolProducerMissing => "resource_pool_producer_missing": warning / none,
        // ---- end of nine6
        // ---- #411 RV-1 (pending): `actinglab resource verify` (contracts/resource-verify.md).
        /// Success. Every step of `resource verify` passed (keys `step_count`).
        ResourceVerifyPassed => "resource_verify_passed": success / none,
        /// Error. One or more steps failed; the causes are each failed step's first failure,
        /// with `step` (keys `failed_step_count`).
        ResourceVerifyFailed => "resource_verify_failed": error / none,
        /// Error. `--work`, `--report` or `<report>.part` exists, or its parent does not; nothing
        /// was read or written (keys `path`).
        ResourceVerifyOutputExists => "resource_verify_output_exists": error / none,
        /// Error. git could not start or exited with a code that is no answer; the cause is the
        /// OS failure or git's stderr as `raw_text` (keys `git_command`, `exit_code`, `step`).
        ResourceVerifyGitFailed => "resource_verify_git_failed": error / none,
        /// Error. `--repo` is not the top of a work tree, HEAD is no commit, or `--commit` is not
        /// HEAD (keys `repo`, `checkout_problem`, `step`).
        ResourceVerifyCheckoutInvalid => "resource_verify_checkout_invalid": error / none,
        /// Error. The checkout is not the commit's clean tree (keys `path_count`, `first_path`,
        /// `dirty_problem`, `step`).
        ResourceVerifyCheckoutDirty => "resource_verify_checkout_dirty": error / none,
        /// Error. `--base` names no commit, or one that is not an ancestor of HEAD (keys `base`,
        /// `base_problem`, `step`).
        ResourceVerifyBaseInvalid => "resource_verify_base_invalid": error / none,
        /// Error. The `BUILD-MANIFEST.json` beside the executable names other bytes or another
        /// commit, or does not read (keys `manifest_path`, `tools_problem`, `step`).
        ResourceVerifyToolsMismatch => "resource_verify_tools_mismatch": error / none,
        /// Error. The repository's layout or a pack's identity breaks a rule of the bundle
        /// source (keys `path`, `layout_problem`, `step`).
        ResourceVerifyLayoutInvalid => "resource_verify_layout_invalid": error / none,
        /// Error. A tracked pack or procedure-manifest JSON carries a pin member (keys `path`,
        /// `json_pointer`, `member`, `step`).
        ResourceVerifyPinCommitted => "resource_verify_pin_committed": error / none,
        /// Error. The bundle `package bundle` wrote is not the source it was given (keys `path`,
        /// `bundle_problem`, `step`).
        ResourceVerifyBundleMismatch => "resource_verify_bundle_mismatch": error / none,
        /// Error. The zip does not read back as written (keys `entry`, `readback_problem`,
        /// `step`).
        ResourceVerifyReadbackMismatch => "resource_verify_readback_mismatch": error / none,
        /// Error. The report could not be written; stdout still carries it (keys `path`).
        ResourceVerifyReportWriteFailed => "resource_verify_report_write_failed": error / none,
        /// Error. `resource check-release` is retired: it answered `checked` without checking.
        /// `resource verify` checks a resource repository's commit.
        ResourceCheckReleaseRetired => "resource_check_release_retired": error / none,
        // ---- end of #411 RV-1
    }
}

// ---- #411 RV-1: the vocabularies of `resource verify`'s values (contracts/resource-verify.md).
actingcommand_contract::outcome_vocabulary! {
    /// The steps of `resource verify`, in the order they run (vocabulary
    /// `resource_verify_step`).
    pub(crate) enum VerifyStep: "resource_verify_step" {
        /// The build that runs: its head and its executable's bytes.
        Tools => "tools",
        /// The precondition: the commit's clean tree.
        Checkout => "checkout",
        /// The declaration selection, read by `resource validate`'s reader.
        Declarations => "declarations",
        /// The committed pin members.
        Pins => "pins",
        /// Every pack admitted as `package preflight` admits it, and its identity.
        Admission => "admission",
        /// `package fmt --check` of every pack.
        Format => "format",
        /// The bundle, its zip, its read-back and its installed root.
        Bundle => "bundle",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// The read-only git command that failed (vocabulary `git_command`).
    pub(crate) enum GitCommand: "git_command" {
        RevParse => "rev_parse",
        MergeBase => "merge_base",
        Status => "status",
        LsFiles => "ls_files",
        HashObject => "hash_object",
        Diff => "diff",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why `--repo` is no checkout of the commit (vocabulary `checkout_problem`).
    pub(crate) enum CheckoutProblem: "checkout_problem" {
        NotTopLevel => "not_top_level",
        HeadNotCommit => "head_not_commit",
        CommitMismatch => "commit_mismatch",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// How the checkout differs from the commit's tree (vocabulary `dirty_problem`).
    pub(crate) enum DirtyProblem: "dirty_problem" {
        TrackedChange => "tracked_change",
        Untracked => "untracked",
        IgnoredInReadPath => "ignored_in_read_path",
        BlobMismatch => "blob_mismatch",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why `--base` is refused (vocabulary `base_problem`).
    pub(crate) enum BaseProblem: "base_problem" {
        Unresolved => "unresolved",
        NotAncestor => "not_ancestor",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// What the build manifest beside the executable does not confirm (vocabulary
    /// `tools_problem`).
    pub(crate) enum ToolsProblem: "tools_problem" {
        ExecutableSha256 => "executable_sha256",
        Commit => "commit",
        ManifestInvalid => "manifest_invalid",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Which rule of the bundle source the repository breaks (vocabulary `layout_problem`).
    pub(crate) enum LayoutProblem: "layout_problem" {
        ApplicationsMissing => "applications_missing",
        MaintenanceMissing => "maintenance_missing",
        PacksMissing => "packs_missing",
        PacksEntryInvalid => "packs_entry_invalid",
        PackNameMismatch => "pack_name_mismatch",
        PackGameMismatch => "pack_game_mismatch",
        PackServerUndeclared => "pack_server_undeclared",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// How the written bundle differs from its source (vocabulary `bundle_problem`).
    pub(crate) enum BundleProblem: "bundle_problem" {
        IndexSchema => "index_schema",
        MaintenanceDiffers => "maintenance_differs",
        ApplicationsDiffers => "applications_differs",
        SourceDiffers => "source_differs",
        PackListDiffers => "pack_list_differs",
        PackCopyDiffers => "pack_copy_differs",
        PackCountsDiffer => "pack_counts_differ",
        DefaultPackageUnmatched => "default_package_unmatched",
        RootFileDiffers => "root_file_differs",
        RootFilesUnread => "root_files_unread",
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// How the zip reads back (vocabulary `readback_problem`).
    pub(crate) enum ReadbackProblem: "readback_problem" {
        EntrySetDiffers => "entry_set_differs",
        BytesDiffer => "bytes_differ",
        InterfacesDiffers => "interfaces_differs",
    }
}
// ---- end of #411 RV-1
