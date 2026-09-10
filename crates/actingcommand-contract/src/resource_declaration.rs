// SPDX-License-Identifier: AGPL-3.0-only

//! Procedure configuration declarations shared by Runtime configuration and read-only tooling.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureBindingConfigFile {
    pub procedure_ref: String,
    #[serde(with = "crate::package::prefixed_reference")]
    pub package_digest: crate::PackageRef,
    pub operation_id: String,
    pub yield_points: Vec<String>,
    #[serde(default)]
    pub scheduled_execution: Option<ScheduledExecutionConfigFile>,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledExecutionConfigFile {
    FixtureSimulation {
        #[serde(default)]
        package_path: Option<PathBuf>,
    },
    DeviceRegistry {
        #[serde(default)]
        package_path: Option<PathBuf>,
    },
}
