// SPDX-License-Identifier: AGPL-3.0-only

//! The watcher thread: every interval it applies the running memory limits to the real counters
//! and samples the GPU counters into the current phase. A violated limit is a safety stop: the
//! watcher writes the failure report to `--out`, names the error on stderr and ends the process
//! at once with a non-zero status, even while an inference is still running.

use crate::error::{BenchCode, BenchError};
use crate::report::{CounterStatus, GpuPhaseStats, PhaseStats, REPORT_SCHEMA, Variant};
use crate::safety::{SafetyLimits, SafetyMonitor};
use crate::win::{self, GpuCounters, OsMemoryReader};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How often the watcher samples.
pub const WATCH_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
struct PhaseAccumulator {
    samples: u64,
    own_private_bytes_max: u64,
    host_available_bytes_min: Option<u64>,
    gpus: BTreeMap<String, GpuAccumulator>,
}

#[derive(Default)]
struct GpuAccumulator {
    samples: u64,
    own_dedicated_bytes_max: u64,
    own_shared_bytes_max: u64,
    own_engine_max: f64,
    own_engine_sum: f64,
    host_engine_max: f64,
    host_engine_sum: f64,
}

#[derive(Default)]
struct Shared {
    phase: Mutex<String>,
    stop: AtomicBool,
    phases: Mutex<Vec<(String, PhaseAccumulator)>>,
    counters: Mutex<CounterStatus>,
}

/// What the watcher writes when it stops the bench.
#[derive(Debug, Clone)]
pub struct FailureSink {
    pub out: PathBuf,
    pub variant: Variant,
}

#[derive(Serialize)]
struct StopReport<'a> {
    schema: &'static str,
    status: &'static str,
    error: &'a BenchError,
    phase_at_failure: &'a str,
    stopped_by: &'static str,
    variant: &'a Variant,
}

impl FailureSink {
    fn stop(&self, error: &BenchError, phase: &str) -> ! {
        let report = StopReport {
            schema: REPORT_SCHEMA,
            status: "failed",
            error,
            phase_at_failure: phase,
            stopped_by: "watcher",
            variant: &self.variant,
        };
        let written = serde_json::to_vec_pretty(&report)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.out)
                    .and_then(|mut file| file.write_all(&bytes))
                    .map_err(|error| error.to_string())
            });
        eprintln!("ac-vision-ep-bench: {error} (phase {phase})");
        if let Err(write_error) = written {
            eprintln!(
                "ac-vision-ep-bench: {}: the stop report could not be written to {}: {write_error}",
                BenchCode::OutputUnwritable,
                self.out.display()
            );
        }
        win::terminate_now(error.code.exit_status())
    }
}

pub struct Watcher {
    shared: Arc<Shared>,
    handle: Option<JoinHandle<()>>,
}

impl Watcher {
    pub fn start(limits: SafetyLimits, sink: FailureSink, phase: &str) -> Result<Self, BenchError> {
        let shared = Arc::new(Shared::default());
        *lock(&shared.phase) = phase.to_owned();
        let thread_shared = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("vision-ep-bench-watch".to_owned())
            .spawn(move || watch(&thread_shared, limits, &sink))
            .map_err(|error| {
                BenchError::new(
                    BenchCode::SamplerFailed,
                    format!("the watcher thread did not start: {error}"),
                )
            })?;
        Ok(Self {
            shared,
            handle: Some(handle),
        })
    }

    pub fn phase(&self, phase: &str) {
        *lock(&self.shared.phase) = phase.to_owned();
    }

    pub fn current_phase(&self) -> String {
        lock(&self.shared.phase).clone()
    }

