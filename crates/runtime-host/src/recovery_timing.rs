// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #381 R6: the start's recovery timing, printed as one stdout line
//! `actingd startup_recovery key=value …` after the last recovery step, or when the start
//! fails during recovery (never a Ledger event). A step that did not run prints `-`; a step
//! that failed prints the time until it failed, and `recovery_ms` stays `-`.

use actingcommand_ledger::LedgerWriterPeak;
use std::fmt;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StartupRecoveryTiming {
    /// `InstanceFactStore::recover`, the tagged instance store load before the held point
    /// (Workflow #422 LN-2; before it, the whole-Ledger replay).
    pub(crate) fact_store_ms: Option<u64>,
    pub(crate) monitor_registry_ms: Option<u64>,
    pub(crate) policy_host_ms: Option<u64>,
    /// `reconcile_policy_dispatches`.
    pub(crate) policy_dispatches_ms: Option<u64>,
    /// `recover_authoritative_policy_outcomes`.
    pub(crate) policy_outcomes_ms: Option<u64>,
    /// `ApprovalProjection::recover`.
    pub(crate) approvals_ms: Option<u64>,
    /// `reconcile_runtime_state`.
    pub(crate) runtime_state_ms: Option<u64>,
    /// `AgentDispatcherState::recover`.
    pub(crate) agent_dispatcher_ms: Option<u64>,
    /// From the start of `MonitorRegistry::open` to the end of `AgentDispatcherState::recover`,
    /// set only when every step succeeded.
    pub(crate) recovery_ms: Option<u64>,
    /// The writer's busiest work from the start of the fact store recovery to the line, the
    /// requests a held start answers included.
    pub(crate) writer: LedgerWriterPeak,
    /// The longest wait for the shared SQLite connection (the writer and the state store hold
    /// the same one) over the same window; set when the line is printed.
    pub(crate) connection_longest_wait_ms: Option<u64>,
}

/// Runs one recovery step and records its milliseconds, whether it succeeds or fails.
pub(crate) fn timed<T, E>(
    slot: &mut Option<u64>,
    step: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let started = Instant::now();
    let result = step();
    *slot = Some(elapsed_ms(started));
    result
}

pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    duration_ms(started.elapsed())
}

pub(crate) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Workflow #485 START-1 (register §1.489): the stages of a start, in their order, from the
/// process's own beginning to the publication of the Runtime's information. The stdout line
/// `actingd start_stages <member>=<ms> … total_ms=<ms>` names each by its member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartStage {
    /// actingd before the host: its arguments, the configuration and its checks, the install
    /// root's adb, the resource packages' validation, the recognition bindings.
    Daemon,
    /// The host's configuration check, the installation record, the owner journal and lock, the
    /// state root's claim.
    Owner,
    /// The state and artifact stores, the ledger maintenance's acquire and the format check.
    Storage,
    /// The ledger writer's open (the phases of `ledger_opened`).
    LedgerOpen,
    /// The frame cleaner's recovery.
    FrameRetention,
    /// The capacity preflight.
    Capacity,
    /// The closes of the earlier owner epochs' scopes.
    PriorEpochs,
    /// The listener, the instance fact store (`fact_store_ms` of `actingd startup_recovery`), the
    /// performance control and the runtime fact store.
    Facts,
    /// The host's shared state, the facts it synchronises and invalidates, the installation's
    /// fact, the IPC thread.
    Shared,
    /// The provider's assembly (the start's discovery of its instances) and the instances it
    /// registers.
    Provider,
    /// The startup packages, the emulator starts and stuck recovery resolved; the tagged state's
    /// marker and the earlier start read.
    Resolve,
    /// The recovery steps of `actingd startup_recovery` (`recovery_ms` there).
    Recovery,
    /// The agent wakes, the start event and the instance bindings.
    StartEvent,
    /// The tagged state's seeding, the facts and configuration in effect, the standard packs, the
    /// policy's settlement facts and rerun signals.
    Configure,
    /// The settlement of the direct runs and held records earlier epochs left
    /// (`actingd contained_settlement`).
    Settlement,
    /// The agent sessions, the scheduling pauses, the idle state, the capped episodes, the agent
    /// wake, the held dispatches and the start pauses.
    Restore,
    /// The instance workers and each instance's preparation.
    Instances,
    /// The rest, through the publication of the Runtime's information.
    Publish,
}

impl StartStage {
    /// Every stage, in its order.
    pub(crate) const ALL: [Self; 18] = [
        Self::Daemon,
        Self::Owner,
        Self::Storage,
        Self::LedgerOpen,
        Self::FrameRetention,
        Self::Capacity,
        Self::PriorEpochs,
        Self::Facts,
        Self::Shared,
        Self::Provider,
        Self::Resolve,
        Self::Recovery,
        Self::StartEvent,
        Self::Configure,
        Self::Settlement,
        Self::Restore,
        Self::Instances,
        Self::Publish,
    ];

