// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 RV-1: the steps of `resource verify` before the bundle: `tools`, the `checkout`
//! precondition, `declarations`, `pins`, `admission` and `format` (contracts/resource-verify.md,
//! "Steps"). Each calls the library code the repository CI's actinglab invocations run, in this
//! process, and records every failure its check can go on to find.

use super::git::{Git, nul_fields, text};
use super::{Counts, StepRun, VerifyRequest, declares, path_value, pointer_value, sha256_hex};
use crate::codes::{
    ActinglabCode, BaseProblem, CheckoutProblem, DirtyProblem, GitCommand, LayoutProblem,
    ToolsProblem, VerifyStep,
};
use crate::resource_runtime_support::resource_declarations::{
    SelectionFailure, ValidatedSelection, validate_declaration_selection,
};
use crate::{CliError, FlagArgs};
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use serde::Serialize;
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The root declaration files the selection holds when the commit tracks them (R5).
const ROOT_DECLARATIONS: [&str; 6] = [
    "applications.json",
    "maintenance.json",
    "declaration-exclusions.json",
    "pages.json",
    "interrupts.json",
    "navigation.json",
];
/// The root files the command reads besides `packs/` and `regression/`, whose bytes must be
/// their blobs.
const ROOT_INPUTS: [&str; 7] = [
    "applications.json",
    "maintenance.json",
    "pages.json",
    "interrupts.json",
    "navigation.json",
    "declaration-exclusions.json",
    "manifest.yaml",
];
const MANIFEST_FILE: &str = "BUILD-MANIFEST.json";
const ACTINGLAB_EXE: &str = "actinglab.exe";
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// `tools` in the report.
#[derive(Serialize)]
pub(super) struct ToolsReport {
    cli_version: &'static str,
    runtime_version: &'static str,
    runtime_head: &'static str,
    executable: Option<String>,
    executable_sha256: Option<String>,
    build_manifest: Option<BuildManifestReport>,
}

#[derive(Serialize)]
struct BuildManifestReport {
    path: String,
    sha256: String,
    commit_sha: Option<String>,
    actinglab_sha256: Option<String>,
}

/// The fields of a Tools `BUILD-MANIFEST.json` the step reads.
#[derive(serde::Deserialize)]
struct BuildManifest {
    commit_sha: String,
    files: Vec<ManifestFile>,
}

#[derive(serde::Deserialize)]
struct ManifestFile {
    path: String,
    sha256: String,
}

/// `tools` (R10): the build that runs, and, when `BUILD-MANIFEST.json` lies beside the
/// executable, whether it names this executable's bytes and this head.
pub(super) fn tools(executable: io::Result<PathBuf>) -> (ToolsReport, StepRun) {
    let mut run = StepRun::start(VerifyStep::Tools);
    let runtime_head = env!("ACTINGCOMMAND_RUNTIME_HEAD");
    let mut report = ToolsReport {
        cli_version: env!("CARGO_PKG_VERSION"),
        runtime_version: crate::RUNTIME_VERSION,
        runtime_head,
        executable: None,
        executable_sha256: None,
        build_manifest: None,
    };
    let executable = match executable.and_then(std::path::absolute) {
        Ok(executable) => executable,
        Err(error) => {
            run.fail(
                mismatch(Path::new(MANIFEST_FILE), ToolsProblem::ExecutableSha256)
                    .caused_by(Outcome::from_io_error(&error, IoOp::Locate)),
            );
            return (report, run.finish());
        }
    };
    let manifest = executable.parent().map_or_else(
        || PathBuf::from(MANIFEST_FILE),
        |dir| dir.join(MANIFEST_FILE),
    );
    report.executable = Some(executable.display().to_string());
    let executable_sha256 = match fs::read(&executable) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(error) => {
            run.fail(
                mismatch(&manifest, ToolsProblem::ExecutableSha256).caused_by(
                    Outcome::from_io_error(&error, IoOp::Read).path(path_value(&executable)),
                ),
            );
            return (report, run.finish());
        }
    };
    report.executable_sha256 = Some(executable_sha256.clone());
    let bytes = match fs::read(&manifest) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (report, run.finish()),
        Err(error) => {
            run.fail(
                mismatch(&manifest, ToolsProblem::ManifestInvalid).caused_by(
                    Outcome::from_io_error(&error, IoOp::Read).path(path_value(&manifest)),
                ),
            );
            return (report, run.finish());
        }
    };
    let mut manifest_report = BuildManifestReport {
        path: manifest.display().to_string(),
        sha256: sha256_hex(&bytes),
        commit_sha: None,
        actinglab_sha256: None,
    };
    match serde_json::from_slice::<BuildManifest>(
        bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&bytes),
    ) {
        Err(error) => run.fail(
            mismatch(&manifest, ToolsProblem::ManifestInvalid)
                .caused_by(foreign_json(&error.to_string()).path(path_value(&manifest))),
        ),
        Ok(parsed) => {
            let actinglab = parsed
                .files
                .iter()
                .find(|file| file.path == ACTINGLAB_EXE)
                .map(|file| file.sha256.to_ascii_lowercase());
            if !parsed.commit_sha.eq_ignore_ascii_case(runtime_head) {
                run.fail(mismatch(&manifest, ToolsProblem::Commit));
            }
            if actinglab.as_deref() != Some(executable_sha256.as_str()) {
                run.fail(mismatch(&manifest, ToolsProblem::ExecutableSha256));
            }
            manifest_report.commit_sha = Some(parsed.commit_sha);
            manifest_report.actinglab_sha256 = actinglab;
        }
    }
    report.build_manifest = Some(manifest_report);
    (report, run.finish())
}

