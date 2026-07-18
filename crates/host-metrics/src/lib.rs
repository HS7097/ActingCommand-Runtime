// SPDX-License-Identifier: AGPL-3.0-only

//! Safe host-counter boundary for platform-specific performance APIs.

#![deny(unsafe_op_in_unsafe_fn)]

use std::collections::BTreeMap;

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
    fn sample(
        &mut self,
        observed_at_unix_ms: u64,
        owned_processes: &BTreeMap<u32, String>,
        top_process_count: usize,
        thresholds: ProcessLoadThresholds,
    ) -> Result<HostSample, &'static str>;
}

#[cfg(windows)]
pub fn system_sampler() -> Result<Box<dyn HostSampler>, &'static str> {
    windows::WindowsHostSampler::new().map(|sampler| Box::new(sampler) as Box<dyn HostSampler>)
}

#[cfg(not(windows))]
pub fn system_sampler() -> Result<Box<dyn HostSampler>, &'static str> {
    Err("performance_sampler_unsupported")
}
