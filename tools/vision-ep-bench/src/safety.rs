// SPDX-License-Identifier: AGPL-3.0-only

//! The memory stops of the 10-24 rule (design §1.5 item 5): the bench does not start while the
//! host has less than 4 GiB available, and stops when the host falls below 2.5 GiB or its own
//! private bytes exceed 3 GiB. The readings come through [`MemoryReader`], so the rule is tested
//! with fake readers; the bench's watcher thread applies it to the real counters.
//!
//! Workflow #407 GPU-0f: the private-bytes cap is `--private-bytes-cap-gib <n>` (1..=16, default
//! 3); a CUDA context with cuDNN and cuBLAS holds more host memory than 3 GiB. The host floors do
//! not change.

use crate::error::{BenchCode, BenchError};
use serde::Serialize;

const GIB: u64 = 1 << 30;

/// The private-bytes cap when `--private-bytes-cap-gib` is not given, in GiB.
pub const DEFAULT_PRIVATE_BYTES_CAP_GIB: u64 = 3;
/// The caps `--private-bytes-cap-gib` accepts, in GiB.
pub const PRIVATE_BYTES_CAP_GIB_RANGE: std::ops::RangeInclusive<u64> = 1..=16;

/// The three memory limits, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SafetyLimits {
    /// The bench refuses to start below this host available memory.
    pub host_available_floor_at_start_bytes: u64,
    /// The bench stops when the host's available memory falls below this while it runs.
    pub host_available_floor_bytes: u64,
    /// The bench stops when its own private bytes exceed this.
    pub own_private_bytes_cap: u64,
}

impl Default for SafetyLimits {
    /// The 10-24 rule: start at 4 GiB or more, stop below 2.5 GiB, stop above 3 GiB of own
    /// private bytes.
    fn default() -> Self {
        Self {
            host_available_floor_at_start_bytes: 4 * GIB,
            host_available_floor_bytes: 5 * GIB / 2,
            own_private_bytes_cap: DEFAULT_PRIVATE_BYTES_CAP_GIB * GIB,
        }
    }
}

impl SafetyLimits {
    /// The default limits with a private-bytes cap of `gib` GiB (the host floors unchanged).
    pub fn with_private_bytes_cap_gib(gib: u64) -> Self {
        Self {
            own_private_bytes_cap: gib.saturating_mul(GIB),
            ..Self::default()
        }
    }
}

/// One reading of the two counters the limits apply to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MemoryReading {
    pub host_available_bytes: u64,
    pub own_private_bytes: u64,
}

/// Where the readings come from: the host's available physical memory and this process's
/// private bytes (`GetProcessMemoryInfo` `PrivateUsage`).
pub trait MemoryReader {
    fn host_available_bytes(&mut self) -> Result<u64, BenchError>;
    fn own_private_bytes(&mut self) -> Result<u64, BenchError>;
}

/// Applies [`SafetyLimits`] to the readings of one [`MemoryReader`].
#[derive(Debug)]
pub struct SafetyMonitor<R> {
    limits: SafetyLimits,
    reader: R,
    /// GPU-0f: the most private bytes any reading showed, the one that broke the cap included.
    own_private_bytes_peak: Option<u64>,
}

impl<R: MemoryReader> SafetyMonitor<R> {
    pub fn new(limits: SafetyLimits, reader: R) -> Self {
        Self {
            limits,
            reader,
            own_private_bytes_peak: None,
        }
    }

    pub fn limits(&self) -> SafetyLimits {
        self.limits
    }

    /// The most private bytes read so far (none before the first whole reading).
    pub fn own_private_bytes_peak(&self) -> Option<u64> {
        self.own_private_bytes_peak
    }

    /// The start check: the host must have at least the start floor available, and the
    /// running limits must hold too.
    pub fn preflight(&mut self) -> Result<MemoryReading, BenchError> {
        let reading = self.read()?;
        if reading.host_available_bytes < self.limits.host_available_floor_at_start_bytes {
            return Err(BenchError::new(
                BenchCode::HostMemoryLowAtStart,
                format!(
                    "the host has {} bytes available, below the start floor of {} bytes; the bench does not start",
                    reading.host_available_bytes, self.limits.host_available_floor_at_start_bytes
                ),
            ));
        }
        self.apply_running_limits(reading)
    }

    /// The running check, made between measurements and by the watcher thread.
    pub fn check(&mut self) -> Result<MemoryReading, BenchError> {
        let reading = self.read()?;
        self.apply_running_limits(reading)
    }

    fn read(&mut self) -> Result<MemoryReading, BenchError> {
        let reading = MemoryReading {
            host_available_bytes: self.reader.host_available_bytes()?,
            own_private_bytes: self.reader.own_private_bytes()?,
        };
        self.own_private_bytes_peak = Some(
            self.own_private_bytes_peak
                .unwrap_or(0)
                .max(reading.own_private_bytes),
        );
        Ok(reading)
    }

    fn apply_running_limits(&self, reading: MemoryReading) -> Result<MemoryReading, BenchError> {
        if reading.host_available_bytes < self.limits.host_available_floor_bytes {
            return Err(BenchError::new(
                BenchCode::HostMemoryLow,
                format!(
                    "the host has {} bytes available, below the running floor of {} bytes; the bench stops",
                    reading.host_available_bytes, self.limits.host_available_floor_bytes
                ),
            ));
        }
        if reading.own_private_bytes > self.limits.own_private_bytes_cap {
            return Err(BenchError::new(
                BenchCode::PrivateBytesExceeded,
                format!(
                    "the bench holds {} private bytes, above its cap of {} bytes; the bench stops",
                    reading.own_private_bytes, self.limits.own_private_bytes_cap
                ),
            ));
        }
        Ok(reading)
    }
}
