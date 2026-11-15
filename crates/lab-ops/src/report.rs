// SPDX-License-Identifier: AGPL-3.0-only

//! `report export` (Workflow #467 RPT-1; `contracts/problem-report.md`; register §1.379; the
//! design `problem-report-export-design.md` with the coordinator's rulings, not the owner's
//! words): one archive of the errors view, written into the directory the caller names, from
//! which one can tell which task failed, at which step, on which page, which target was looked
//! for and what it scored against which threshold, what the screen showed and whether the
//! device's frames were the package's size or reduced.
//!
//! The view `errors` (the coordinator's addition of 11-14, from the owner's answer to the
//! design's second question): the newest failed and unfinished runs of the window (at most
//! [`MAX_RUNS`], [`PER_PACKAGE_AND_ENDING`] a package and terminal code), with the run that
//! ended normally before the newest of them on its instance, and the newest rows of the
//! ledger's `errors` view of each instance in the window (at most [`ERRORS_PER_INSTANCE`]),
//! read after the runs (fix round 1, item 7).
//!
//! Read only, and nothing leaves the machine: the row refuses an output inside the
//! installation root or the state root before it reads anything ([`refuse_output`]), reads
//! only through [`Source`] (every read names a request id, or an instance with the window),
//! captures, pins and confirms nothing, opens no file of the state root, and writes one
//! archive into the output directory. Secrets are never in it: the state root's key file and
//! the private binding file are never opened; the configuration is read for its
//! `standard_packs.root` alone and nothing of it is written, its salt included (fix round 1,
//! item 6); and every text part passes through the [`Mask`].

mod archive;
mod scores;
mod source;
mod summary;

pub use archive::{COMPUTER_MARK, Mask, SHORTEST_ALONE, SHORTEST_ANYWHERE, USER_MARK};
// Fix round 2, item 4: the tests read the requests the errors view's search builds through it.
#[cfg(test)]
pub(crate) use source::errors_view;
pub use source::{
    DisplayFacts, Errors, Geometry, Instance, Listed, Missed, PageVerdict, Recognition, RunRow,
    RuntimeSource, Source, TargetVerdict, Which, Window,
};

use crate::cli_line::{self, Exit};
use crate::codes::{
    ActingctlCode, ArgumentProblem, ReportFit, ReportOutputRoot, ReportPart, ReportPartReason,
    ReportWriteStep, RunState,
};
use crate::run_evidence::{Evidence, StepEvidence};
use crate::{material_export, overview, packs};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Category, IoOp, Outcome};
use actingcommand_contract::{InstalledProcess, STANDARD_PACK_BUNDLE_FILE};
use archive::{END_BYTES, Entry};
use scores::Scores;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The row's words in the operation table.
pub const ROW: &str = "report export";
/// The archive's schema.
pub const SCHEMA: &str = "actingcommand.problem-report.v1";
/// The one view this slice builds (the parameter `view`; `package` and `run` are reserved for
/// a later slice).
pub const VIEW_ERRORS: &str = "errors";
/// The window when no `since` is given, and the longest one.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
pub const MAX_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// `max_runs`: its default and its range.
pub const DEFAULT_RUNS: u64 = 5;
pub const MAX_RUNS: u64 = 10;
/// `max_mib`: its default and its range.
pub const DEFAULT_MIB: u64 = 24;
pub const MIN_MIB: u64 = 4;
pub const MAX_MIB: u64 = 64;
pub const MIB: u64 = 1024 * 1024;
/// A diagnostic stream larger than this is not read (the coordinator's ruling Q4).
pub const MAX_STREAM_BYTES: u64 = 8 * MIB;
/// The failing step's records of the stream, at most this many bytes, the newest kept.
pub const MAX_EXCERPT_BYTES: usize = 2 * 1024 * 1024;
/// The rows of the errors view kept for each instance, the newest.
pub const ERRORS_PER_INSTANCE: usize = 200;
/// Of them, the newest this many go into the summary's table.
pub const ERRORS_ON_SUMMARY: usize = 10;
/// The failed and unfinished runs one instance's listing is asked for.
pub const CANDIDATES_PER_INSTANCE: usize = 20;
/// At most this many runs of one package that ended with one code.
pub const PER_PACKAGE_AND_ENDING: usize = 2;
/// A run's steps in `steps.json`, its last.
pub const MAX_STEPS: usize = 1000;
/// Every read of one report within this, and each read within [`READ_STEP`].
pub const READ_BUDGET: Duration = Duration::from_secs(120);
pub const READ_STEP: Duration = Duration::from_secs(20);
/// The largest build manifest and configuration read.
const MAX_MANIFEST_BYTES: u64 = 4 * MIB;
const MAX_CONFIG_BYTES: u64 = MIB;
const BUILD_MANIFEST: &str = "BUILD-MANIFEST.json";
/// The members of an installation whose build manifest is read.
const MEMBERS: [&str; 3] = ["runtime", "tools", "ui"];
/// The members of a build manifest the versions keep, beside its `interfaces`.
const MANIFEST_KEYS: [&str; 4] = [
    "repository",
    "commit_sha",
    "tree_sha",
    "source_artifact_name",
];
/// Every entry's folder in the archive.
const PREFIX: &str = "report/";
const SUMMARY_PAGE: &str = "SUMMARY.md";
const SUMMARY_DATA: &str = "summary.json";
const MANIFEST: &str = "manifest.json";
const VERSIONS: &str = "versions.json";
/// What the report does not hold yet, and the slice that adds it.
const NOT_YET: [(&str, &str); 7] = [
    (
        "the tails of the Runtime's and the console's logs (logs/*.tail.txt)",
        "RPT-3",
    ),
    (
        "the configuration without its secrets (config.redacted.json)",
        "RPT-3",
    ),
    (
        "each instance's schedule and status (instances/<alias>/schedule.json, status.json)",
        "RPT-3",
    ),
    ("the machine (host.json)", "RPT-3"),
    ("the plan of a report before it is written", "RPT-3"),
    ("a report while the Runtime is stopped", "RPT-3"),
    ("the views package and run (successes included)", "RPT-2"),
];
/// Why a part is left out.
const WHY_SIZE: &str = "size_bound";
const WHY_NO_FRAMES: &str = "no_frames";
/// What `no_frames` leaves out, as `left_out` names it.
const NO_FRAMES_FILES: &str = "runs/*/frames/*.png";
/// What the manifest says of the configuration and the secrets (fix round 1, item 6).
const SECRETS: &str = "the configuration is read for its standard_packs.root alone and nothing \
     of it is written, its secret_fingerprint_salt included; the state root's key file and the \
     private device bindings are never opened; no device serial is read";

