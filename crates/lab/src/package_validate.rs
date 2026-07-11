// SPDX-License-Identifier: AGPL-3.0-only

use crate::{Lab, LabPorts, LabResult, PackageValidateRequest, PackageValidationResponse};

impl<P: LabPorts> Lab<P> {
    pub fn package_validate(
        &mut self,
        request: PackageValidateRequest,
    ) -> LabResult<PackageValidationResponse> {
        actingcommand_resource_tooling::validate_package(request)
    }
}
