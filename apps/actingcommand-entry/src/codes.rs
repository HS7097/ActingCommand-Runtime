// SPDX-License-Identifier: AGPL-3.0-only

//! The fixed entries' outcome codes (`contracts/outcome-codes/actingcommand-entry.json`;
//! `contracts/stable-entry.md`, "Codes"). An entry's own result line is `OUTCOME
//! actingcommand-entry:` on stderr (console) or the one stdout line `{"outcome": {"code",
//! "values"}}` (graphical); its exit code is its code's category: 0 success or info, 1 error,
//! 2 fatal. Every entry is pending; R-15c emits them.

actingcommand_contract::outcome_codes! {
    /// The codes of the fixed entries (Workflow #429 R-15b, since 0.12.35).
    pub enum EntryCode {
        /// The graphical entry's child still runs after 10 seconds, or exited 0 within them.
        Launched => "entry_launched": info / none,
        /// `install\active.json` is missing, unreadable or not install-selection v2.
        SelectionUnavailable => "entry_selection_unavailable": error / none,
        /// An inherited selection differs from `install\active.json`.
        SelectionInheritedStale => "entry_selection_inherited_stale": error / none,
        /// The selected member's manifest does not list the entry's program.
        TargetNotInMember => "entry_target_not_in_member": error / none,
        /// The target's size or SHA-256 differs from its manifest.
        TargetModified => "entry_target_modified": error / none,
        /// The actingd entry meets an unfinished installer operation row 7a'' stands aside for.
        OperationUnfinished => "entry_operation_unfinished": error / none,
        /// The entry lies outside `runtime\` and `ui\` of an A/B root.
        LayoutUnknown => "entry_layout_unknown": error / none,
        /// The target could not be started.
        SpawnFailed => "entry_spawn_failed": fatal / none,
        /// The graphical entry's child exited non-zero within 10 seconds.
        ChildFailedEarly => "entry_child_failed_early": fatal / none,
        /// `--entry-report` printed the identity line.
        ReportDone => "entry_report_done": success / none,
    }
}
