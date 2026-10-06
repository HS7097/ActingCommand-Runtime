// SPDX-License-Identifier: AGPL-3.0-only

use crate::{
    Lab, LabPorts, LabResult, PackageBundleRequest, PackageBundleResponse, PackageDigestRequest,
    PackageDigestResponse,
};

impl<P: LabPorts> Lab<P> {
    /// Workflow #288 A2b: the content-directory reference of one package directory.
    pub fn package_digest(
        &mut self,
        request: PackageDigestRequest,
    ) -> LabResult<PackageDigestResponse> {
        actingcommand_resource_tooling::package_digest(request)
    }

    /// Workflow #288 A2b: the standard package's resource section from pack directories.
    pub fn package_bundle(
        &mut self,
        request: PackageBundleRequest,
    ) -> LabResult<PackageBundleResponse> {
        actingcommand_resource_tooling::package_bundle(request)
    }
}
