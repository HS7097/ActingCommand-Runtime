// SPDX-License-Identifier: AGPL-3.0-only

//! §3.7.1: the device entry × gate table, as one table that every entry consults under the
//! scheduler lock.

use crate::model::{Entry, GateKind};
use serde::Serialize;

/// One cell of §3.7.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reaction {
    /// 放行.
    Allow,
    /// 不影响 (the entry does not look at this gate).
    NoEffect,
    /// 拒: no call; `skipped_gate:<gate>`.
    Deny,
    /// 拒；撤下画面: as `Deny`, and the viewers get `withdrawn`.
    DenyWithdraw,
    /// 拒新的: no new work; work in progress is not drained.
    DenyNew,
    /// 拒新的；在跑的排空.
    DenyNewDrain,
    /// 关: deny, drain the in-flight work, close the backend and confirm it (§3.5).
    Close,
    /// 不关: the session of the formal owner stays open (E3 under the blackout).
    Keep,
    /// 不存在: the resource does not exist while the gate holds.
    NotPresent,
    /// 只在关闭链里放行.
    CloseChainOnly,
}

impl Reaction {
    /// Whether an attempt at the entry may go ahead under this gate.
    pub fn admits(self, in_close_chain: bool) -> bool {
        match self {
            Self::Allow | Self::NoEffect | Self::Keep => true,
            Self::CloseChainOnly => in_close_chain,
            Self::Deny
            | Self::DenyWithdraw
            | Self::DenyNew
            | Self::DenyNewDrain
            | Self::Close
            | Self::NotPresent => false,
        }
    }

    /// Whether the gate closes the entry's resource.
    pub fn closes(self) -> bool {
        matches!(self, Self::Close)
    }

    /// Whether the gate withdraws the preview picture.
    pub fn withdraws(self) -> bool {
        matches!(self, Self::DenyWithdraw)
    }

    /// Whether running work is drained before the gate counts as in force.
    pub fn drains(self) -> bool {
        matches!(self, Self::DenyNewDrain | Self::Close)
    }
}

/// The reaction of `entry` to `gate` (§3.7.1, row by row).
pub const fn reaction(entry: Entry, gate: GateKind) -> Reaction {
    use GateKind::*;
    use Reaction::*;
    match entry {
        Entry::PreviewSample => match gate {
            Blackout | Fence | HoldRequest => DenyWithdraw,
            Holding | StopClass => Deny,
            RoutinePause => NoEffect,
        },
        Entry::ScrcpyProducer => match gate {
            Blackout | Fence | HoldRequest | StopClass => Close,
            Holding => NotPresent,
            RoutinePause => NoEffect,
        },
        Entry::NemuSession => match gate {
            Blackout => Keep,
            Fence | HoldRequest | StopClass => Close,
            Holding => NotPresent,
            RoutinePause => NoEffect,
        },
        Entry::ManualRefresh => match gate {
            Blackout | RoutinePause => Allow,
            Fence | HoldRequest | Holding | StopClass => Deny,
        },
        Entry::Calibration | Entry::ApkControl | Entry::AutoReopen => match gate {
            Blackout | Fence | HoldRequest | Holding | StopClass => Deny,
            RoutinePause => Allow,
        },
        Entry::FormalLoop => match gate {
            Blackout | Fence | Holding => Deny,
            HoldRequest | StopClass => DenyNewDrain,
            RoutinePause => DenyNew,
        },
        Entry::DeviceSideCheck => match gate {
            Blackout | RoutinePause => Allow,
            Fence | HoldRequest | StopClass => CloseChainOnly,
            Holding => Deny,
        },
    }
}

/// The first gate among `gates` that refuses `entry`, or `None` when the entry may go ahead.
pub fn refusing_gate(
    entry: Entry,
    gates: impl IntoIterator<Item = GateKind>,
    in_close_chain: bool,
) -> Option<GateKind> {
    gates
        .into_iter()
        .find(|gate| !reaction(entry, *gate).admits(in_close_chain))
}
