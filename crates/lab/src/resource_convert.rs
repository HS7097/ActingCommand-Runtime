// SPDX-License-Identifier: AGPL-3.0-only

use crate::{Lab, LabPorts, LabResult, ResourceConvertRequest, ResourceConvertResponse};

impl<P: LabPorts> Lab<P> {
    pub fn resource_convert(
        &mut self,
        request: ResourceConvertRequest,
    ) -> LabResult<ResourceConvertResponse> {
        actingcommand_resource_tooling::resource_convert(request)
    }
}
