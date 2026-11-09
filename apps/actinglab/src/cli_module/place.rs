// SPDX-License-Identifier: AGPL-3.0-only

//! Where the CLI module acts (Workflow #434 LAB-2; `contracts/lab-two-entries.md` §9.3): no
//! `--root` and no `--state-root`. The root is located from the program's own place and the state
//! root from the selected configuration (`lab_ops::locate`). `ACTINGCOMMAND_RUNTIME_STATE_ROOT`,
//! which the MCP server's actinglab child and the tests set today, still names the state root when
//! it is set.

use crate::RUNTIME_STATE_ROOT_ENV;
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::locate;
use std::path::PathBuf;

/// The developer text of a state root that could not be located.
pub(crate) const UNLOCATED: &str = "actinglab finds its root from its own place and the state \
     root from the selected configuration: run the installed actinglab, or name the state root \
     with ACTINGCOMMAND_RUNTIME_STATE_ROOT";

/// The root and the state root of this process.
pub(crate) struct Place {
    /// The state root, or why it is unknown.
    pub(crate) state_root: Result<PathBuf, Outcome>,
    /// The install root, when the program runs from one.
    pub(crate) root: Option<PathBuf>,
}

/// This process's place.
pub(crate) fn place() -> Place {
    let located = locate::locate();
    let root = located.as_ref().ok().map(|location| location.root.clone());
    if let Some(state_root) =
        std::env::var_os(RUNTIME_STATE_ROOT_ENV).filter(|value| !value.is_empty())
    {
        return Place {
            state_root: Ok(PathBuf::from(state_root)),
            root,
        };
    }
    Place {
        state_root: located.map(|location| location.state_root),
        root,
    }
}
