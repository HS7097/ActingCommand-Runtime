// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeClockSample {
    pub unix_ms: u64,
    pub monotonic_ms: u64,
}

/// Runtime-owned clock used for authoritative lifecycle and budget accounting.
pub trait RuntimeClock: Send + Sync {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample>;
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
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let monotonic_ms = u64::try_from(self.started.elapsed().as_millis()).map_err(|_| {
            RuntimeHostError::fatal(
                "monotonic_clock_overflow",
                "read_runtime_clock",
                RuntimeErrorCode::RuntimeFatal,
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
                "system_clock_invalid",
                "read_system_clock",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?
        .as_millis();
    u64::try_from(value).map_err(|_| {
        RuntimeHostError::fatal(
            "system_clock_overflow",
            "read_system_clock",
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}
