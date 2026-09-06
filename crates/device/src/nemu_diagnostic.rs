// SPDX-License-Identifier: AGPL-3.0-only

use crate::MumuInstallSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemuResolutionReason {
    InstallationAbsent,
    InstallationAmbiguous,
    SharedAdbMultipleDllVersions,
    ConfiguredAdbIdentityUnrecognized,
    RootMismatch,
    DllOutsideRoot,
    DllVersionMissing,
    VersionMismatch,
    CandidateAbsent,
    CandidateOutsideRoot,
    RunningExecutableTopologyInvalid,
    RunningVersionMissing,
    TargetIdentityUnavailable,
    TargetIdentityInvalid,
    TargetIdentityMismatch,
    TargetProcessAbsent,
    TargetProcessAmbiguous,
    TargetExecutableMissing,
    CaptureIdentityUncoordinated,
}

impl NemuResolutionReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::InstallationAbsent => "installation_absent",
            Self::InstallationAmbiguous => "installation_ambiguous",
            Self::SharedAdbMultipleDllVersions => "shared_adb_multiple_dll_versions",
            Self::ConfiguredAdbIdentityUnrecognized => "configured_adb_identity_unrecognized",
            Self::RootMismatch => "root_mismatch",
            Self::DllOutsideRoot => "dll_outside_root",
            Self::DllVersionMissing => "dll_version_missing",
            Self::VersionMismatch => "version_mismatch",
            Self::CandidateAbsent => "candidate_absent",
            Self::CandidateOutsideRoot => "candidate_outside_root",
            Self::RunningExecutableTopologyInvalid => "running_executable_topology_invalid",
            Self::RunningVersionMissing => "running_version_missing",
            Self::TargetIdentityUnavailable => "target_identity_unavailable",
            Self::TargetIdentityInvalid => "target_identity_invalid",
            Self::TargetIdentityMismatch => "target_identity_mismatch",
            Self::TargetProcessAbsent => "target_process_absent",
            Self::TargetProcessAmbiguous => "target_process_ambiguous",
            Self::TargetExecutableMissing => "target_executable_missing",
            Self::CaptureIdentityUncoordinated => "capture_identity_uncoordinated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemuResolutionCountKind {
    InstallationRoots,
    DllVersions,
    CaptureDllFiles,
    AdbExecutables,
    MatchedTargetProcesses,
}

impl NemuResolutionCountKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::InstallationRoots => "installation_roots",
            Self::DllVersions => "dll_versions",
            Self::CaptureDllFiles => "capture_dll_files",
            Self::AdbExecutables => "adb_executables",
            Self::MatchedTargetProcesses => "matched_target_processes",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemuConfiguredAdbClass {
    Absent,
    Generic,
    SharedMumu,
    VersionedMumu,
}

impl NemuConfiguredAdbClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Generic => "generic",
            Self::SharedMumu => "shared_mumu",
            Self::VersionedMumu => "versioned_mumu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NemuResolutionContext {
    reason: NemuResolutionReason,
    count: Option<(NemuResolutionCountKind, Option<u64>, bool)>,
    source: Option<MumuInstallSource>,
    configured_adb: Option<NemuConfiguredAdbClass>,
    explicit_root: Option<bool>,
    explicit_dll: Option<bool>,
}

impl NemuResolutionContext {
    pub const fn new(reason: NemuResolutionReason) -> Self {
        Self {
            reason,
            count: None,
            source: None,
            configured_adb: None,
            explicit_root: None,
            explicit_dll: None,
        }
    }

    pub fn with_count(
        mut self,
        kind: NemuResolutionCountKind,
        count: usize,
        at_least: bool,
    ) -> Self {
        self.count = Some((kind, u64::try_from(count).ok(), at_least));
        self
    }

    pub const fn with_source(mut self, source: MumuInstallSource) -> Self {
        self.source = Some(source);
        self
    }

    pub const fn with_provenance(
        mut self,
        configured_adb: Option<NemuConfiguredAdbClass>,
        explicit_root: bool,
        explicit_dll: bool,
    ) -> Self {
        if self.configured_adb.is_none() {
            self.configured_adb = configured_adb;
        }
        if self.explicit_root.is_none() {
            self.explicit_root = Some(explicit_root);
        }
        if self.explicit_dll.is_none() {
            self.explicit_dll = Some(explicit_dll);
        }
        self
    }

    pub(crate) fn render(self) -> String {
        let (kind, count, bound) = match self.count {
            Some((kind, value, at_least)) => (
                kind.as_str(),
                value.map_or_else(|| "overflow".to_owned(), |value| value.to_string()),
                if at_least { "at_least" } else { "exact" },
            ),
            None => ("unavailable", "unavailable".to_owned(), "unavailable"),
        };
        let observed_bool = |value: Option<bool>| match value {
            Some(true) => "true",
            Some(false) => "false",
            None => "unavailable",
        };
        format!(
            "reason={} count_kind={kind} count={count} count_bound={bound} source={} configured_adb={} explicit_root={} explicit_dll={}",
            self.reason.as_str(),
            self.source.map_or("unavailable", MumuInstallSource::as_str),
            self.configured_adb
                .map_or("unavailable", NemuConfiguredAdbClass::as_str),
            observed_bool(self.explicit_root),
            observed_bool(self.explicit_dll),
        )
    }
}