/// One report's request, read by the row's code.
#[derive(Debug, Clone)]
pub struct Request {
    /// The instance the caller named (alias, `instance_id` or ADB port); none: every instance.
    pub instance: Option<String>,
    /// The output directory, absolute.
    pub out: PathBuf,
    pub since_unix_ms: u64,
    pub until_unix_ms: Option<u64>,
    /// Now, which names the archive and ends the window when no `until_unix_ms` is given.
    pub now_unix_ms: u64,
    pub max_runs: usize,
    /// The archive's bound in bytes.
    pub max_bytes: u64,
    /// Whether frames (and with them OCR text) go into the archive.
    pub frames: bool,
}

/// What the installation the program runs from holds of the versions: each member's build
/// manifest, each game root's bundle source, the Lab's own version; and the warnings of what
/// could not be read.
#[derive(Debug, Clone)]
pub struct Installed {
    pub versions: Value,
    pub warnings: Vec<Outcome>,
}

/// A report written.
#[derive(Debug, Clone)]
pub struct Exported {
    pub path: PathBuf,
    /// `sha256:<hex>` of the archive.
    pub sha256: String,
    pub size: u64,
    pub instances: Vec<String>,
    pub window: Window,
    /// Each run: `{request_id, run_id?, alias, state, package?}`.
    pub runs: Vec<Value>,
    /// The frames the archive holds.
    pub frames: usize,
    /// Each part left out: `{file, why}`.
    pub left_out: Vec<Value>,
    /// Fix round 1, item 4: whether the archive, as written, is within its size bound.
    pub within_bound: bool,
    pub warnings: Vec<Outcome>,
}

/// One frame read: its archive name, its role and its bytes.
struct Frame {
    name: String,
    bytes: Vec<u8>,
}

/// One run read whole.
struct RunReport {
    row: RunRow,
    /// Its folder in the archive (`runs/<run or request id>`).
    dir: String,
    status: Option<Value>,
    evidence: Option<Evidence>,
    /// The failing step's place in `evidence.steps`.
    failing: Option<usize>,
    geometry: Vec<Geometry>,
    recognition: Option<Recognition>,
    scores: Scores,
    excerpt: Vec<Value>,
    excerpt_cut: bool,
    frames: Vec<Frame>,
    warnings: Vec<Outcome>,
}

impl RunReport {
    fn step(&self) -> Option<&StepEvidence> {
        self.evidence.as_ref()?.steps.get(self.failing?)
    }

    /// The newest geometry observation that names the device frame, else the newest.
    fn geometry(&self) -> Option<&Geometry> {
        self.geometry
            .iter()
            .rev()
            .find(|geometry| geometry.device.is_some())
            .or_else(|| self.geometry.last())
    }

    fn excerpt_name(&self) -> String {
        format!("{}/overlay-excerpt.jsonl", self.dir)
    }
}

/// The errors view of one instance: what was read, or why nothing was.
struct InstanceErrors {
    alias: String,
    file: String,
    read: Option<Errors>,
    unread: Option<ReportPartReason>,
}

/// Fix round 1, item 3: whether the listing of one instance's failed and unfinished runs was
/// read to its end; when not, why. Only a listing read to its end says the window holds none.
struct InstanceListing {
    alias: String,
    cut: Option<ReportPartReason>,
}

/// What the summary and its data are written from.
struct Page<'a> {
    request: &'a Request,
    instances: &'a [String],
    versions: &'a Value,
    listings: &'a [InstanceListing],
    reports: &'a [RunReport],
    previous: Option<&'a RunRow>,
    errors: &'a [InstanceErrors],
    /// The names of the parts kept.
    kept: &'a [String],
    left_out: &'a [Value],
    /// Fix round 1, item 4: whether the archive fits its bound.
    fit: ReportFit,
    warnings: &'a [Outcome],
    /// Fix round 2, item 2: the entries' names, kept as they are where the page names them.
    entries: &'a BTreeSet<String>,
}

/// One part of the archive, before the summary and the manifest.
struct Part {
    name: String,
    bytes: Vec<u8>,
    stored: bool,
}

/// A part measured once (fix round 1, item 8): what it adds to the archive in packed bytes, and
/// its digest for the manifest.
struct Measured {
    part: Part,
    packed: u64,
    sha256: String,
}

impl Part {
    /// The part as an archive entry, its bytes copied.
    fn to_entry(&self) -> Entry {
        Entry {
            name: format!("{PREFIX}{}", self.name),
            bytes: self.bytes.clone(),
            stored: self.stored,
        }
    }

    /// The part measured: its packed size and its digest; one that cannot be packed is
    /// `report_write_failed` at the step `pack`.
    fn measured(self, out: &Path) -> Result<Measured, Outcome> {
        let packed =
            archive::packed_size(&self.to_entry()).map_err(|error| pack_failed(out, &error))?;
        let sha256 = hex_sha256(&self.bytes);
        Ok(Measured {
            part: self,
            packed,
            sha256,
        })
    }

    fn json(name: String, value: Value, mask: &Mask) -> Self {
        Self::json_naming(name, value, mask, &BTreeSet::new())
    }

    /// A JSON part whose strings that name an entry of `entries` are kept as they are (fix round
    /// 2, item 2: the manifest and the summary's data name each entry as the archive does).
    fn json_naming(
        name: String,
        mut value: Value,
        mask: &Mask,
        entries: &BTreeSet<String>,
    ) -> Self {
        mask.value_naming(&mut value, entries);
        Self {
            name,
            bytes: format!("{value:#}\n").into_bytes(),
            stored: false,
        }
    }

    fn lines(name: String, records: &[Value], mask: &Mask) -> Self {
        let mut text = String::new();
        for record in records {
            let mut record = record.clone();
            mask.value(&mut record);
            text.push_str(&record.to_string());
            text.push('\n');
        }
        Self {
            name,
            bytes: text.into_bytes(),
            stored: false,
        }
    }

    fn entry(self) -> Entry {
        Entry {
            name: format!("{PREFIX}{}", self.name),
            bytes: self.bytes,
            stored: self.stored,
        }
    }
}

