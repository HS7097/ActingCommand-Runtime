// SPDX-License-Identifier: AGPL-3.0-only

use crate::MumuInstallSource;
use crate::codes::NemuResolutionReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NemuResolutionCountKind {
    InstallationRoots,
    DllVersions,
    CaptureDllFiles,
    AdbExecutables,
    MatchedTargetProcesses,
    ManagerExecutables,
}

/// Workflow #378 A7: the `count_kind` vocabulary, the value an outcome names a MuMu resolution
/// count's kind with.
impl actingcommand_contract::outcome::VocabularyToken for NemuResolutionCountKind {
    const VOCABULARY: &'static str = "count_kind";

    fn token(self) -> &'static str {
        self.as_str()
    }
}

impl NemuResolutionCountKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::InstallationRoots => "installation_roots",
            Self::DllVersions => "dll_versions",
            Self::CaptureDllFiles => "capture_dll_files",
            Self::AdbExecutables => "adb_executables",
            Self::MatchedTargetProcesses => "matched_target_processes",
            Self::ManagerExecutables => "manager_executables",
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

/// Workflow #378 A7: the `configured_adb_class` vocabulary, the value an outcome names the
/// configured adb's class with.
impl actingcommand_contract::outcome::VocabularyToken for NemuConfiguredAdbClass {
    const VOCABULARY: &'static str = "configured_adb_class";

    fn token(self) -> &'static str {
        self.as_str()
    }
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

    pub const fn reason(self) -> NemuResolutionReason {
        self.reason
    }

    // Workflow #378 A7: read accessors of the context the resolving step recorded, so an
    // outcome can carry it as values (`count_kind`, `count`, `count_known`, `install_source`,
    // `configured_adb_class`, `explicit_root`, `explicit_dll`). Each is `None` when the step
    // recorded nothing for it.

    /// What the resolving step counted.
    pub fn count_kind(self) -> Option<NemuResolutionCountKind> {
        self.count.map(|(kind, _, _)| kind)
    }

    /// How many the resolving step counted; `None` also when the count does not fit a `u64`.
    pub fn count(self) -> Option<u64> {
        self.count.and_then(|(_, value, _)| value)
    }

    /// Whether [`Self::count`] is exact (`true`) or a lower bound (`false`); `None` without a
    /// count.
    pub fn count_known(self) -> Option<bool> {
        self.count
            .and_then(|(_, value, at_least)| value.map(|_| !at_least))
    }

    /// Which resolver rung chose the MuMu installation.
    pub const fn source(self) -> Option<MumuInstallSource> {
        self.source
    }

    /// The class of the configured adb.
    pub const fn configured_adb(self) -> Option<NemuConfiguredAdbClass> {
        self.configured_adb
    }

    /// Whether the MuMu folder was configured explicitly.
    pub const fn explicit_root(self) -> Option<bool> {
        self.explicit_root
    }

    /// Whether the Nemu capture DLL was configured explicitly.
    pub const fn explicit_dll(self) -> Option<bool> {
        self.explicit_dll
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
