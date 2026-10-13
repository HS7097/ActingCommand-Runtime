// SPDX-License-Identifier: AGPL-3.0-only

//! The instance selector the CLI and MCP share (#389 v2.2.3 §3.3 item 3; MCP-0 command table
//! v1.1 §2.1, D4): an `instance` argument names one instance of the Runtime's status by its
//! alias, its instance_id or its ADB port. The MCP tools reach it through
//! `mcp::observer::select_instance`; `actingctl observe --export` calls it directly.

use crate::codes::ActingctlCode;
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_contract::RuntimeInstanceStatus;
use actingcommand_contract::outcome::Code;
use serde_json::json;

/// Why a selector names no single instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectorMiss {
    /// No instance has this alias, instance_id or ADB port.
    Unknown,
    /// More than one instance matches it.
    Ambiguous,
}

impl SelectorMiss {
    /// `instance_unknown` or `instance_ambiguous`.
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

/// The one instance of `instances` whose alias, instance_id or ADB port is `selector`.
pub(crate) fn select<'s>(
    instances: &'s [RuntimeInstanceStatus],
    selector: &str,
) -> Result<&'s RuntimeInstanceStatus, SelectorMiss> {
    let port = selector.parse::<u16>().ok();
    let mut matching = instances.iter().filter(|instance| {
        instance.instance_alias() == selector
            || json!(instance.instance_id()).as_str() == Some(selector)
            || port.is_some_and(|port| instance.adb_port() == Some(port))
    });
    let selected = matching.next().ok_or(SelectorMiss::Unknown)?;
    if matching.next().is_some() {
        return Err(SelectorMiss::Ambiguous);
    }
    Ok(selected)
}
