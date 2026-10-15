// SPDX-License-Identifier: AGPL-3.0-only

use crate::{Lab, LabPorts, LabResult, PackageBundleRequest, PackageBundleResponse};

impl<P: LabPorts> Lab<P> {
    /// Workflow #288 A2b: the standard package's resource section from pack directories
    /// (bundle v4, Workflow #336 PKG-S5).
    pub fn package_bundle(
        &mut self,
        request: PackageBundleRequest,
    ) -> LabResult<PackageBundleResponse> {
        actingcommand_resource_tooling::package_bundle(request)
    }
}
