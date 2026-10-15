// SPDX-License-Identifier: AGPL-3.0-only

//! Procedure configuration declarations shared by Runtime configuration and read-only tooling.

use serde::Deserialize;
use std::path::PathBuf;

/// Workflow #336 PKG-S4 (option 丙): a procedure binding names no package content; its package
/// is found by `scheduled_execution.package_path`. A binding that still names `package_digest`
/// does not decode (`deny_unknown_fields`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcedureBindingConfigFile {
    pub procedure_ref: String,
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