fn mismatch(manifest: &Path, problem: ToolsProblem) -> Outcome {
    Outcome::new(ActinglabCode::ResourceVerifyToolsMismatch)
        .manifest_path(path_value(manifest))
        .tools_problem(problem)
}

/// What the checkout step learned, reported whether or not the precondition held.
#[derive(Default)]
pub(super) struct CheckoutFacts {
    pub(super) commit: Option<String>,
    pub(super) tree: Option<String>,
    pub(super) base: Option<String>,
}

/// A checkout that is the commit's clean tree.
pub(super) struct CheckoutState {
    pub(super) commit: String,
    pub(super) base: Option<String>,
    /// Every path the index tracks.
    pub(super) tracked: BTreeSet<String>,
}

/// `checkout` (R4): the precondition. `None` when it does not hold.
pub(super) fn checkout(
    git: &Git,
    request: &VerifyRequest,
) -> (CheckoutFacts, Option<CheckoutState>, StepRun) {
    let mut run = StepRun::start(VerifyStep::Checkout);
    let mut facts = CheckoutFacts::default();
    let state = match checkout_state(git, request, &mut facts, &mut run) {
        Ok(state) => state,
        Err(outcome) => {
            run.fail(outcome);
            None
        }
    };
    (facts, state, run.finish())
}

