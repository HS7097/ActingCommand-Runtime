// SPDX-License-Identifier: AGPL-3.0-only

//! Safe host-counter boundary for platform-specific performance APIs.

#![deny(unsafe_op_in_unsafe_fn)]

use actingcommand_contract::outcome::Code;
use std::collections::BTreeMap;
use std::path::Path;

mod capacity;
pub mod codes;
// Workflow #382 H-5 (test plan v2.2 §3.6 G20): capacity sampling with an injected volume
// resolver and free-space query.
#[cfg(test)]
mod gate_382_h5;
pub use capacity::{CapacitySample, CapacityTarget, CapacityUnavailable, sample_capacity};
pub use codes::{ConnectivityReason, HostMetricsCode, HostMetricsLocation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalMemorySample {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Uses the same platform memory observation as the existing host RAM counter.
pub fn sample_physical_memory() -> Result<PhysicalMemorySample, Code> {
    #[cfg(windows)]
    {
        windows::sample_physical_memory()
    }
    #[cfg(not(windows))]
    {
        Err(HostMetricsCode::PerformanceSamplerUnsupported.code())
    }
}

/// Resolve the current binding without taking another free-space sample.
pub fn capacity_volume(path: &Path) -> Result<String, CapacityUnavailable> {
    capacity::volume(path)
}

/// One process id as the operating system reports it now, read with the same limited
/// query and creation time as the process sampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessProbe {
    /// A running process has this id; its creation time in Unix milliseconds.
    Running { created_at_unix_ms: u64 },
    /// No running process has this id: none exists, or the one that exists has exited.
    NotRunning,
    /// The operating system did not decide, for example because it refused the query.
    Unknown(Code),
}

pub fn probe_process(pid: u32) -> ProcessProbe {
    #[cfg(windows)]
    {
        windows::probe_process(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        ProcessProbe::Unknown(HostMetricsCode::ProcessProbeUnsupported.code())
    }
}

/// What the operating system reports about the host's internet connectivity (Workflow #369 S6a,
/// from #361 P3). It is the operating system's own status (on Windows
/// `GetNetworkConnectivityHint`, which its network connectivity status indicator keeps): reading
/// it opens no socket, sends nothing and resolves no name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkConnectivity {
    /// Internet access, constrained internet access (for example behind a captive portal)
    /// included.
    Internet,
    /// No internet access: no network, local access only, or a hidden network.
    NoInternet,
    /// The operating system did not decide: why, with its error code when it gave one.
    Unknown {
        reason: ConnectivityReason,
        os_error: Option<u32>,
    },
}

/// Reads the operating system's connectivity status once; see [`NetworkConnectivity`].
pub fn network_connectivity() -> NetworkConnectivity {
    #[cfg(windows)]
    {
        windows::network_connectivity()
    }
    #[cfg(not(windows))]
    {
        NetworkConnectivity::Unknown {
            reason: ConnectivityReason::UnsupportedOs,
            os_error: None,
        }
    }
}

#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostMetric {
    CpuPerCore,
    DiskQueueDepth,
    DiskLatency,
    Gpu,
    ProcessCpu,
    ProcessRam,
    ProcessIo,
    ForegroundProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessOwnership {
    Runtime,
    Owned,
    ThirdParty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSample {
    pub pid: u32,
    pub process_name: String,
    pub ownership: ProcessOwnership,
    pub cpu_basis_points: u16,
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: Option<u64>,
    pub process_created_at_windows_100ns: Option<u64>,
    pub io_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundSample {
    pub process: ProcessSample,
    pub fullscreen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessMetricCoverage {
    pub enumerated_processes: u32,
    pub sampled_processes: u32,
    pub audited_protected_processes_excluded: u32,
    pub unconfirmed_access_denied_processes: u32,
    pub unexpectedly_inaccessible_processes: u32,
    pub readable_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSample {
    pub observed_at_unix_ms: u64,
    pub cpu_total_basis_points: u16,
    pub cpu_per_core_basis_points: Vec<u16>,
    pub ram_used_basis_points: u16,
    pub disk_queue_depth_milli: Option<u32>,
    pub disk_latency_micros: Option<u64>,
    pub gpu_basis_points: Option<u16>,
    pub foreground: Option<ForegroundSample>,
    pub owned_processes: Vec<ProcessSample>,
    pub third_party_high_load: Vec<ProcessSample>,
    pub process_coverage: ProcessMetricCoverage,
    pub unavailable_metrics: Vec<HostMetric>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLoadThresholds {
    pub cpu_basis_points: u16,
    pub io_bytes_per_second: u64,
}

pub trait HostSampler: Send {
    fn sample_capacity(&mut self, targets: &[CapacityTarget]) -> Vec<CapacitySample> {
        sample_capacity(targets)
    }

    fn sample(
        &mut self,
        observed_at_unix_ms: u64,
        owned_processes: &BTreeMap<u32, String>,
        top_process_count: usize,
        thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, Code>;
}

#[cfg(windows)]
pub fn system_sampler() -> Result<Box<dyn HostSampler>, Code> {
    windows::WindowsHostSampler::new().map(|sampler| Box::new(sampler) as Box<dyn HostSampler>)
}

#[cfg(not(windows))]
pub fn system_sampler() -> Result<Box<dyn HostSampler>, Code> {
    Err(HostMetricsCode::PerformanceSamplerUnsupported.code())
}
