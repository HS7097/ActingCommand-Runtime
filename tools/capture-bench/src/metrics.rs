// SPDX-License-Identifier: AGPL-3.0-only

//! §5.5: the metrics sampling, once a second: the bench's own working set (the 1 GB cap of
//! ruling 141), the host's memory (recorded only, never a gate), CPU and GPU, the run volume's
//! free space (the 10 GB floor) and the watched processes.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One sample; a value the host did not give is `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MetricsSample {
    pub own_working_set_bytes: Option<u64>,
    pub own_peak_working_set_bytes: Option<u64>,
    pub host_available_bytes: Option<u64>,
    pub host_total_bytes: Option<u64>,
    pub host_cpu_bp: Option<u16>,
    pub gpu_bp: Option<u16>,
    pub disk_free_bytes: Option<u64>,
    pub processes: Vec<ProcessLine>,
    pub unavailable: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessLine {
    pub pid: u32,
    pub name: String,
    pub cpu_bp: u16,
    pub working_set_bytes: u64,
    pub io_bytes_per_second: u64,
}

pub trait MetricsSource: Send {
    fn sample(&mut self) -> MetricsSample;
}

/// A fake source that replays scripted samples (the D1 working-set and disk triggers).
pub struct ScriptedMetrics {
    pub samples: Vec<MetricsSample>,
    pub next: usize,
}

impl MetricsSource for ScriptedMetrics {
    fn sample(&mut self) -> MetricsSample {
        let sample = self
            .samples
            .get(self.next)
            .or_else(|| self.samples.last())
            .cloned()
            .unwrap_or_default();
        self.next += 1;
        sample
    }
}

/// The host's metrics through `actingcommand-host-metrics`.
pub struct HostMetrics {
    sampler: Option<Box<dyn actingcommand_host_metrics::HostSampler>>,
    sampler_error: Option<String>,
    run_dir: PathBuf,
    own_pid: u32,
    watched: BTreeMap<u32, String>,
}

impl HostMetrics {
    pub fn new(run_dir: PathBuf) -> Self {
        let (sampler, sampler_error) = match actingcommand_host_metrics::system_sampler(
            actingcommand_host_metrics::GpuAggregate::default(),
        ) {
            Ok(sampler) => (Some(sampler), None),
            Err(code) => (None, Some(format!("{code:?}"))),
        };
        let own_pid = std::process::id();
        let mut watched = BTreeMap::new();
        watched.insert(own_pid, "ac-capture-bench".to_owned());
        Self {
            sampler,
            sampler_error,
            run_dir,
            own_pid,
            watched,
        }
    }
}

impl MetricsSource for HostMetrics {
    fn sample(&mut self) -> MetricsSample {
        let mut sample = MetricsSample::default();
        match actingcommand_host_metrics::sample_physical_memory() {
            Ok(memory) => {
                sample.host_available_bytes = Some(memory.available_bytes);
                sample.host_total_bytes = Some(memory.total_bytes);
            }
            Err(code) => sample
                .unavailable
                .push(format!("physical_memory: {code:?}")),
        }
        let capacity = actingcommand_host_metrics::sample_capacity(&[
            actingcommand_host_metrics::CapacityTarget {
                path: self.run_dir.clone(),
            },
        ]);
        match capacity
            .first()
            .map(|sample| sample.available_bytes.clone())
        {
            Some(Ok(bytes)) => sample.disk_free_bytes = Some(bytes),
            Some(Err(error)) => sample.unavailable.push(format!("disk: {}", error.detail)),
            None => sample.unavailable.push("disk: no sample".to_owned()),
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        match self.sampler.as_mut() {
            Some(sampler) => {
                let thresholds = actingcommand_host_metrics::ProcessLoadThresholds {
                    cpu_basis_points: 500,
                    io_bytes_per_second: 10 * 1024 * 1024,
                    min_coverage_basis_points: 0,
                };
                match sampler.sample(now, &self.watched, 8, thresholds) {
                    Ok(host) => {
                        sample.host_cpu_bp = Some(host.cpu_total_basis_points);
                        sample.gpu_bp = host.gpu_basis_points;
                        for process in host
                            .owned_processes
                            .iter()
                            .chain(host.third_party_high_load.iter())
                        {
                            if process.pid == self.own_pid {
                                sample.own_working_set_bytes = Some(process.working_set_bytes);
                                sample.own_peak_working_set_bytes = process.peak_working_set_bytes;
                            }
                            sample.processes.push(ProcessLine {
                                pid: process.pid,
                                name: process.process_name.clone(),
                                cpu_bp: process.cpu_basis_points,
                                working_set_bytes: process.working_set_bytes,
                                io_bytes_per_second: process.io_bytes_per_second,
                            });
                        }
                        for metric in &host.unavailable_metrics {
                            sample.unavailable.push(format!("{metric:?}"));
                        }
                    }
                    Err(code) => sample.unavailable.push(format!("host_sampler: {code:?}")),
                }
            }
            None => sample.unavailable.push(format!(
                "host_sampler: {}",
                self.sampler_error.as_deref().unwrap_or("unavailable")
            )),
        }
        if sample.own_working_set_bytes.is_none() {
            sample
                .unavailable
                .push("own_working_set: not in the process sample".to_owned());
        }
        sample
    }
}
