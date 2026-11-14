// SPDX-License-Identifier: AGPL-3.0-only

//! The rescue module's `vision place --from <dir>` (Workflow #453 S9; `contracts/install-cli.md`,
//! "The Lab's rescue module", `vision place`; `contracts/vision-model-folders.md`, "Placing a
//! vision root from a folder"): the files the configured vision root lacks or holds wrong, taken
//! from a folder a person gives, each verified against the installed build's manifest before
//! anything is written, and placed while the Runtime is stopped. Nothing is fetched and nothing
//! names an address.
//!
//! - **Which files, and where.** The live build's `actingd vision check --config <the root's
//!   configuration> --install-root <root>` (Workflow #453 S7, `super::programs::vision_check`,
//!   within `VISION_CHECK_TIMEOUT_MS` and ended there) resolves the configured vision root and its
//!   provider as a start does, and answers every file of that provider's manifest with its size,
//!   its SHA-256 and its state there; the files whose state is not `ok` are the ones to place. The
//!   module links no vision crate: the manifest is the live build's own. A `directml`
//!   configuration's manifest holds every file of the `cpu` one and DirectML's own, so a
//!   configuration that can fall back to the CPU (`gpu_fallback` `cpu`) gets what both need; one
//!   without a `vision` section, or of `cuda`, has no manifest, and actingd refuses it at its
//!   stage (`vision_root_unresolved`, as the row `vision check` answers it).
//! - **Verify before any write.** `--from` is laid out as a vision root. Each file to place is
//!   read from it, its size and then its SHA-256 compared with its pin; one that is missing or
//!   differs refuses the whole command `vision_source_invalid` with its path before anything is
//!   written. Every file of this build's manifest is pinned, its license files too (Workflow #453
//!   S7); a report that names a file without a size and a SHA-256 is no report the row can place
//!   from (`vision_report_unanswered`), so no file is ever copied unverified. A file below `--from`
//!   that the manifest does not name is neither read nor copied; the answer counts them
//!   (`ignored`).
//! - **Only while stopped.** Under the writer lock, with no operation unfinished, the stopped rule
//!   of the update path (`super::launch::refuse_running`: the state root's owner lock held
//!   `owner_lock_held`, an actingd under the root `runtime_running`, a probe that cannot decide
//!   `processes_unreadable`), each before anything is written.
//! - **The operation** (`kind` `vision_place`, off watchdog row 7a″'s pass list, so no Runtime
//!   starts on a half-placed vision root): one `vision_file` step per file to place, in the
//!   manifest's order (the file copied from `--from` to `<dst>.new-<ms>.part` and its SHA-256
//!   checked; a file in its place renamed `<dst>.old-<ms>` and kept; the part renamed in and
//!   checked again), then `post_check`, the same `actingd vision check` again: every manifest file
//!   `ok`, every manifest model's identity matching, the same vision root. A failed post-check
//!   ends the command `post_check_failed` with `undo` as its next step.
//! - **A complete root** answers `installation_changed: false` and opens no operation.
//!
//! Its `resume` and its `undo` take the writer lock and apply the stopped rule again
//! (`super::update::resume`, `super::undo`); `undo` sets each placed file aside as
//! `.rolledback-<ms2>` and renames a kept `.old-<ms>` back. Nothing is deleted, and no file the
//! manifest does not name, and no other folder under `models\`, is ever touched.

use super::deploy::StepEnd;
use super::files;
use super::host::Host;
use super::journal::{self, Journal, Opening, Operation, PlannedStep, Previous, Step, StepKind};
use super::lock::WriterLock;
use super::manifest::Half;
use super::plan::{self, Loaded};
use super::programs::{self, Unanswered};
use super::state::{KindLocation, OperationKind, Paths};
use crate::cli_line::{self, Exit};
use crate::codes::{ArgumentProblem, InstallLocation, InstallRefusal};
use crate::rescue::Context;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{
    Category, CauseRelation, FileState, IoOp, Outcome, RawSource,
};
use actingcommand_lab_ops::core_reads::Params;
use actingcommand_lab_ops::initiator::Initiator;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