    /// Stops the watcher and returns its per-phase statistics; a watcher that panicked is
    /// `bench_sampler_failed` (its safety stops were not in force to the end).
    pub fn finish(mut self) -> Result<(Vec<PhaseStats>, CounterStatus), BenchError> {
        if !self.stop_thread() {
            return Err(BenchError::new(
                BenchCode::SamplerFailed,
                "the watcher thread panicked; the memory stops were not in force to the end",
            ));
        }
        let phases = std::mem::take(&mut *lock(&self.shared.phases));
        let counters = lock(&self.shared.counters).clone();
        let stats = phases
            .into_iter()
            .map(|(phase, accumulator)| PhaseStats {
                phase,
                samples: accumulator.samples,
                own_private_bytes_max: accumulator.own_private_bytes_max,
                host_available_bytes_min: accumulator.host_available_bytes_min,
                gpus: accumulator
                    .gpus
                    .into_iter()
                    .map(|(luid, gpu)| {
                        let samples = gpu.samples.max(1) as f64;
                        (
                            luid,
                            GpuPhaseStats {
                                samples: gpu.samples,
                                own_dedicated_bytes_max: gpu.own_dedicated_bytes_max,
                                own_shared_bytes_max: gpu.own_shared_bytes_max,
                                own_busiest_engine_percent_max: gpu.own_engine_max,
                                own_busiest_engine_percent_mean: gpu.own_engine_sum / samples,
                                host_busiest_engine_percent_max: gpu.host_engine_max,
                                host_busiest_engine_percent_mean: gpu.host_engine_sum / samples,
                            },
                        )
                    })
                    .collect(),
            })
            .collect();
        Ok((stats, counters))
    }

    /// Stops and joins the thread; false when it panicked.
    fn stop_thread(&mut self) -> bool {
        self.shared.stop.store(true, Ordering::Release);
        match self.handle.take() {
            Some(handle) => {
                handle.thread().unpark();
                handle.join().is_ok()
            }
            None => true,
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if !self.stop_thread() {
            eprintln!(
                "ac-vision-ep-bench: {}: the watcher thread panicked",
                BenchCode::SamplerFailed
            );
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn watch(shared: &Shared, limits: SafetyLimits, sink: &FailureSink) {
    let pid = win::current_process_id();
    let gpu = match GpuCounters::open() {
        Ok(counters) => {
            lock(&shared.counters).available = true;
            Some(counters)
        }
        Err(error) => {
            eprintln!("ac-vision-ep-bench: GPU counters unavailable: {error}");
            *lock(&shared.counters) = CounterStatus {
                available: false,
                error: Some(error),
            };
            None
        }
    };
    let mut monitor = SafetyMonitor::new(limits, OsMemoryReader);
    while !shared.stop.load(Ordering::Acquire) {
        thread::park_timeout(WATCH_INTERVAL);
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let phase = lock(&shared.phase).clone();
        let reading = match monitor.check() {
            Ok(reading) => reading,
            Err(error) => sink.stop(&error, &phase),
        };
        let gpus = gpu
            .as_ref()
            .filter(|counters| counters.collect())
            .map(|counters| counters.sample(pid));
        let mut phases = lock(&shared.phases);
        if phases.last().is_none_or(|(name, _)| *name != phase) {
            phases.push((phase, PhaseAccumulator::default()));
        }
        let Some((_, accumulator)) = phases.last_mut() else {
            continue;
        };
        accumulator.samples += 1;
        accumulator.own_private_bytes_max = accumulator
            .own_private_bytes_max
            .max(reading.own_private_bytes);
        accumulator.host_available_bytes_min = Some(
            accumulator
                .host_available_bytes_min
                .map_or(reading.host_available_bytes, |min| {
                    min.min(reading.host_available_bytes)
                }),
        );
        for (luid, sample) in gpus.into_iter().flatten() {
            let stats = accumulator.gpus.entry(luid).or_default();
            stats.samples += 1;
            stats.own_dedicated_bytes_max = stats
                .own_dedicated_bytes_max
                .max(sample.own_dedicated_bytes);
            stats.own_shared_bytes_max = stats.own_shared_bytes_max.max(sample.own_shared_bytes);
            stats.own_engine_max = stats.own_engine_max.max(sample.own_busiest_engine_percent);
            stats.own_engine_sum += sample.own_busiest_engine_percent;
            stats.host_engine_max = stats
                .host_engine_max
                .max(sample.host_busiest_engine_percent);
            stats.host_engine_sum += sample.host_busiest_engine_percent;
        }
    }
}
