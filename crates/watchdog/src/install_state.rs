// SPDX-License-Identifier: AGPL-3.0-only

//! `<root>\install\current.json`, the installer's one unfinished operation (Workflow #421
//! INST-1, `contracts/install-cli.md`, "Documents"), as watchdog row 7a″ reads it.
//!
//! Workflow #434 LAB-4: moved unchanged from actingctl's `install::state` (the document, its
//! kinds and [`unfinished_operation`]) and `install::journal` (the operation id's form), so that
//! actingwatch's own tick reads it as actingctl's does. `actingctl install` still writes the
//! document through these types and reads them from here.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The prefix of an operation id, `op-<unix ms>`.
pub const OPERATION_PREFIX: &str = "op-";

/// The kinds of operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    ReleaseStage,
    Adopt,
    Update,
    Rollback,
    // Workflow #421 INST-2: the formal stop with its hold, the installer's start, the release
    // of a hold.
    Stop,
    Start,
    ReleaseHold,
    // Workflow #434 LAB-3: the Lab's rescue module's `config replace` (and `config restore`),
    // which swaps the configuration file, and `place`, which places a bundle's game root. Neither
    // is on row 7a″'s pass list: while one is unfinished the watchdog stands aside.
    ConfigReplace,
    PackPlace,
}

/// `install\current.json`: the one unfinished operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Current {
    pub operation_id: String,
    pub kind: OperationKind,
    pub started_at_unix_ms: u64,
}

/// The Unix milliseconds an operation id names; `None` for anything else.
pub fn operation_ms(operation_id: &str) -> Option<u64> {
    let digits = operation_id.strip_prefix(OPERATION_PREFIX)?;
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Workflow #421 INST-1, watchdog row 7a″: `current.json` of `root` as the watchdog and
/// `actingctl start` read it. `None` when there is none, or when it names an unfinished
/// `release stage` or `adopt`, which change only `install\` (review S1 F7: the row is for the
/// operations that swap program files, an update or a rollback and their undo); otherwise the
/// unfinished operation's id when the file can be read and names one, `Some(None)` when it
/// cannot.
pub fn unfinished_operation(root: &Path) -> Option<Option<String>> {
    let path = root.join("install").join("current.json");
    if !path.exists() {
        return None;
    }
    match read_current(&path) {
        // Workflow #421 INST-2: a standalone stop, start or release of a hold swaps no program
        // file either.
        Some(current)
            if matches!(
                current.kind,
                OperationKind::ReleaseStage
                    | OperationKind::Adopt
                    | OperationKind::Stop
                    | OperationKind::Start
                    | OperationKind::ReleaseHold
            ) =>
        {
            None
        }
        Some(current) => Some(
            Some(current.operation_id).filter(|operation_id| operation_ms(operation_id).is_some()),
        ),
        None => Some(None),
    }
}

/// `current.json` read whole and parsed, a leading UTF-8 byte order mark skipped (as actingctl's
/// `install::files::read_json` reads it); `None` when it cannot be read or is not this shape.
fn read_current(path: &Path) -> Option<Current> {
    let bytes = std::fs::read(path).ok()?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    serde_json::from_slice(bytes).ok()
}