fn checkout_state(
    git: &Git,
    request: &VerifyRequest,
    facts: &mut CheckoutFacts,
    run: &mut StepRun,
) -> Result<Option<CheckoutState>, Outcome> {
    let invalid = |problem: CheckoutProblem| {
        Outcome::new(ActinglabCode::ResourceVerifyCheckoutInvalid)
            .repo(path_value(&request.repo))
            .checkout_problem(problem)
    };
    let (_, top) = git.run(GitCommand::RevParse, &["rev-parse", "--show-toplevel"], &[])?;
    let top = text(&top);
    let same = match (
        fs::canonicalize(&request.repo),
        fs::canonicalize(top.trim()),
    ) {
        (Ok(repo), Ok(top)) => repo == top,
        _ => false,
    };
    if !same {
        return Err(invalid(CheckoutProblem::NotTopLevel));
    }
    let head = git
        .resolve("HEAD^{commit}")?
        .ok_or_else(|| invalid(CheckoutProblem::HeadNotCommit))?;
    facts.commit = Some(head.clone());
    let tree = git
        .resolve("HEAD^{tree}")?
        .ok_or_else(|| invalid(CheckoutProblem::HeadNotCommit))?;
    facts.tree = Some(tree);
    if request
        .commit
        .as_ref()
        .is_some_and(|commit| *commit != head)
    {
        return Err(invalid(CheckoutProblem::CommitMismatch));
    }
    let base = match &request.base {
        None => None,
        Some(rev) => {
            let base_invalid = |problem: BaseProblem| {
                Outcome::new(ActinglabCode::ResourceVerifyBaseInvalid)
                    .base(rev.as_str())
                    .base_problem(problem)
            };
            let commit = git
                .resolve(&format!("{rev}^{{commit}}"))?
                .ok_or_else(|| base_invalid(BaseProblem::Unresolved))?;
            facts.base = Some(commit.clone());
            if !git.is_ancestor(&commit, &head)? {
                return Err(base_invalid(BaseProblem::NotAncestor));
            }
            Some(commit)
        }
    };
    // The tree is clean: nothing changed, nothing untracked, nothing ignored where it reads.
    let (_, status) = git.run(
        GitCommand::Status,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
        ],
        &[],
    )?;
    let mut changed = Vec::new();
    let mut untracked = Vec::new();
    let mut ignored = Vec::new();
    let fields = status.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        index += 1;
        if field.len() < 4 {
            continue;
        }
        let path = text(&field[3..]);
        match (field[0], field[1]) {
            (b'?', b'?') => untracked.push(path),
            (b'!', b'!') => {
                if read_path(path.trim_end_matches('/')) {
                    ignored.push(path);
                }
            }
            (x, y) => {
                // A rename or copy is followed by its source path.
                if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
                    index += 1;
                }
                changed.push(path);
            }
        }
    }
    for (paths, problem) in [
        (&changed, DirtyProblem::TrackedChange),
        (&untracked, DirtyProblem::Untracked),
        (&ignored, DirtyProblem::IgnoredInReadPath),
    ] {
        if let Some(first) = paths.first() {
            run.fail(dirty(paths.len(), first, problem));
        }
    }
    // Every file it reads hashes, without filters, to its blob.
    let (_, listing) = git.run(GitCommand::LsFiles, &["ls-files", "-s", "-z"], &[])?;
    let changed = changed.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut tracked = BTreeSet::new();
    let mut expected = Vec::new();
    for entry in nul_fields(&listing) {
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        tracked.insert(path.to_owned());
        let meta = meta.split(' ').collect::<Vec<_>>();
        if let [mode, blob, "0"] = meta.as_slice()
            && *mode != "160000"
            && read_path(path)
            && !changed.contains(path)
        {
            expected.push((path.to_owned(), (*blob).to_owned()));
        }
    }
    let mut mismatched = Vec::new();
    let mut hashed = Vec::new();
    for (path, blob) in &expected {
        if fs::symlink_metadata(request.repo.join(path)).is_ok_and(|metadata| metadata.is_file()) {
            hashed.push((path, blob));
        } else {
            mismatched.push(path.clone());
        }
    }
    if !hashed.is_empty() {
        let input = hashed
            .iter()
            .map(|(path, _)| format!("{path}\n"))
            .collect::<String>();
        let answer = git.run_with_input(
            GitCommand::HashObject,
            &["hash-object", "--no-filters", "--stdin-paths"],
            input.as_bytes(),
        )?;
        let answer = text(&answer);
        let blobs = answer.lines().collect::<Vec<_>>();
        if blobs.len() != hashed.len() {
            return Err(Outcome::new(ActinglabCode::ResourceVerifyGitFailed)
                .git_command(GitCommand::HashObject));
        }
        for ((path, blob), actual) in hashed.iter().zip(blobs) {
            if actual.trim() != blob.as_str() {
                mismatched.push((*path).clone());
            }
        }
        mismatched.sort();
    }
    if let Some(first) = mismatched.first() {
        run.fail(dirty(mismatched.len(), first, DirtyProblem::BlobMismatch));
    }
    run.counts = Counts::Checkout {
        files_checked: expected.len(),
    };
    if run.failed() {
        return Ok(None);
    }
    Ok(Some(CheckoutState {
        commit: head,
        base,
        tracked,
    }))
}

fn dirty(count: usize, first: &str, problem: DirtyProblem) -> Outcome {
    Outcome::new(ActinglabCode::ResourceVerifyCheckoutDirty)
        .path_count(count)
        .first_path(path_value(first))
        .dirty_problem(problem)
}

/// A file the command reads: under `packs/` or `regression/`, or a root input.
fn read_path(path: &str) -> bool {
    path == "packs"
        || path == "regression"
        || path.starts_with("packs/")
        || path.starts_with("regression/")
        || ROOT_INPUTS.contains(&path)
}

