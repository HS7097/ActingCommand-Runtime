// SPDX-License-Identifier: AGPL-3.0-only

//! The bench's shared vocabulary: paths, owners, dispositions, gates, results and exit codes
//! (plan v2.1 §3.3-§3.11, §5.4, §5.7).

use serde::{Deserialize, Serialize};
use std::fmt;

/// The three paths (§0): `nemu` (N), `scrcpy` (S) and `b_model`, the local pipeline cost model
/// on the Nemu backend (§5.9).
/// The names are the analysis schema's (`rd-1031/exp402/analysis/schema.md` §3): `N`, `S`,
/// `B_model`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PathKind {
    #[serde(rename = "N")]
    Nemu,
    #[serde(rename = "S")]
    Scrcpy,
    #[serde(rename = "B_model")]
    BModel,
}

impl PathKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nemu => "N",
            Self::Scrcpy => "S",
            Self::BModel => "B_model",
        }
    }

    /// The backend the path samples through; `b_model` shares the Nemu backend (§3.11.4 table A).
    pub fn backend(self) -> BackendKind {
        match self {
            Self::Nemu | Self::BModel => BackendKind::Nemu,
            Self::Scrcpy => BackendKind::Scrcpy,
        }
    }
}

impl fmt::Display for PathKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The two device backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BackendKind {
    #[serde(rename = "N")]
    Nemu,
    #[serde(rename = "S")]
    Scrcpy,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Nemu => "N",
            Self::Scrcpy => "S",
        }
    }

    /// The paths that stop with this backend (§3.11.4 "共用同一后端的路径").
    pub fn paths(self) -> &'static [PathKind] {
        match self {
            Self::Nemu => &[PathKind::Nemu, PathKind::BModel],
            Self::Scrcpy => &[PathKind::Scrcpy],
        }
    }
}

/// Who a native call serves (§3.3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOwner {
    Formal,
    Preview,
    Manual,
    Reopen,
}

/// The device entries of §3.7.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    /// E1: a preview sample (an N native call; publishing the S slot).
    PreviewSample,
    /// E2: the S producer (server, forward, reader, decoder).
    ScrcpyProducer,
    /// E3: the Nemu session the preview shares with formal work.
    NemuSession,
    /// E4: a manual refresh of one frame.
    ManualRefresh,
    /// E5: calibration (time-service round trips, periodic recalibration).
    Calibration,
    /// E6: test-pattern APK control (mode switch, pushed pictures, rotation requests).
    ApkControl,
    /// E7: the simulated formal loop (simulated policy dispatch).
    FormalLoop,
    /// E8: automatic reopen (S cold reopen, N reconnect).
    AutoReopen,
    /// E9: the device-side check (§3.5.1 identity queries, `forward --list`).
    DeviceSideCheck,
}

impl Entry {
    pub const ALL: [Entry; 9] = [
        Entry::PreviewSample,
        Entry::ScrcpyProducer,
        Entry::NemuSession,
        Entry::ManualRefresh,
        Entry::Calibration,
        Entry::ApkControl,
        Entry::FormalLoop,
        Entry::AutoReopen,
        Entry::DeviceSideCheck,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreviewSample => "e1_preview_sample",
            Self::ScrcpyProducer => "e2_scrcpy_producer",
            Self::NemuSession => "e3_nemu_session",
            Self::ManualRefresh => "e4_manual_refresh",
            Self::Calibration => "e5_calibration",
            Self::ApkControl => "e6_apk_control",
            Self::FormalLoop => "e7_formal_loop",
            Self::AutoReopen => "e8_auto_reopen",
            Self::DeviceSideCheck => "e9_device_side_check",
        }
    }
}

/// The gate columns of §3.7.1. `StopClass` stands for STOP, ABORT, the global deadline, the
/// budget triggers and the bench's own working-set cap (ruling 141 removed the host memory
/// gates); the cell-end gate of §3.11.2 A acts as it within its cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GateKind {
    /// 静默 (`instance.<id>.scheduling.blackout`).
    #[serde(rename = "blackout")]
    Blackout,
    /// #403 resource fence.
    #[serde(rename = "fence")]
    Fence,
    /// Hold request → drain → hand back.
    #[serde(rename = "hold_request")]
    HoldRequest,
    /// Held.
    #[serde(rename = "held")]
    Holding,
    /// STOP class, budget, working set.
    #[serde(rename = "stop")]
    StopClass,
    /// Ordinary scheduling pause (the 116 kind).
    #[serde(rename = "pause")]
    RoutinePause,
}

impl GateKind {
    pub const ALL: [GateKind; 6] = [
        GateKind::Blackout,
        GateKind::Fence,
        GateKind::HoldRequest,
        GateKind::Holding,
        GateKind::StopClass,
        GateKind::RoutinePause,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blackout => "blackout",
            Self::Fence => "fence",
            Self::HoldRequest => "hold_request",
            Self::Holding => "held",
            Self::StopClass => "stop",
            Self::RoutinePause => "pause",
        }
    }

    /// A hard gate never waits for the S soft-unsubscribe grace (§3.4).
    pub fn is_hard(self) -> bool {
        !matches!(self, Self::RoutinePause)
    }
}

/// Where a global gate comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalGate {
    /// The whole-run stop gate (§3.11.2 B).
    RunStop,
    /// The cell-end gate (§3.11.2 A).
    CellEnd,
}

/// The disposition of one opportunity (§5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "disposition", content = "detail")]
pub enum Disposition {
    Delivered,
    SkippedBusy,
    SkippedDemand,
    SkippedBudget,
    SkippedNoLease,
    SkippedGate(String),
    Failed(String),
    DiscardedStale(String),
}

