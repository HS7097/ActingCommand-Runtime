// SPDX-License-Identifier: AGPL-3.0-only

//! The tick's decision table (`contracts/runtime-watchdog.md`, "Decision"), as pure functions
//! of what one tick observed and of the watchdog state. First match wins.

use super::state::{StartRecord, WatchdogState};

/// A start record counts against the budget for this long.
pub(crate) const BUDGET_WINDOW_MS: u64 = 30 * 60 * 1000;
/// At most this many starts inside the budget window.
pub(crate) const BUDGET_STARTS: usize = 3;
/// `status` reports repeated restarts from this many watchdog starts within a day.
pub(crate) const REPEATED_RESTARTS: usize = 2;
pub(crate) const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// A task tick more than this after the end of the previous one (one and a half tick
/// periods: a logon, a reboot, a resume) opens the grace. A scheduled reboot leaves gaps of
/// about 100-180 s, so a tick from before the current boot opens it as well.
pub(crate) const GAP_MS: u64 = 90 * 1000;
/// The grace leaves a logon starter (a logon script, acsetup's autostart) time to start the
/// Runtime itself, before the watchdog considers a start.
pub(crate) const GRACE_MS: u64 = 300 * 1000;
/// A Runtime process under the root without the owner lock for this long is an error, and so
/// is (in `status`) an owner lock held this long without an answering owner.
pub(crate) const PROCESS_WITHOUT_OWNER_MS: u64 = 10 * 60 * 1000;
/// A start that has not answered within this window is a start timeout.
pub(crate) const READY_TIMEOUT_MS: u64 = 180 * 1000;
/// Clock slack between a spawn and the started owner's recorded start.
pub(crate) const CLOCK_SLACK_MS: u64 = 2 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerLock {
    Locked,
    Unlocked,
    Missing,
}

impl OwnerLock {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Unlocked => "unlocked",
            Self::Missing => "missing",
        }
    }
}

/// The last owner record of the journal (a v1 or v2 record, or a checkpoint's `last_record`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerRecord {
    pub(crate) schema_version: String,
    pub(crate) owner_epoch: String,
    pub(crate) pid: u32,
    pub(crate) started_at_unix_ms: u64,
    pub(crate) active: bool,
    pub(crate) closed_at_unix_ms: Option<u64>,
    pub(crate) resource_disposition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Journal {
    /// No journal, an empty one, or no complete line yet.
    Absent,
    Record {
        record: OwnerRecord,
        modified_unix_ms: u64,
    },
    Unrecognised {
        detail: String,
    },
}

/// The owner that answered a health request through runtime-info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveOwner {
    pub(crate) pid: u32,
    pub(crate) owner_epoch: String,
    pub(crate) started_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fatal {
    pub(crate) log: String,
    pub(crate) line: String,
}

/// Rows 2-6: what the tick observed before any start is considered.
pub(crate) struct Observed {
    pub(crate) lock: OwnerLock,
    /// Only with `lock == Locked`: the owner answered, or why it did not.
    pub(crate) live: Option<Result<LiveOwner, String>>,
    pub(crate) journal: Journal,
    pub(crate) fatal: Option<Fatal>,
}