/// A value's lowercase hexadecimal SHA-256.
fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `text` as one word of an archive name: every character but `A-Z a-z 0-9 _ -` as `_` (fix
/// round 2, item 2: its callers hand it text that went through the mask, so a mark is written
/// `_user_` or `_computer_`).
fn file_word(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// `report_part_missing` of `part` for `miss`'s reason (fix round 1: `report_part` and
/// `report_part_reason` say which part and why), `place` adding where it belongs (the instance,
/// the run, the archive's entry), caused by the read's outcome; a fatal cause ends the report
/// instead (the Runtime connection is gone).
fn missing(
    part: ReportPart,
    miss: Missed,
    place: impl FnOnce(Outcome) -> Outcome,
) -> Result<Outcome, Outcome> {
    let Missed { why, cause } = miss;
    let warning = place(
        Outcome::new(ActingctlCode::ReportPartMissing)
            .report_part(part)
            .report_part_reason(why),
    );
    match cause {
        Some(cause) if cause.category() == Category::Fatal => Err(cause),
        Some(cause) => Ok(warning.caused_by(cause)),
        None => Ok(warning),
    }
}

/// A run's part that could not be read: its `request_id` and the archive's `file`.
fn run_missing(
    part: ReportPart,
    miss: Missed,
    request_id: &str,
    file: &str,
) -> Result<Outcome, Outcome> {
    missing(part, miss, |warning| {
        warning.request_id(request_id).file(file)
    })
}

/// A read's value, or none with its warning ([`run_missing`]).
fn read_part<T>(
    read: Result<T, Missed>,
    part: ReportPart,
    file: &str,
    request_id: &str,
    warnings: &mut Vec<Outcome>,
) -> Result<Option<T>, Outcome> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(miss) => {
            warnings.push(run_missing(part, miss, request_id, file)?);
            Ok(None)
        }
    }
}

/// `report_write_failed` at the step `pack`: the archive could not be packed in memory.
fn pack_failed(out: &Path, error: &io::Error) -> Outcome {
    archive::write_failed(ReportWriteStep::Pack, out, IoOp::Write, error)
}

/// The run's row as the archive holds it.
fn row_json(row: &RunRow) -> Value {
    json!({
        "request_id": row.request_id,
        "run_id": row.run_id,
        "alias": row.alias,
        "state": row.state.as_str(),
        "package": row.package,
        "ended_with": row.ended_with,
        "final_page": row.final_page,
        "first_recorded_unix_ms": row.first_unix_ms,
        "ended_unix_ms": row.ended_unix_ms,
    })
}

fn window_json(window: Window) -> Value {
    json!({
        "since_unix_ms": window.since_unix_ms,
        "until_unix_ms": window.until_unix_ms,
    })
}

/// What the report does not hold yet: `{part, slice}`.
fn not_yet() -> Value {
    Value::Array(
        NOT_YET
            .iter()
            .map(|(part, slice)| json!({"part": part, "slice": slice}))
            .collect(),
    )
}

// ---- the output

/// `path` without `.` and with each `..` taken back.
fn lexical(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other.as_os_str()),
        }
    }
    normal
}

/// `path` as a comparable text: its longest existing part with its junctions and links
/// resolved and its letters in the file system's case, the rest as written; `/` for every
/// separator, ASCII lowercase, no verbatim prefix and no separator at the end.
fn resolved(path: &Path) -> String {
    let mut existing = lexical(path);
    let mut rest: Vec<OsString> = Vec::new();
    let real = loop {
        if let Ok(real) = fs::canonicalize(existing.as_path()) {
            break real;
        }
        let name = existing.file_name().map(OsString::from);
        match (name, existing.parent().map(Path::to_path_buf)) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent;
            }
            _ => break existing,
        }
    };
    let mut full = real;
    for name in rest.iter().rev() {
        full.push(name);
    }
    let lowered = full
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if let Some(share) = lowered.strip_prefix("//?/unc/") {
        return format!("//{}", share.trim_end_matches('/'));
    }
    lowered
        .strip_prefix("//?/")
        .unwrap_or(&lowered)
        .trim_end_matches('/')
        .to_owned()
}

/// Whether the resolved `child` is `parent` or lies below it.
fn inside(child: &str, parent: &str) -> bool {
    child == parent
        || child
            .strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The output's refusal, before anything is read: a path that is no directory is
/// `actingctl_usage_invalid` (`--out`, `unusable`); one inside the installation root `root` or
/// the state root `state_root` (each resolved, junctions and letter case included) is
/// `report_output_refused` with the path, `report_output_root` saying which root, and that root.
pub fn refuse_output(out: &Path, root: Option<&Path>, state_root: &Path) -> Result<(), Outcome> {
    if out.exists() && !out.is_dir() {
        return Err(cli_line::usage_invalid(
            Some("--out"),
            ArgumentProblem::Unusable,
        ));
    }
    let target = resolved(out);
    if let Some(root) = root
        && inside(&target, &resolved(root))
    {
        return Err(Outcome::new(ActingctlCode::ReportOutputRefused)
            .path(out)
            .report_output_root(ReportOutputRoot::InstallRoot)
            .install_root(root));
    }
    if inside(&target, &resolved(state_root)) {
        return Err(Outcome::new(ActingctlCode::ReportOutputRefused)
            .path(out)
            .report_output_root(ReportOutputRoot::StateRoot)
            .state_root(state_root));
    }
    Ok(())
}

// ---- the versions

/// A JSON document of at most `limit` bytes, or why not.
fn read_json(path: &Path, limit: u64) -> Result<Value, Outcome> {
    let bytes = overview::read_bounded(path, limit)?;
    serde_json::from_slice(&bytes).map_err(|error| crate::foreign_data_error(&error).path(path))
}

/// One member's build manifest: its [`MANIFEST_KEYS`] and `interfaces`; null when the member
/// has none; an unreadable one null with the warning `install_build_manifest_unreadable`.
fn manifest(path: &Path, warnings: &mut Vec<Outcome>) -> Value {
    if !path.is_file() {
        return Value::Null;
    }
    match read_json(path, MAX_MANIFEST_BYTES) {
        Ok(document) => {
            let mut build = Map::new();
            for key in MANIFEST_KEYS {
                if let Some(value) = document.get(key).filter(|value| value.is_string()) {
                    build.insert(key.to_owned(), value.clone());
                }
            }
            if let Some(interfaces) = document.get("interfaces") {
                build.insert("interfaces".to_owned(), interfaces.clone());
            }
            Value::Object(build)
        }
        Err(cause) => {
            warnings.push(
                Outcome::new(ActingctlCode::InstallBuildManifestUnreadable)
                    .path(path)
                    .caused_by(cause),
            );
            Value::Null
        }
    }
}

/// Each game root's bundle source `{repository, commit}` (the coordinator's ruling Q3: a
/// package is named by its `package_id` and its bundle's source, no digest), from the selected
/// configuration's `standard_packs.root`; null without one. The configuration is read whole (at
/// most 1 MiB) and only that member is used; nothing of it is written (fix round 1, item 6).
fn bundles(root: &Path, warnings: &mut Vec<Outcome>) -> Result<Value, Outcome> {
    let config = match InstalledProcess::read_active(root) {
        Ok(Some(process)) => process.config_path(),
        Ok(None) => root.join(packs::ROOT_CONFIG_FILE),
        Err(error) => return Err(Outcome::new(error.code())),
    };
    if !config.is_file() {
        return Ok(Value::Null);
    }
    let document = read_json(&config, MAX_CONFIG_BYTES)?;
    let Some(games) = document
        .pointer("/standard_packs/root")
        .and_then(Value::as_str)
    else {
        return Ok(Value::Null);
    };
    let games = config
        .parent()
        .map_or_else(|| PathBuf::from(games), |base| base.join(games));
    let entries = fs::read_dir(games.as_path())
        .map_err(|error| Outcome::from_io_error(&error, IoOp::ReadDir).path(games.as_path()))?;
    let mut sources = BTreeMap::new();
    for entry in entries.flatten() {
        let bundle = entry.path().join(STANDARD_PACK_BUNDLE_FILE);
        if !bundle.is_file() {
            continue;
        }
        match read_json(&bundle, MAX_CONFIG_BYTES) {
            Ok(document) => {
                sources.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    json!({
                        "repository": document.pointer("/source/repository"),
                        "commit": document.pointer("/source/commit"),
                    }),
                );
            }
            Err(cause) => warnings.push(versions_missing(cause)),
        }
    }
    Ok(json!(sources))
}

