// SPDX-License-Identifier: AGPL-3.0-only

//! The ledger's outcome codes (Workflow #378). Every entry is listed in the fragment
//! `contracts/outcome-codes/ledger.json`; the outcome guard (G1) checks both directions.
//! Until slice A2c converts this crate, a code travels as its spelling (`LedgerCode::as_str`)
//! through `GlobalLedgerError`, with no detail.

actingcommand_contract::outcome_codes! {
    /// Codes the ledger registered ahead of the A2c sweep (Workflow #375).
    pub(crate) enum LedgerCode {
        /// Error. A query on a selected ledger read (`GlobalLedger::open_selected`) named no
        /// event type, a type the read did not select, or a view (no keys).
        SelectionQueryUnsupported => "ledger_selection_query_unsupported": error,
        // Workflow #383 AO-0 (ledger observer, `contracts/ledger-observer.md` section "Outcome
        // codes"): registered pending before the slices that emit them (AO-1, AO-2, AO-5, AO-6,
        // AO-8). Nothing emits them yet. Begin.
        /// Warning. A batch was committed after retries (`attempts`).
        WriteRetried => "ledger_write_retried": warning,
        /// Fatal. Every attempt failed or one stalled; the unflushed records went to the dump.
        WriteFailed => "ledger_write_failed": fatal,
        /// Info. A start imported a dump file and deleted it.
        DumpImported => "ledger_dump_imported": info,
        /// Warning. A dump without its trailer ended in a truncated line; gap recorded.
        DumpIncomplete => "ledger_dump_incomplete": warning,
        /// Error. A middle line of a dump failed its hash; prefix imported, file kept.
        DumpInvalid => "ledger_dump_invalid": error,
        /// Fatal. A dump declares a newer `dump_version` than this build reads.
        DumpUnsupported => "ledger_dump_unsupported": fatal,
        /// Fatal. The dump could be written neither in the state root nor in the fallback folder.
        DumpFailed => "ledger_dump_failed": fatal,
        /// Warning. The previous run ended without a close record or a complete dump.
        TailUnconfirmed => "ledger_tail_unconfirmed": warning,
        /// Error. One captured record failed assembly or sanitizing; single-record gap.
        RecordRejected => "ledger_record_rejected": error,
        /// Error. A revisioned source's revisions jumped without a covering gap.
        StoreRevisionSkipped => "ledger_store_revision_skipped": error,
        /// Warning. The capture queue crossed watermark tier 1 (or fell back).
        CaptureBufferBusy => "ledger_capture_buffer_busy": warning,
        /// Warning. The capture queue crossed tier 2 and spills to the dump (or stopped).
        CaptureSpilled => "ledger_capture_spilled": warning,
        /// Warning. The capture queue crossed tier 3; spilling does not keep up.
        CaptureBufferOverloaded => "ledger_capture_buffer_overloaded": warning,
        /// Warning. The oldest unflushed record is older than `flush_lag_warn_ms` (or fell back).
        FlushDelayed => "ledger_flush_delayed": warning,
        /// Error. A store's value differs from the ledger index's latest committed record.
        StoreMismatch => "ledger_store_mismatch": error,
        /// Info. The consistency probe found nothing (status only).
        ProbeDone => "ledger_probe_done": info,
        /// Warning. A workbench key matches neither tag list.
        TagKeyUnclassified => "ledger_tag_key_unclassified": warning,
        /// Info. The first tagged start replayed history once and wrote value records.
        TaggedStateSeeded => "ledger_tagged_state_seeded": info,
        /// Info. The memory stores were loaded from the ledger index.
        TaggedStateLoaded => "ledger_tagged_state_loaded": info,
        /// Info. Tagged entries of instances no longer configured were not loaded.
        TaggedEntriesDropped => "ledger_tagged_entries_dropped": info,
        /// Fatal. An index row disagrees with the record it points to.
        TagIndexMismatch => "ledger_tag_index_mismatch": fatal,
        // Workflow #383 AO-0. End.
    }
}

actingcommand_contract::outcome_locations! {
    /// Locations the ledger registered ahead of the A2c sweep (Workflow #375).
    pub(crate) enum LedgerLocation {
        /// Answering a query on a selected ledger read.
        QuerySelection => "ledger_query_selection",
    }
}
