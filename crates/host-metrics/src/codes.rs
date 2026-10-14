// SPDX-License-Identifier: AGPL-3.0-only

//! The host counters' outcome codes and locations (Workflow #378 A2b). Every entry is listed in
//! the fragment `contracts/outcome-codes/host-metrics.json`; the outcome guard (G1) checks both
//! directions. Spellings the Runtime host also emits (`performance_sample_time_invalid`,
//! `performance_process_count_overflow`, the operation `sample_capacity`) are the contract's
//! (`SharedCode`, `ContractLocation`, decision D3). Every entry is pending (B2 settles them).

actingcommand_contract::outcome_codes! {
    /// Why a host sample or a process probe failed.
    pub enum HostMetricsCode {
        PerformanceSamplerUnsupported => "performance_sampler_unsupported": error / none,
        PerformanceProcessSnapshotFailed => "performance_process_snapshot_failed": error / none,
        PerformanceProcessStructureInvalid => "performance_process_structure_invalid": error / none,
        PerformanceProcessEnumerationFailed => "performance_process_enumeration_failed": error / none,
        PerformancePdhOpenFailed => "performance_pdh_open_failed": error / none,
        PerformancePdhSeedFailed => "performance_pdh_seed_failed": error / none,
        PerformanceSystemCounterInvalid => "performance_system_counter_invalid": error / none,
        PerformanceSystemTimesFailed => "performance_system_times_failed": error / none,
        PerformanceMemoryStructureInvalid => "performance_memory_structure_invalid": error / none,
        PerformanceMemoryStatusFailed => "performance_memory_status_failed": error / none,
        ProcessIdReserved => "process_id_reserved": error / none,
        ProcessQueryRefused => "process_query_refused": error / none,
        ProcessOpenFailed => "process_open_failed": error / none,
        ProcessExitCodeUnavailable => "process_exit_code_unavailable": error / none,
        ProcessTimesUnavailable => "process_times_unavailable": error / none,
        ProcessCreationTimeInvalid => "process_creation_time_invalid": error / none,
        ProcessProbeUnsupported => "process_probe_unsupported": error / none,
    }
}

actingcommand_contract::outcome_vocabulary! {
    /// Why the operating system's connectivity status is unknown (vocabulary
    /// `connectivity_reason`; Workflow #369 S6a): the reason `network_connectivity` gives with
    /// `NetworkConnectivity::Unknown`.
    pub enum ConnectivityReason: "connectivity_reason" {
        /// The operating system reported its connectivity level as unknown, or a level this
        /// build does not know.
        LevelUnknown => "level_unknown",
        /// The query returned an error code (`os_error`).
        QueryFailed => "query_failed",
        /// The system library has no such query (a Windows before 10 2004).
        QueryUnsupported => "query_unsupported",
        /// The system library could not be loaded (`os_error`).
        LibraryUnavailable => "library_unavailable",
        /// This operating system has no such query.
        UnsupportedOs => "unsupported_os",
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