/// The command of every outcome here.
const PLACE: InstallLocation = InstallLocation::VisionPlace;
/// The row's parameter, the folder the files are taken from; the journal's input and the
/// answer's member of the same name.
const FROM: &str = "from";
/// Workflow #453 S7 fix 2: the key of an outcome's stage, which actingd's `vision check` names
/// only where it loads, assembles and resolves the vision configuration.
const STAGE: &str = "stage";
/// The journal's inputs and the answer's members: the provider and the vision root actingd's
/// check resolved, and how many files below `--from` the manifest does not name.
const PROVIDER: &str = "provider";
const VISION_ROOT: &str = "vision_root";
const IGNORED: &str = "ignored";
/// The member of a `post_check` intent that marks it as this operation's.
const VISION_CHECK: &str = "vision";
/// The id of the post-check's one row.
const CHECK_ROW: &str = "vision_check";
/// The markers of the names a part an earlier attempt left and a placed file an undo set aside
/// are kept under (`contracts/install-cli.md`, "Layout").
const ABANDONED: &str = "abandoned";
const ROLLED_BACK: &str = "rolledback";
/// The chunk a file is hashed in.
const CHUNK_BYTES: usize = 64 * 1024;

/// One file of the manifest as actingd's check answers it.
#[derive(Debug, Clone)]
struct Pinned {
    /// Its place below the vision root, `/`-separated.
    vision_root_path: String,
    size: u64,
    sha256: String,
    license: bool,
    /// What the vision root holds there: `ok`, `missing`, `sha256_mismatch` or `unreadable`.
    state: String,
}

/// What the live build's `actingd vision check` of the root's configuration answered.
struct Checked {
    received: Outcome,
    report: Value,
    provider: String,
    /// The vision root it resolved, as it spells it (canonical).
    vision_root: PathBuf,
    files: Vec<Pinned>,
    /// The manifest's models with whether their folder's identity matches.
    models: Vec<Value>,
}

/// One `vision_file` step's intent: the file's place, where it is copied from, its part, where a
/// file in its place is kept, and its pin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileIntent {
    vision_root_path: String,
    /// The file below `--from`.
    source: String,
    dst: String,
    part: String,
    old: String,
    size: u64,
    sha256: String,
    /// Whether the vision root held a file there (of another SHA-256, or one it could not read)
    /// when the operation began.
    replaces: bool,
    license: bool,
}

/// What a placement records before its first step.
struct Placing<'a> {
    checked: &'a Checked,
    /// The folder `--from` names, canonical.
    from: &'a Path,
    ignored: usize,
    wanted: Vec<&'a Pinned>,
}

/// What an ended `vision_place` operation answers: its members and its outcome.
pub(crate) struct Finished {
    pub(crate) members: Map<String, Value>,
    pub(crate) outcome: Outcome,
}

