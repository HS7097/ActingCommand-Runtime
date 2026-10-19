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
    }
}