/// Rows 7a-7b: the probes taken only when a start is considered.
pub(crate) struct StartProbes {
    pub(crate) writer_busy: bool,
    pub(crate) selection_changed: bool,
    /// Runtime processes whose executable is under the root, or whose path is unknown.
    pub(crate) processes: Vec<RuntimeProcess>,
    /// When the machine last booted, read in the same probe as the processes.
    pub(crate) boot_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeProcess {
    pub(crate) pid: u32,
    pub(crate) path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    RunInProgress,
    Misconfigured {
        code: String,
        detail: String,
    },
    Alive {
        owner: LiveOwner,
        started_by_watchdog: bool,
    },
    OwnerLockHeld {
        detail: String,
    },
    NeverStarted,
    OwnerRetainedUnconfirmed {
        owner_epoch: String,
    },
    FatalHold(Fatal),
    FormalClose {
        owner_epoch: String,
    },
    InstallerBusy,
    SelectionChanged,
    RuntimeProcessPresent {
        processes: Vec<RuntimeProcess>,
    },
    RuntimeProcessWithoutOwner {
        processes: Vec<RuntimeProcess>,
    },
    BudgetExhausted {
        since_unix_ms: u64,
    },
    Grace {
        until_unix_ms: u64,
    },
    /// Row 7f before the start ran; `status` reports it as `start_due`.
    Start,
    Started {
        method: String,
        owner: LiveOwner,
        log: String,
    },
    StartFailed {
        code: String,
        detail: String,
    },
}

/// The first stage: rows 2-6, or a start is to be considered.
pub(crate) enum Stage {
    Decided(Decision),
    ConsiderStart,
}

impl Decision {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::RunInProgress => "run_in_progress",
            Self::Misconfigured { .. } => "misconfigured",
            Self::Alive { .. } => "alive",
            Self::OwnerLockHeld { .. } => "owner_lock_held",
            Self::NeverStarted => "never_started",
            Self::OwnerRetainedUnconfirmed { .. } => "owner_retained_unconfirmed",
            Self::FatalHold(_) => "fatal_hold",
            Self::FormalClose { .. } => "formal_close",
            Self::InstallerBusy => "installer_busy",
            Self::SelectionChanged => "selection_changed",
            Self::RuntimeProcessPresent { .. } => "runtime_process_present",
            Self::RuntimeProcessWithoutOwner { .. } => "runtime_process_without_owner",
            Self::BudgetExhausted { .. } => "budget_exhausted",
            Self::Grace { .. } => "grace",
            Self::Start => "start_due",
            Self::Started { .. } => "started",
            Self::StartFailed { .. } => "start_failed",
        }
    }

    /// The exit code of `run-once` (`status` adds its attention codes on top).
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::OwnerRetainedUnconfirmed { .. } | Self::FatalHold(_) => 10,
            Self::BudgetExhausted { .. } => 11,
            Self::Start | Self::StartFailed { .. } => 12,
            Self::Misconfigured { .. } => 13,
            Self::RuntimeProcessWithoutOwner { .. } => 15,
            _ => 0,
        }
    }

    /// Log on change: a tick writes a line only when this key differs from the last one.
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Misconfigured { code, .. } => format!("misconfigured:{code}"),
            Self::Alive { owner, .. } => format!("alive:{}", owner.owner_epoch),
            Self::OwnerRetainedUnconfirmed { owner_epoch } => {
                format!("owner_retained_unconfirmed:{owner_epoch}")
            }
            Self::FatalHold(fatal) => format!("fatal_hold:{}", fatal.log),
            Self::FormalClose { owner_epoch } => format!("formal_close:{owner_epoch}"),
            Self::StartFailed { code, .. } => format!("start_failed:{code}"),
            other => other.name().to_owned(),
        }
    }
}

/// Rows 2-6 (row 1, misconfiguration, is decided while observing).
pub(crate) fn decide_observed(observed: &Observed, state: &WatchdogState) -> Stage {
    if observed.lock == OwnerLock::Locked {
        return Stage::Decided(match &observed.live {
            Some(Ok(owner)) => Decision::Alive {
                started_by_watchdog: started_by_watchdog(owner, &state.starts),
                owner: owner.clone(),
            },
            Some(Err(detail)) => Decision::OwnerLockHeld {
                detail: detail.clone(),
            },
            None => Decision::OwnerLockHeld {
                detail: "runtime_info_unavailable".to_owned(),
            },
        });
    }
    let record = match (&observed.lock, &observed.journal) {
        (OwnerLock::Missing, _) | (_, Journal::Absent) => {
            return Stage::Decided(Decision::NeverStarted);
        }
        (_, Journal::Unrecognised { detail }) => {
            return Stage::Decided(Decision::Misconfigured {
                code: "watchdog_journal_unrecognised".to_owned(),
                detail: detail.clone(),
            });
        }
        (_, Journal::Record { record, .. }) => record,
    };
    // Review M2 (i): a retained owner fails every start until `actingd unlock-owner`.
    if record.active && record.resource_disposition.as_deref() == Some("unconfirmed") {
        return Stage::Decided(Decision::OwnerRetainedUnconfirmed {
            owner_epoch: record.owner_epoch.clone(),
        });
    }
    // Row 5 precedes row 6 (review M7): a FATAL exit also closes the journal.
    if let Some(fatal) = &observed.fatal {
        return Stage::Decided(Decision::FatalHold(fatal.clone()));
    }
    if !record.active {
        return Stage::Decided(Decision::FormalClose {
            owner_epoch: record.owner_epoch.clone(),
        });
    }
    Stage::ConsiderStart
}

