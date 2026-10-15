// SPDX-License-Identifier: AGPL-3.0-only

//! Read-only package qualifications and the shared prerequisite-chain predicate. All facts
//! originate from PreparedContainedTask; callers retain material loading and execution.

use crate::codes::ExecutionKernelCode;
use crate::{ContainedTaskError, PreparedContainedTask};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{BundleIndexV3, MaintenanceUse, PackageRef, ReaderProblem};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PREREQUISITE_DEPTH: usize = 3;
pub const MAX_GATED_STEPS: u64 = 1_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskPackageDescriptor {
    package_id: String,
    entry_task_id: String,
    reference: PackageRef,
    execution_mode: String,
    game: String,
    server: String,
    resolution: (u32, u32),
    prerequisite_package_id: Option<String>,
    /// Workflow #373 NAV-3a (M-373 §4.2): the entry a prerequisite gate checks (was the
    /// `linear_steps` first step page alone): `None` when the package has no page entry.
    entry_pages: Option<Vec<String>>,
    maximum_executed_steps: u32,
    startup_incompatibility: Option<&'static str>,
    prerequisite_incompatibility: Option<&'static str>,
    /// Workflow #384 D2-S2: why the package cannot be a recognition maintenance package (its
    /// token is [`TaskPackageDescriptor::use_incompatibility`]'s answer).
    #[serde(skip)]
    recognition_incompatibility: Option<ReaderProblem>,
    /// Workflow #384 D2-S2: the instance fact keys its reading marks write, in declaration
    /// order.
    reading_keys: Vec<String>,
}

impl PreparedContainedTask {
    /// Available only after containment and full offline preparation.
    /// Missing identity or declarations fail preparation; callers must not infer them from
    /// an index or path. Creating this description runs no task or provider.
    pub fn package_descriptor(&self) -> TaskPackageDescriptor {
        TaskPackageDescriptor {
            package_id: self.package_label().to_owned(),
            entry_task_id: self.task_label().to_owned(),
            reference: self.package_sha256().clone(),
            execution_mode: self.execution_mode().to_owned(),
            game: self.game().to_owned(),
            server: self.server().to_owned(),
            resolution: self.resolution(),
            prerequisite_package_id: self.prerequisite_package_id().map(str::to_owned),
            entry_pages: self.gate_entry_pages(),
            maximum_executed_steps: self.maximum_executed_steps(),
            startup_incompatibility: self.startup_incompatibility(),
            prerequisite_incompatibility: self.prerequisite_incompatibility(),
            recognition_incompatibility: self.recognition_incompatibility(),
            reading_keys: self.reading_keys(),
        }
    }
}

