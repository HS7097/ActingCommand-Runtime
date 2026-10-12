// SPDX-License-Identifier: AGPL-3.0-only

//! The instance selector of actingctl's commands and MCP tools (Workflow #389 R3; MCP-0 command
//! table v1.1 §2.1, D4): an alias, an instance_id or an ADB port, matched against the instances
//! the Runtime's status lists. CLI and MCP resolve a selector with this one function.

use crate::codes::ActingctlCode;
use actingcommand_contract::outcome::Code;
use actingcommand_contract::{RuntimeErrorCode, RuntimeInstanceStatus};
use serde_json::json;

/// Why a selector names no single instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectorError {
    /// No listed instance has this alias, instance_id or ADB port.
    Unknown,
    /// More than one listed instance matches it.
    Ambiguous,
}

impl SelectorError {
    pub(crate) fn code(self) -> Code {
        match self {
            Self::Unknown => RuntimeErrorCode::InstanceUnknown.code(),
            Self::Ambiguous => ActingctlCode::InstanceAmbiguous.code(),
        }
    }

    pub(crate) fn message(self, selector: &str) -> String {
        match self {
            Self::Unknown => {
                format!("no configured instance has the alias, instance_id or ADB port {selector}")
            }
            Self::Ambiguous => {
                format!("{selector} names more than one instance; pass its alias or instance_id")
            }
        }
    }
}

/// The one instance `selector` names by alias, instance_id or ADB port.
pub(crate) fn select<'s>(
    instances: &'s [RuntimeInstanceStatus],
    selector: &str,
) -> Result<&'s RuntimeInstanceStatus, SelectorError> {
    let port = selector.parse::<u16>().ok();
    let mut matching = instances.iter().filter(|instance| {
        instance.instance_alias() == selector
            || json!(instance.instance_id()).as_str() == Some(selector)
            || port.is_some_and(|port| instance.adb_port() == Some(port))
    });
    let selected = matching.next().ok_or(SelectorError::Unknown)?;
    if matching.next().is_some() {
        return Err(SelectorError::Ambiguous);
    }
    Ok(selected)
}
