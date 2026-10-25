// SPDX-License-Identifier: AGPL-3.0-only

//! What `actingctl install` reads from the machine, behind one seam (Workflow #421 INST-1): the
//! clock, the running installer, a work folder, and the test hook after each journalled step.
//! Production is [`SystemHost`]; the tests (`gate_inst1_*`) use fakes with a temporary root.

use actingcommand_contract::outcome::Outcome;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The running installer, as an operation's journal names it (`installer {commit, path}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Installer {
    /// The commit its own build manifest names; `None` when the program runs outside a build
    /// (a development build).
    pub(crate) commit: Option<String>,
    pub(crate) path: String,
}

pub(crate) trait Host {
    /// The wall clock, in Unix milliseconds: operation ids and the `<ms>` of kept names.
    fn now_unix_ms(&self) -> u64;
    /// The running installer.
    fn installer(&self) -> Installer;
    /// Where `release verify` extracts the zips (a new folder under it per call).
    fn work_dir(&self) -> PathBuf;
    /// Called after each journalled step of `operation_id` is recorded done. Production never
    /// fails here; a test returns the failure of a crash at that point, and the operation stops
    /// there exactly as if the process had died (the journal and `current.json` stay).
    fn after_step(&self, _operation_id: &str, _step: usize) -> Result<(), Outcome> {
        Ok(())
    }
}

/// The machine.
pub(crate) struct SystemHost;

impl Host for SystemHost {
    fn now_unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn installer(&self) -> Installer {
        let executable = std::env::current_exe().ok();
        let commit = executable
            .as_deref()
            .and_then(std::path::Path::parent)
            .and_then(|directory| super::manifest::read_manifest(directory).ok())
            .map(|manifest| manifest.commit_sha);
        Installer {
            commit,
            path: executable.map_or_else(String::new, |path| path.display().to_string()),
        }
    }

    fn work_dir(&self) -> PathBuf {
        std::env::temp_dir()
    }
}