/// `declarations` (R5): the selection built from Git, read by `resource validate`'s reader once
/// in full, then, after a refusal, each remaining path on its own.
pub(super) fn declarations(git: &Git, repo: &Path, checkout: &CheckoutState) -> StepRun {
    let mut run = StepRun::start(VerifyStep::Declarations);
    let mut selection = BTreeMap::<String, bool>::new();
    match git.run(
        GitCommand::LsFiles,
        &["ls-files", "-z", "--", "packs/*.json"],
        &[],
    ) {
        Ok((_, listed)) => {
            for path in nul_fields(&listed) {
                selection.insert(path, false);
            }
        }
        Err(outcome) => {
            run.fail(outcome);
            return run.finish();
        }
    }
    for name in ROOT_DECLARATIONS {
        if checkout.tracked.contains(name) {
            selection.insert(name.to_owned(), false);
        }
    }
    if let Some(base) = &checkout.base {
        let diff = git.run(
            GitCommand::Diff,
            &[
                "diff",
                "--no-renames",
                "--name-status",
                "-z",
                base,
                "HEAD",
                "--",
                "*.json",
                "*.yaml",
                "*.yml",
            ],
            &[],
        );
        let fields = match diff {
            Ok((_, answer)) => nul_fields(&answer),
            Err(outcome) => {
                run.fail(outcome);
                return run.finish();
            }
        };
        let (pairs, rest) = fields.as_chunks::<2>();
        let diff_failed =
            || Outcome::new(ActinglabCode::ResourceVerifyGitFailed).git_command(GitCommand::Diff);
        if !rest.is_empty() {
            run.fail(diff_failed());
            return run.finish();
        }
        for [status, path] in pairs {
            let deleted = match status.as_str() {
                "D" => true,
                "A" | "M" | "T" => false,
                _ => {
                    run.fail(diff_failed());
                    return run.finish();
                }
            };
            selection.insert(path.clone(), deleted);
        }
    }
    let entries = selection
        .iter()
        .map(|(path, deleted)| (PathBuf::from(path), *deleted))
        .collect::<Vec<_>>();
    let mut tally = Tally {
        selected: entries.len(),
        deleted: entries.iter().filter(|(_, deleted)| *deleted).count(),
        ..Tally::default()
    };
    match validate_declaration_selection(repo, entries.clone()) {
        Ok(validated) => tally.validated(&mut run, validated),
        Err(failure) => {
            let failed = failure.failed.clone();
            let mut named = BTreeSet::new();
            let first = declaration_failure(&failure.error, failed.as_deref(), false);
            named.insert(failure_name(&first));
            run.fail(first);
            tally.refused(&mut run, failure);
            // The paths after the one it stopped at, each validated on its own.
            if let Some(failed) = failed {
                for (path, deleted) in entries.into_iter().filter(|(path, _)| *path > failed) {
                    match validate_declaration_selection(repo, vec![(path.clone(), deleted)]) {
                        Ok(validated) => tally.validated(&mut run, validated),
                        Err(further) => {
                            let outcome =
                                declaration_failure(&further.error, Some(path.as_path()), true);
                            if named.insert(failure_name(&outcome)) {
                                tally.found_after_first += 1;
                                run.fail(outcome);
                            }
                            tally.refused(&mut run, further);
                        }
                    }
                }
            }
        }
    }
    run.counts = Counts::Declarations {
        selected: tally.selected,
        deleted: tally.deleted,
        valid: tally.valid,
        excluded: tally.excluded,
        read_files: tally.read_files.len(),
        read_bytes: tally.read_bytes,
        found_after_first: tally.found_after_first,
    };
    run.finish()
}

/// What the declaration step counts across its validations.
#[derive(Default)]
struct Tally {
    selected: usize,
    deleted: usize,
    valid: usize,
    excluded: usize,
    read_files: BTreeSet<String>,
    read_bytes: u64,
    found_after_first: usize,
}

impl Tally {
    fn entries(&mut self, entries: &[Value]) {
        for entry in entries {
            match entry.get("status").and_then(Value::as_str) {
                Some("valid") => self.valid += 1,
                Some("excluded") => self.excluded += 1,
                _ => {}
            }
        }
    }

    fn validated(&mut self, run: &mut StepRun, validated: ValidatedSelection) {
        self.entries(&validated.entries);
        self.read_files.extend(validated.read_files);
        self.read_bytes += validated.read_bytes;
        for warning in validated.warnings {
            run.warn(warning);
        }
    }

