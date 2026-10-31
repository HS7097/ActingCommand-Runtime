// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow 114-A (register `contracts/v012-register.md` §1.236, since 0.12.35): the eleven
//! closed vocabularies of the read `ScheduleHistory` (design `rd-1031/design-114-reasons-v5.md`
//! §7.2 and §11.3, with its freeze addendum). They are frozen here, before the read: 114-D adds
//! the request, the answer, the segments, the cursor, the constants and the codes with
//! `runtime-client` revision 5, and uses these words without adding one; a new word is a new
//! `runtime-client` revision. The answer's `head.retired.reason` uses the ledger's own
//! `NotRunRetiredReason` (three words).

use serde::{Deserialize, Serialize};

/// A closed wire vocabulary: a plain serde enum whose every word is spelled once, with its
/// frozen order (`ALL`).
macro_rules! schedule_history_vocabulary {
    (
        $(#[$meta:meta])*
        $name:ident : $vocabulary:literal {
            $( $(#[$variant_meta:meta])* $variant:ident => $word:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$variant_meta])* #[serde(rename = $word)] $variant ),+
        }

        impl $name {
            /// The vocabulary's name in the design and the contract.
            pub const VOCABULARY: &'static str = $vocabulary;
            /// Every word, in its frozen order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The word's spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word),+
                }
            }
        }
    };
}

schedule_history_vocabulary! {
    /// What a segment is a timeline of: the alias's coverage, its control block, its
    /// not-running row, or one task.
    ScheduleHistoryScope: "schedule_history_scope" {
        Coverage => "coverage",
        InstanceControl => "instance_control",
        InstanceSummary => "instance_summary",
        Task => "task",
    }
}

schedule_history_vocabulary! {
    /// What a task segment holds.
    ScheduleHistoryTaskKind: "schedule_history_task_kind" {
        Row => "row",
        Selected => "selected",
        TaskAbsent => "task_absent",
        CatalogChanged => "catalog_changed",
        Retired => "retired",
    }
}

schedule_history_vocabulary! {
    /// How much a segment knows. `turn` is ruling 118 A's waiting-for-a-turn class (the freeze
    /// addendum, minor 11: the design's `class`, renamed so it does not share the segment
    /// member's name).
    ScheduleHistoryPrecision: "schedule_history_precision" {
        Exact => "exact",
        Turn => "turn",
        Merged => "merged",
        TailUnconfirmed => "tail_unconfirmed",
        NotRunning => "not_running",
        NotEvaluated => "not_evaluated",
        Unknown => "unknown",
        Inconsistent => "inconsistent",
    }
}

schedule_history_vocabulary! {
    /// Why a coverage segment has no ground.
    ScheduleHistoryUnknownBecause: "schedule_history_unknown_because" {
        NotStarted => "not_started",
        EpochUnrecorded => "epoch_unrecorded",
        Unregistered => "unregistered",
        BadBaseline => "bad_baseline",
        ReadLimit => "read_limit",
        RecorderFaulted => "recorder_faulted",
    }
}

schedule_history_vocabulary! {
    /// Which records do not agree.
    ScheduleHistoryInconsistentKind: "schedule_history_inconsistent_kind" {
        SeqGap => "seq_gap",
        SeqDuplicate => "seq_duplicate",
        PrevMismatch => "prev_mismatch",
        FromMismatch => "from_mismatch",
        ControlMismatch => "control_mismatch",
        LedgerGap => "ledger_gap",
    }
}

schedule_history_vocabulary! {
    /// Why an answer stops before its end.
    ScheduleHistoryIncompleteBecause: "schedule_history_incomplete_because" {
        Page => "page",
        SegmentCap => "segment_cap",
        Frame => "frame",
        ReadBudget => "read_budget",
    }
}

schedule_history_vocabulary! {
    /// Where a selected segment's dispatch stands: `ScheduleExplanation`'s five words, and
    /// `unknown` (the epoch ended before the result) and `beyond_window` (the result is past
    /// the asked window).
    ScheduleHistoryDispatch: "schedule_history_dispatch" {
        Pending => "pending",
        Intent => "intent",
        Admitted => "admitted",
        Rejected => "rejected",
        Completed => "completed",
        Unknown => "unknown",
        BeyondWindow => "beyond_window",
    }
}

schedule_history_vocabulary! {
    /// Where the alias's stream stands now. `retired`: the alias is gone; `recorder_faulted`: it
    /// stopped writing explanations in this owner epoch, its scheduling goes on.
    ScheduleHistoryHeadState: "schedule_history_head_state" {
        Exact => "exact",
        Merging => "merging",
        NotEvaluated => "not_evaluated",
        Unregistered => "unregistered",
        Retired => "retired",
        RecorderFaulted => "recorder_faulted",
    }
}

schedule_history_vocabulary! {
    /// How the Runtime stopped before a not-running segment.
    ScheduleHistoryClose: "schedule_history_close" {
        Clean => "clean",
        Fatal => "fatal",
        Unconfirmed => "unconfirmed",
    }
}

schedule_history_vocabulary! {
    /// Why a segment's alias is gone: never `recorder_faulted`, which only a coverage segment
    /// `unknown{recorder_faulted}` and `head.retired` say.
    ScheduleHistoryRetiredReason: "schedule_history_retired_reason" {
        Unconfigured => "unconfigured",
        Unregistered => "unregistered",
    }
}

schedule_history_vocabulary! {
    /// How a segment came about, the last part of its `segment_id`
    /// (`<scope>:<task_id or ->:<anchor>:<role>`).
    ScheduleHistorySegmentRole: "schedule_history_segment_role" {
        Record => "record",
        MergeWait => "merge_wait",
        NotEvaluated => "not_evaluated",
        NotRunning => "not_running",
        Tail => "tail",
        EpochUnrecorded => "epoch_unrecorded",
        NotStarted => "not_started",
        Unregistered => "unregistered",
        BadBaseline => "bad_baseline",
        ReadLimit => "read_limit",
        LedgerGap => "ledger_gap",
    }
}
