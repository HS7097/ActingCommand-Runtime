// SPDX-License-Identifier: AGPL-3.0-only

//! Read-only package qualifications and the shared prerequisite-chain predicate. All facts
//! originate from PreparedContainedTask; callers retain material loading and execution.

use crate::{ContainedTaskError, PreparedContainedTask};
use actingcommand_contract::{BundleIndexV3, MaintenanceUse, PackageRef};
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
    linear_entry_page: Option<String>,
    maximum_executed_steps: u32,
    startup_incompatibility: Option<&'static str>,
    prerequisite_incompatibility: Option<&'static str>,
}

impl PreparedContainedTask {
    /// Available only after hash verification, containment and full offline preparation.
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
            linear_entry_page: self.linear_entry_page().map(str::to_owned),
            maximum_executed_steps: self.maximum_executed_steps(),
            startup_incompatibility: self.startup_incompatibility(),
            prerequisite_incompatibility: self.prerequisite_incompatibility(),
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
    pub fn linear_entry_page(&self) -> Option<&str> {
        self.linear_entry_page.as_deref()
    }
    pub fn maximum_executed_steps(&self) -> u32 {
        self.maximum_executed_steps
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
        }
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

    pub fn next(&self, return_home_package: Option<&str>) -> Option<PrerequisiteLink> {
        let next = match &self.dependent.prerequisite_package_id {
            Some(package_id) => (package_id.clone(), false),
            None if self.return_home || self.dependent.linear_entry_page.is_none() => return None,
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
            Some("contained_task_prerequisite_unbound")
        } else if !self.visited.insert(link.package_id.clone()) {
            Some("contained_task_prerequisite_cycle")
        } else if !link.return_home && self.declared >= MAX_PREREQUISITE_DEPTH {
            Some("contained_task_prerequisite_depth_exceeded")
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
                "contained_task_prerequisite_mismatch",
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
                "contained_task_prerequisite_incompatible",
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
                "contained_task_prerequisite_step_limit",
                format!("maximum_executed_steps={}", self.steps),
            ));
        }
        Ok(())
    }
}

/// Validate actual content-directory identities, every declared use and the complete
/// chains against precisely the bindings this bundle will supply. Pure and shared with
/// installers; descriptors must be obtained from each hash-admitted pack in this index.
pub fn validate_bundle_maintenance(
    index: &BundleIndexV3,
    packages: &[TaskPackageDescriptor],
) -> Result<(), ContainedTaskError> {
    let invalid =
        |detail: String| ContainedTaskError::with_detail("bundle_maintenance_invalid", detail);
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
            || !matches!(&actual.reference, PackageRef::ContentDirectory(reference) if reference.sha256 == pack.digest)
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