/// `report_part_missing` of the versions (`versions.json`), caused by `cause`: a file that is no
/// JSON is unreadable, any other failure a failed read (a fatal cause is the warning itself).
fn versions_missing(cause: Outcome) -> Outcome {
    let why = if cause.is(ContractCode::ForeignDataError) {
        ReportPartReason::Unreadable
    } else {
        ReportPartReason::ReadFailed
    };
    crate::with_cause(
        Outcome::new(ActingctlCode::ReportPartMissing)
            .report_part(ReportPart::Versions)
            .report_part_reason(why)
            .file(VERSIONS),
        cause,
    )
}

/// The versions of the installation at `root` (none: only the Lab's own).
pub fn installed(root: Option<&Path>) -> Installed {
    let mut warnings = Vec::new();
    let mut versions = Map::new();
    versions.insert("lab_ops".to_owned(), json!(env!("CARGO_PKG_VERSION")));
    if let Some(root) = root {
        for member in MEMBERS {
            let path = root.join(member).join(BUILD_MANIFEST);
            versions.insert(member.to_owned(), manifest(&path, &mut warnings));
        }
        let sources = match bundles(root, &mut warnings) {
            Ok(sources) => sources,
            Err(cause) => {
                warnings.push(versions_missing(cause));
                Value::Null
            }
        };
        versions.insert("bundles".to_owned(), sources);
    }
    Installed {
        versions: Value::Object(versions),
        warnings,
    }
}

// ---- the runs

/// The step the run ended in: the last that carries the run's failure, else the last that did
/// not finish, else the last.
fn failing_step(evidence: &Evidence) -> Option<usize> {
    let steps = &evidence.steps;
    steps
        .iter()
        .rposition(|step| step.failure.is_some())
        .or_else(|| {
            steps
                .iter()
                .rposition(|step| step.finished_unix_ms.is_none())
        })
        .or_else(|| steps.len().checked_sub(1))
}

/// The frames of a run, at most three, each once: the failing step's decided frame and its
/// last frame, and the run's last frame; each its material ref and its role.
fn frame_choice(evidence: &Evidence, failing: Option<usize>) -> Vec<(String, &'static str)> {
    let mut chosen: Vec<(String, &'static str)> = Vec::new();
    let mut add = |reference: &str, role: &'static str| {
        if !chosen.iter().any(|(known, _)| known == reference) {
            chosen.push((reference.to_owned(), role));
        }
    };
    if let Some(step) = failing.and_then(|index| evidence.steps.get(index)) {
        if step.decided_frame
            && let Some((_, reference)) = step.frames.first()
        {
            add(reference.as_str(), "decided");
        }
        if let Some((_, reference)) = step.frames.last() {
            add(reference.as_str(), "step-last");
        }
    }
    if let Some(reference) = evidence.final_["frame_ref"].as_str() {
        add(reference, "run-last");
    }
    chosen
}

/// The newest of `candidates`, at most `max_runs`, at most [`PER_PACKAGE_AND_ENDING`] of one
/// package that ended with one code.
fn select(mut candidates: Vec<RunRow>, max_runs: usize) -> Vec<RunRow> {
    candidates.sort_by(|left, right| {
        right
            .first_unix_ms
            .cmp(&left.first_unix_ms)
            .then_with(|| left.alias.cmp(&right.alias))
            .then_with(|| left.request_id.cmp(&right.request_id))
    });
    let mut taken: Vec<RunRow> = Vec::new();
    for run in candidates {
        if taken.len() == max_runs {
            break;
        }
        let alike = taken
            .iter()
            .filter(|kept| kept.package == run.package && kept.ended_with == run.ended_with)
            .count();
        if alike < PER_PACKAGE_AND_ENDING
            && !taken.iter().any(|kept| kept.request_id == run.request_id)
        {
            taken.push(run);
        }
    }
    taken
}