    fn refused(&mut self, run: &mut StepRun, failure: SelectionFailure) {
        self.entries(&failure.entries);
        self.read_files.extend(failure.read_files);
        self.read_bytes += failure.read_bytes;
        for warning in failure.warnings {
            run.warn(warning);
        }
    }
}

/// The refusal of `resource validate`'s reader as an outcome: its code, the declaration file it
/// names (else the selected path) and the JSON pointer of the member it refused.
fn declaration_failure(error: &CliError, selected: Option<&Path>, after_first: bool) -> Outcome {
    developer_line(VerifyStep::Declarations, error);
    let details = error.details.as_ref();
    let path = details
        .and_then(|details| details.get("declaration_file"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| selected.map(|path| path.to_string_lossy().replace('\\', "/")));
    let pointer = details
        .and_then(|details| details.get("field_path"))
        .and_then(Value::as_str)
        .filter(|pointer| pointer.is_empty() || pointer.starts_with('/'));
    let mut outcome = relayed(error);
    if let Some(path) = path
        && declares(&outcome, "path")
    {
        outcome = outcome.path(path_value(path));
    }
    if let Some(pointer) = pointer
        && declares(&outcome, "json_pointer")
    {
        outcome = outcome.json_pointer(pointer_value(pointer));
    }
    if after_first && declares(&outcome, "found_after_first") {
        outcome = outcome.found_after_first(true);
    }
    outcome
}

/// A failure's identity for the list of further failures: its code and what it names.
fn failure_name(outcome: &Outcome) -> String {
    let value = |key: &str| {
        outcome
            .value(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    format!(
        "{}\0{}\0{}",
        outcome.code(),
        value("path"),
        value("json_pointer")
    )
}

/// `pins` (R6): no tracked pack or procedure-manifest JSON carries `package_digest`, and no pack
/// JSON a member named exactly `sha256`, at any depth.
pub(super) fn pins(git: &Git, repo: &Path) -> StepRun {
    let mut run = StepRun::start(VerifyStep::Pins);
    let listed = match git.run(
        GitCommand::LsFiles,
        &[
            "ls-files",
            "-z",
            "--",
            "packs/*.json",
            "*procedure-manifest*.json",
        ],
        &[],
    ) {
        Ok((_, listed)) => nul_fields(&listed),
        Err(outcome) => {
            run.fail(outcome);
            return run.finish();
        }
    };
    for path in &listed {
        let bytes = match fs::read(repo.join(path)) {
            Ok(bytes) => bytes,
            Err(error) => {
                run.fail(declaration_invalid(
                    path,
                    Outcome::from_io_error(&error, IoOp::Read).path(path_value(path)),
                ));
                continue;
            }
        };
        match pin_members(&bytes, path.starts_with("packs/")) {
            Ok(hits) => {
                for (pointer, member) in hits {
                    run.fail(
                        Outcome::new(ActinglabCode::ResourceVerifyPinCommitted)
                            .path(path_value(path))
                            .json_pointer(pointer_value(&pointer))
                            .member(member),
                    );
                }
            }
            Err(reason) => run.fail(declaration_invalid(
                path,
                foreign_json(&reason).path(path_value(path)),
            )),
        }
    }
    run.counts = Counts::Pins {
        files: listed.len(),
    };
    run.finish()
}

/// Every `package_digest` member, and in a pack file every `sha256` member, with its RFC 6901
/// pointer, each duplicate member too; or why the bytes are no UTF-8 JSON (a BOM is accepted).
fn pin_members(bytes: &[u8], pack: bool) -> Result<Vec<(String, &'static str)>, String> {
    let bytes = bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let mut hits = Vec::new();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    PinWalk {
        pointer: String::new(),
        pack,
        hits: &mut hits,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    Ok(hits)
}

/// Walks one JSON value, visiting every member as the document holds it.
struct PinWalk<'a> {
    pointer: String,
    pack: bool,
    hits: &'a mut Vec<(String, &'static str)>,
}

impl<'de> DeserializeSeed<'de> for PinWalk<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for PinWalk<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        let mut index = 0_usize;
        loop {
            let element = PinWalk {
                pointer: format!("{}/{index}", self.pointer),
                pack: self.pack,
                hits: &mut *self.hits,
            };
            if sequence.next_element_seed(element)?.is_none() {
                return Ok(());
            }
            index += 1;
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(member) = map.next_key::<String>()? {
            let pointer = format!(
                "{}/{}",
                self.pointer,
                member.replace('~', "~0").replace('/', "~1")
            );
            if member == "package_digest" {
                self.hits.push((pointer.clone(), "package_digest"));
            } else if self.pack && member == "sha256" {
                self.hits.push((pointer.clone(), "sha256"));
            }
            map.next_value_seed(PinWalk {
                pointer,
                pack: self.pack,
                hits: &mut *self.hits,
            })?;
        }
        Ok(())
    }
}

/// The parts of `applications.json` the identity and bundle checks read.
pub(super) struct Applications {
    pub(super) game: String,
    pub(super) servers: BTreeSet<String>,
    /// `servers.<server>.default_package_id`, where declared.
    pub(super) defaults: BTreeMap<String, String>,
}

/// `applications.json`'s game, servers and defaults; `None` when it is missing or does not read
/// so (the declaration step names why).
pub(super) fn read_applications(repo: &Path) -> Option<Applications> {
    let bytes = fs::read(repo.join("applications.json")).ok()?;
    let value: Value =
        serde_json::from_slice(bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&bytes)).ok()?;
    let game = value.get("game")?.as_str()?.to_owned();
    let servers = value.get("servers")?.as_object()?;
    Some(Applications {
        game,
        servers: servers.keys().cloned().collect(),
        defaults: servers
            .iter()
            .filter_map(|(server, entry)| {
                let package = entry.get("default_package_id")?.as_str()?;
                Some((server.clone(), package.to_owned()))
            })
            .collect(),
    })
}

