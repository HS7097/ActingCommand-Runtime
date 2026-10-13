// SPDX-License-Identifier: AGPL-3.0-only

//! The tick's decision table (`contracts/runtime-watchdog.md`, "Decision"), as pure functions
//! of what one tick observed and of the watchdog state. First match wins. Each decision is one
//! registered code with its values (Workflow #378 A5, model v2.2 §7.5).

use super::state::{StartRecord, WatchdogState};
use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::{Code, Outcome};

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
/// The developer line actingd prints for a Runtime host error: the top code comes next.
const RUNTIME_FATAL_PREFIX: &str = "FATAL actingd: runtime host error ";

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
    /// `watchdog_journal_unrecognised`, with what was not recognised.
    Unrecognised { failure: Outcome },
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
    pub(crate) live: Option<Result<LiveOwner, Outcome>>,
    pub(crate) journal: Journal,
    pub(crate) fatal: Option<Fatal>,
    /// Workflow #381 A R5b: with J closed but no log covering its epoch, the FATAL of the last
    /// earlier owner whose epoch a log covers, if that log holds one.
    pub(crate) earlier_fatal: Option<Fatal>,
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
    /// Row 1: the failure that keeps the watchdog from deciding.
    Misconfigured(Outcome),
    Alive {
        owner: LiveOwner,
        started_by_watchdog: bool,
    },
    /// The owner lock is held by an owner that does not answer: the client's failure, when
    /// there is one.
    OwnerLockHeld {
        failure: Option<Outcome>,
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
    /// Row 7f's start failed: the start's failure.
    StartFailed(Outcome),
}

/// The first stage: rows 2-6, or a start is to be considered.
pub(crate) enum Stage {
    Decided(Decision),
    ConsiderStart,
}

impl Decision {
    /// The decision's registered code (its category sets the exit, model §7.6).
    pub(crate) fn code(&self) -> ActingctlCode {
        match self {
            Self::RunInProgress => ActingctlCode::WatchdogRunInProgress,
            Self::Misconfigured(_) => ActingctlCode::WatchdogMisconfigured,
            Self::Alive { .. } => ActingctlCode::WatchdogAlive,
            Self::OwnerLockHeld { .. } => ActingctlCode::WatchdogOwnerLockHeld,
            Self::NeverStarted => ActingctlCode::WatchdogNeverStarted,
            Self::OwnerRetainedUnconfirmed { .. } => {
                ActingctlCode::WatchdogOwnerRetainedUnconfirmed
            }
            Self::FatalHold(_) => ActingctlCode::WatchdogFatalHold,
            Self::FormalClose { .. } => ActingctlCode::WatchdogFormalClose,
            Self::InstallerBusy => ActingctlCode::WatchdogInstallerBusy,
            Self::SelectionChanged => ActingctlCode::WatchdogSelectionChanged,
            Self::RuntimeProcessPresent { .. } => ActingctlCode::WatchdogRuntimeProcessPresent,
            Self::RuntimeProcessWithoutOwner { .. } => {
                ActingctlCode::WatchdogRuntimeProcessWithoutOwner
            }
            Self::BudgetExhausted { .. } => ActingctlCode::WatchdogBudgetExhausted,
            Self::Grace { .. } => ActingctlCode::WatchdogGrace,
            Self::Start => ActingctlCode::WatchdogStartDue,
            Self::Started { .. } => ActingctlCode::WatchdogStartedRuntime,
            Self::StartFailed(_) => ActingctlCode::WatchdogStartFailed,
        }
    }