/// The run first recorded on the newest run's instance just before it in the window, when it
/// ended normally. A listing that failed, or was not read to its end, is `report_part_missing`
/// (`run_before`) naming the instance.
fn run_before(
    source: &dyn Source,
    instances: &[Instance],
    newest: &RunRow,
    window: Window,
    warnings: &mut Vec<Outcome>,
) -> Result<Option<RunRow>, Outcome> {
    let Some(instance) = instances
        .iter()
        .find(|instance| instance.alias == newest.alias)
    else {
        return Ok(None);
    };
    if newest.first_unix_ms <= window.since_unix_ms {
        return Ok(None);
    }
    let before = Window {
        since_unix_ms: window.since_unix_ms,
        until_unix_ms: Some(newest.first_unix_ms - 1),
    };
    let alias = instance.alias.as_str();
    match source.runs(instance, before, Which::Any, 1) {
        Ok(listed) => {
            let found = listed.runs.into_iter().next();
            if found.is_none()
                && let Some(why) = listed.cut
            {
                let miss = Missed { why, cause: None };
                warnings.push(missing(ReportPart::RunBefore, miss, |warning| {
                    warning.alias(alias)
                })?);
            }
            Ok(found.filter(|run| run.state == RunState::Succeeded))
        }
        Err(miss) => {
            warnings.push(missing(ReportPart::RunBefore, miss, |warning| {
                warning.alias(alias)
            })?);
            Ok(None)
        }
    }
}

/// The failed and unfinished runs of every instance, and for each instance whether its listing
/// was read to its end (fix round 1, item 3): a listing that failed or was cut is
/// `report_part_missing` (`run_listing`) naming the instance.
fn list_runs(
    source: &dyn Source,
    instances: &[Instance],
    window: Window,
    warnings: &mut Vec<Outcome>,
) -> Result<(Vec<RunRow>, Vec<InstanceListing>), Outcome> {
    let mut candidates = Vec::new();
    let mut listings = Vec::new();
    for instance in instances {
        let alias = instance.alias.as_str();
        let place = |warning: Outcome| warning.alias(alias);
        let cut = match source.runs(instance, window, Which::Unfinished, CANDIDATES_PER_INSTANCE) {
            Ok(listed) => {
                candidates.extend(listed.runs);
                if let Some(why) = listed.cut {
                    let miss = Missed { why, cause: None };
                    warnings.push(missing(ReportPart::RunListing, miss, place)?);
                }
                listed.cut
            }
            Err(miss) => {
                let why = miss.why;
                warnings.push(missing(ReportPart::RunListing, miss, place)?);
                Some(why)
            }
        };
        listings.push(InstanceListing {
            alias: alias.to_owned(),
            cut,
        });
    }
    Ok((candidates, listings))
}

/// The folder of a run in the archive: `runs/` and its run id, else its request id, through the
/// mask (fix round 2, item 2; an id is kept as it is) as one word of a name.
fn run_dir(row: &RunRow, mask: &Mask) -> String {
    let id = match &row.run_id {
        Some(run_id) => mask.member("run_id", run_id),
        None => mask.member("request_id", &row.request_id),
    };
    format!("runs/{}", file_word(&id))
}

/// The file of an instance's errors view in the archive: `instances/`, its alias through the
/// mask as one word of a name, `/errors.jsonl` (fix round 2, item 2).
fn errors_file(alias: &str, mask: &Mask) -> String {
    format!(
        "instances/{}/errors.jsonl",
        file_word(&mask.member("alias", alias))
    )
}

/// One run read whole: its status, steps, geometry, the failing step's recognition record and
/// stream records with their scores, and its frames.
fn run_report(
    source: &dyn Source,
    row: RunRow,
    request: &Request,
    mask: &Mask,
) -> Result<RunReport, Outcome> {
    let id = row.request_id.clone();
    let dir = run_dir(&row, mask);
    let mut warnings = Vec::new();
    let status = read_part(
        source.run(&id),
        ReportPart::RunStatus,
        &format!("{dir}/run.json"),
        &id,
        &mut warnings,
    )?;
    let evidence = read_part(
        source.evidence(&id),
        ReportPart::RunSteps,
        &format!("{dir}/steps.json"),
        &id,
        &mut warnings,
    )?;
    let geometry = read_part(
        source.geometry(&id),
        ReportPart::RunGeometry,
        &format!("{dir}/geometry.json"),
        &id,
        &mut warnings,
    )?
    .unwrap_or_default();
    let failing = evidence.as_ref().and_then(failing_step);
    let scores_file = format!("{dir}/scores.json");
    // The failing step's span of the ledger: from its first record to the next step's.
    let mut step_index = None;
    let mut recognition = None;
    if let (Some(evidence), Some(index)) = (&evidence, failing) {
        let step = &evidence.steps[index];
        step_index = Some(step.step_index);
        let through = evidence
            .steps
            .get(index + 1)
            .map(|next| next.first_sequence.saturating_sub(1));
        recognition = read_part(
            source.recognitions(&id, step.first_sequence, through),
            ReportPart::RunScores,
            &scores_file,
            &id,
            &mut warnings,
        )?
        .and_then(|mut found| found.pop());
    }
    let mut scores = scores::from_recognition(recognition.as_ref());
    let mut excerpt = Vec::new();
    let mut excerpt_cut = false;
    let excerpt_file = format!("{dir}/overlay-excerpt.jsonl");
    let stream = evidence
        .as_ref()
        .and_then(|evidence| evidence.overlay.as_ref())
        .and_then(|overlay| overlay["ref"].as_str())
        .map(str::to_owned);
    let read_stream = match stream {
        // A run whose steps were read and that has no stream records none; one whose steps
        // could not be read is named missing for that.
        None if evidence.is_some() => Err(Missed {
            why: ReportPartReason::NotRecorded,
            cause: None,
        }),
        None => Ok(None),
        Some(reference) => source.material(&reference, MAX_STREAM_BYTES).map(Some),
    };
    match read_stream {
        Ok(None) => {}
        Ok(Some(bytes)) => {
            let read = scores::stream(&bytes);
            let mut records = scores::span(&read.records, step_index);
            let mut removed = false;
            if !request.frames {
                for record in &mut records {
                    removed |= scores::strip_record(record);
                }
            }
            let from_stream = scores::from_stream(&records);
            if from_stream.source != scores::ScoreSource::Nothing {
                scores = from_stream;
            }
            scores.text_removed = removed;
            (excerpt, excerpt_cut) = scores::bounded(records, MAX_EXCERPT_BYTES);
            if read.unreadable > 0 {
                let cause = crate::foreign_data_error(&format!(
                    "{} lines of the diagnostic stream are no JSON record",
                    read.unreadable
                ));
                let miss = Missed::new(ReportPartReason::Unreadable, cause);
                warnings.push(run_missing(
                    ReportPart::RunExcerpt,
                    miss,
                    &id,
                    &excerpt_file,
                )?);
            }
        }
        Err(miss) => {
            warnings.push(run_missing(
                ReportPart::RunExcerpt,
                miss,
                &id,
                &excerpt_file,
            )?);
        }
    }
    let mut frames = Vec::new();
    if request.frames
        && let Some(evidence) = &evidence
    {
        for (reference, role) in frame_choice(evidence, failing) {
            let sequence = reference.split(':').next().unwrap_or_default();
            let name = format!("{dir}/frames/{sequence}-{role}.png");
            match source.material(&reference, material_export::MAX_EXPORT_BYTES as u64) {
                Ok(bytes) => frames.push(Frame { name, bytes }),
                Err(miss) => warnings.push(run_missing(ReportPart::Frame, miss, &id, &name)?),
            }
        }
    }
    Ok(RunReport {
        row,
        dir,
        status,
        evidence,
        failing,
        geometry,
        recognition,
        scores,
        excerpt,
        excerpt_cut,
        frames,
        warnings,
    })
}