/// `packs/` as the bundle source reads it.
pub(super) struct PacksLayout {
    present: bool,
    /// The pack directories, in name order.
    pub(super) directories: Vec<String>,
    /// The entries that are neither a pack directory nor `README.md`.
    invalid: Vec<String>,
}

pub(super) fn read_layout(repo: &Path) -> PacksLayout {
    let mut layout = PacksLayout {
        present: false,
        directories: Vec::new(),
        invalid: Vec::new(),
    };
    let Ok(entries) = fs::read_dir(repo.join("packs")) else {
        return layout;
    };
    layout.present = true;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match fs::symlink_metadata(entry.path()) {
            Ok(metadata) if metadata.is_dir() => layout.directories.push(name),
            Ok(metadata) if metadata.is_file() && name == "README.md" => {}
            _ => layout.invalid.push(name),
        }
    }
    layout.directories.sort();
    layout.invalid.sort();
    layout
}

/// What a source pack's control declares, read by its admission.
pub(super) struct PackIdentity {
    pub(super) server: String,
    pub(super) entry_task_id: String,
}

/// `admission` (R7): the layout of `packs/`, and every pack directory admitted as `package
/// preflight` admits it, named by its `package_id`, of the game and a declared server. The
/// identities are those of the packs that passed, by directory name.
pub(super) fn admission(
    repo: &Path,
    applications: Option<&Applications>,
    layout: &PacksLayout,
) -> (BTreeMap<String, PackIdentity>, StepRun) {
    let mut run = StepRun::start(VerifyStep::Admission);
    let layout_invalid = |path: &str, problem: LayoutProblem| {
        Outcome::new(ActinglabCode::ResourceVerifyLayoutInvalid)
            .path(path_value(path))
            .layout_problem(problem)
    };
    for (name, problem) in [
        ("applications.json", LayoutProblem::ApplicationsMissing),
        ("maintenance.json", LayoutProblem::MaintenanceMissing),
    ] {
        if !fs::symlink_metadata(repo.join(name)).is_ok_and(|metadata| metadata.is_file()) {
            run.fail(layout_invalid(name, problem));
        }
    }
    if !layout.present || layout.directories.is_empty() {
        run.fail(layout_invalid("packs", LayoutProblem::PacksMissing));
    }
    for name in &layout.invalid {
        run.fail(layout_invalid(
            format!("packs/{name}").as_str(),
            LayoutProblem::PacksEntryInvalid,
        ));
    }
    let mut identities = BTreeMap::new();
    for name in &layout.directories {
        let path = format!("packs/{name}");
        let prepared = match preflight(&repo.join("packs").join(name)) {
            Ok(prepared) => prepared,
            Err(error) => {
                run.fail(admission_failure(VerifyStep::Admission, &error, &path));
                continue;
            }
        };
        let mut admitted = true;
        if prepared.package_label() != name.as_str() {
            run.fail(layout_invalid(
                path.as_str(),
                LayoutProblem::PackNameMismatch,
            ));
            admitted = false;
        }
        if let Some(applications) = applications {
            if prepared.game() != applications.game.as_str() {
                run.fail(layout_invalid(
                    path.as_str(),
                    LayoutProblem::PackGameMismatch,
                ));
                admitted = false;
            }
            if !applications.servers.contains(prepared.server()) {
                run.fail(layout_invalid(
                    path.as_str(),
                    LayoutProblem::PackServerUndeclared,
                ));
                admitted = false;
            }
        }
        if admitted {
            identities.insert(
                name.clone(),
                PackIdentity {
                    server: prepared.server().to_owned(),
                    entry_task_id: prepared.task_label().to_owned(),
                },
            );
        }
    }
    run.counts = Counts::Admission {
        packs: layout.directories.len(),
        admitted: identities.len(),
    };
    (identities, run.finish())
}

