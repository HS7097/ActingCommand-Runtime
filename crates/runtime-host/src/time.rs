// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_contract::codes::SharedCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeClockSample {
    pub unix_ms: u64,
    pub monotonic_ms: u64,
}

/// Runtime-owned clock used for authoritative lifecycle and budget accounting.
pub trait RuntimeClock: Send + Sync {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample>;

    /// Test plan E-3 (EXEC-I3, test plan §3.10): the monotonic instant the contained-task
    /// kernel's timers read, where `origin` is the instant at which this clock's monotonic
    /// reading was `origin_ms`. The default counts this clock's milliseconds from `origin`.
    fn monotonic_instant(&self, origin: Instant, origin_ms: u64) -> RuntimeHostResult<Instant> {
        let elapsed_ms = self
            .sample()?
            .monotonic_ms
            .checked_sub(origin_ms)
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::MonotonicClockRegressed,
                    HostLocation::ReadRuntimeClock,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })?;
        origin
            .checked_add(Duration::from_millis(elapsed_ms))
            .ok_or_else(|| {
                RuntimeHostError::fatal(
                    HostCode::MonotonicClockOverflow,
                    HostLocation::ReadRuntimeClock,
                    RuntimeErrorCode::RuntimeHalted,
                )
            })
    }

    /// Test plan E-3 (EXEC-I3): a wait of the contained-task kernel. The default sleeps the
    /// thread for `duration`; a test clock moves itself forward by `duration` instead.
    fn pause(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

#[derive(Debug)]
pub struct SystemRuntimeClock {
    started: Instant,
}

impl SystemRuntimeClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for SystemRuntimeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeClock for SystemRuntimeClock {
    /// The process's monotonic clock itself, which `sample` counts in milliseconds: the kernel
    /// reads the same clock at the same granularity as before test plan E-3.
    fn monotonic_instant(&self, _origin: Instant, _origin_ms: u64) -> RuntimeHostResult<Instant> {
        Ok(Instant::now())
    }

    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let monotonic_ms = u64::try_from(self.started.elapsed().as_millis()).map_err(|_| {
            RuntimeHostError::fatal(
                HostCode::MonotonicClockOverflow,
                HostLocation::ReadRuntimeClock,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?;
        Ok(RuntimeClockSample {
            unix_ms: unix_ms_now()?,
            monotonic_ms,
        })
    }
}

pub(crate) fn unix_ms_now() -> RuntimeHostResult<u64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            RuntimeHostError::fatal(
                SharedCode::SystemClockInvalid,
                HostLocation::ReadSystemClock,
                RuntimeErrorCode::RuntimeHalted,
            )
        })?
        .as_millis();
    u64::try_from(value).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::SystemClockOverflow,
            HostLocation::ReadSystemClock,
            RuntimeErrorCode::RuntimeHalted,
        )
    })
}