/// Rows 7a-7f.
pub(crate) fn decide_start(
    probes: &StartProbes,
    state: &WatchdogState,
    now_unix_ms: u64,
    from_task: bool,
) -> Decision {
    if probes.writer_busy {
        return Decision::InstallerBusy;
    }
    if probes.selection_changed {
        return Decision::SelectionChanged;
    }
    if !probes.processes.is_empty() {
        let since = state.process_present_since_unix_ms.unwrap_or(now_unix_ms);
        return if now_unix_ms.saturating_sub(since) >= PROCESS_WITHOUT_OWNER_MS {
            Decision::RuntimeProcessWithoutOwner {
                processes: probes.processes.clone(),
            }
        } else {
            Decision::RuntimeProcessPresent {
                processes: probes.processes.clone(),
            }
        };
    }
    if let Some(since_unix_ms) = state.exhausted_since_unix_ms {
        return Decision::BudgetExhausted { since_unix_ms };
    }
    if from_task
        && let Some(until_unix_ms) = state.grace_until_unix_ms
        && until_unix_ms > now_unix_ms
    {
        return Decision::Grace { until_unix_ms };
    }
    if starts_in_budget(state, now_unix_ms) >= BUDGET_STARTS {
        return Decision::BudgetExhausted {
            since_unix_ms: now_unix_ms,
        };
    }
    Decision::Start
}

/// A live owner is the watchdog's own when a start record names its epoch, or when a start
/// whose outcome is still open (pending or timed out) spawned it: its recorded start falls in
/// `[at - 2 s, at + 180 s]`. Every other live owner is a formal start (review M1: decided from
/// a live, answering owner only, and again on every tick).
pub(crate) fn started_by_watchdog(owner: &LiveOwner, starts: &[StartRecord]) -> bool {
    starts.iter().any(|start| {
        start.owner_epoch.as_deref() == Some(owner.owner_epoch.as_str())
            || (start.owner_epoch.is_none()
                && matches!(start.outcome.as_str(), "pending" | "start_timeout")
                && owner.started_at_unix_ms >= start.at_unix_ms.saturating_sub(CLOCK_SLACK_MS)
                && owner.started_at_unix_ms <= start.at_unix_ms.saturating_add(READY_TIMEOUT_MS))
    })
}

/// Starts since the last observed formal start, within the budget window.
pub(crate) fn starts_in_budget(state: &WatchdogState, now_unix_ms: u64) -> usize {
    starts_since(state, now_unix_ms.saturating_sub(BUDGET_WINDOW_MS))
}

/// Watchdog starts since the last observed formal start, within the last day (review M8).
pub(crate) fn starts_in_day(state: &WatchdogState, now_unix_ms: u64) -> usize {
    starts_since(state, now_unix_ms.saturating_sub(DAY_MS))
}

fn starts_since(state: &WatchdogState, floor_unix_ms: u64) -> usize {
    let floor = floor_unix_ms.max(state.formal_start_at_unix_ms.unwrap_or(0));
    state
        .starts
        .iter()
        .filter(|start| start.at_unix_ms > floor)
        .count()
}