impl Disposition {
    pub fn label(&self) -> String {
        match self {
            Self::Delivered => "delivered".to_owned(),
            Self::SkippedBusy => "skipped_busy".to_owned(),
            Self::SkippedDemand => "skipped_demand".to_owned(),
            Self::SkippedBudget => "skipped_budget".to_owned(),
            Self::SkippedNoLease => "skipped_no_lease".to_owned(),
            Self::SkippedGate(gate) => format!("skipped_gate:{gate}"),
            Self::Failed(code) => format!("failed:{code}"),
            Self::DiscardedStale(reason) => format!("discarded_stale:{reason}"),
        }
    }

    pub fn is_delivered(&self) -> bool {
        matches!(self, Self::Delivered)
    }
}

/// Why the whole run stops (§3.11.2 B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "cause", content = "detail")]
pub enum StopCause {
    StopFile,
    AbortFile,
    Deadline,
    TotalDuration,
    WorkingSetCap { bytes: u64 },
    DiskLow { bytes: u64 },
    Unconfirmed(String),
    Poisoned(String),
    StdioUnrestored(String),
    SafetyBoundary(String),
    InternalInvariant(String),
}

impl StopCause {
    pub fn label(&self) -> &'static str {
        match self {
            Self::StopFile => "stop",
            Self::AbortFile => "abort",
            Self::Deadline => "deadline",
            Self::TotalDuration => "total_duration",
            Self::WorkingSetCap { .. } => "working_set_cap",
            Self::DiskLow { .. } => "disk_low",
            Self::Unconfirmed(_) => "unconfirmed",
            Self::Poisoned(_) => "poisoned",
            Self::StdioUnrestored(_) => "stdio_unrestored",
            Self::SafetyBoundary(_) => "safety_boundary",
            Self::InternalInvariant(_) => "internal_invariant",
        }
    }

    /// The run result this cause implies on its own (§3.11.3).
    pub fn result(&self) -> RunResult {
        match self {
            Self::StopFile
            | Self::Deadline
            | Self::TotalDuration
            | Self::WorkingSetCap { .. }
            | Self::DiskLow { .. } => RunResult::Incomplete,
            Self::AbortFile => RunResult::Aborted,
            Self::Unconfirmed(_) | Self::Poisoned(_) => RunResult::Unconfirmed,
            Self::StdioUnrestored(_) => RunResult::StdioUnrestored,
            Self::SafetyBoundary(_) => RunResult::Failed,
            Self::InternalInvariant(_) => RunResult::InternalInvariant,
        }
    }
}

/// §3.11.3: the run's results and their exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Completed,
    Failed,
    Incomplete,
    Aborted,
    RefusedStart,
    Unconfirmed,
    StdioUnrestored,
    InternalInvariant,
}

impl RunResult {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Completed => 0,
            Self::Failed => 10,
            Self::Incomplete => 11,
            Self::Aborted => 12,
            Self::RefusedStart => 13,
            Self::Unconfirmed => 20,
            Self::StdioUnrestored => 21,
            Self::InternalInvariant => 30,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Incomplete => "incomplete",
            Self::Aborted => "aborted",
            Self::RefusedStart => "refused_start",
            Self::Unconfirmed => "unconfirmed",
            Self::StdioUnrestored => "stdio_unrestored",
            Self::InternalInvariant => "internal_invariant",
        }
    }

    /// §3.11.3: when several apply, the exit code is the highest of 21 > 20 > 30 > 10 > 12 > 11
    /// > 0 (refusing to start touches no device and stands alone).
    fn priority(self) -> u8 {
        match self {
            Self::StdioUnrestored => 7,
            Self::Unconfirmed => 6,
            Self::InternalInvariant => 5,
            Self::Failed => 4,
            Self::Aborted => 3,
            Self::Incomplete => 2,
            Self::Completed => 1,
            Self::RefusedStart => 8,
        }
    }

    /// The result that decides the exit code among `results`; `completed` without any.
    pub fn decisive(results: &[RunResult]) -> RunResult {
        results
            .iter()
            .copied()
            .max_by_key(|result| result.priority())
            .unwrap_or(RunResult::Completed)
    }
}

/// §3.11.2 A: a (cell, instance, path) result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result", content = "reason")]
pub enum CellResult {
    CellCompleted,
    CellIncomplete(String),
    CellFailed(String),
    /// Not run, with the reason (未测).
    Untested(String),
    /// A conditional skip planned for, with the reason (条件跳过).
    ConditionalSkip(String),
}

impl CellResult {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CellCompleted => "cell_completed",
            Self::CellIncomplete(_) => "cell_incomplete",
            Self::CellFailed(_) => "cell_failed",
            Self::Untested(_) => "untested",
            Self::ConditionalSkip(_) => "conditional_skip",
        }
    }

    /// The run result this cell result contributes (§3.11.3).
    pub fn run_result(&self) -> RunResult {
        match self {
            Self::CellCompleted | Self::ConditionalSkip(_) => RunResult::Completed,
            Self::CellIncomplete(_) | Self::Untested(_) => RunResult::Incomplete,
            Self::CellFailed(_) => RunResult::Failed,
        }
    }
}

/// A resource close's result: confirmed, or unconfirmed with why (§3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "close", content = "detail")]
pub enum CloseOutcome {
    Confirmed,
    Unconfirmed(String),
}

impl CloseOutcome {
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed)
    }
}