// ---- the parts

fn steps_json(evidence: &Evidence) -> Value {
    let skipped = evidence.steps.len().saturating_sub(MAX_STEPS);
    let mut document = evidence.identity.clone();
    document.insert(
        "steps".to_owned(),
        Value::Array(
            evidence.steps[skipped..]
                .iter()
                .map(StepEvidence::full_row)
                .collect(),
        ),
    );
    if skipped > 0 {
        document.insert("steps_left_out".to_owned(), json!(skipped));
    }
    document.insert("final".to_owned(), evidence.final_.clone());
    if let Some(overlay) = &evidence.overlay {
        document.insert("overlay".to_owned(), overlay.clone());
    }
    Value::Object(document)
}

fn recognition_json(recognition: &Recognition) -> Value {
    json!({
        "sequence": recognition.sequence,
        "matched_page": recognition.matched_page,
        "frame": {"width": recognition.frame.0, "height": recognition.frame.1},
        "targets": recognition.targets.iter().map(|target| json!({
            "target": target.target,
            "matched": target.matched,
        })).collect::<Vec<_>>(),
        "pages": recognition.pages.iter().map(|page| json!({
            "page": page.page,
            "matched": page.matched,
            "best_milli": page.best_milli,
            "first_failed_target": page.first_failed,
        })).collect::<Vec<_>>(),
        "pages_omitted": recognition.omitted,
    })
}

fn geometry_json(report: &RunReport) -> Value {
    json!({
        "display": summary::display_line(report.geometry()),
        "observations": report.geometry.iter().map(|geometry| json!({
            "device": geometry.device.map(|(width, height)| json!({"width": width, "height": height})),
            "display_map": geometry.map.as_ref().map(|map| json!({
                "kind": map.kind,
                "scale": format!("{}/{}", map.scale_num, map.scale_den),
                "scale_frame": {"width": map.scale_width, "height": map.scale_height},
                "package": {"width": map.base_width, "height": map.base_height},
            })),
            "conclusion": geometry.conclusion,
        })).collect::<Vec<_>>(),
    })
}

fn report_parts(report: &RunReport, mask: &Mask, parts: &mut Vec<Part>) {
    let dir = &report.dir;
    let run = json!({"listing": row_json(&report.row), "status": report.status});
    parts.push(Part::json(format!("{dir}/run.json"), run, mask));
    if let Some(evidence) = &report.evidence {
        parts.push(Part::json(
            format!("{dir}/steps.json"),
            steps_json(evidence),
            mask,
        ));
    }
    let scores = json!({
        "step": report.step().map(|step| step.step_index),
        "scores": report.scores.json(),
        "recognition": report.recognition.as_ref().map(recognition_json),
        "excerpt_cut": report.excerpt_cut,
    });
    parts.push(Part::json(format!("{dir}/scores.json"), scores, mask));
    parts.push(Part::json(
        format!("{dir}/geometry.json"),
        geometry_json(report),
        mask,
    ));
    if !report.excerpt.is_empty() {
        parts.push(Part::lines(report.excerpt_name(), &report.excerpt, mask));
    }
    for frame in &report.frames {
        parts.push(Part {
            name: frame.name.clone(),
            bytes: frame.bytes.clone(),
            stored: true,
        });
    }
}

/// The parts the size bound leaves out first, in order: the frames of the older runs (the
/// oldest first, each run's last frame first), then the newest run's; then the stream
/// excerpts, the oldest run's first; then the errors files, the last instance's first.
fn drop_order(reports: &[RunReport], errors: &[InstanceErrors]) -> Vec<String> {
    let mut order = Vec::new();
    for report in reports.iter().rev() {
        for frame in report.frames.iter().rev() {
            order.push(frame.name.clone());
        }
    }
    for report in reports.iter().rev() {
        if !report.excerpt.is_empty() {
            order.push(report.excerpt_name());
        }
    }
    for instance in errors.iter().rev() {
        if instance.read.is_some() {
            order.push(instance.file.clone());
        }
    }
    order
}

/// Fix round 2, item 2: the name of every entry the report made or names, each built from text
/// that went through the mask ([`run_dir`], [`errors_file`]): the parts, each run's frames and
/// excerpt, each instance's errors file, every file a warning names (a part that is missing),
/// the three entries rendered last, the versions and what `no_frames` leaves out; each with and
/// without the archive's folder.
fn entry_names(
    parts: &[Part],
    reports: &[RunReport],
    errors: &[InstanceErrors],
    warnings: &[Outcome],
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    names.extend(parts.iter().map(|part| part.name.clone()));
    for report in reports {
        names.extend(report.frames.iter().map(|frame| frame.name.clone()));
        names.insert(report.excerpt_name());
    }
    names.extend(errors.iter().map(|instance| instance.file.clone()));
    names.extend(
        warnings
            .iter()
            .filter_map(|warning| warning.value("file")?.as_str().map(str::to_owned)),
    );
    names.extend(
        [
            SUMMARY_PAGE,
            MANIFEST,
            SUMMARY_DATA,
            VERSIONS,
            NO_FRAMES_FILES,
        ]
        .map(str::to_owned),
    );
    let in_folder = names
        .iter()
        .map(|name| format!("{PREFIX}{name}"))
        .collect::<Vec<_>>();
    names.extend(in_folder);
    names
}

