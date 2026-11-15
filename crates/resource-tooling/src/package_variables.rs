// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 V3 (register §1.383; contracts/package-variables.md, "Reading and setting a
//! value by name"): `actinglab package variables --package <directory>`, a core row of the Lab
//! that needs no Runtime. It reads one package's control declaration with the Runtime's metadata
//! reader (`read_remedy_declarations`, the one `standard-pack check` and the Runtime's remedy table
//! read it with) and answers what it declares: the package id, each variable in declaration order
//! (its name, its type, its default as compact JSON and its declaration as written) and, beside
//! them, each parameter its `provides` declares, which `remedy set` writes and no goal does. It
//! reads nothing else and writes nothing; a package the reader refuses is `package_invalid` naming
//! its directory.

use crate::package_directory::PACK_DEADLINE;
use crate::standard_pack_check::CHECK_LABEL;
use actingcommand_contract::{LabError, LabResult, variable_value_json};
use actingcommand_execution_kernel::PreparedContainedTask;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

/// What the package at `package` declares (see the module's description).
pub fn package_variables(package: &Path) -> LabResult<Value> {
    let read =
        PreparedContainedTask::read_remedy_declarations(CHECK_LABEL, package, Instant::now() + PACK_DEADLINE)
            .map_err(|error| {
                LabError::package_invalid(format!("{}: {error}", package.display()))
            })?;
    let variables = read
        .variables
        .iter()
        .map(|variable| {
            json!({
                "name": variable.name,
                "type": variable.variable_type().as_str(),
                "default_json": variable_value_json(&variable.default_value()),
                "declaration": variable.declaration,
            })
        })
        .collect::<Vec<_>>();
    let parameters = read
        .provides
        .iter()
        .flat_map(|capability| {
            capability.parameters().iter().map(move |parameter| {
                json!({
                    "capability": capability.capability(),
                    "irreversible": capability.irreversible(),
                    "parameter_name": parameter.name(),
                    "options": parameter.options(),
                    "default_option": parameter.default_option(),
                })
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "package": package.display().to_string(),
        "package_id": read.package_id,
        "variables": variables,
        "parameters": parameters,
    }))
}