    /// The decision as its outcome: the code with its values (`owner_epoch`, `pid`, `log`,
    /// `count`, …), a failure as its `caused_by` link with its code as `failure_code`.
    /// Log-on-change compares the code and the values (model §7.5).
    pub(crate) fn outcome(&self) -> Outcome {
        let outcome = Outcome::new(self.code());
        match self {
            Self::RunInProgress
            | Self::NeverStarted
            | Self::InstallerBusy
            | Self::SelectionChanged
            | Self::Start
            | Self::OwnerLockHeld { failure: None } => outcome,
            Self::Misconfigured(failure) | Self::StartFailed(failure) => {
                let outcome = match registered_code(failure) {
                    Some(code) => outcome.failure_code(code),
                    None => outcome,
                };
                outcome.caused_by(failure.clone())
            }
            Self::OwnerLockHeld {
                failure: Some(failure),
            } => outcome.caused_by(failure.clone()),
            Self::Alive { owner, .. } => outcome
                .owner_epoch(owner.owner_epoch.clone())
                .pid(owner.pid),
            Self::OwnerRetainedUnconfirmed { owner_epoch } | Self::FormalClose { owner_epoch } => {
                outcome.owner_epoch(owner_epoch.clone())
            }
            Self::FatalHold(fatal) => outcome.log(&fatal.log),
            Self::RuntimeProcessPresent { processes }
            | Self::RuntimeProcessWithoutOwner { processes } => outcome.count(processes.len()),
            Self::BudgetExhausted { since_unix_ms } => outcome
                .at_unix_ms(*since_unix_ms)
                .limit_count(BUDGET_STARTS),
            Self::Grace { until_unix_ms } => outcome.next_eligible_unix_ms(*until_unix_ms),
            Self::Started { owner, log, .. } => outcome
                .owner_epoch(owner.owner_epoch.clone())
                .pid(owner.pid)
                .log(log),
        }
    }
}

/// An outcome's top code as a registered [`Code`], for a `failure_code` value: its spelling is
/// read from the catalog this build embeds, which lives as long as the process. `None` when the
/// catalog does not list it.
pub(crate) fn registered_code(outcome: &Outcome) -> Option<Code> {
    let codes = actingcommand_contract::outcome::catalog_json()
        .ok()?
        .get("codes")?
        .as_object()?;
    let spelling = codes
        .keys()
        .find(|spelling| spelling.as_str() == outcome.code().as_str())?;
    Code::relayed(spelling)
}

