// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl mcp-entry plan|list` (Workflow #421 MCP-ENTRY D3, D6; contracts/install-cli.md,
//! "The MCP entry"). The entry `actingcommand-mcp-entry` keeps no knowledge of builds: what
//! may change later lives here, in the hot-switchable `actingctl`.
//!
//! - `plan --root <root> --entry-protocol 1 [--current-build <commit>] [-- <child arguments>]`
//!   names the build installed at `<root>` and the child to run for it, and whether the entry
//!   should switch to it. It refuses while an install operation holds `install\writer.lock` or
//!   left `install\current.json` (`writer_lock_failed`, `operation_unfinished`), and when the
//!   installed files differ from their manifests (S1's `verify_live`: `live_build_invalid`,
//!   `live_build_modified`). The tunables come from `install.mcp_entry.*`.
//! - `list --root <root> [--all]` prints the entries' status files
//!   (`<root>\mcp-entry\state\entry-<pid>.json`), each with `stale` when its pid is gone; stale
//!   ones only with `--all`. Nothing is deleted.

use super::state::{self, Paths};
use super::{Answer, Flags, adopt, files, refusal, resolve_root};
use crate::cli_line;
use crate::codes::{ArgumentProblem, InstallLocation, InstallRefusal};
use actingcommand_contract::outcome::{IoOp, Outcome};
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, File, TryLockError};
use std::io::ErrorKind;

/// The contract version between the entry and `mcp-entry plan` (D1: `entry_protocol` 1).
const ENTRY_PROTOCOL: u64 = 1;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum PlanBasis {
    /// The installed build differs from the entry's current child's.
    BuildChanged,
    /// The installed build is the current child's.
    SameBuild,
    /// The entry named no current build: the answer names the installed one.
    NoCurrentBuild,
}

#[derive(Serialize)]
struct PlannedBuild {
    commit: String,
    manifest_sha256: String,
}

#[derive(Serialize)]
struct Tunables {
    poll_ms: u64,
    settle_ms: u64,
    verify_timeout_ms: u64,
    drain_max_ms: u64,
}

/// `mcp-entry plan` (D3).
pub(super) fn plan(words: &[&str]) -> Result<Answer, Outcome> {
    let operation = InstallLocation::McpEntryPlan;
    let (flag_words, child) = match words.iter().position(|word| *word == "--") {
        Some(split) => (&words[..split], &words[split + 1..]),
        None => (words, &words[words.len()..]),
    };
    let flags = Flags::parse(
        flag_words,
        &["--root", "--entry-protocol", "--current-build"],
        &[],
    )?;
    if flags.required("--entry-protocol")?.parse::<u64>().ok() != Some(ENTRY_PROTOCOL) {
        return Err(cli_line::usage_invalid(
            Some("--entry-protocol"),
            ArgumentProblem::NotRecognised,
        ));
    }
    let paths = resolve_root(&flags)?;
    super::refuse_ab(operation, &paths)?;
    if !paths.is_single_location() {
        return Err(refusal(operation, InstallRefusal::NotSingleLocation).path(&paths.root));
    }
    // E-8: never while an install operation is under way.
    if state::unfinished_operation(&paths.root).is_some() {
        return Err(refusal(operation, InstallRefusal::OperationUnfinished).path(paths.current()));
    }
    writer_lock_free(&paths, operation)?;
    let build = adopt::verify_live(operation, &paths)?;
    let manifest = paths.root.join("runtime").join("BUILD-MANIFEST.json");
    let manifest_bytes = files::read_all(&manifest).map_err(|cause| {
        refusal(operation, InstallRefusal::LiveBuildInvalid)
            .path(&manifest)
            .caused_by(cause)
    })?;
    let settings = install_settings(&paths, operation)?;
    let (switch, basis) = match flags.value("--current-build") {
        None => (false, PlanBasis::NoCurrentBuild),
        Some(current) if current.eq_ignore_ascii_case(&build.commit) => {
            (false, PlanBasis::SameBuild)
        }
        Some(_) => (true, PlanBasis::BuildChanged),
    };
    let mut command = vec![files::plain(
        &paths.root.join("runtime").join("actingctl.exe"),
    )];
    command.extend(child.iter().map(|word| (*word).to_owned()));
    Answer::new(super::done(operation))
        .with("switch", switch)?
        .with("reason", basis)?
        .with(
            "build",
            PlannedBuild {
                commit: build.commit,
                manifest_sha256: files::sha256_bytes(&manifest_bytes),
            },
        )?
        .with("child", command)?
        .with(
            "tunables",
            Tunables {
                poll_ms: settings.mcp_entry_poll_ms,
                settle_ms: settings.mcp_entry_settle_ms,
                verify_timeout_ms: settings.mcp_entry_verify_timeout_ms,
                drain_max_ms: settings.mcp_entry_drain_max_ms,
            },
        )?
        .with("entry_protocol", ENTRY_PROTOCOL)
}

