// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    EnvMarkerResolutionRequest, Lab, LabPorts, LabResult, PackageBuildCatalogMetadata,
    PackageBuildCatalogRequest, PackageBuildTaskRequest, PackageBuildTaskResponse,
    PackageEnvOptions, PackageFullArchiveRequest, PackageTaskArchiveRequest,
};
use actingcommand_resource_tooling::{
    AuthoringEnvironmentSnapshot, PackageBuildCatalog as ResourcePackageBuildCatalog,
    prepare_package_build_task,
};
use std::path::Path;

impl<P: LabPorts> Lab<P> {
    pub fn package_build_task(
        &mut self,
        request: PackageBuildTaskRequest,
    ) -> LabResult<PackageBuildTaskResponse> {
        let env = request.env.clone();
        let prepared = prepare_package_build_task(request)?;
        let environment = resolve_environment_snapshot(
            self,
            &env,
            prepared.resource_root(),
            prepared.required_environment_keys()?,
        )?;
        prepared.build(&environment)
    }
}

pub struct PackageBuildCatalog {
    inner: ResourcePackageBuildCatalog,
}

impl PackageBuildCatalog {
    pub fn open(request: PackageBuildCatalogRequest) -> LabResult<Self> {
        Ok(Self {
            inner: ResourcePackageBuildCatalog::open(request)?,
        })
    }

    pub fn metadata(&self) -> PackageBuildCatalogMetadata {
        self.inner.metadata()
    }

    pub fn task_ids(&self) -> Vec<String> {
        self.inner.task_ids()
    }

    pub fn default_entry_task(&self) -> String {
        self.inner.default_entry_task()
    }

    pub fn build_task_archive<P: LabPorts>(
        &self,
        lab: &mut Lab<P>,
        request: PackageTaskArchiveRequest,
    ) -> LabResult<crate::LabPackageValidationResponse> {
        let environment = resolve_environment_snapshot(
            lab,
            &request.env,
            self.inner.resource_root(),
            self.inner.task_environment_keys(&request.task_id)?,
        )?;
        self.inner.build_task_archive(&environment, request)
    }

    /// Builds into caller-owned staging so a multi-package publication can commit once.
    pub fn build_task_archive_staged<P: LabPorts>(
        &self,
        lab: &mut Lab<P>,
        request: PackageTaskArchiveRequest,
    ) -> LabResult<crate::LabPackageValidationResponse> {
        let environment = resolve_environment_snapshot(
            lab,
            &request.env,
            self.inner.resource_root(),
            self.inner.task_environment_keys(&request.task_id)?,
        )?;
        self.inner.build_task_archive_staged(&environment, request)
    }

    pub fn build_full_archive<P: LabPorts>(
        &self,
        lab: &mut Lab<P>,
        request: PackageFullArchiveRequest,
    ) -> LabResult<crate::LabPackageValidationResponse> {
        let environment = resolve_environment_snapshot(
            lab,
            &request.env,
            self.inner.resource_root(),
            self.inner.full_environment_keys()?,
        )?;
        self.inner.build_full_archive(&environment, request)
    }

    pub fn cleanup(self) -> LabResult<()> {
        self.inner.cleanup()
    }
}

fn resolve_environment_snapshot<P: LabPorts>(
    lab: &mut Lab<P>,
    env: &PackageEnvOptions,
    resource_root: &Path,
    keys: Vec<String>,
) -> LabResult<AuthoringEnvironmentSnapshot> {
    if keys.is_empty() {
        return Ok(AuthoringEnvironmentSnapshot::default());
    }
    let mut markers = keys
        .into_iter()
        .map(|key| format!("{{env:{key}}}"))
        .collect::<Vec<_>>();
    let resolved = lab.resolve_env_markers(
        EnvMarkerResolutionRequest {
            resource_root: resource_root.to_path_buf(),
            instance: env.instance.clone(),
            game: env.game.clone(),
            server: env.server.clone(),
            env_task: env.env_task.clone(),
        },
        &mut markers,
    )?;
    AuthoringEnvironmentSnapshot::from_resolved(resolved)
}