/// What every rendering of the summary and the manifest is made from: what was read, and the
/// warnings and the masks applied before the size bound's trimming.
struct Read<'a> {
    request: &'a Request,
    aliases: &'a [String],
    versions: &'a Value,
    listings: &'a [InstanceListing],
    reports: &'a [RunReport],
    previous: Option<&'a RunRow>,
    errors: &'a [InstanceErrors],
    warnings: &'a [Outcome],
    masks_before: usize,
    /// Fix round 2, item 2: the name of every entry the report made, each from masked text,
    /// with and without its folder: kept as it is wherever the summary and the manifest name it.
    entries: &'a BTreeSet<String>,
}

/// The summary page, the manifest and the summary's data, rendered for one choice of the parts
/// kept, and the archive's packed size with them.
struct Rendered {
    summary_page: Part,
    manifest: Part,
    summary_data: Part,
    left_out: Vec<Value>,
    warnings: Vec<Outcome>,
    total: u64,
}

/// One row of the manifest's `files`.
fn file_row(name: &str, size: usize, sha256: &str) -> Value {
    json!({"path": format!("{PREFIX}{name}"), "size": size, "sha256": sha256})
}

/// The summary page, the manifest and the summary's data with the parts `measured` less those
/// `left` marks (the names `dropped`, in the order they were left out) and the bound's verdict
/// `fit`; and the archive's size in packed bytes: the parts' own and the three rendered ones',
/// each measured as the archive holds it, and the archive's end record (fix round 1, item 8).
fn render(
    read: &Read<'_>,
    measured: &[Measured],
    left: &[bool],
    dropped: &[String],
    fit: ReportFit,
    mask: &Mask,
) -> Result<Rendered, Outcome> {
    let request = read.request;
    let mut left_out = dropped
        .iter()
        .map(|file| json!({"file": file, "why": WHY_SIZE}))
        .collect::<Vec<_>>();
    if !request.frames {
        left_out.push(json!({"file": NO_FRAMES_FILES, "why": WHY_NO_FRAMES}));
    }
    let mut warnings = read.warnings.to_vec();
    if !dropped.is_empty() || fit == ReportFit::StillOver {
        warnings.push(
            Outcome::new(ActingctlCode::ReportTrimmed)
                .limit_bytes(request.max_bytes)
                .count(dropped.len())
                .report_fit(fit),
        );
    }
    let kept = measured
        .iter()
        .zip(left)
        .filter(|(_, left)| !**left)
        .map(|(part, _)| part)
        .collect::<Vec<_>>();
    let kept_names = kept
        .iter()
        .map(|measured| measured.part.name.clone())
        .collect::<Vec<_>>();
    let page = Page {
        request,
        instances: read.aliases,
        versions: read.versions,
        listings: read.listings,
        reports: read.reports,
        previous: read.previous,
        errors: read.errors,
        kept: &kept_names,
        left_out: &left_out,
        fit,
        warnings: &warnings,
        entries: read.entries,
    };
    // Fix round 2, item 1: the page masks each value as it writes it, so that its labels, codes,
    // ids and entry names stay whole.
    let masks_start = mask.applied();
    let summary_page = Part {
        name: SUMMARY_PAGE.to_owned(),
        bytes: summary::page(&page, mask).into_bytes(),
        stored: false,
    };
    let summary_data = Part::json_naming(
        SUMMARY_DATA.to_owned(),
        summary::data(&page),
        mask,
        read.entries,
    );
    let masks_applied = read.masks_before + (mask.applied() - masks_start);
    let files = [&summary_page, &summary_data]
        .iter()
        .map(|part| file_row(&part.name, part.bytes.len(), &hex_sha256(&part.bytes)))
        .chain(kept.iter().map(|measured| {
            file_row(
                &measured.part.name,
                measured.part.bytes.len(),
                &measured.sha256,
            )
        }))
        .collect::<Vec<_>>();
    let manifest = json!({
        "schema_version": SCHEMA,
        "view": VIEW_ERRORS,
        "tool": {"program": "actinglab", "lab_ops": env!("CARGO_PKG_VERSION")},
        "generated_unix_ms": request.now_unix_ms,
        "instances": read.aliases,
        "window": window_json(Window {
            since_unix_ms: request.since_unix_ms,
            until_unix_ms: request.until_unix_ms,
        }),
        "max_runs": request.max_runs,
        "max_bytes": request.max_bytes,
        "within_bound": fit == ReportFit::Fits,
        "runtime_online": true,
        "frames": request.frames,
        "files": files,
        "left_out": left_out,
        "not_in_this_report_yet": not_yet(),
        "secrets": SECRETS,
        "masks_applied": masks_applied,
        "names_not_looked_for": mask.not_looked_for(),
        "warnings": warnings.iter().map(cli_line::warning).collect::<Vec<_>>(),
    });
    let manifest = Part::json_naming(MANIFEST.to_owned(), manifest, mask, read.entries);
    let mut total = END_BYTES + kept.iter().map(|measured| measured.packed).sum::<u64>();
    for part in [&summary_page, &manifest, &summary_data] {
        total += archive::packed_size(&part.to_entry())
            .map_err(|error| pack_failed(&request.out, &error))?;
    }
    Ok(Rendered {
        summary_page,
        manifest,
        summary_data,
        left_out,
        warnings,
        total,
    })
}

