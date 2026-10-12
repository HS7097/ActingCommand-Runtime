// SPDX-License-Identifier: AGPL-3.0-only

//! The host counters' outcome codes and locations (Workflow #378 A2b). Every entry is listed in
//! the fragment `contracts/outcome-codes/host-metrics.json`; the outcome guard (G1) checks both
//! directions. Spellings the Runtime host also emits (`performance_sample_time_invalid`,
//! `performance_process_count_overflow`, the operation `sample_capacity`) are the contract's
//! (`SharedCode`, `ContractLocation`, decision D3). Every entry is pending (B2 settles them).

actingcommand_contract::outcome_codes! {
    /// Why a host sample or a process probe failed.
    pub enum HostMetricsCode {
        PerformanceSamplerUnsupported => "performance_sampler_unsupported": error,
        PerformanceProcessSnapshotFailed => "performance_process_snapshot_failed": error,
        PerformanceProcessStructureInvalid => "performance_process_structure_invalid": error,
        PerformanceProcessEnumerationFailed => "performance_process_enumeration_failed": error,
        PerformancePdhOpenFailed => "performance_pdh_open_failed": error,
        PerformancePdhSeedFailed => "performance_pdh_seed_failed": error,
        PerformanceSystemCounterInvalid => "performance_system_counter_invalid": error,
        PerformanceSystemTimesFailed => "performance_system_times_failed": error,
        PerformanceMemoryStructureInvalid => "performance_memory_structure_invalid": error,
        PerformanceMemoryStatusFailed => "performance_memory_status_failed": error,
        ProcessIdReserved => "process_id_reserved": error,
        ProcessQueryRefused => "process_query_refused": error,
        ProcessOpenFailed => "process_open_failed": error,
        ProcessExitCodeUnavailable => "process_exit_code_unavailable": error,
        ProcessTimesUnavailable => "process_times_unavailable": error,
        ProcessCreationTimeInvalid => "process_creation_time_invalid": error,
        ProcessProbeUnsupported => "process_probe_unsupported": error,
    }
}

actingcommand_contract::outcome_locations! {
    /// The steps of a capacity sample (`CapacityUnavailable.operation`).
    pub enum HostMetricsLocation {
        ResolveCapacityTarget => "resolve_capacity_target",
        CanonicalizeCapacityTarget => "canonicalize_capacity_target",
        IdentifyCapacityVolume => "identify_capacity_volume",
    }
}