/// `package preflight`'s admission of the directory `directory`.
pub(super) fn preflight(
    directory: &Path,
) -> Result<actingcommand_lab::PreparedContainedTask, CliError> {
    let flags = FlagArgs {
        flags: BTreeMap::from([(
            "--package".to_owned(),
            vec![directory.to_string_lossy().into_owned()],
        )]),
        positionals: Vec::new(),
    };
    crate::package_cli::preflight_admission(&flags)
}

/// A refused admission as an outcome: its code (`package_invalid`) caused by the admission's own
/// code, naming the pack.
pub(super) fn admission_failure(step: VerifyStep, error: &CliError, path: &str) -> Outcome {
    developer_line(step, error);
    let mut outcome = relayed(error);
    if let Some(code) = error
        .details
        .as_ref()
        .and_then(|details| details.get("code"))
        .and_then(Value::as_str)
    {
        outcome = outcome.caused_by(Outcome::relayed(code));
    }
    if declares(&outcome, "path") {
        outcome = outcome.path(path_value(path));
    }
    outcome
}

/// `format` (R8): `package fmt --check` of every pack directory.
pub(super) fn format(repo: &Path, layout: &PacksLayout) -> StepRun {
    let mut run = StepRun::start(VerifyStep::Format);
    let mut json_files = 0;
    let mut unformatted = 0;
    for name in &layout.directories {
        let path = format!("packs/{name}");
        match crate::package_tool::canonical_changes(&repo.join("packs").join(name)) {
            Ok((files, changes)) => {
                json_files += files;
                if changes.is_empty() {
                    continue;
                }
                unformatted += changes.len();
                let mut outcome = Outcome::new(ActinglabCode::PackageFmtCheckFailed);
                if declares(&outcome, "path") {
                    outcome = outcome.path(path_value(&path));
                }
                if declares(&outcome, "unformatted") {
                    outcome = outcome.unformatted(
                        changes
                            .iter()
                            .map(|(relative, _)| path_value(relative))
                            .collect::<Vec<_>>(),
                    );
                }
                run.fail(outcome);
            }
            Err(error) => run.fail(admission_failure(VerifyStep::Format, &error, &path)),
        }
    }
    run.counts = Counts::Format {
        packs: layout.directories.len(),
        json_files,
        unformatted,
    };
    run.finish()
}

/// A Lab error's code as an outcome, with no value: its text is developer text, logged.
pub(super) fn relayed(error: &CliError) -> Outcome {
    Outcome::relayed(&error.code)
}

/// The developer line of a failure the report carries as an outcome.
pub(super) fn developer_line(step: VerifyStep, error: &CliError) {
    eprintln!(
        "WARN actinglab: resource verify {step}: {}: {}",
        error.code, error.message
    );
}

/// `resource_declaration_invalid` naming the repository file `path`, caused by `cause`.
pub(super) fn declaration_invalid(path: &str, cause: Outcome) -> Outcome {
    let outcome = Outcome::new(SharedCode::ResourceDeclarationInvalid);
    let outcome = if declares(&outcome, "path") {
        outcome.path(path_value(path))
    } else {
        outcome
    };
    outcome.caused_by(cause)
}

/// `foreign_data_error` for a JSON reader's refusal.
pub(super) fn foreign_json(reason: &str) -> Outcome {
    let mut end = reason.len().min(4096);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(&reason[..end])
}
