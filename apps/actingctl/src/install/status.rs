// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install status` and `install leftovers` (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Update and rollback": Status, Leftovers). Both read only.

use super::files;
use super::host::Host;
use super::journal::{self, Operation};
use super::plan::{self, PlanInputs, PlanKind};
use super::state::{self, CurrentState, Paths, StateProblem};
use super::update;
use super::{Answer, Flags};
use crate::codes::{InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::Outcome;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// How many finished operations `status` lists, the newest first.
const LAST_OPERATIONS: usize = 5;
/// The markers of the names the installer keeps (`contracts/install-cli.md`, "Layout"); a deploy
/// kit's `.old-<stamp>` and `.rolledback-<stamp>` files carry the same markers.
const KEPT_MARKERS: [&str; 6] = [
    ".old-",
    ".rolledback-",
    ".abandoned-",
    ".prev-",
    ".finished-",
    ".part",
];
/// The markers whose `<ms>` names the operation that kept the item.
const OPERATION_MARKERS: [&str; 6] = [
    ".old-",
    ".rolledback-",
    ".abandoned-",
    ".prev-",
    ".finished-",
    ".new-",
];

/// One kept file or folder.
#[derive(Debug, Serialize)]
struct Leftover {
    path: String,
    /// The operation that kept it; `None` for an item no journalled operation of this root made
    /// (a deploy kit's `.old-<stamp>` or `.rolledback-<stamp>` file, say).
    operation_id: Option<String>,
    size_bytes: u64,
    held: bool,
}

/// The kept paths the journals record (an undo's `kept`, an abandoned part), by their plain
/// spelling in lower case, each with the operation whose journal records it.
fn recorded_kept(host: &dyn Host, paths: &Paths) -> BTreeMap<String, String> {
    let mut recorded = BTreeMap::new();
    for id in journal::operation_ids(paths).unwrap_or_default() {
        let Ok(operation) = Operation::load(host, paths, &id, InstallLocation::Leftovers) else {
            continue;
        };
        for evidence in operation
            .journal()
            .steps
            .iter()
            .filter_map(|step| step.evidence.as_ref())
        {
            let kept = evidence
                .get("kept")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .chain(evidence.get("abandoned"));
            for path in kept.filter_map(Value::as_str) {
                recorded.insert(path.to_lowercase(), id.clone());
            }
        }
    }
    recorded
}

/// The operation that kept the item at `path` (named `name`): the one whose journal records it,
/// the one a retired `current.json.finished-<ms>` names, or `op-<ms>` of its `<marker>-<ms>` or
/// `.new-<ms>.part` when the root keeps that operation's journal. An item no journalled
/// operation of the root made (a deploy kit's `<file>.old-<stamp>` or `.rolledback-<stamp>`,
/// whatever the stamp's form) has none.
fn operation_of(
    paths: &Paths,
    recorded: &BTreeMap<String, String>,
    path: &Path,
    name: &str,
) -> Option<String> {
    if let Some(operation_id) = recorded.get(&files::plain(path).to_lowercase()) {
        return Some(operation_id.clone());
    }
    if name.starts_with("current.json.finished-") {
        return files::read_json::<state::Current>(path)
            .ok()
            .map(|current| current.operation_id);
    }
    OPERATION_MARKERS.iter().find_map(|marker| {
        let index = name.rfind(marker)?;
        let digits = name[index + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        let operation_id = journal::operation_id(digits.parse::<u64>().ok()?);
        paths
            .journal(&operation_id)
            .is_file()
            .then_some(operation_id)
    })
}

fn size_of(path: &Path) -> u64 {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        Ok(_) => files::list_files(path)
            .map(|listed| {
                listed
                    .iter()
                    .filter_map(|relative| {
                        fs::metadata(files::join_relative(path, relative))
                            .ok()
                            .map(|metadata| metadata.len())
                    })
                    .sum()
            })
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Every kept item below the root's program folders, the root, `install\` and `install\builds\`.
/// A program folder that cannot be listed is the OS's failure; the other folders may be absent.
fn leftovers_of(paths: &Paths) -> Result<Vec<std::path::PathBuf>, Outcome> {
    let kept = |name: &str| KEPT_MARKERS.iter().any(|marker| name.contains(marker));
    let mut found = Vec::new();
    for half in ["runtime", "tools"] {
        let base = paths.root.join(half);
        if base.is_dir() {
            for relative in files::list_files(&base)? {
                let name = relative.rsplit('/').next().unwrap_or(&relative).to_owned();
                if kept(&name) {
                    found.push(files::join_relative(&base, &relative));
                }
            }
        }
    }
    for directory in [paths.root.clone(), paths.install(), paths.builds()] {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if kept(&name) && !(directory == paths.root && (name == "runtime" || name == "tools")) {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// `install leftovers --root <root>`: every kept file and folder with its operation, size and
/// whether a running process holds it. Nothing is deleted.
pub(crate) fn leftovers(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    const LEFTOVERS: InstallLocation = InstallLocation::Leftovers;
    let flags = Flags::parse(words, &["--root"], &[])?;
    let paths = super::resolve_root(&flags)?;
    let images = host.processes_under(&paths.root).map_err(|cause| {
        super::refusal(LEFTOVERS, InstallRefusal::ProcessesUnreadable).caused_by(cause)
    })?;
    let found = leftovers_of(&paths).map_err(|cause| {
        super::refusal(LEFTOVERS, InstallRefusal::LiveBuildInvalid)
            .path(&paths.root)
            .caused_by(cause)
    })?;
    let recorded = recorded_kept(host, &paths);
    let rows = found
        .into_iter()
        .map(|path| {
            let plain = files::plain(&path);
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            Leftover {
                held: images
                    .iter()
                    .any(|image| image.path.eq_ignore_ascii_case(&plain)),
                operation_id: operation_of(&paths, &recorded, &path, &name),
                size_bytes: size_of(&path),
                path: plain,
            }
        })
        .collect::<Vec<_>>();
    Answer::new(super::done(LEFTOVERS).count(rows.len())).with("leftovers", rows)
}

/// `install status --root <root> [--operation <id>] [--check-rollback]`.
pub(crate) fn status(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    const STATUS: InstallLocation = InstallLocation::Status;
    let flags = Flags::parse(words, &["--root", "--operation"], &["--check-rollback"])?;
    let paths = super::resolve_root(&flags)?;
    let layout = if paths.selection().exists() {
        "ab"
    } else if paths.is_single_location() {
        "single"
    } else {
        "unknown"
    };
    let state = match state::load_state(&paths) {
        Ok(state) => Some(state),
        Err(StateProblem::Missing) => None,
        Err(StateProblem::Invalid(cause)) => {
            return Err(journal::state_invalid(STATUS, paths.state(), Some(cause)));
        }
    };
    let unfinished = match state::read_current(&paths) {
        CurrentState::None => Value::Null,
        CurrentState::Unfinished(current) => {
            serde_json::to_value(current).map_err(|error| files::json_failure(&error))?
        }
        CurrentState::Unreadable(cause) => {
            return Err(journal::state_invalid(STATUS, paths.current(), Some(cause)));
        }
    };
    let ids = journal::operation_ids(&paths)?;
    let last = ids
        .iter()
        .rev()
        .filter_map(|id| Operation::load(host, &paths, id, STATUS).ok())
        .filter(|operation| operation.journal().finished())
        .take(LAST_OPERATIONS)
        .map(|operation| {
            let journal = operation.journal();
            json!({
                "operation_id": journal.operation_id,
                "kind": journal.kind,
                "finished_at_unix_ms": journal.finished_at_unix_ms,
                "result": journal.result.as_ref().and_then(|result| result.get("code")).cloned(),
                "undone": journal.undo.is_some(),
            })
        })
        .collect::<Vec<_>>();
    let candidate = paths
        .install()
        .join("candidate")
        .join(state::CONFIG_FILE)
        .is_file();
    let live_config = files::read_all(&paths.config())
        .ok()
        .map(|bytes| format!("sha256:{}", files::sha256_bytes(&bytes)));
    let leftovers = leftovers_of(&paths).map(|found| found.len()).unwrap_or(0);
    // The rollback window's cheap parts toward each kept build other than the live one.
    let live_speaks = super::adopt::verify_live(STATUS, &paths)
        .ok()
        .and_then(|live| live.speaks());
    let window = state
        .as_ref()
        .map(|state| {
            state
                .builds
                .iter()
                .filter(|build| {
                    state
                        .live
                        .as_ref()
                        .is_none_or(|live| live.commit != build.commit)
                })
                .map(|build| {
                    let target = manifest_speaks(&paths.build(&build.commit));
                    let ledger_edge = match (&live_speaks, &target) {
                        (Some(live), Some(target)) => {
                            match (live.get("ledger"), target.get("ledger")) {
                                (Some(live), Some(target)) => {
                                    Value::Bool(target[0] <= live[1] && live[1] <= target[1])
                                }
                                _ => Value::Null,
                            }
                        }
                        _ => Value::Null,
                    };
                    json!({"commit": build.commit, "ledger_edge_passes": ledger_edge})
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut answer = Answer::new(super::done(STATUS))
        .with("root", &paths.plain)?
        .with("layout", layout)?
        .with("adopted", state.is_some())?
        .with("live", state.as_ref().and_then(|state| state.live.clone()))?
        .with("builds", state.as_ref().map(|state| state.builds.clone()))?
        .with("packs", state.as_ref().map(|state| state.packs.clone()))?
        .with(
            "config_sha256",
            json!({
                "committed": state.as_ref().and_then(|state| state.config_sha256.clone()),
                "live": live_config,
                "candidate": candidate,
            }),
        )?
        .with("unfinished", unfinished)?
        .with("last_operations", last)?
        .with("leftovers", leftovers)?
        .with("rollback_window", window)?;
    if let Some(operation_id) = flags.value("--operation") {
        let operation = Operation::load(host, &paths, operation_id, STATUS)
            .map_err(|_| super::unusable("--operation"))?;
        answer = answer.with("journal", operation.journal())?;
    }
    if flags.switch("--check-rollback") {
        answer = answer.with("rollback_gate", check_rollback(host, &paths)?)?;
    }
    Ok(answer)
}

/// The whole rollback gate toward the build the live one replaced.
fn check_rollback(host: &dyn Host, paths: &Paths) -> Result<Value, Outcome> {
    const STATUS: InstallLocation = InstallLocation::Status;
    let loaded = plan::load(paths, STATUS)?;
    let Some(operation_id) = loaded
        .state
        .live
        .as_ref()
        .and_then(|live| live.operation_id.clone())
    else {
        return Ok(json!({"previous": Value::Null}));
    };
    let operation = Operation::load(host, paths, &operation_id, STATUS)?;
    let Some(previous) = operation.journal().previous.live.clone() else {
        return Ok(json!({"previous": Value::Null}));
    };
    let planned = match plan::plan(
        host,
        paths,
        PlanInputs {
            kind: PlanKind::Rollback,
            commit: previous.clone(),
            accept_ledger_one_way: false,
            preview: true,
            operation: STATUS,
        },
    ) {
        Ok(planned) => planned,
        Err(refused) => {
            let answer = refused.into_answer();
            return Ok(json!({
                "previous": previous,
                "refused": crate::cli_line::warning(&answer.outcome),
            }));
        }
    };
    let gate = update::gate(host, paths, &planned);
    let refusal = gate.refusal(&planned);
    // The plan's caveats (a target checked without `--install-root`, say) and the gate's.
    let notes = gate.with_notes(plan::with_notes(super::done(STATUS), &planned.warnings));
    Ok(json!({
        "previous": previous,
        "passes": refusal.is_none(),
        "parts": gate.rows,
        "refused": refusal.as_ref().map(crate::cli_line::warning),
        "noted": notes
            .causes()
            .iter()
            .map(|link| crate::cli_line::warning(link.outcome()))
            .collect::<Vec<_>>(),
    }))
}

/// The Runtime manifest's declaration of the build below `base`.
fn manifest_speaks(base: &Path) -> Option<super::manifest::Speaks> {
    super::manifest::read_manifest(&base.join("runtime"))
        .ok()?
        .speaks()
}
