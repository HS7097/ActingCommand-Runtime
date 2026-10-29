// SPDX-License-Identifier: AGPL-3.0-only

//! RB-2 (Alice's ruling of 2026-10-29 10:2x, 「设回正常」; `contracts/runtime-config.md`, "The
//! `host` section"): at start, right after the configuration is read and before the Runtime
//! host starts (so before the ledger opens), actingd sets its own memory priority and I/O
//! priority to the levels of `host.process_memory_priority` and `host.process_io_priority`
//! and reads both back. The CPU priority class is left as the process was started: the seam
//! has no call for it.
//!
//! The start records one outcome, as its daemon lines: `host_process_priority_applied` (info)
//! with the levels asked for and read back, or `host_process_priority_failed` (warning) with the
//! same values when a call failed (each failed call a `caused_by` link, its operation naming
//! it) or a level read back is not the one asked for. Neither stops the start. On an operating
//! system without these priorities nothing is set or read, and the record carries only the
//! levels asked for.

use crate::codes::ActingdCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_host_metrics::process_priority as system;
use actingcommand_runtime_config::{HostProcessConfig, ProcessPriorityLevel};

/// The operating system's calls on this process's own memory and I/O priority, the seam the
/// start applies the `host` section through. A read answers `None` where the operating system
/// has no such priority; a failed call is its `foreign_os_error` with its operation.
pub(crate) trait ProcessPriorityApi {
    fn set_memory_priority(&self, level: u32) -> Result<(), Outcome>;
    fn memory_priority(&self) -> Result<Option<u32>, Outcome>;
    fn set_io_priority(&self, level: u32) -> Result<(), Outcome>;
    fn io_priority(&self) -> Result<Option<u32>, Outcome>;
}

/// The operating system itself, through `actingcommand_host_metrics::process_priority`.
pub(crate) struct SystemProcessPriority;

impl ProcessPriorityApi for SystemProcessPriority {
    fn set_memory_priority(&self, level: u32) -> Result<(), Outcome> {
        system::set_own_memory_priority(level)
    }

    fn memory_priority(&self) -> Result<Option<u32>, Outcome> {
        system::own_memory_priority()
    }

    fn set_io_priority(&self, level: u32) -> Result<(), Outcome> {
        system::set_own_io_priority(level)
    }

    fn io_priority(&self) -> Result<Option<u32>, Outcome> {
        system::own_io_priority()
    }
}

/// The operating system's memory priority of a configured level: `MEMORY_PRIORITY_NORMAL` (5)
/// or `MEMORY_PRIORITY_LOW` (2).
pub(crate) const fn memory_level(level: ProcessPriorityLevel) -> u32 {
    match level {
        ProcessPriorityLevel::Normal => system::MEMORY_PRIORITY_NORMAL,
        ProcessPriorityLevel::Low => system::MEMORY_PRIORITY_LOW,
    }
}

/// The operating system's I/O priority hint of a configured level: `IoPriorityNormal` (2) or
/// `IoPriorityLow` (1).
pub(crate) const fn io_level(level: ProcessPriorityLevel) -> u32 {
    match level {
        ProcessPriorityLevel::Normal => system::IO_PRIORITY_NORMAL,
        ProcessPriorityLevel::Low => system::IO_PRIORITY_LOW,
    }
}

/// One priority: set to `requested`, then read back whether or not the set succeeded; each
/// failed call goes to `failures`. The level read back, when there is one.
fn set_and_read(
    set: Result<(), Outcome>,
    read: impl FnOnce() -> Result<Option<u32>, Outcome>,
    failures: &mut Vec<Outcome>,
) -> Option<u32> {
    if let Err(failure) = set {
        failures.push(failure);
    }
    match read() {
        Ok(level) => level,
        Err(failure) => {
            failures.push(failure);
            None
        }
    }
}

/// Applies `config` to this process through `api` (memory priority first, then I/O priority)
/// and returns the one outcome the start records.
pub(crate) fn apply(config: HostProcessConfig, api: &dyn ProcessPriorityApi) -> Outcome {
    let memory_requested = memory_level(config.memory_priority);
    let io_requested = io_level(config.io_priority);
    let mut failures = Vec::new();
    let memory_read = set_and_read(
        api.set_memory_priority(memory_requested),
        || api.memory_priority(),
        &mut failures,
    );
    let io_read = set_and_read(
        api.set_io_priority(io_requested),
        || api.io_priority(),
        &mut failures,
    );
    let differs = memory_read.is_some_and(|read| read != memory_requested)
        || io_read.is_some_and(|read| read != io_requested);
    let code = if failures.is_empty() && !differs {
        ActingdCode::HostProcessPriorityApplied
    } else {
        ActingdCode::HostProcessPriorityFailed
    };
    let mut outcome = Outcome::new(code)
        .memory_priority_requested(memory_requested)
        .io_priority_requested(io_requested);
    if let Some(read) = memory_read {
        outcome = outcome.memory_priority_read(read);
    }
    if let Some(read) = io_read {
        outcome = outcome.io_priority_read(read);
    }
    failures
        .into_iter()
        .fold(outcome, |outcome, failure| outcome.caused_by(failure))
}
