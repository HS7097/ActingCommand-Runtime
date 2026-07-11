// SPDX-License-Identifier: AGPL-3.0-only

use crate::{Lab, LabPorts, LabResult, ResourceConvertRequest, ResourceConvertResponse};

pub(crate) use actingcommand_resource_tooling::{
    Bundle, ConvertOutputs, OperationConverter, canonical_game, resolve_resource_root,
};

impl<P: LabPorts> Lab<P> {
    pub fn resource_convert(
        &mut self,
        request: ResourceConvertRequest,
    ) -> LabResult<ResourceConvertResponse> {
        actingcommand_resource_tooling::resource_convert(request)
    }
}
