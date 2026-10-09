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
    /// `InstanceFactStore::recover`, the whole-Ledger replay before the held point.
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
pub(super) fn timed<T, E>(
    slot: &mut Option<u64>,
    step: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let started = Instant::now();
    let result = step();
    *slot = Some(elapsed_ms(started));
    result
}

pub(super) fn elapsed_ms(started: Instant) -> u64 {
    duration_ms(started.elapsed())
}

pub(super) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