/// The gap grace (review L5): opened when the previous task tick ended more than `GAP_MS`
/// ago or before the current boot; the first tick ever has none.
pub(crate) fn grace_after_gap(
    last_task_tick_end: Option<u64>,
    now_unix_ms: u64,
    boot_unix_ms: Option<u64>,
) -> Option<u64> {
    last_task_tick_end
        .filter(|end| {
            now_unix_ms.saturating_sub(*end) > GAP_MS
                || boot_unix_ms.is_some_and(|boot| *end < boot)
        })
        .map(|_| now_unix_ms.saturating_add(GRACE_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn record(active: bool, disposition: Option<&str>) -> OwnerRecord {
        OwnerRecord {
            schema_version: "actingcommand.runtime-owner.v2".to_owned(),
            owner_epoch: "epoch_a".to_owned(),
            pid: 41,
            started_at_unix_ms: NOW - 60_000,
            active,
            closed_at_unix_ms: (!active).then_some(NOW - 1_000),
            resource_disposition: disposition.map(str::to_owned),
        }
    }

    fn observed(lock: OwnerLock, journal: Journal, fatal: Option<Fatal>) -> Observed {
        Observed {
            lock,
            live: None,
            journal,
            fatal,
        }
    }

    fn journal(record: OwnerRecord) -> Journal {
        Journal::Record {
            record,
            modified_unix_ms: NOW - 1_000,
        }
    }

    fn fatal() -> Fatal {
        Fatal {
            log: "actingd-1.log".to_owned(),
            line: "FATAL actingd: config: bind_port_invalid".to_owned(),
        }
    }

    fn decided(stage: Stage) -> Decision {
        match stage {
            Stage::Decided(decision) => decision,
            Stage::ConsiderStart => panic!("expected a decision before the start rows"),
        }
    }

    fn start(at_unix_ms: u64, outcome: &str, owner_epoch: Option<&str>) -> StartRecord {
        StartRecord {
            at_unix_ms,
            outcome: outcome.to_owned(),
            owner_epoch: owner_epoch.map(str::to_owned),
            ..StartRecord::default()
        }
    }

    fn probes() -> StartProbes {
        StartProbes {
            writer_busy: false,
            selection_changed: false,
            processes: Vec::new(),
            boot_unix_ms: None,
        }
    }

    #[test]
    fn observed_rows_follow_the_table_order() {
        let state = WatchdogState::default();
        let owner = LiveOwner {
            pid: 7,
            owner_epoch: "epoch_b".to_owned(),
            started_at_unix_ms: NOW - 5_000,
        };
        // Row 2: an answering owner is alive; a held lock without one only stands aside.
        let mut locked = observed(OwnerLock::Locked, Journal::Absent, Some(fatal()));
        locked.live = Some(Ok(owner.clone()));
        assert_eq!(
            decided(decide_observed(&locked, &state)),
            Decision::Alive {
                owner,
                started_by_watchdog: false
            }
        );
        locked.live = Some(Err("runtime_info_unavailable".to_owned()));
        assert_eq!(
            decided(decide_observed(&locked, &state)).name(),
            "owner_lock_held"
        );
        // Row 3: never started.
        for (lock, journal) in [
            (OwnerLock::Missing, Journal::Absent),
            (OwnerLock::Unlocked, Journal::Absent),
        ] {
            assert_eq!(
                decided(decide_observed(&observed(lock, journal, None), &state)),
                Decision::NeverStarted
            );
        }
        // Row 4: an unknown schema is a misconfiguration.
        let unknown = observed(
            OwnerLock::Unlocked,
            Journal::Unrecognised {
                detail: "schema".to_owned(),
            },
            None,
        );
        assert_eq!(decided(decide_observed(&unknown, &state)).exit_code(), 13);
        // M2 (i): a retained owner holds, whatever the logs say.
        let retained = observed(
            OwnerLock::Unlocked,
            journal(record(true, Some("unconfirmed"))),
            None,
        );
        assert_eq!(
            decided(decide_observed(&retained, &state)).name(),
            "owner_retained_unconfirmed"
        );
        // Row 5 precedes row 6: a FATAL exit also wrote `active=false`.
        let fatal_close = observed(
            OwnerLock::Unlocked,
            journal(record(false, Some("none"))),
            Some(fatal()),
        );
        let decision = decided(decide_observed(&fatal_close, &state));
        assert_eq!((decision.name(), decision.exit_code()), ("fatal_hold", 10));
        let fatal_active = observed(
            OwnerLock::Unlocked,
            journal(record(true, Some("none"))),
            Some(fatal()),
        );
        assert_eq!(
            decided(decide_observed(&fatal_active, &state)).name(),
            "fatal_hold"
        );
        // Row 6: a formal close stands aside.
        let closed = observed(
            OwnerLock::Unlocked,
            journal(record(false, Some("none"))),
            None,
        );
        let decision = decided(decide_observed(&closed, &state));
        assert_eq!((decision.name(), decision.exit_code()), ("formal_close", 0));
        // A v1 record without a disposition that is still active considers a start.
        let crashed = observed(OwnerLock::Unlocked, journal(record(true, None)), None);
        assert!(matches!(
            decide_observed(&crashed, &state),
            Stage::ConsiderStart
        ));
    }

    #[test]
    fn start_rows_follow_the_table_order() {
        let mut state = WatchdogState::default();
        let mut busy = probes();
        busy.writer_busy = true;
        busy.selection_changed = true;
        assert_eq!(
            decide_start(&busy, &state, NOW, true),
            Decision::InstallerBusy
        );
        busy.writer_busy = false;
        assert_eq!(
            decide_start(&busy, &state, NOW, true),
            Decision::SelectionChanged
        );
        // Rows 7b and 7b': a Runtime process under the root without the owner lock.
        let mut present = probes();
        present.processes = vec![RuntimeProcess { pid: 9, path: None }];
        assert_eq!(
            decide_start(&present, &state, NOW, true).name(),
            "runtime_process_present"
        );
        state.process_present_since_unix_ms = Some(NOW - PROCESS_WITHOUT_OWNER_MS);
        let decision = decide_start(&present, &state, NOW, true);
        assert_eq!(
            (decision.name(), decision.exit_code()),
            ("runtime_process_without_owner", 15)
        );
        state.process_present_since_unix_ms = None;
        // Row 7c: sticky exhaustion outlives the window.
        state.exhausted_since_unix_ms = Some(NOW - 2 * BUDGET_WINDOW_MS);
        assert_eq!(
            decide_start(&probes(), &state, NOW, true),
            Decision::BudgetExhausted {
                since_unix_ms: NOW - 2 * BUDGET_WINDOW_MS
            }
        );
        state.exhausted_since_unix_ms = None;
        // Row 7d: grace holds only a task tick; a manual run starts at once.
        state.grace_until_unix_ms = Some(NOW + 1);
        assert_eq!(
            decide_start(&probes(), &state, NOW, true),
            Decision::Grace {
                until_unix_ms: NOW + 1
            }
        );
        assert_eq!(decide_start(&probes(), &state, NOW, false), Decision::Start);
        state.grace_until_unix_ms = Some(NOW);
        assert_eq!(decide_start(&probes(), &state, NOW, true), Decision::Start);
        // Row 7e: the fourth would-be start in 30 minutes exhausts the budget.
        state.starts = (1..=3)
            .map(|minute| start(NOW - minute * 60_000, "exited_during_startup", None))
            .collect();
        let decision = decide_start(&probes(), &state, NOW, true);
        assert_eq!(
            (decision.name(), decision.exit_code()),
            ("budget_exhausted", 11)
        );
        // Outside the window, or before an observed formal start, a start no longer counts.
        state.starts[2].at_unix_ms = NOW - BUDGET_WINDOW_MS;
        assert_eq!(decide_start(&probes(), &state, NOW, true), Decision::Start);
        state.starts[2].at_unix_ms = NOW - 3 * 60_000;
        state.formal_start_at_unix_ms = Some(NOW - 150_000);
        assert_eq!(starts_in_budget(&state, NOW), 2);
        assert_eq!(decide_start(&probes(), &state, NOW, true), Decision::Start);
    }

    #[test]
    fn formal_starts_are_told_from_the_watchdogs_own() {
        let owner = LiveOwner {
            pid: 7,
            owner_epoch: "epoch_c".to_owned(),
            started_at_unix_ms: NOW,
        };
        // Named by epoch.
        assert!(started_by_watchdog(
            &owner,
            &[start(NOW - 600_000, "started", Some("epoch_c"))]
        ));
        // An open start that spawned it shortly before.
        assert!(started_by_watchdog(
            &owner,
            &[start(NOW - 30_000, "start_timeout", None)]
        ));
        assert!(started_by_watchdog(
            &owner,
            &[start(NOW + 1_000, "pending", None)]
        ));
        // A failed start's process is gone; another epoch's start is not this owner.
        assert!(!started_by_watchdog(
            &owner,
            &[
                start(NOW - 1_000, "exited_during_startup", None),
                start(NOW - 1_000, "started", Some("epoch_d")),
                start(NOW - READY_TIMEOUT_MS - 1, "start_timeout", None),
            ]
        ));
    }

    #[test]
    fn repeated_restarts_count_a_day_since_the_last_formal_start() {
        let mut state = WatchdogState {
            starts: vec![
                start(NOW - DAY_MS - 1, "started", Some("epoch_1")),
                start(NOW - 3_600_000, "started", Some("epoch_2")),
                start(NOW - 60_000, "started", Some("epoch_3")),
            ],
            ..WatchdogState::default()
        };
        assert_eq!(starts_in_day(&state, NOW), 2);
        state.formal_start_at_unix_ms = Some(NOW - 120_000);
        assert_eq!(starts_in_day(&state, NOW), 1);
    }

    #[test]
    fn grace_opens_after_a_gap_or_a_boot_between_task_ticks() {
        let boot_long_ago = Some(NOW - DAY_MS);
        // The first tick ever, and ordinary ticks a minute apart, have none.
        assert_eq!(grace_after_gap(None, NOW, Some(NOW - 1_000)), None);
        assert_eq!(
            grace_after_gap(Some(NOW - 60_000), NOW, boot_long_ago),
            None
        );
        assert_eq!(
            grace_after_gap(Some(NOW - GAP_MS), NOW, boot_long_ago),
            None
        );
        // The measured gaps of a scheduled reboot, 120 s and 180 s, open it.
        for gap in [120_000, 180_000] {
            assert_eq!(
                grace_after_gap(Some(NOW - gap), NOW, boot_long_ago),
                Some(NOW + GRACE_MS)
            );
        }
        // So does a previous tick from before the current boot, however short the gap.
        assert_eq!(
            grace_after_gap(Some(NOW - 61_000), NOW, Some(NOW - 30_000)),
            Some(NOW + GRACE_MS)
        );
        // An unknown boot time leaves the gap rule alone.
        assert_eq!(grace_after_gap(Some(NOW - 61_000), NOW, None), None);
    }

    #[test]
    fn log_keys_change_once_per_log_file_and_epoch() {
        let first = Decision::FatalHold(fatal());
        let second = Decision::FatalHold(Fatal {
            log: "actingd-2.log".to_owned(),
            line: fatal().line,
        });
        assert_ne!(first.key(), second.key());
        assert_eq!(
            Decision::FormalClose {
                owner_epoch: "epoch_a".to_owned()
            }
            .key(),
            "formal_close:epoch_a"
        );
        assert_eq!(Decision::InstallerBusy.key(), "installer_busy");
    }
}