/// Rows 2-6 (row 1, misconfiguration, is decided while observing).
pub(crate) fn decide_observed(observed: &Observed, state: &WatchdogState) -> Stage {
    if observed.lock == OwnerLock::Locked {
        return Stage::Decided(match &observed.live {
            Some(Ok(owner)) => Decision::Alive {
                started_by_watchdog: started_by_watchdog(owner, &state.starts),
                owner: owner.clone(),
            },
            Some(Err(failure)) => Decision::OwnerLockHeld {
                failure: Some(failure.clone()),
            },
            None => Decision::OwnerLockHeld { failure: None },
        });
    }
    let record = match (&observed.lock, &observed.journal) {
        (OwnerLock::Missing, _) | (_, Journal::Absent) => {
            return Stage::Decided(Decision::NeverStarted);
        }
        (_, Journal::Unrecognised { failure }) => {
            return Stage::Decided(Decision::Misconfigured(failure.clone()));
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
        return fatal_stage(fatal);
    }
    if !record.active {
        // Row 6′ (Workflow #381 A R5b): a close no log covers may follow the FATAL of an owner
        // before it, which row 5 cannot see behind the later record.
        if let Some(fatal) = &observed.earlier_fatal {
            return fatal_stage(fatal);
        }
        return Stage::Decided(Decision::FormalClose {
            owner_epoch: record.owner_epoch.clone(),
        });
    }
    Stage::ConsiderStart
}

/// Rows 5 and 5′ (Workflow #381 A R5′): a FATAL holds, unless its top code is one of a
/// held start that stopped; that one considers a start.
fn fatal_stage(fatal: &Fatal) -> Stage {
    /// Workflow #381 A R5′: the registered top codes of a held start that stopped (runtime-host
    /// `HostCode`). Such a halt is one a restart may fix, so the start rows 7a-7f decide, as
    /// after a crash. Only the top code counts, never the operation or a cause.
    const RESTARTABLE_STOP_CODES: [&str; 3] =
        ["install_startup_stopped", "held_timeout", "release_timeout"];
    let top_code = fatal
        .line
        .strip_prefix(RUNTIME_FATAL_PREFIX)
        .and_then(|rest| rest.split_whitespace().next());
    if top_code.is_some_and(|code| RESTARTABLE_STOP_CODES.contains(&code)) {
        Stage::ConsiderStart
    } else {
        Stage::Decided(Decision::FatalHold(fatal.clone()))
    }
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
/// whose result is still open (`watchdog_start_pending` or `start_timeout`) spawned it: its
/// recorded start falls in `[at - 2 s, at + 180 s]`. Every other live owner is a formal start
/// (review M1: decided from a live, answering owner only, and again on every tick).
pub(crate) fn started_by_watchdog(owner: &LiveOwner, starts: &[StartRecord]) -> bool {
    starts.iter().any(|start| {
        start.owner_epoch.as_deref() == Some(owner.owner_epoch.as_str())
            || (start.owner_epoch.is_none()
                && (start.start_result.is(ActingctlCode::WatchdogStartPending)
                    || start.start_result.is(ActingctlCode::StartTimeout))
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
    use super::super::state::Recorded;
    use super::*;
    use actingcommand_contract::outcome::Category;
    use actingcommand_runtime_client::ClientCode;

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
            earlier_fatal: None,
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

    fn start(at_unix_ms: u64, result: ActingctlCode, owner_epoch: Option<&str>) -> StartRecord {
        StartRecord {
            at_unix_ms,
            start_result: Recorded::of(&Outcome::new(result)),
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

    /// The exit of a process whose top outcome is this decision (model v2.2 §7.6).
    fn exit(decision: &Decision) -> u8 {
        crate::cli_line::exit_code(decision.outcome().category(), false)
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
        let unanswered = Outcome::new(ClientCode::RuntimeInfoUnavailable);
        locked.live = Some(Err(unanswered.clone()));
        assert_eq!(
            decided(decide_observed(&locked, &state)),
            Decision::OwnerLockHeld {
                failure: Some(unanswered)
            }
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
        let unrecognised = Outcome::new(ActingctlCode::WatchdogJournalUnrecognised)
            .declared_schema("actingcommand.runtime-owner.v9");
        let unknown = observed(
            OwnerLock::Unlocked,
            Journal::Unrecognised {
                failure: unrecognised.clone(),
            },
            None,
        );
        let decision = decided(decide_observed(&unknown, &state));
        assert_eq!(decision, Decision::Misconfigured(unrecognised));
        assert_eq!(exit(&decision), 1);
        // M2 (i): a retained owner holds, whatever the logs say.
        let retained = observed(
            OwnerLock::Unlocked,
            journal(record(true, Some("unconfirmed"))),
            None,
        );
        assert_eq!(
            decided(decide_observed(&retained, &state)).code(),
            ActingctlCode::WatchdogOwnerRetainedUnconfirmed
        );
        // Row 5 precedes row 6: a FATAL exit also wrote `active=false`.
        let fatal_close = observed(
            OwnerLock::Unlocked,
            journal(record(false, Some("none"))),
            Some(fatal()),
        );
        let decision = decided(decide_observed(&fatal_close, &state));
        assert_eq!(
            (decision.code(), exit(&decision)),
            (ActingctlCode::WatchdogFatalHold, 2)
        );
        let fatal_active = observed(
            OwnerLock::Unlocked,
            journal(record(true, Some("none"))),
            Some(fatal()),
        );
        assert_eq!(
            decided(decide_observed(&fatal_active, &state)).code(),
            ActingctlCode::WatchdogFatalHold
        );
        // Row 6: a formal close stands aside.
        let closed = observed(
            OwnerLock::Unlocked,
            journal(record(false, Some("none"))),
            None,
        );
        let decision = decided(decide_observed(&closed, &state));
        assert_eq!(
            (decision.code(), exit(&decision)),
            (ActingctlCode::WatchdogFormalClose, 0)
        );
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
            decide_start(&present, &state, NOW, true).code(),
            ActingctlCode::WatchdogRuntimeProcessPresent
        );
        state.process_present_since_unix_ms = Some(NOW - PROCESS_WITHOUT_OWNER_MS);
        let decision = decide_start(&present, &state, NOW, true);
        assert_eq!(
            (decision.code(), exit(&decision)),
            (ActingctlCode::WatchdogRuntimeProcessWithoutOwner, 1)
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
            .map(|minute| {
                start(
                    NOW - minute * 60_000,
                    ActingctlCode::ExitedDuringStartup,
                    None,
                )
            })
            .collect();
        let decision = decide_start(&probes(), &state, NOW, true);
        assert_eq!(
            (decision.code(), exit(&decision)),
            (ActingctlCode::WatchdogBudgetExhausted, 1)
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
            &[start(
                NOW - 600_000,
                ActingctlCode::WatchdogStartedRuntime,
                Some("epoch_c")
            )]
        ));
        // An open start that spawned it shortly before.
        assert!(started_by_watchdog(
            &owner,
            &[start(NOW - 30_000, ActingctlCode::StartTimeout, None)]
        ));
        assert!(started_by_watchdog(
            &owner,
            &[start(
                NOW + 1_000,
                ActingctlCode::WatchdogStartPending,
                None
            )]
        ));
        // A failed start's process is gone; another epoch's start is not this owner.
        assert!(!started_by_watchdog(
            &owner,
            &[
                start(NOW - 1_000, ActingctlCode::ExitedDuringStartup, None),
                start(
                    NOW - 1_000,
                    ActingctlCode::WatchdogStartedRuntime,
                    Some("epoch_d")
                ),
                start(
                    NOW - READY_TIMEOUT_MS - 1,
                    ActingctlCode::StartTimeout,
                    None
                ),
            ]
        ));
    }

    #[test]
    fn repeated_restarts_count_a_day_since_the_last_formal_start() {
        let started = ActingctlCode::WatchdogStartedRuntime;
        let mut state = WatchdogState {
            starts: vec![
                start(NOW - DAY_MS - 1, started, Some("epoch_1")),
                start(NOW - 3_600_000, started, Some("epoch_2")),
                start(NOW - 60_000, started, Some("epoch_3")),
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

    /// Log on change compares the last decision's code and values (model v2.2 §7.5): a new
    /// FATAL log or another epoch is a change; a decision without values is its code alone.
    #[test]
    fn log_keys_change_once_per_log_file_and_epoch() {
        let first = Recorded::of(&Decision::FatalHold(fatal()).outcome());
        let second = Recorded::of(
            &Decision::FatalHold(Fatal {
                log: "actingd-2.log".to_owned(),
                line: fatal().line,
            })
            .outcome(),
        );
        assert_ne!(first, second);
        let closed = Recorded::of(
            &Decision::FormalClose {
                owner_epoch: "epoch_a".to_owned(),
            }
            .outcome(),
        );
        assert!(closed.is(ActingctlCode::WatchdogFormalClose));
        assert_eq!(
            closed.values.get("owner_epoch"),
            Some(&serde_json::Value::from("epoch_a"))
        );
        let busy = Recorded::of(&Decision::InstallerBusy.outcome());
        assert!(busy.is(ActingctlCode::WatchdogInstallerBusy) && busy.values.is_empty());
    }

    /// Workflow #378 A5 (model v2.2 §7.5, §7.6; design item 3): every decision is one registered
    /// code; its category sets the exit (0 success, info or warning, 1 error, 2 fatal); the alive,
    /// formal-close, retained and started decisions name the `owner_epoch`, a FATAL hold and a
    /// start name the `log`, and a failure travels as the `caused_by` link with its code as the
    /// `failure_code`.
    #[test]
    fn every_decision_has_its_code_category_and_exit() {
        let owner = LiveOwner {
            pid: 7,
            owner_epoch: "epoch_e".to_owned(),
            started_at_unix_ms: NOW,
        };
        let processes = vec![RuntimeProcess { pid: 9, path: None }];
        let rows: [(Decision, &str, Category, u8); 17] = [
            (
                Decision::RunInProgress,
                "watchdog_run_in_progress",
                Category::Info,
                0,
            ),
            (
                Decision::Misconfigured(
                    Outcome::new(ActingctlCode::WatchdogLayoutUnknown).path(r"C:\Install"),
                ),
                "watchdog_misconfigured",
                Category::Error,
                1,
            ),
            (
                Decision::Alive {
                    owner: owner.clone(),
                    started_by_watchdog: true,
                },
                "watchdog_alive",
                Category::Info,
                0,
            ),
            (
                Decision::OwnerLockHeld {
                    failure: Some(Outcome::new(ClientCode::RuntimeInfoUnavailable)),
                },
                "watchdog_owner_lock_held",
                Category::Info,
                0,
            ),
            (
                Decision::NeverStarted,
                "watchdog_never_started",
                Category::Info,
                0,
            ),
            (
                Decision::OwnerRetainedUnconfirmed {
                    owner_epoch: "epoch_e".to_owned(),
                },
                "watchdog_owner_retained_unconfirmed",
                Category::Error,
                1,
            ),
            (
                Decision::FatalHold(fatal()),
                "watchdog_fatal_hold",
                Category::Fatal,
                2,
            ),
            (
                Decision::FormalClose {
                    owner_epoch: "epoch_e".to_owned(),
                },
                "watchdog_formal_close",
                Category::Info,
                0,
            ),
            (
                Decision::InstallerBusy,
                "watchdog_installer_busy",
                Category::Info,
                0,
            ),
            (
                Decision::SelectionChanged,
                "watchdog_selection_changed",
                Category::Info,
                0,
            ),
            (
                Decision::RuntimeProcessPresent {
                    processes: processes.clone(),
                },
                "watchdog_runtime_process_present",
                Category::Info,
                0,
            ),
            (
                Decision::RuntimeProcessWithoutOwner { processes },
                "watchdog_runtime_process_without_owner",
                Category::Error,
                1,
            ),
            (
                Decision::BudgetExhausted { since_unix_ms: NOW },
                "watchdog_budget_exhausted",
                Category::Error,
                1,
            ),
            (
                Decision::Grace {
                    until_unix_ms: NOW + 1,
                },
                "watchdog_grace",
                Category::Info,
                0,
            ),
            (Decision::Start, "watchdog_start_due", Category::Info, 0),
            (
                Decision::Started {
                    method: "wmi".to_owned(),
                    owner,
                    log: r"C:\Install\watchdog\actingd-1.log".to_owned(),
                },
                "watchdog_started_runtime",
                Category::Info,
                0,
            ),
            (
                Decision::StartFailed(
                    Outcome::new(ActingctlCode::ExitedDuringStartup)
                        .log(r"C:\Install\watchdog\actingd-1.log")
                        .exit_code(1),
                ),
                "watchdog_start_failed",
                Category::Error,
                1,
            ),
        ];
        for (decision, code, category, exit_code) in rows {
            let outcome = decision.outcome();
            assert_eq!(outcome.code().as_str(), code, "{decision:?}");
            assert_eq!(decision.code().as_str(), code, "{decision:?}");
            assert_eq!(outcome.category(), category, "{decision:?}");
            assert_eq!(exit(&decision), exit_code, "{decision:?}");
            let value = |key: &str| outcome.value(key).cloned();
            match &decision {
                Decision::Alive { .. }
                | Decision::FormalClose { .. }
                | Decision::OwnerRetainedUnconfirmed { .. } => {
                    assert_eq!(value("owner_epoch"), Some("epoch_e".into()), "{decision:?}");
                }
                Decision::Started { log, .. } => {
                    assert_eq!(value("owner_epoch"), Some("epoch_e".into()));
                    assert_eq!(value("log"), Some(log.as_str().into()));
                }
                Decision::FatalHold(fatal) => {
                    assert_eq!(value("log"), Some(fatal.log.as_str().into()));
                }
                Decision::Misconfigured(cause) | Decision::StartFailed(cause) => {
                    assert_eq!(value("failure_code"), Some(cause.code().as_str().into()));
                    assert_eq!(outcome.causes().len(), 1, "{decision:?}");
                    assert_eq!(outcome.causes()[0].outcome(), cause, "{decision:?}");
                }
                Decision::OwnerLockHeld {
                    failure: Some(cause),
                } => {
                    assert_eq!(outcome.causes().len(), 1, "{decision:?}");
                    assert_eq!(outcome.causes()[0].outcome(), cause, "{decision:?}");
                }
                _ => {}
            }
        }
    }
}