/// The report `request` asks for, read through `source`, with the installation's versions
/// `installed`, written into the request's output directory with every text part masked by
/// `mask`. The instances' read and a fatal read end it with that read's outcome; any other
/// read that fails, or does not reach its end, names its part with the warning
/// `report_part_missing`. The runs are read first, then each instance's errors view (fix round
/// 1, item 7). The size bound is measured on the packed archive; the parts of [`drop_order`]
/// are left out in turn until it fits, and when it does not fit with all of them left out the
/// manifest, the summary and the answer say so (fix round 1, items 4 and 8).
pub fn export(
    source: &dyn Source,
    request: &Request,
    installed: &Installed,
    mask: &Mask,
) -> Result<Exported, Outcome> {
    let window = Window {
        since_unix_ms: request.since_unix_ms,
        until_unix_ms: request.until_unix_ms,
    };
    let instances = source.instances(request.instance.as_deref())?;
    let aliases = instances
        .iter()
        .map(|instance| instance.alias.clone())
        .collect::<Vec<_>>();
    let mut warnings = installed.warnings.clone();
    // The failed and unfinished runs, instance by instance, and each run whole.
    let (candidates, listings) = list_runs(source, &instances, window, &mut warnings)?;
    let selected = select(candidates, request.max_runs);
    let previous = match selected.first() {
        Some(newest) => run_before(source, &instances, newest, window, &mut warnings)?,
        None => None,
    };
    let mut reports = Vec::new();
    for row in selected {
        let report = run_report(source, row, request, mask)?;
        warnings.extend(report.warnings.iter().cloned());
        reports.push(report);
    }
    // Then the errors view, instance by instance: its newest rows.
    let mut errors = Vec::new();
    for instance in &instances {
        let alias = instance.alias.as_str();
        let file = errors_file(alias, mask);
        let place = |warning: Outcome| warning.alias(alias).file(file.as_str());
        let (read, unread) = match source.errors(instance, window, ERRORS_PER_INSTANCE) {
            Ok(mut read) => {
                if !request.frames {
                    for row in &mut read.rows {
                        scores::strip_row(row);
                    }
                }
                // Fix round 2, item 5: a search that stopped above the window's first record
                // says why, the Runtime's cut (`not_read_to_end`) or its windows spent
                // (`search_windows_spent`).
                if let Some(why) = read.stopped {
                    let miss = Missed { why, cause: None };
                    warnings.push(missing(ReportPart::ErrorsView, miss, place)?);
                }
                (Some(read), None)
            }
            Err(miss) => {
                let why = miss.why;
                warnings.push(missing(ReportPart::ErrorsView, miss, place)?);
                (None, Some(why))
            }
        };
        errors.push(InstanceErrors {
            alias: instance.alias.clone(),
            file,
            read,
            unread,
        });
    }

    // The parts, each measured once.
    let mut parts = vec![Part::json(
        VERSIONS.to_owned(),
        installed.versions.clone(),
        mask,
    )];
    for report in &reports {
        report_parts(report, mask, &mut parts);
    }
    for instance in &errors {
        if let Some(read) = &instance.read {
            parts.push(Part::lines(instance.file.clone(), &read.rows, mask));
        }
    }
    let entries = entry_names(&parts, &reports, &errors, &warnings);
    let measured = parts
        .into_iter()
        .map(|part| part.measured(&request.out))
        .collect::<Result<Vec<_>, _>>()?;
    let read = Read {
        request,
        aliases: &aliases,
        versions: &installed.versions,
        listings: &listings,
        reports: &reports,
        previous: previous.as_ref(),
        errors: &errors,
        warnings: &warnings,
        masks_before: mask.applied(),
        entries: &entries,
    };
    // Within the size bound: the parts of the drop order left out in turn while the archive is
    // over it.
    let order = drop_order(&reports, &errors);
    let mut pending = order.iter();
    let mut left = vec![false; measured.len()];
    let mut dropped: Vec<String> = Vec::new();
    let (rendered, fit) = loop {
        let rendered = render(&read, &measured, &left, &dropped, ReportFit::Fits, mask)?;
        if rendered.total <= request.max_bytes {
            break (rendered, ReportFit::Fits);
        }
        let next = pending.find_map(|name| {
            measured
                .iter()
                .position(|kept| kept.part.name == *name)
                .filter(|place| !left[*place])
                .map(|place| (place, name))
        });
        match next {
            Some((place, name)) => {
                left[place] = true;
                dropped.push(name.clone());
            }
            None => {
                let over = render(
                    &read,
                    &measured,
                    &left,
                    &dropped,
                    ReportFit::StillOver,
                    mask,
                )?;
                break (over, ReportFit::StillOver);
            }
        }
    };
    let Rendered {
        summary_page,
        manifest,
        summary_data,
        left_out,
        warnings: written_warnings,
        ..
    } = rendered;
    let entries = [summary_page, manifest, summary_data]
        .into_iter()
        .chain(
            measured
                .into_iter()
                .zip(left)
                .filter(|(_, left)| !left)
                .map(|(kept, _)| kept.part),
        )
        .map(Part::entry)
        .collect::<Vec<_>>();
    let bytes = archive::zipped(&entries).map_err(|error| pack_failed(&request.out, &error))?;
    let digest = hex_sha256(&bytes);
    let name = format!(
        "problem-report-{}Z-{}.zip",
        summary::compact(request.now_unix_ms),
        &digest[..8]
    );
    let path = archive::write(&request.out, &name, &bytes)?;
    Ok(Exported {
        path,
        sha256: format!("sha256:{digest}"),
        size: bytes.len() as u64,
        instances: aliases,
        window,
        runs: reports
            .iter()
            .map(|report| {
                json!({
                    "request_id": report.row.request_id,
                    "run_id": report.row.run_id,
                    "alias": report.row.alias,
                    "state": report.row.state.as_str(),
                    "package": report.row.package,
                })
            })
            .collect(),
        frames: entries.iter().filter(|entry| entry.stored).count(),
        left_out,
        within_bound: fit == ReportFit::Fits && bytes.len() as u64 <= request.max_bytes,
        warnings: written_warnings,
    })
}

/// The row's line: `report_exported` with the archive's `path`, and `{view, path, sha256,
/// size, within_bound, instances, window, runs, frames, left_out, not_in_this_report_yet,
/// warnings?}`.
pub fn line(exported: &Exported) -> Exit {
    let mut members = Map::new();
    members.insert("view".to_owned(), json!(VIEW_ERRORS));
    members.insert(
        "path".to_owned(),
        json!(exported.path.display().to_string()),
    );
    members.insert("sha256".to_owned(), json!(exported.sha256));
    members.insert("size".to_owned(), json!(exported.size));
    members.insert("within_bound".to_owned(), json!(exported.within_bound));
    members.insert("instances".to_owned(), json!(exported.instances));
    members.insert("window".to_owned(), window_json(exported.window));
    members.insert("runs".to_owned(), Value::Array(exported.runs.clone()));
    members.insert("frames".to_owned(), json!(exported.frames));
    members.insert(
        "left_out".to_owned(),
        Value::Array(exported.left_out.clone()),
    );
    members.insert("not_in_this_report_yet".to_owned(), not_yet());
    if !exported.warnings.is_empty() {
        members.insert(
            "warnings".to_owned(),
            Value::Array(exported.warnings.iter().map(cli_line::warning).collect()),
        );
    }
    cli_line::success(
        members,
        &Outcome::new(ActingctlCode::ReportExported).path(exported.path.as_path()),
    )
}
