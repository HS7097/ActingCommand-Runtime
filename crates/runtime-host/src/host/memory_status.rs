// SPDX-License-Identifier: AGPL-3.0-only

//! #407 PERF-READ (register §1.280; design rd-1031/perf-407/model-perf-012.md §1.1, §2): the
//! performance status's `memory` and `vision`, each part read from the memory of its owner:
//! the capture frame stores (their residency registry), the vision provider (its loaded models
//! and its per-model times), the ledger's capture queue, the running runs' packages and the two
//! fact stores; and the process's own memory counters, sampled after the parts. Nothing is read
//! from or written to the ledger.

use super::{HostShared, lock, lock_poison_error};
use crate::RuntimeHostResult;
use crate::codes::{HostLocation, PacingCode};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    FactStoresMemoryStatus, FrameStoresMemoryStatus, LedgerCaptureMemoryStatus,
    MAX_PERFORMANCE_STATUS_VISION_MODELS, PackagesMemoryStatus, ProcessMemoryStatus,
    RuntimeMemoryStatus, VisionModelTimingStatus,
};
use actingcommand_host_metrics::OwnProcessMemory;
use std::sync::Arc;

/// The process's own memory counters, read at each status read (`GetProcessMemoryInfo` on
/// itself in the Runtime; a fake in tests).
pub(super) type ProcessMemorySampler =
    Arc<dyn Fn() -> Result<OwnProcessMemory, Code> + Send + Sync>;

/// The Runtime's sampler: the host-metrics read of this process.
pub(super) fn live_process_memory() -> ProcessMemorySampler {
    Arc::new(actingcommand_host_metrics::sample_own_process_memory)
}

impl HostShared {
    /// The `memory` block of the status read, and the warning it leaves when the process's
    /// counters are absent. The parts are read first, each under its own lock alone (never the
    /// performance lock), then the process is sampled. The process is absent, never zero, when
    /// its sample failed (`perf_memory_unavailable` caused by the sampler's failure) or when the
    /// parts claim more than its private bytes (`perf_memory_unavailable` with the parts' bytes
    /// as `actual_bytes` and the private bytes as `limit_bytes`).
    pub(super) fn memory_status(
        &self,
        warnings: &mut Vec<Outcome>,
    ) -> RuntimeHostResult<RuntimeMemoryStatus> {
        // A poisoned registry is a poisoned lock of the Runtime's state, as every other one is.
        let frames = self
            .frame_store_residency
            .totals()
            .map_err(|_| lock_poison_error(HostLocation::ReadPerformanceStatus))?;
        let mut vision_models = self
            .execution
            .get()
            .and_then(|kernel| kernel.vision_provider())
            .map(|provider| provider.loaded_models())
            .unwrap_or_default();
        vision_models.truncate(MAX_PERFORMANCE_STATUS_VISION_MODELS);
        let capture = self.ledger.capture_status();
        let packages = {
            let runs = lock(&self.contained_runs, HostLocation::ReadPerformanceStatus)?;
            runs.values()
                .filter_map(|control| control.package_load())
                .fold(PackagesMemoryStatus::default(), |sum, load| {
                    PackagesMemoryStatus {
                        runs: sum.runs.saturating_add(1),
                        resident_bytes: sum.resident_bytes.saturating_add(load.bytes),
                    }
                })
        };
        let instance_facts =
            lock(&self.facts, HostLocation::ReadPerformanceStatus)?.active_count() as u64;
        let runtime_facts =
            lock(&self.runtime_facts, HostLocation::ReadPerformanceStatus)?.len() as u64;
        let mut memory = RuntimeMemoryStatus {
            process: None,
            frame_stores: FrameStoresMemoryStatus {
                stores: frames.stores,
                resident_bytes: frames.resident_bytes,
                resident_frames: frames.resident_frames,
                largest_resident_bytes: frames.largest_resident_bytes,
            },
            vision_models,
            ledger_capture: LedgerCaptureMemoryStatus {
                bytes: capture.memory_bytes,
                entries: capture.memory_records,
            },
            packages,
            facts: FactStoresMemoryStatus {
                instance_facts,
                runtime_facts,
            },
        };
        match (self.process_memory)() {
            Ok(sample) => {
                let process = ProcessMemoryStatus {
                    private_bytes: sample.private_bytes,
                    working_set_bytes: sample.working_set_bytes,
                    peak_working_set_bytes: sample
                        .peak_working_set_bytes
                        .max(sample.working_set_bytes),
                };
                match memory.parts_bytes() {
                    Some(parts) if parts <= process.private_bytes => {
                        memory.process = Some(process);
                    }
                    parts => warnings.push(
                        Outcome::new(PacingCode::MemoryUnavailable)
                            .actual_bytes(parts.unwrap_or(u64::MAX))
                            .limit_bytes(process.private_bytes),
                    ),
                }
            }
            Err(code) => warnings
                .push(Outcome::new(PacingCode::MemoryUnavailable).caused_by(Outcome::new(code))),
        }
        Ok(memory)
    }

    /// The `vision` block of the status read: each vision model's requests since the start, from
    /// the provider's memory; none without a provider.
    pub(super) fn vision_timings(&self) -> Vec<VisionModelTimingStatus> {
        let mut timings = self
            .execution
            .get()
            .and_then(|kernel| kernel.vision_provider())
            .map(|provider| provider.model_timings())
            .unwrap_or_default();
        timings.truncate(MAX_PERFORMANCE_STATUS_VISION_MODELS);
        timings
    }
}