/// `install\writer.lock` free: tried shared and released at once. Held, it is
/// `writer_lock_failed`; a file that does not exist is free.
fn writer_lock_free(paths: &Paths, operation: InstallLocation) -> Result<(), Outcome> {
    let path = paths.install().join("writer.lock");
    let failed = || refusal(operation, InstallRefusal::WriterLockFailed).path(&path);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(failed().caused_by(files::os_failure(&path, &error, IoOp::Open)));
        }
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => Err(failed()),
        Err(TryLockError::Error(error)) => {
            Err(failed().caused_by(files::os_failure(&path, &error, IoOp::Lock)))
        }
    }
}

/// The `install` section of the root's configuration.
fn install_settings(
    paths: &Paths,
    operation: InstallLocation,
) -> Result<actingcommand_runtime_config::install::InstallSettings, Outcome> {
    let path = paths.config();
    let unreadable = |cause: Outcome| {
        refusal(operation, InstallRefusal::ConfigUnreadable)
            .path(&path)
            .caused_by(cause)
    };
    let bytes = files::read_all(&path).map_err(unreadable)?;
    let config: Value = serde_json::from_slice(files::without_bom(&bytes))
        .map_err(|error| unreadable(files::json_failure(&error)))?;
    actingcommand_runtime_config::install::settings(&config).map_err(|failure| {
        refusal(operation, InstallRefusal::InstallSectionInvalid)
            .path(&path)
            .caused_by(failure.outcome())
    })
}

/// `mcp-entry list` (D6).
pub(super) fn list(words: &[&str]) -> Result<Answer, Outcome> {
    let operation = InstallLocation::McpEntryList;
    let flags = Flags::parse(words, &["--root"], &["--all"])?;
    let paths = resolve_root(&flags)?;
    let all = flags.switch("--all");
    let directory = paths.root.join("mcp-entry").join("state");
    let mut entries = Vec::new();
    let listing = match fs::read_dir(&directory) {
        Ok(listing) => Some(listing),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(refusal(operation, InstallRefusal::StateInvalid)
                .path(&directory)
                .caused_by(files::os_failure(&directory, &error, IoOp::ReadDir)));
        }
    };
    let mut names = Vec::new();
    for item in listing.into_iter().flatten() {
        let item = item.map_err(|error| {
            refusal(operation, InstallRefusal::StateInvalid)
                .path(&directory)
                .caused_by(files::os_failure(&directory, &error, IoOp::ReadDir))
        })?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with("entry-") && name.ends_with(".json") {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        let path = directory.join(&name);
        let Ok(status) = files::read_json::<Value>(&path) else {
            entries.push(serde_json::json!({"file": name, "unreadable": true}));
            continue;
        };
        let pid = status
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok());
        let alive = match pid {
            Some(pid) => crate::process_probe::pid_alive(pid)?,
            None => false,
        };
        if !alive && !all {
            continue;
        }
        entries.push(serde_json::json!({"file": name, "stale": !alive, "status": status}));
    }
    Answer::new(super::done(operation))
        .with("state_dir", files::plain(&directory))?
        .with("entries", entries)
}
