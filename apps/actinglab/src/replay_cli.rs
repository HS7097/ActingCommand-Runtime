// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #308 SEL-L6a (FCM T-01, T-03): `actinglab replay --package <path> --frames <dir>
//! [--facts <file>]`. The whole package is replayed over a saved frame sequence with the
//! production interpreter (`replay_contained_task`): the entry check, every step with its
//! operation, its destination pages, its guarded inputs and the page it reached, every page
//! recognition, and how the run ended. OCR targets are evaluated with the Runtime's OCR provider
//! (Workflow #396 LAB-3); without an OCR model a package with an OCR target is refused with
//! `lab_ocr_unavailable` before anything is evaluated. The command reads files only: it opens
//! no Runtime connection, touches no device, writes nothing and asks nothing, so a CI job can
//! run it; a replayed run that does not complete exits with `replay_run_refused`.

use super::codes::ActinglabCode;
use super::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{ContainedTaskRequest, InstanceFactSnapshot};
use actingcommand_lab::{
    ExternallyVerifiedBundle, OfflineReplayEnd, OfflineReplayError, OfflineReplayFacts,
    PreparedContainedTask, replay_contained_task,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The instance a replayed package is admitted for.
const REPLAY_INSTANCE: &str = "offline.replay";
/// A Runtime answers one instance's facts within one 1 MiB IPC frame; a saved answer stays well
/// inside this bound.
const MAX_FACTS_BYTES: u64 = 4 * 1024 * 1024;

pub(super) fn run_replay(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    let args = ReplayArgs::parse(global, flags)?;
    let facts = match &args.facts {
        Some(path) => read_facts(path)?,
        // A select step evaluates the empty snapshot at the instant the replay started.
        None => OfflineReplayFacts::Empty {
            now_unix_ms: process_now()?,
        },
    };
    let facts_used = facts_report(&facts);
    let paths = frame_paths(&args.frames)?;
    let task = admit(&args.package)?;
    let frames = paths
        .iter()
        .map(|path| crate::package_cli::offline::read_fixture_frame(path.as_path()));
    let result = replay_contained_task(&task, frames, facts).map_err(|error| match error {
        OfflineReplayError::Frames(error) => error,
        OfflineReplayError::Invariant(error) => CliError::new(
            ErrorKind::UsageValidation,
            error.code().as_str(),
            error.to_string(),
            &[],
        ),
    })?;
    let mut payload = serde_json::to_value(&result)
        .map_err(|error| CliError::device(format!("failed to encode the replay: {error}")))?;
    payload["frames_given"] = json!(paths.len());
    payload["facts"] = facts_used;
    payload["ocr"] = crate::offline_ocr::status();
    payload["production_global_ledger_written"] = json!(false);
    match &result.end {
        OfflineReplayEnd::Completed { .. } => Ok(payload),
        OfflineReplayEnd::Refused { outcome } => Err(CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayRunRefused.as_str(),
            format!(
                "the replayed run ended with {} after {} of {} frames",
                outcome.code().as_str(),
                result.frames_read,
                paths.len()
            ),
            &[],
        )
        .with_details(payload)),
    }
}

struct ReplayArgs {
    package: PathBuf,
    frames: PathBuf,
    facts: Option<PathBuf>,
}

impl ReplayArgs {
    fn parse(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Self> {
        flags.expect_positionals("replay", 0)?;
        if global.run_root.is_some()
            || global.instance.is_some()
            || global.instances.is_some()
            || global.profile.is_some()
            || global.resource_root.is_some()
            || global.dry_run
            || global.verbose
            || global.quiet
            || global.game.is_some()
            || global.server.is_some()
            || global.runtime_endpoint.is_some()
            || global.capture_backend.is_some()
            || global.touch_backend.is_some()
        {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                "offline_device_scope_forbidden",
                "replay accepts no global Runtime, device, selector, backend, resource, or execution flags",
                &[],
            ));
        }
        if let Some(name) = flags
            .flags
            .keys()
            .find(|name| !matches!(name.as_str(), "--package" | "--frames" | "--facts"))
        {
            return Err(CliError::usage(format!("replay does not accept {name}")));
        }
        Ok(Self {
            package: single_path(flags, "--package")?
                .ok_or_else(|| CliError::usage("replay requires --package <path>"))?,
            frames: single_path(flags, "--frames")?
                .ok_or_else(|| CliError::usage("replay requires --frames <directory>"))?,
            facts: single_path(flags, "--facts")?,
        })
    }
}

/// The one value of `name`, `None` when it is not given.
fn single_path(flags: &FlagArgs, name: &str) -> CliOutcome<Option<PathBuf>> {
    match flags.values(name).as_slice() {
        [] => Ok(None),
        [value] if value != "true" && !value.trim().is_empty() => Ok(Some(PathBuf::from(value))),
        _ => Err(CliError::usage(format!(
            "replay takes exactly one non-empty {name} <path>"
        ))),
    }
}