/// `vision place --from <dir>` (the module's description): the files the configured vision root
/// lacks or holds with another SHA-256, verified below `--from` and placed while the Runtime is
/// stopped, then checked by the live build's `actingd vision check`.
pub(crate) fn row(context: &Context<'_>, params: Params<'_>) -> Exit {
    let given = params.get(FROM).and_then(Value::as_str).map(PathBuf::from);
    let run = || -> Result<Exit, Outcome> {
        let Some(given) = given.as_deref() else {
            return Err(cli_line::usage_invalid(
                Some("--from"),
                ArgumentProblem::Required,
            ));
        };
        let paths = Paths::resolve(&context.location.root).map_err(|cause| {
            super::refusal(PLACE, InstallRefusal::StateInvalid)
                .path(&context.location.root)
                .caused_by(cause)
        })?;
        let loaded = plan::load(&paths, PLACE)?;
        let from = source_folder(given)?;
        let checked = check(context.host, &paths, PLACE)?;
        let ignored = count_ignored(&from, &checked.files)?;
        let wanted = checked
            .files
            .iter()
            .filter(|file| file.state != FileState::Ok.as_str())
            .collect::<Vec<_>>();
        let mut members = Map::new();
        members.insert(PROVIDER.to_owned(), json!(checked.provider));
        members.insert(
            VISION_ROOT.to_owned(),
            json!(files::plain(&checked.vision_root)),
        );
        members.insert(FROM.to_owned(), json!(files::plain(&from)));
        members.insert(IGNORED.to_owned(), json!(ignored));
        if wanted.is_empty() {
            // A complete root: nothing to place, no operation opened, nothing written.
            members.insert("placed".to_owned(), json!([]));
            members.insert("report".to_owned(), checked.report.clone());
            let done = super::done(PLACE)
                .installation_changed(false)
                .link(CauseRelation::Related, checked.received.clone());
            return Ok(cli_line::success(members, &done));
        }
        if let Some(refused) = refuse_source(&from, &wanted, members) {
            return Ok(refused);
        }
        let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, PLACE)?;
        journal::refuse_unfinished(PLACE, &paths)?;
        super::launch::refuse_running(context.host, &paths, &loaded.state_root, PLACE)?;
        let operation = open(
            context.host,
            context.initiator,
            &paths,
            &loaded,
            &Placing {
                checked: &checked,
                from: &from,
                ignored,
                wanted,
            },
        )?;
        let answer = super::update::run(operation, &loaded)?;
        Ok(super::print(crate::rescue::module_answer(context, answer)))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// The folder `--from` names, canonical; one that does not resolve, or is no folder, is
/// `vision_source_invalid` with the path as given.
fn source_folder(given: &Path) -> Result<PathBuf, Outcome> {
    let invalid = || super::refusal(PLACE, InstallRefusal::VisionSourceInvalid).path(given);
    let folder = fs::canonicalize(given).map_err(|error| {
        invalid().caused_by(files::os_failure(given, &error, IoOp::Canonicalize))
    })?;
    if !folder.is_dir() {
        return Err(invalid());
    }
    Ok(folder)
}

/// How many files below `from` the manifest does not name (by their place, in any ASCII case, as
/// Windows names them): neither read nor copied. A folder that cannot be listed is
/// `vision_source_invalid` with the OS failure.
fn count_ignored(from: &Path, pinned: &[Pinned]) -> Result<usize, Outcome> {
    let named = pinned
        .iter()
        .map(|file| file.vision_root_path.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let listed = files::list_files(from).map_err(|cause| {
        super::refusal(PLACE, InstallRefusal::VisionSourceInvalid)
            .path(from)
            .caused_by(cause)
    })?;
    Ok(listed
        .iter()
        .filter(|relative| !named.contains(&relative.to_ascii_lowercase()))
        .count())
}

/// Each file to place read below `from` and compared with its pin, before anything is written:
/// the first that is missing, of another size or of another SHA-256, or that cannot be read,
/// refuses the command `vision_source_invalid` with its path (the OS failure as the cause for
/// one that cannot be read), the answer's `source` naming its place, its state and its pin.
/// `None` when every one holds its pin.
fn refuse_source(from: &Path, wanted: &[&Pinned], mut members: Map<String, Value>) -> Option<Exit> {
    for file in wanted {
        let source = files::join_relative(from, &file.vision_root_path);
        let (state, cause) = compare(&source, file.size, &file.sha256);
        if state == FileState::Ok {
            continue;
        }
        let refusal = super::refusal(PLACE, InstallRefusal::VisionSourceInvalid)
            .path(Path::new(&files::plain(&source)));
        let refusal = match cause {
            Some(cause) => refusal.caused_by(cause),
            None => refusal,
        };
        members.insert(
            "source".to_owned(),
            json!({
                "vision_root_path": file.vision_root_path,
                "state": state.as_str(),
                "size": file.size,
                "sha256": file.sha256,
            }),
        );
        return Some(cli_line::receipt(members, &refusal, true));
    }
    None
}

/// The refusal of `command` for a report of actingd's check this row cannot use:
/// `vision_report_unanswered`, `detail` (what is wrong with it) its `foreign_data_error` cause.
fn unusable_report(command: InstallLocation, detail: String) -> Outcome {
    super::refusal(command, InstallRefusal::VisionReportUnanswered).caused_by(
        Outcome::new(ContractCode::ForeignDataError)
            .raw_source(RawSource::Json)
            .raw_text(detail),
    )
}

/// The live build's `actingd vision check` of the root's configuration, within
/// `programs::VISION_CHECK_TIMEOUT_MS`, read as the row `vision check` reads it (Workflow #453
/// S7): a refusal at a stage is a root not resolved (`vision_root_unresolved`), every other
/// failure `vision_report_unanswered` (its outcome or the child's failure as the cause, or
/// `timeout_ms` for a child ended at its bound). A report whose provider, vision root (an absolute
/// path), files or models cannot be read, or that names a file without a plain place, a size and a
/// SHA-256, is `vision_report_unanswered` too: no file is placed that cannot be verified.
fn check(host: &dyn Host, paths: &Paths, command: InstallLocation) -> Result<Checked, Outcome> {
    let reported = programs::vision_check(
        host,
        &paths.root.join(Half::Runtime.dir()),
        &paths.config(),
        &paths.root,
        &[],
        programs::VISION_CHECK_TIMEOUT_MS,
    )
    .map_err(|unanswered| {
        let refusal = super::refusal(command, InstallRefusal::VisionReportUnanswered);
        match unanswered {
            Unanswered::Failed(cause) => refusal.joined(CauseRelation::CausedBy, cause),
            Unanswered::Ended => refusal.timeout_ms(programs::VISION_CHECK_TIMEOUT_MS),
        }
    })?;
    if !reported.completed() {
        let resolving = reported.received.category() != Category::Success
            && reported.received.value(STAGE).is_some();
        let refusal = if resolving {
            InstallRefusal::VisionRootUnresolved
        } else {
            InstallRefusal::VisionReportUnanswered
        };
        return Err(
            super::refusal(command, refusal).joined(CauseRelation::CausedBy, reported.received)
        );
    }
    let report = reported.report;
    let provider = report[PROVIDER]
        .as_str()
        .ok_or_else(|| unusable_report(command, "the check report names no provider".to_owned()))?
        .to_owned();
    let vision_root = report[VISION_ROOT]
        .as_str()
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .ok_or_else(|| {
            unusable_report(
                command,
                "the check report names no absolute vision root".to_owned(),
            )
        })?;
    let entries = report["files"]
        .as_array()
        .ok_or_else(|| unusable_report(command, "the check report has no list files".to_owned()))?;
    let models = report["models"].as_array().cloned().ok_or_else(|| {
        unusable_report(command, "the check report has no list models".to_owned())
    })?;
    let mut places = BTreeSet::new();
    let mut pinned = Vec::new();
    for entry in entries {
        let file = pinned_file(command, entry)?;
        if !places.insert(file.vision_root_path.to_ascii_lowercase()) {
            return Err(unusable_report(
                command,
                format!("the check report names {} twice", file.vision_root_path),
            ));
        }
        pinned.push(file);
    }
    Ok(Checked {
        received: reported.received,
        report,
        provider,
        vision_root,
        files: pinned,
        models,
    })
}

/// One file of a check report: its plain place, its pin (`pinned`, a size and a SHA-256) and its
/// state; anything else is [`unusable_report`].
fn pinned_file(command: InstallLocation, entry: &Value) -> Result<Pinned, Outcome> {
    let place = entry["vision_root_path"]
        .as_str()
        .filter(|place| files::is_plain_relative(place))
        .ok_or_else(|| {
            unusable_report(
                command,
                format!("a manifest file names no plain place below the vision root: {entry}"),
            )
        })?;
    let state = entry["state"]
        .as_str()
        .ok_or_else(|| unusable_report(command, format!("{place} has no state")))?;
    let size = entry["size"].as_u64();
    let sha256 = entry["sha256"]
        .as_str()
        .filter(|sha256| files::is_sha256_hex(sha256));
    let (Some(size), Some(sha256), true) = (size, sha256, entry["pinned"] == true) else {
        return Err(unusable_report(
            command,
            format!("{place} has no pin (a size and a SHA-256): no file is placed unverified"),
        ));
    };
    Ok(Pinned {
        vision_root_path: place.to_owned(),
        size,
        sha256: sha256.to_owned(),
        license: entry["license"] == true,
        state: state.to_owned(),
    })
}

/// The SHA-256 of the file at `path` when it is a regular file of `size` bytes, read at most one
/// byte past `size`; `None` when it is not one (no regular file, another size, or another length
/// while it was read).
fn pinned_digest(path: &Path, size: u64) -> io::Result<Option<String>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() != size {
        return Ok(None);
    }
    let mut bounded = File::open(path)?.take(size.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    let mut hashed = 0_u64;
    loop {
        match bounded.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                hashed = hashed.saturating_add(read as u64);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok((hashed == size).then(|| format!("{:x}", hasher.finalize())))
}

/// How the file at `path` compares with the pin `size` and `sha256`: `ok`; `missing`;
/// `sha256_mismatch` (no regular file, another size or another SHA-256); `unreadable`, with the OS
/// failure.
fn compare(path: &Path, size: u64, sha256: &str) -> (FileState, Option<Outcome>) {
    match pinned_digest(path, size) {
        Ok(Some(digest)) if digest == sha256 => (FileState::Ok, None),
        Ok(_) => (FileState::Sha256Mismatch, None),
        Err(error) if error.kind() == ErrorKind::NotFound => (FileState::Missing, None),
        Err(error) => (
            FileState::Unreadable,
            Some(files::os_failure(path, &error, IoOp::Read)),
        ),
    }
}

/// `Ok` when the file at `path` holds the intent's pin; else `vision_source_invalid` of `command`
/// with `path` (the OS failure as the cause for a file that cannot be read): the copy of a file
/// below `--from`, or the placed file, is not the file that was verified.
fn verified(command: InstallLocation, path: &Path, intent: &FileIntent) -> Result<(), Outcome> {
    match compare(path, intent.size, &intent.sha256) {
        (FileState::Ok, _) => Ok(()),
        (_, cause) => {
            let refusal = super::refusal(command, InstallRefusal::VisionSourceInvalid).path(path);
            Err(match cause {
                Some(cause) => refusal.caused_by(cause),
                None => refusal,
            })
        }
    }
}

/// Copies at most `size` and one byte of the file `source` to `part` (its parent created),
/// flushed to disk; what was copied is checked by its caller.
fn copy_at_most(source: &Path, part: &Path, size: u64) -> Result<(), Outcome> {
    if let Some(parent) = part.parent() {
        files::create_dir_all(parent)?;
    }
    let file = File::open(source).map_err(|error| files::os_failure(source, &error, IoOp::Open))?;
    let mut target =
        File::create(part).map_err(|error| files::os_failure(part, &error, IoOp::Create))?;
    io::copy(&mut file.take(size.saturating_add(1)), &mut target)
        .map_err(|error| files::os_failure(part, &error, IoOp::Write))?;
    target
        .sync_all()
        .map_err(|error| files::os_failure(part, &error, IoOp::Sync))
}

/// The `next` of an ended placement: its `undo`.
fn undo_next(paths: &Paths, operation_id: &str) -> Value {
    json!({"undo": ["install", "undo", "--root", paths.plain, "--operation", operation_id]})
}

/// Opens the placement's operation (`vision_place`), the caller holding the writer lock and the
/// Runtime stopped: its inputs (the live build, the provider, the vision root, `--from`, the
/// files it ignores, the configuration's SHA-256), no plan, no S0, no guards, one `vision_file`
/// step per file to place and the `post_check`.
fn open<'h>(
    host: &'h dyn Host,
    initiator: Initiator,
    paths: &Paths,
    loaded: &Loaded,
    placing: &Placing<'_>,
) -> Result<Operation<'h>, Outcome> {
    let live = loaded.state.live.as_ref().map(|live| live.commit.clone());
    let checked = placing.checked;
    let vision_root = files::plain(&checked.vision_root);
    let mut inputs = json!({
        "commit": live,
        "config_sha256": loaded.config_sha256,
    });
    inputs[PROVIDER] = json!(checked.provider);
    inputs[VISION_ROOT] = json!(vision_root);
    inputs[FROM] = json!(files::plain(placing.from));
    inputs[IGNORED] = json!(placing.ignored);
    let opening = Opening {
        kind: OperationKind::VisionPlace,
        initiator: Some(initiator),
        one_way: false,
        inputs,
        plan_id: None,
        plan: None,
        acknowledged: Vec::new(),
        s0: None,
        previous: Previous { live, state: None },
    };
    Operation::open(host, paths, opening, |ms| {
        let mut steps = placing
            .wanted
            .iter()
            .map(|file| {
                let dst = files::join_relative(&checked.vision_root, &file.vision_root_path);
                let intent = FileIntent {
                    vision_root_path: file.vision_root_path.clone(),
                    source: files::plain(&files::join_relative(
                        placing.from,
                        &file.vision_root_path,
                    )),
                    dst: files::plain(&dst),
                    part: files::plain(&files::with_suffix(&dst, &format!(".new-{ms}.part"))),
                    old: files::plain(&files::with_suffix(&dst, &format!(".old-{ms}"))),
                    size: file.size,
                    sha256: file.sha256.clone(),
                    replaces: file.state != FileState::Missing.as_str(),
                    license: file.license,
                };
                (
                    StepKind::VisionFile,
                    intent.dst.clone(),
                    serde_json::to_value(&intent).unwrap_or(Value::Null),
                )
            })
            .collect::<Vec<PlannedStep>>();
        let mut intent = Map::new();
        intent.insert(VISION_CHECK.to_owned(), Value::Bool(true));
        steps.push((
            StepKind::PostCheck,
            vision_root.clone(),
            Value::Object(intent),
        ));
        steps
    })
}

/// One `vision_file` step: the file placed, each half done only when it is missing, so a step
/// interrupted anywhere (a crash between the two renames included) is finished. A file in its
/// place that holds the pin is the step done (a resumed step); one that does not, or that cannot
/// be read, is replaced and kept as `<dst>.old-<ms>`. A part an earlier attempt left that is not
/// the file is kept aside as `.abandoned-<ms>` and written again from `--from`; the part and then
/// the placed file are checked against the pin (`vision_source_invalid` with its path otherwise,
/// which the operation's runner makes `install_operation_interrupted`).
pub(crate) fn file_step(operation: &mut Operation<'_>, n: usize) -> Result<StepEnd, Outcome> {
    let intent: FileIntent = operation.intent(n)?;
    let command = operation.journal().kind.location();
    operation.start(n)?;
    let host = operation.host();
    let dst = PathBuf::from(&intent.dst);
    let part = PathBuf::from(&intent.part);
    let old = PathBuf::from(&intent.old);
    // A file in its place that cannot be read is no verified file: it is replaced, and kept.
    let (in_place, _) = compare(&dst, intent.size, &intent.sha256);
    if in_place != FileState::Ok {
        if part.exists() && compare(&part, intent.size, &intent.sha256).0 != FileState::Ok {
            super::undo::set_aside(&part, ABANDONED, host.now_unix_ms())?;
        }
        if !part.exists() {
            copy_at_most(Path::new(&intent.source), &part, intent.size)?;
        }
        verified(command, &part, &intent)?;
        let operation_id = operation.id().to_owned();
        super::update::swap(command, &dst, &part, Some(old.clone()), || {
            host.between_renames(&operation_id, n)
        })?;
        verified(command, &dst, &intent)?;
    }
    let kept = if old.exists() {
        vec![files::plain(&old)]
    } else {
        Vec::new()
    };
    Ok(StepEnd::Done(Some(json!({
        "vision_root_path": intent.vision_root_path,
        "dst": intent.dst,
        "license": intent.license,
        "kept": kept,
    }))))
}

/// `undo` of one `vision_file` step: the placed file set aside as `.rolledback-<ms2>` when it
/// holds the pin, the kept `.old-<ms>` put back in its place, a part left behind set aside as
/// `.abandoned-<ms2>`. The paths it set aside.
pub(crate) fn reverse_file(step: &Step, ms2: u64) -> Result<Vec<String>, Outcome> {
    let intent: FileIntent =
        serde_json::from_value(step.intent.clone()).map_err(|error| files::json_failure(&error))?;
    let dst = PathBuf::from(&intent.dst);
    let old = PathBuf::from(&intent.old);
    let mut kept = Vec::new();
    if compare(&dst, intent.size, &intent.sha256).0 == FileState::Ok {
        kept.extend(super::undo::set_aside(&dst, ROLLED_BACK, ms2)?);
    }
    if old.exists() && !dst.exists() {
        files::rename(&old, &dst)?;
    }
    kept.extend(super::undo::set_aside(
        Path::new(&intent.part),
        ABANDONED,
        ms2,
    )?);
    Ok(kept)
}

/// Whether a `post_check` step's intent is a placement's of vision files.
pub(crate) fn is_vision_check(intent: &Value) -> bool {
    intent.get(VISION_CHECK).and_then(Value::as_bool) == Some(true)
}

/// The check after the placement: the live build's `actingd vision check` again, one row
/// `vision_check` that passes when every manifest file is `ok`, every manifest model's identity
/// matches and the vision root is the one the operation placed into. A row that fails (or a check
/// that gives no usable report) ends the command `post_check_failed`, the step closed with what it
/// read in the write that ends the operation, the files placed, and `next` naming `undo`.
pub(crate) fn check_step(operation: &mut Operation<'_>, n: usize) -> Result<StepEnd, Outcome> {
    let command = operation.journal().kind.location();
    let expected = operation
        .journal()
        .inputs
        .get(VISION_ROOT)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    operation.start(n)?;
    let paths = operation.paths().clone();
    let row = match check(operation.host(), &paths, command) {
        Ok(checked) => judged(&checked, &expected),
        Err(refused) => json!({
            "id": CHECK_ROW,
            "result": "fail",
            "values": {"failure": cli_line::warning(&refused)},
        }),
    };
    let failed = if row["result"] == "pass" {
        Vec::new()
    } else {
        vec![CHECK_ROW]
    };
    let evidence = json!({"vision_root": expected, "checks": [row], "failed": failed});
    if failed.is_empty() {
        return Ok(StepEnd::Done(Some(evidence)));
    }
    operation.close_with_end(n, evidence.clone());
    let mut members = Map::new();
    members.insert("checks".to_owned(), evidence);
    members.insert("next".to_owned(), undo_next(&paths, operation.id()));
    Ok(StepEnd::Ends(
        super::refusal(command, InstallRefusal::PostCheckFailed).count(failed.len()),
        members,
    ))
}

/// The post-check's row from actingd's report: the files that are not `ok`, the models whose
/// identity does not match, and the vision root against `expected`.
fn judged(checked: &Checked, expected: &str) -> Value {
    let not_ok = checked
        .files
        .iter()
        .filter(|file| file.state != FileState::Ok.as_str())
        .map(|file| json!({"vision_root_path": file.vision_root_path, "state": file.state}))
        .collect::<Vec<_>>();
    let unmatched = checked
        .models
        .iter()
        .filter(|model| model["identity_matches"] != true)
        .map(|model| model["model_ref"].clone())
        .collect::<Vec<_>>();
    let vision_root = files::plain(&checked.vision_root);
    let result = if not_ok.is_empty() && unmatched.is_empty() && vision_root == expected {
        "pass"
    } else {
        "fail"
    };
    json!({
        "id": CHECK_ROW,
        "result": result,
        "values": {
            "provider": checked.provider,
            "vision_root": vision_root,
            "files": checked.files.len(),
            "not_ok": not_ok,
            "models_unmatched": unmatched,
        },
    })
}

/// The answer of a `vision_place` operation whose steps all ran: the provider, the vision root,
/// `--from`, the files it ignored, each file placed (its step's evidence), the post-check, and
/// `next` naming its `undo`; `install_command_done` with the operation's id.
pub(crate) fn finished(journal: &Journal, paths: &Paths) -> Finished {
    let mut members = Map::new();
    for name in [PROVIDER, VISION_ROOT, FROM, IGNORED] {
        members.insert(
            name.to_owned(),
            journal.inputs.get(name).cloned().unwrap_or(Value::Null),
        );
    }
    members.insert(
        "placed".to_owned(),
        Value::Array(
            journal
                .steps
                .iter()
                .filter(|step| step.kind == StepKind::VisionFile)
                .filter_map(|step| step.evidence.clone())
                .collect(),
        ),
    );
    members.insert(
        "checks".to_owned(),
        journal
            .steps
            .iter()
            .find(|step| step.kind == StepKind::PostCheck)
            .and_then(|step| step.evidence.clone())
            .unwrap_or(Value::Null),
    );
    members.insert("next".to_owned(), undo_next(paths, &journal.operation_id));
    Finished {
        members,
        outcome: super::done(journal.kind.location())
            .install_operation_id(journal.operation_id.clone())
            .installation_changed(true),
    }
}