impl TaskPackageDescriptor {
    pub fn package_id(&self) -> &str {
        &self.package_id
    }
    pub fn entry_task_id(&self) -> &str {
        &self.entry_task_id
    }
    pub fn reference(&self) -> &PackageRef {
        &self.reference
    }
    pub fn execution_mode(&self) -> &str {
        &self.execution_mode
    }
    pub fn game(&self) -> &str {
        &self.game
    }
    pub fn server(&self) -> &str {
        &self.server
    }
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }
    pub fn prerequisite_package_id(&self) -> Option<&str> {
        self.prerequisite_package_id.as_deref()
    }
    /// Workflow #373 NAV-3a: the entry a prerequisite gate checks; `None` without a page entry.
    pub fn entry_pages(&self) -> Option<&[String]> {
        self.entry_pages.as_deref()
    }
    pub fn maximum_executed_steps(&self) -> u32 {
        self.maximum_executed_steps
    }
    /// Workflow #384 D2-S2: the instance fact keys the package's reading marks write.
    pub fn reading_keys(&self) -> &[String] {
        &self.reading_keys
    }
    /// Workflow #384 D2-S2 (model §3.2): why the package cannot be a recognition maintenance
    /// package, as the binding check reports it; `None` for a package that qualifies.
    pub fn recognition_problem(&self) -> Option<ReaderProblem> {
        self.recognition_incompatibility
    }

    pub fn use_incompatibility(&self, purpose: MaintenanceUse) -> Option<&'static str> {
        match purpose {
            MaintenanceUse::Startup => self.startup_incompatibility,
            MaintenanceUse::Prerequisite => self.prerequisite_incompatibility,
            MaintenanceUse::ReturnHome => self.prerequisite_incompatibility.or_else(|| {
                self.prerequisite_package_id
                    .is_some()
                    .then_some("return_home_declares_prerequisite")
            }),
            MaintenanceUse::Recognition => {
                self.recognition_incompatibility.map(ReaderProblem::as_str)
            }
        }
    }

    /// Workflow #336 PKG-S4 (M-PKG D7): a return-home package is the package its registry entry
    /// names by `package_id` (Workflow #373 NAV-3a: the standard-pack registry of its game), the
    /// identity its control declares (never a digest); another package is refused as a chain
    /// layer is (`contained_task_prerequisite_mismatch`).
    pub fn require_configured_package_id(
        &self,
        package_id: &str,
    ) -> Result<(), ContainedTaskError> {
        if self.package_id == package_id {
            return Ok(());
        }
        Err(ContainedTaskError::with_detail(
            ExecutionKernelCode::ContainedTaskPrerequisiteMismatch.code(),
            format!(
                "package_id={package_id} declared_package_id={} source=return_home",
                self.package_id
            ),
        ))
    }

    /// The configured return-home consumers also require no prerequisite of their own.
    /// Chain layers use the same identity and prerequisite qualification without that extra
    /// restriction, preserving their existing declared-chain behavior.
    pub fn prerequisite_incompatibility_with(
        &self,
        game: &str,
        server: &str,
        resolution: (u32, u32),
        configured_return_home: bool,
    ) -> Option<&'static str> {
        self.prerequisite_incompatibility.or_else(|| {
            if self.game != game {
                Some("game")
            } else if self.server != server {
                Some("server")
            } else if self.resolution != resolution {
                Some("resolution")
            } else if configured_return_home && self.prerequisite_package_id.is_some() {
                Some("return_home_declares_prerequisite")
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct PrerequisiteLink {
    pub package_id: String,
    pub return_home: bool,
    layer: usize,
}

impl PrerequisiteLink {
    pub fn detail(&self, extra: &str) -> String {
        let source = if self.return_home {
            " source=return_home"
        } else {
            ""
        };
        format!(
            "layer={} package_id={}{extra}{source}",
            self.layer, self.package_id
        )
    }
}

/// Incremental pure predicate so Host can record and load each requested layer in its
/// existing order, retaining its deadline, admission errors and failure facts.
pub struct PrerequisiteChain {
    dependent: TaskPackageDescriptor,
    visited: BTreeSet<String>,
    declared: usize,
    layers: usize,
    return_home: bool,
    steps: u64,
}

impl PrerequisiteChain {
    pub fn new(root: TaskPackageDescriptor) -> Self {
        Self {
            visited: BTreeSet::from([root.package_id.clone()]),
            steps: u64::from(root.maximum_executed_steps),
            dependent: root,
            declared: 0,
            layers: 0,
            return_home: false,
        }
    }

    pub fn dependent(&self) -> &TaskPackageDescriptor {
        &self.dependent
    }

    /// Workflow #373 NAV-3a (M-373 §4.2): whether the dependent layer, which declares no
    /// prerequisite package, falls back to the return-home package of its game and server
    /// (R16): it has a page entry and is not the return-home layer itself. A page-graph layer
    /// falls back as a linear one does; a page-graph package run as the run's own package keeps
    /// its home entry instead (the request's recovery binding, else the registry's return-home
    /// package, R23), so it falls back through no chain.
    pub fn may_fall_back(&self) -> bool {
        self.dependent.prerequisite_package_id.is_none()
            && !self.return_home
            && self.dependent.entry_pages.is_some()
            && (self.layers > 0 || self.dependent.execution_mode == "linear_steps")
    }

    /// The next layer: the dependent layer's declared prerequisite package, or the return-home
    /// fallback `return_home_package` when [`Self::may_fall_back`] and it is not in the chain.
    pub fn next(&self, return_home_package: Option<&str>) -> Option<PrerequisiteLink> {
        let next = match &self.dependent.prerequisite_package_id {
            Some(package_id) => (package_id.clone(), false),
            None if !self.may_fall_back() => return None,
            None => {
                let package_id = return_home_package.filter(|id| !self.visited.contains(*id))?;
                (package_id.to_owned(), true)
            }
        };
        Some(PrerequisiteLink {
            package_id: next.0,
            return_home: next.1,
            layer: self.layers + 1,
        })
    }

    /// Call before loading; binding availability, cycle and depth retain their precedence.
    pub fn begin(
        &mut self,
        link: &PrerequisiteLink,
        bound: bool,
    ) -> Result<(), ContainedTaskError> {
        let code = if !bound {
            Some(ExecutionKernelCode::ContainedTaskPrerequisiteUnbound.code())
        } else if !self.visited.insert(link.package_id.clone()) {
            Some(ExecutionKernelCode::ContainedTaskPrerequisiteCycle.code())
        } else if !link.return_home && self.declared >= MAX_PREREQUISITE_DEPTH {
            Some(ExecutionKernelCode::ContainedTaskPrerequisiteDepthExceeded.code())
        } else {
            None
        };
        if let Some(code) = code {
            return Err(ContainedTaskError::with_detail(code, link.detail("")));
        }
        if !link.return_home {
            self.declared += 1;
        }
        Ok(())
    }

    pub fn admit(
        &mut self,
        link: &PrerequisiteLink,
        package: TaskPackageDescriptor,
    ) -> Result<(), ContainedTaskError> {
        if package.package_id != link.package_id {
            return Err(ContainedTaskError::with_detail(
                ExecutionKernelCode::ContainedTaskPrerequisiteMismatch.code(),
                link.detail(&format!(" declared_package_id={}", package.package_id)),
            ));
        }
        if let Some(reason) = package.prerequisite_incompatibility_with(
            &self.dependent.game,
            &self.dependent.server,
            self.dependent.resolution,
            false,
        ) {
            return Err(ContainedTaskError::with_detail(
                SharedCode::ContainedTaskPrerequisiteIncompatible.code(),
                link.detail(&format!(" reason={reason}")),
            ));
        }
        self.steps += u64::from(package.maximum_executed_steps);
        self.dependent = package;
        self.layers += 1;
        self.return_home = link.return_home;
        Ok(())
    }

    pub fn finish(&self) -> Result<(), ContainedTaskError> {
        if self.layers > 0 && self.steps > MAX_GATED_STEPS {
            return Err(ContainedTaskError::with_detail(
                SharedCode::ContainedTaskPrerequisiteStepLimit.code(),
                format!("maximum_executed_steps={}", self.steps),
            ));
        }
        Ok(())
    }
}

/// Validate each pack's identity `(package_id, server, entry_task_id)`, every declared use and
/// the complete chains against precisely the bindings this bundle will supply. Pure and shared
/// with installers; descriptors must be obtained from each admitted pack in this index. The
/// index's digests are compared with nothing (Workflow #336 PKG-S1).
pub fn validate_bundle_maintenance(
    index: &BundleIndexV3,
    packages: &[TaskPackageDescriptor],
) -> Result<(), ContainedTaskError> {
    let invalid = |detail: String| {
        ContainedTaskError::with_detail(
            ExecutionKernelCode::BundleMaintenanceInvalid.code(),
            detail,
        )
    };
    index
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    let by_id: BTreeMap<_, _> = packages
        .iter()
        .map(|pack| (pack.package_id.as_str(), pack))
        .collect();
    if packages.len() != index.packs.len() || by_id.len() != packages.len() {
        return Err(invalid(
            "admitted package set does not match index".to_owned(),
        ));
    }
    for pack in &index.packs {
        let actual = by_id
            .get(pack.package_id.as_str())
            .ok_or_else(|| invalid(format!("package_id={} identity unknown", pack.package_id)))?;
        if actual.game != index.game
            || actual.server != pack.server
            || actual.entry_task_id != pack.entry_task_id
        {
            return Err(invalid(format!(
                "package_id={} index/material mismatch",
                pack.package_id
            )));
        }
    }
    let mut bindings = BTreeSet::new();
    let mut return_home = BTreeMap::new();
    for entry in &index.maintenance {
        let actual = by_id[entry.package_id.as_str()];
        for purpose in &entry.uses {
            if let Some(reason) = actual.use_incompatibility(*purpose) {
                return Err(invalid(format!(
                    "package_id={} use={purpose:?} reason={reason}",
                    entry.package_id
                )));
            }
            if matches!(
                purpose,
                MaintenanceUse::Prerequisite | MaintenanceUse::ReturnHome
            ) {
                bindings.insert(entry.package_id.as_str());
            }
            if *purpose == MaintenanceUse::ReturnHome {
                return_home.insert(entry.server.as_str(), entry.package_id.as_str());
            }
        }
    }
    for root in packages {
        let mut chain = PrerequisiteChain::new(root.clone());
        while let Some(link) = chain.next(return_home.get(chain.dependent().server()).copied()) {
            chain.begin(&link, bindings.contains(link.package_id.as_str()))?;
            chain.admit(&link, by_id[link.package_id.as_str()].clone())?;
        }
        chain.finish()?;
    }
    Ok(())
}

// Test plan v2.2 E-1 G4a (exec key 12): the prerequisite chain and bundle maintenance tables.
#[cfg(test)]
mod gate_e1;

// Workflow #373 NAV-3a, test row N3a-3 (exec key 67): the chain through page-graph layers.
#[cfg(test)]
mod gate_373_nav3a;

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{
        BundleIndexV3Version, BundlePackV2, ContentDirectory, ContentDirectoryVersion,
    };

    /// A described pack whose recorded reference is `reference`.
    fn descriptor(package_id: &str, reference: &str) -> TaskPackageDescriptor {
        TaskPackageDescriptor {
            package_id: package_id.to_owned(),
            entry_task_id: "neutral_task".to_owned(),
            reference: PackageRef::ContentDirectory(ContentDirectory {
                schema_version: ContentDirectoryVersion::V1,
                sha256: reference.to_owned(),
            }),
            execution_mode: "navigable_route".to_owned(),
            game: "neutral".to_owned(),
            server: "test".to_owned(),
            resolution: (2, 1),
            prerequisite_package_id: None,
            entry_pages: None,
            maximum_executed_steps: 1,
            startup_incompatibility: None,
            prerequisite_incompatibility: None,
            recognition_incompatibility: None,
            reading_keys: Vec::new(),
        }
    }

    /// A one-pack bundle index whose pack entry names `package_id` and the digest `a…a`.
    fn index(package_id: &str) -> BundleIndexV3 {
        let digest = "a".repeat(64);
        BundleIndexV3 {
            schema_version: BundleIndexV3Version::V3,
            game: "neutral".to_owned(),
            source: None,
            packs: vec![BundlePackV2 {
                package_id: package_id.to_owned(),
                server: "test".to_owned(),
                entry_task_id: "neutral_task".to_owned(),
                path: format!("packs/{digest}"),
                digest,
                file_count: 3,
                byte_count: 64,
            }],
            maintenance: Vec::new(),
        }
    }

    // Workflow #336 PKG-S1, model M-PKG v1.1 §4.2 S1-T6: bundle maintenance matches each pack
    // by `(package_id, server, entry_task_id)`; the index digest is compared with nothing.
    #[test]
    fn s1_t6_bundle_maintenance_matches_packs_by_identity_and_ignores_the_index_digest() {
        validate_bundle_maintenance(
            &index("neutral.pack"),
            &[descriptor("neutral.pack", &"b".repeat(64))],
        )
        .expect("an index digest that differs from the admitted content does not refuse");
        let error = validate_bundle_maintenance(
            &index("neutral.pack"),
            &[descriptor("neutral.other", &"a".repeat(64))],
        )
        .expect_err("a pack with another package_id is refused");
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::BundleMaintenanceInvalid.as_str()
        );
    }
}