fn process_now() -> CliOutcome<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|now| *now > 0)
        .ok_or_else(|| {
            CliError::new(
                ErrorKind::UsageValidation,
                "offline_clock_invalid",
                "the Lab process clock is before the Unix epoch",
                &[],
            )
        })
}

/// The frames of `directory`: every entry, each a PNG file, in file-name order.
fn frame_paths(directory: &Path) -> CliOutcome<Vec<PathBuf>> {
    let unreadable = |error: std::io::Error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayFramesUnreadable.as_str(),
            format!("failed to list --frames {}: {error}", directory.display()),
            &[],
        )
    };
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let path = entry.path();
        let png = entry.file_type().map_err(unreadable)?.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
        if !png {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                ActinglabCode::ReplayFramesInvalid.as_str(),
                format!(
                    "--frames {} holds {}, which is not a PNG file",
                    directory.display(),
                    path.display()
                ),
                &[],
            ));
        }
        paths.push(path);
    }
    paths.sort();
    if paths.is_empty() {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            SharedCode::OfflineFixtureMissing.as_str(),
            format!("--frames {} holds no PNG frame", directory.display()),
            &[],
        ));
    }
    Ok(paths)
}

/// The package at `package`, read by its path (Workflow #336 option 丙: no reference is given
/// or compared) with the Runtime's OCR provider when actinglab has one.
fn admit(package: &Path) -> CliOutcome<PreparedContainedTask> {
    let package = if package.is_absolute() {
        package.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::package_invalid(error.to_string()))?
            .join(package)
    };
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| CliError::package_invalid("package admission deadline overflow"))?;
    let bundle = ExternallyVerifiedBundle::load_path(
        REPLAY_INSTANCE,
        &package,
        false,
        crate::offline_ocr::lab_ocr().provider(),
        deadline,
    )
    .map_err(|error| CliError::package_invalid(error.to_string()))?;
    crate::offline_ocr::require_package(&bundle)?;
    PreparedContainedTask::from_bundle(bundle).map_err(|error| {
        CliError::package_invalid(format!(
            "{}: {}",
            error.code(),
            error.detail().unwrap_or("task preparation failed")
        ))
    })
}

/// One instance's facts as a Runtime answers them: the result of `actingctl facts --instance`
/// (the members of its line; the line's `outcome` is not read) or of MCP `ac_facts` with scope
/// `instance`.
#[derive(Deserialize)]
struct InstanceFactsAnswer {
    scope: String,
    read_at_unix_ms: u64,
    snapshot: InstanceFactSnapshot,
    records_total: u64,
    records_kept: u64,
}

/// The `--facts` snapshot, evaluated at the instant the Runtime read it. An answer read with a
/// key prefix is refused: it leaves out records a select step may read.
fn read_facts(path: &Path) -> CliOutcome<OfflineReplayFacts> {
    let unreadable = |error: std::io::Error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayFactsUnreadable.as_str(),
            format!("failed to read --facts {}: {error}", path.display()),
            &[],
        )
    };
    let invalid = |problem: String| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::ReplayFactsInvalid.as_str(),
            format!("--facts {}: {problem}", path.display()),
            &[],
        )
    };
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_FACTS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_FACTS_BYTES {
        return Err(invalid(format!("larger than {MAX_FACTS_BYTES} bytes")));
    }
    let answer: InstanceFactsAnswer = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("not an instance fact answer: {error}")))?;
    if answer.scope != "instance" {
        return Err(invalid(format!(
            "an answer of scope {}, not instance",
            answer.scope
        )));
    }
    if answer.records_kept != answer.records_total {
        return Err(invalid(format!(
            "an answer read with a key prefix: {} of {} records kept",
            answer.records_kept, answer.records_total
        )));
    }
    if answer.read_at_unix_ms == 0 {
        return Err(invalid("read_at_unix_ms is 0".to_string()));
    }
    answer
        .snapshot
        .validate()
        .map_err(|error| invalid(format!("the snapshot is invalid: {error}")))?;
    Ok(OfflineReplayFacts::Snapshot {
        snapshot: answer.snapshot,
        read_at_unix_ms: answer.read_at_unix_ms,
    })
}

/// What the replay evaluated a select step with: the given snapshot and the instant it was
/// read, or the empty snapshot and the replay's own instant.
fn facts_report(facts: &OfflineReplayFacts) -> Value {
    match facts {
        OfflineReplayFacts::Empty { now_unix_ms } => {
            json!({"source": "empty", "now_unix_ms": now_unix_ms})
        }
        OfflineReplayFacts::Snapshot {
            snapshot,
            read_at_unix_ms,
        } => json!({
            "source": "snapshot",
            "snapshot_id": snapshot.snapshot_id,
            "records": snapshot.records.len(),
            "read_at_unix_ms": read_at_unix_ms
        }),
    }
}