    /// The stage's member of `actingd start_stages`.
    pub(crate) const fn member(self) -> &'static str {
        match self {
            Self::Daemon => "daemon_ms",
            Self::Owner => "owner_ms",
            Self::Storage => "storage_ms",
            Self::LedgerOpen => "ledger_open_ms",
            Self::FrameRetention => "frame_retention_ms",
            Self::Capacity => "capacity_ms",
            Self::PriorEpochs => "prior_epochs_ms",
            Self::Facts => "facts_ms",
            Self::Shared => "shared_ms",
            Self::Provider => "provider_ms",
            Self::Resolve => "resolve_ms",
            Self::Recovery => "recover_ms",
            Self::StartEvent => "start_event_ms",
            Self::Configure => "configure_ms",
            Self::Settlement => "settle_ms",
            Self::Restore => "restore_ms",
            Self::Instances => "instances_ms",
            Self::Publish => "publish_ms",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// Workflow #485 START-1: what a start's stages measured, each stage's end in whole milliseconds
/// from the stages' origin. A stage's member is its end less the end of the last stage before it
/// that ended (the origin when none did), so the members of the stages that ended add up to
/// `total_ms`, the last end, exactly; a stage that did not end prints `-`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StartStageTimes {
    ends: [Option<u64>; 18],
}

impl StartStageTimes {
    /// The milliseconds of `stage`; `None` when it did not end.
    pub(crate) fn member_ms(&self, stage: StartStage) -> Option<u64> {
        let end = self.ends[stage.index()]?;
        let before = self.ends[..stage.index()]
            .iter()
            .rev()
            .find_map(|end| *end)
            .unwrap_or(0);
        Some(end.saturating_sub(before))
    }

    /// The milliseconds from the origin to the end of the last stage that ended.
    pub(crate) fn total_ms(&self) -> Option<u64> {
        self.ends.iter().rev().find_map(|end| *end)
    }
}

impl fmt::Display for StartStageTimes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for stage in StartStage::ALL {
            write!(
                formatter,
                "{}={} ",
                stage.member(),
                Millis(self.member_ms(stage))
            )?;
        }
        write!(formatter, "total_ms={}", Millis(self.total_ms()))
    }
}

/// Workflow #485 START-1: a start's stages as they end, on a reading of whole milliseconds from
/// their origin (the process's beginning, or the host's start).
pub(crate) struct StartStages {
    reading: Box<dyn Fn() -> u64>,
    times: StartStageTimes,
}

impl StartStages {
    /// The stages counted from `origin` on the process's monotonic clock.
    pub(crate) fn since(origin: Instant) -> Self {
        Self::with_reading(Box::new(move || elapsed_ms(origin)))
    }

    /// The stages counted on `reading`, the milliseconds since their origin (a test's script).
    pub(crate) fn with_reading(reading: Box<dyn Fn() -> u64>) -> Self {
        Self {
            reading,
            times: StartStageTimes::default(),
        }
    }

    /// `stage` ends now.
    pub(crate) fn end(&mut self, stage: StartStage) {
        self.times.ends[stage.index()] = Some((self.reading)());
    }

    pub(crate) const fn times(&self) -> StartStageTimes {
        self.times
    }
}

struct Millis(Option<u64>);

impl fmt::Display for Millis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(formatter, "{value}"),
            None => formatter.write_str("-"),
        }
    }
}

impl fmt::Display for StartupRecoveryTiming {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let longest_ms = self
            .writer
            .longest
            .map(|(_, duration)| duration_ms(duration));
        let longest_command = self
            .writer
            .longest
            .map_or_else(|| "-".to_owned(), |(command, _)| format!("{command:?}"));
        write!(
            formatter,
            "fact_store_ms={} monitor_registry_ms={} policy_host_ms={} policy_dispatches_ms={} policy_outcomes_ms={} approvals_ms={} runtime_state_ms={} agent_dispatcher_ms={} recovery_ms={} writer_commands={} writer_longest_ms={} writer_longest_command={} writer_largest_read_events={} writer_whole_ledger_queries={} connection_longest_wait_ms={}",
            Millis(self.fact_store_ms),
            Millis(self.monitor_registry_ms),
            Millis(self.policy_host_ms),
            Millis(self.policy_dispatches_ms),
            Millis(self.policy_outcomes_ms),
            Millis(self.approvals_ms),
            Millis(self.runtime_state_ms),
            Millis(self.agent_dispatcher_ms),
            Millis(self.recovery_ms),
            self.writer.commands,
            Millis(longest_ms),
            longest_command,
            self.writer.largest_read_events,
            self.writer.whole_ledger_queries,
            Millis(self.connection_longest_wait_ms),
        )
    }
}
