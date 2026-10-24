// SPDX-License-Identifier: AGPL-3.0-only

//! `actingctl install release verify` and `release stage` (Workflow #421 INST-1;
//! `contracts/install-cli.md`, "Release verify and stage"): a release folder (the Runtime zip,
//! the Tools zip and `SHA256SUMS`, with an optional release-facts file) checked as the release
//! workflow wrote it, every failure listed; `stage` keeps the verified build under
//! `install\builds\<commit>` with its `release.json` and records it in `state.json`.

use super::files::{self, join_relative};
use super::journal::{self, Opening, Operation, PlannedStep, Previous, StepKind, StepState};
use super::manifest::{self, Half, Speaks};
use super::state::{self, BuildOrigin, KeptBuild, OperationKind};
use super::{Answer, Flags, Host};
use crate::cli_line;
use crate::codes::{ActingctlCode, ArgumentProblem, InstallRefusal, ReleaseCheck};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

const RUNTIME_ZIP_PREFIX: &str = "actingcommand-runtime-";
const TOOLS_ZIP_PREFIX: &str = "actingcommand-tools-";
const ZIP_SUFFIX: &str = ".zip";
const SUMS_FILE: &str = "SHA256SUMS";
const FACTS_SCHEMA: &str = "actingcommand.release-facts.v1";
const RELEASE_RECORD_SCHEMA: &str = "actingcommand.install-release.v1";
const RELEASE_RECORD_FILE: &str = "release.json";
/// The shortest `--expect-commit` prefix.
const MIN_COMMIT_PREFIX: usize = 8;

/// The inputs of one verify or stage, as the journal keeps them for `resume`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseInputs {
    pub(crate) dir: String,
    pub(crate) facts: Option<String>,
    pub(crate) expect_tag: Option<String>,
    pub(crate) expect_commit: Option<String>,
    pub(crate) local_build: bool,
}

/// `actingcommand.release-facts.v1`, written by a fetch script or a UI downloader from the
/// release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseFacts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) schema_version: Option<String>,
    pub(crate) repository: String,
    pub(crate) tag: String,
    pub(crate) commit: String,
    pub(crate) prerelease: bool,
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseAsset {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) digest: String,
}

/// One failed check, with the file it concerns and the digests when they apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) check: ReleaseCheck,
    pub(crate) file: Option<String>,
    pub(crate) expected_sha256: Option<String>,
    pub(crate) actual_sha256: Option<String>,
}

impl Failure {
    fn new(check: ReleaseCheck) -> Self {
        Self {
            check,
            file: None,
            expected_sha256: None,
            actual_sha256: None,
        }
    }

    fn file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    fn digests(mut self, expected: Option<String>, actual: Option<String>) -> Self {
        self.expected_sha256 = expected;
        self.actual_sha256 = actual;
        self
    }
}

/// A failure as the result line's `failures` lists it.
#[derive(Serialize)]
struct FailureRow<'a> {
    check: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_sha256: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual_sha256: Option<&'a str>,
}

/// `install_release_refused` with every failed check (`release_checks`, once each in order) and
/// the result's `failures`.
fn refused(failures: &[Failure]) -> Result<Answer, Outcome> {
    let mut checks = Vec::new();
    for failure in failures {
        if !checks.contains(&failure.check) {
            checks.push(failure.check);
        }
    }
    let rows = failures
        .iter()
        .map(|failure| FailureRow {
            check: failure.check.as_str(),
            file: failure.file.as_deref(),
            expected_sha256: failure.expected_sha256.as_deref(),
            actual_sha256: failure.actual_sha256.as_deref(),
        })
        .collect::<Vec<_>>();
    Answer::new(Outcome::new(ActingctlCode::InstallReleaseRefused).release_checks(checks))
        .with("failures", rows)
}

/// The release folder's two zips, found by name.
#[derive(Debug, Clone)]
struct Located {
    commit: String,
    runtime_zip: PathBuf,
    tools_zip: PathBuf,
    facts: Option<ReleaseFacts>,
}

impl Located {
    fn zip(&self, half: Half) -> &Path {
        match half {
            Half::Runtime => &self.runtime_zip,
            Half::Tools => &self.tools_zip,
        }
    }
}

/// The checks that need no extraction: the zip set, `SHA256SUMS`, the facts and the expected
/// tag and commit.
struct Prechecked {
    located: Option<Located>,
    failures: Vec<Failure>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// The commit of `<prefix><40 hex>.zip`.
fn zip_commit<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let commit = name.strip_prefix(prefix)?.strip_suffix(ZIP_SUFFIX)?;
    files::is_commit(commit).then_some(commit)
}

/// `SHA256SUMS`: `<hex>  <file name>` lines (a `*` before the name allowed), by file name.
fn parse_sums(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim_end_matches('\r').split_once(' ')?;
            let name = name.trim_start_matches([' ', '*']);
            (!name.is_empty()).then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// The facts file; `None` when it cannot be read or is not `actingcommand.release-facts.v1`.
fn read_facts(path: &Path) -> Option<ReleaseFacts> {
    let facts: ReleaseFacts = files::read_json(path).ok()?;
    let schema_ok = facts
        .schema_version
        .as_deref()
        .is_none_or(|schema| schema == FACTS_SCHEMA);
    (schema_ok && files::is_commit(&facts.commit)).then_some(facts)
}

fn precheck(inputs: &ReleaseInputs) -> Result<Prechecked, Outcome> {
    let dir = PathBuf::from(&inputs.dir);
    let entries = fs::read_dir(&dir).map_err(|error| {
        super::unusable("--dir").caused_by(files::os_failure(&dir, &error, IoOp::ReadDir))
    })?;
    let mut runtime = Vec::new();
    let mut tools = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            super::unusable("--dir").caused_by(files::os_failure(&dir, &error, IoOp::ReadDir))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(commit) = zip_commit(&name, RUNTIME_ZIP_PREFIX) {
            runtime.push(commit.to_owned());
        } else if let Some(commit) = zip_commit(&name, TOOLS_ZIP_PREFIX) {
            tools.push(commit.to_owned());
        }
    }
    let mut failures = Vec::new();
    let facts = match &inputs.facts {
        None => None,
        Some(path) => {
            let facts = read_facts(Path::new(path));
            if facts.is_none() {
                failures.push(Failure::new(ReleaseCheck::FactsInvalid).file(path.clone()));
            }
            facts
        }
    };
    let commit = match (runtime.as_slice(), tools.as_slice()) {
        ([runtime], [tools]) if runtime == tools => runtime.clone(),
        _ => {
            failures.push(Failure::new(ReleaseCheck::ZipSet));
            return Ok(Prechecked {
                located: None,
                failures,
            });
        }
    };
    let located = Located {
        runtime_zip: dir.join(format!("{RUNTIME_ZIP_PREFIX}{commit}{ZIP_SUFFIX}")),
        tools_zip: dir.join(format!("{TOOLS_ZIP_PREFIX}{commit}{ZIP_SUFFIX}")),
        commit,
        facts,
    };
    // SHA256SUMS lists both zips with their SHA-256.
    let sums_path = dir.join(SUMS_FILE);
    let sums = fs::read(&sums_path)
        .ok()
        .map(|bytes| parse_sums(&String::from_utf8_lossy(&bytes)));
    if sums.is_none() {
        failures.push(Failure::new(ReleaseCheck::SumsMissing).file(SUMS_FILE));
    }
    let mut digests = Vec::new();
    for zip in [&located.runtime_zip, &located.tools_zip] {
        let name = file_name(zip);
        let actual = files::sha256_file(zip).ok().map(|(sha, _)| sha);
        match (&sums, &actual) {
            (_, None) => {
                failures.push(Failure::new(ReleaseCheck::ZipUnreadable).file(name.clone()))
            }
            (None, Some(_)) => {}
            (Some(sums), Some(actual)) => match sums.get(&name) {
                None => failures.push(Failure::new(ReleaseCheck::SumsMissing).file(name.clone())),
                Some(listed) if listed != actual => failures.push(
                    Failure::new(ReleaseCheck::SumsMismatch)
                        .file(name.clone())
                        .digests(Some(listed.clone()), Some(actual.clone())),
                ),
                Some(_) => {}
            },
        }
        digests.push((name, actual));
    }
    digests.push((
        SUMS_FILE.to_owned(),
        files::sha256_file(&sums_path).ok().map(|(sha, _)| sha),
    ));
    if let Some(facts) = &located.facts {
        // Each zip's and SHA256SUMS' SHA-256 equals its asset digest.
        for (name, actual) in &digests {
            let expected = facts
                .assets
                .iter()
                .find(|asset| &asset.name == name)
                .and_then(|asset| asset.digest.strip_prefix("sha256:"))
                .map(str::to_ascii_lowercase);
            if expected.is_none() || actual.is_none() || expected != *actual {
                failures.push(
                    Failure::new(ReleaseCheck::DigestMismatch)
                        .file(name.clone())
                        .digests(expected, actual.clone()),
                );
            }
        }
        if facts.commit != located.commit {
            failures.push(Failure::new(ReleaseCheck::FactsCommitMismatch));
        }
        if let Some(tag) = &inputs.expect_tag
            && *tag != facts.tag
        {
            failures.push(Failure::new(ReleaseCheck::TagMismatch));
        }
    }
    if let Some(prefix) = &inputs.expect_commit
        && !located.commit.starts_with(prefix.as_str())
    {
        failures.push(Failure::new(ReleaseCheck::CommitPrefixMismatch));
    }
    Ok(Prechecked {
        located: Some(located),
        failures,
    })
}

/// Why an extraction stopped.
enum Extracted {
    /// The zip cannot be read: a release check (`zip_unreadable`), with the reader's failure.
    Unreadable(Outcome),
    /// Writing the extracted files failed: the operation's own failure.
    Failed(Outcome),
}

/// Extracts `zip_path` into `into` (created), overwriting files of an earlier attempt.
fn extract(zip_path: &Path, into: &Path) -> Result<(), Extracted> {
    let unreadable = |error: &zip::result::ZipError| {
        Extracted::Unreadable(files::zip_failure(error).path(zip_path))
    };
    let file = File::open(zip_path)
        .map_err(|error| Extracted::Unreadable(files::os_failure(zip_path, &error, IoOp::Open)))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| unreadable(&error))?;
    files::create_dir_all(into).map_err(Extracted::Failed)?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| unreadable(&error))?;
        let Some(name) = entry.enclosed_name().map(Path::to_path_buf) else {
            return Err(Extracted::Unreadable(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Zip)
                    .raw_text(entry.name())
                    .path(zip_path),
            ));
        };
        let target = into.join(&name);
        if entry.is_dir() {
            files::create_dir_all(&target).map_err(Extracted::Failed)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            files::create_dir_all(parent).map_err(Extracted::Failed)?;
        }
        let mut out = File::create(&target)
            .map_err(|error| Extracted::Failed(files::os_failure(&target, &error, IoOp::Create)))?;
        io::copy(&mut entry, &mut out).map_err(|error| {
            Extracted::Unreadable(files::os_failure(zip_path, &error, IoOp::Read))
        })?;
        out.sync_all()
            .map_err(|error| Extracted::Failed(files::os_failure(&target, &error, IoOp::Sync)))?;
    }
    Ok(())
}

/// What the extracted build verified as.
struct Verified {
    tree: Option<String>,
    speaks: Option<Speaks>,
    files: usize,
}

/// The checks of the extracted files below `base` (`runtime\`, `tools\`), every failure listed.
fn check_extracted(
    base: &Path,
    located: &Located,
    failures: &mut Vec<Failure>,
) -> Option<Verified> {
    let mut speaks = Vec::new();
    let mut tree = None;
    let mut count = 0;
    for half in [Half::Runtime, Half::Tools] {
        let dir = base.join(half.dir());
        let label = |path: &str| format!("{}/{path}", half.dir());
        let manifest = match manifest::read_manifest(&dir) {
            Ok(manifest) if manifest.paths_valid() => manifest,
            _ => {
                failures.push(
                    Failure::new(ReleaseCheck::ManifestInvalid)
                        .file(label(manifest::MANIFEST_FILE)),
                );
                continue;
            }
        };
        if half.layout_of(&manifest) != Some(half.layout()) {
            failures.push(
                Failure::new(ReleaseCheck::ManifestLayout).file(label(manifest::MANIFEST_FILE)),
            );
        }
        if let Some(facts) = &located.facts
            && manifest.repository != facts.repository
        {
            failures.push(
                Failure::new(ReleaseCheck::ManifestRepository).file(label(manifest::MANIFEST_FILE)),
            );
        }
        if manifest.commit_sha != located.commit {
            failures.push(
                Failure::new(ReleaseCheck::ManifestCommit).file(label(manifest::MANIFEST_FILE)),
            );
        }
        match manifest::unlisted_files(&dir, &manifest) {
            Ok(unlisted) => {
                let missing = manifest
                    .files
                    .iter()
                    .any(|file| !join_relative(&dir, &file.path).is_file());
                if !unlisted.is_empty() || missing {
                    failures.push(Failure::new(ReleaseCheck::FileSet).file(half.dir()));
                }
            }
            Err(_) => failures.push(Failure::new(ReleaseCheck::FileSet).file(half.dir())),
        }
        for problem in manifest::file_problems(&dir, &manifest) {
            failures.push(
                Failure::new(ReleaseCheck::FileMismatch)
                    .file(label(&problem.listed))
                    .digests(Some(problem.expected_sha256), problem.actual_sha256),
            );
        }
        match manifest.speaks() {
            Some(declared) => speaks.push(declared),
            None => failures.push(
                Failure::new(ReleaseCheck::InterfacesInvalid).file(label(manifest::MANIFEST_FILE)),
            ),
        }
        if half == Half::Runtime {
            tree = manifest.tree_sha.clone();
        }
        count += manifest.files.len();
    }
    if let [runtime, tools] = speaks.as_slice()
        && runtime != tools
    {
        failures.push(Failure::new(ReleaseCheck::InterfacesDiffer));
    }
    failures.is_empty().then(|| Verified {
        tree,
        speaks: speaks.into_iter().next(),
        files: count,
    })
}

/// What a verified release folder answers with.
#[derive(Serialize)]
struct VerifiedRelease<'a> {
    commit: &'a str,
    tree: Option<&'a str>,
    tag: Option<&'a str>,
    prerelease: Option<bool>,
    interfaces: Option<&'a Speaks>,
    files: usize,
}

impl<'a> VerifiedRelease<'a> {
    fn of(located: &'a Located, verified: &'a Verified) -> Self {
        Self {
            commit: &located.commit,
            tree: verified.tree.as_deref(),
            tag: located.facts.as_ref().map(|facts| facts.tag.as_str()),
            prerelease: located.facts.as_ref().map(|facts| facts.prerelease),
            interfaces: verified.speaks.as_ref(),
            files: verified.files,
        }
    }
}

/// `release.json` of a kept build: the facts and the verification, nothing of the operation,
/// so a resumed stage writes the same bytes.
#[derive(Serialize)]
struct ReleaseRecord<'a> {
    schema_version: &'static str,
    #[serde(flatten)]
    verified: VerifiedRelease<'a>,
    local_build: bool,
    facts: Option<&'a ReleaseFacts>,
}

/// The parsed flags of verify and stage.
fn inputs(flags: &Flags<'_>) -> Result<ReleaseInputs, Outcome> {
    let expect_commit = match flags.value("--expect-commit") {
        None => None,
        Some(prefix) => {
            let prefix = prefix.to_ascii_lowercase();
            if prefix.len() < MIN_COMMIT_PREFIX
                || prefix.len() > 40
                || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(cli_line::usage_invalid(
                    Some("--expect-commit"),
                    ArgumentProblem::NotRecognised,
                ));
            }
            Some(prefix)
        }
    };
    if flags.value("--expect-tag").is_some() && flags.value("--facts").is_none() {
        return Err(cli_line::usage_invalid(
            Some("--expect-tag"),
            ArgumentProblem::Conflicting,
        ));
    }
    Ok(ReleaseInputs {
        dir: flags.required("--dir")?.to_owned(),
        facts: flags.value("--facts").map(str::to_owned),
        expect_tag: flags.value("--expect-tag").map(str::to_owned),
        expect_commit,
        local_build: flags.switch("--local-build"),
    })
}

/// `install release verify --dir <dir> [--facts <file>] [--expect-tag <tag>] [--expect-commit
/// <hex8+>]`: read-only; the zips are extracted into a new work folder under the host's work
/// directory, left there and named in the result.
pub(crate) fn verify(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    let flags = Flags::parse(
        words,
        &["--dir", "--facts", "--expect-tag", "--expect-commit"],
        &[],
    )?;
    let inputs = inputs(&flags)?;
    let Prechecked {
        located,
        mut failures,
    } = precheck(&inputs)?;
    let Some(located) = located else {
        return refused(&failures);
    };
    let mut ms = host.now_unix_ms();
    let work = loop {
        let candidate = host
            .work_dir()
            .join(format!("actingcommand-release-verify-{ms}"));
        if !candidate.exists() {
            break candidate;
        }
        ms += 1;
    };
    let mut extracted = true;
    for half in [Half::Runtime, Half::Tools] {
        match extract(located.zip(half), &work.join(half.dir())) {
            Ok(()) => {}
            Err(Extracted::Unreadable(_)) => {
                extracted = false;
                failures.push(
                    Failure::new(ReleaseCheck::ZipUnreadable).file(file_name(located.zip(half))),
                );
            }
            Err(Extracted::Failed(cause)) => return Err(cause),
        }
    }
    // Every failure is listed: the extracted files are checked whatever the checks before found.
    let verified = if extracted {
        check_extracted(&work, &located, &mut failures)
    } else {
        None
    };
    let Some(verified) = verified.filter(|_| failures.is_empty()) else {
        return refused(&failures);
    };
    let outcome = match &located.facts {
        Some(facts) => Outcome::new(ActingctlCode::InstallReleaseVerified)
            .commit_sha(located.commit.clone())
            .tag(facts.tag.clone()),
        None => Outcome::new(ActingctlCode::InstallReleaseVerifiedLocal)
            .commit_sha(located.commit.clone()),
    };
    let release = VerifiedRelease::of(&located, &verified);
    Answer::new(outcome)
        .with("commit", release.commit)?
        .with("tree", release.tree)?
        .with("tag", release.tag)?
        .with("prerelease", release.prerelease)?
        .with("interfaces", release.interfaces)?
        .with("files", release.files)?
        .with("work_dir", files::plain(&work))
}

/// The intent of a stage's steps: the build and its part folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageIntent {
    commit: String,
    /// `install/builds/<commit>.new-<ms>.part`.
    part: String,
    /// `install/builds/<commit>`.
    build: String,
}

fn stage_steps(commit: &str, ms: u64) -> Vec<PlannedStep> {
    let part = format!("install/builds/{commit}.new-{ms}.part");
    let build = format!("install/builds/{commit}");
    let intent = serde_json::json!({"commit": commit, "part": part, "build": build});
    vec![
        (StepKind::Extract, part.clone(), intent.clone()),
        (StepKind::VerifyTree, part.clone(), intent.clone()),
        (StepKind::PlaceBuild, build.clone(), intent.clone()),
        (
            StepKind::RecordState,
            "install/state.json".to_owned(),
            intent,
        ),
    ]
}

/// `install release stage --root <root> --dir <dir> [--facts …] [--expect-tag …]
/// [--expect-commit …] [--local-build]`.
pub(crate) fn stage(words: &[&str], host: &dyn Host) -> Result<Answer, Outcome> {
    let flags = Flags::parse(
        words,
        &[
            "--root",
            "--dir",
            "--facts",
            "--expect-tag",
            "--expect-commit",
        ],
        &["--local-build"],
    )?;
    let inputs = inputs(&flags)?;
    let paths = super::resolve_root(&flags)?;
    super::refuse_ab(&paths)?;
    let Prechecked {
        located,
        mut failures,
    } = precheck(&inputs)?;
    if inputs.facts.is_none() && !inputs.local_build {
        failures.insert(0, Failure::new(ReleaseCheck::FactsMissing));
    }
    let Some(located) = located else {
        return refused(&failures);
    };
    if !failures.is_empty() {
        return refused(&failures);
    }
    journal::refuse_unfinished(&paths)?;
    let build = paths.build(&located.commit);
    if build.exists() {
        match manifest::verify_build(&build) {
            Ok(kept) if kept.commit == located.commit => {
                let state = state::load_state_or_default(&paths)
                    .map_err(|cause| journal::state_invalid(paths.state(), Some(cause)))?;
                if state.kept(&located.commit).is_some() {
                    return Answer::new(
                        Outcome::new(ActingctlCode::InstallBuildStaged)
                            .commit_sha(located.commit.clone()),
                    )
                    .with("commit", located.commit.as_str())?
                    .with("unchanged", true)?
                    .with("build", files::plain(&build));
                }
            }
            _ => {
                return Err(Outcome::new(ActingctlCode::InstallUpdateRefused)
                    .install_refusal(InstallRefusal::BuildInvalid)
                    .commit_sha(located.commit.clone())
                    .path(&build));
            }
        }
    }
    let previous = state::load_state_or_default(&paths)
        .map_err(|cause| journal::state_invalid(paths.state(), Some(cause)))?;
    let opening = Opening {
        kind: OperationKind::ReleaseStage,
        inputs: serde_json::to_value(&inputs).map_err(|error| files::json_failure(&error))?,
        plan_id: None,
        plan: None,
        acknowledged: if inputs.local_build {
            vec!["local_build".to_owned()]
        } else {
            Vec::new()
        },
        previous: Previous {
            live: previous.live.as_ref().map(|live| live.commit.clone()),
            state: Some(previous),
        },
    };
    let commit = located.commit.clone();
    let operation = Operation::open(host, &paths, opening, |ms| stage_steps(&commit, ms))?;
    run(operation, &located, &inputs)
}

/// Continues an unfinished stage from its journal: the release folder is checked again first.
pub(crate) fn resume(operation: Operation<'_>) -> Result<Answer, Outcome> {
    let inputs: ReleaseInputs = serde_json::from_value(operation.journal().inputs.clone())
        .map_err(|error| operation.interrupted(files::json_failure(&error)))?;
    let Prechecked { located, failures } =
        precheck(&inputs).map_err(|cause| operation.interrupted(cause))?;
    let located = match located {
        Some(located) if failures.is_empty() => located,
        _ => {
            let refusal = refused(&failures)?;
            return Err(operation.interrupted(refusal.outcome));
        }
    };
    run(operation, &located, &inputs)
}

/// The stage's steps, each skipped when its postcondition already holds on disk.
fn run(
    mut operation: Operation<'_>,
    located: &Located,
    inputs: &ReleaseInputs,
) -> Result<Answer, Outcome> {
    let intent: StageIntent = operation
        .intent(1)
        .map_err(|cause| operation.interrupted(cause))?;
    let paths = operation.paths().clone();
    let part = paths.at(&intent.part);
    let build = paths.at(&intent.build);
    let placed = || manifest::verify_build(&build).is_ok_and(|kept| kept.commit == located.commit);
    let mut failures = Vec::new();
    // 1. extract
    if operation.journal().steps[0].state != StepState::Done {
        if !placed() {
            operation
                .start(1)
                .map_err(|cause| operation.interrupted(cause))?;
            for half in [Half::Runtime, Half::Tools] {
                match extract(located.zip(half), &part.join(half.dir())) {
                    Ok(()) => {}
                    Err(Extracted::Unreadable(_)) => failures.push(
                        Failure::new(ReleaseCheck::ZipUnreadable)
                            .file(file_name(located.zip(half))),
                    ),
                    Err(Extracted::Failed(cause)) => return Err(operation.interrupted(cause)),
                }
            }
            if !failures.is_empty() {
                return abandon(operation, &part, &failures);
            }
        }
        operation
            .done(1, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    // 2. verify_tree
    let mut verified = None;
    if operation.journal().steps[1].state != StepState::Done {
        if !placed() {
            operation
                .start(2)
                .map_err(|cause| operation.interrupted(cause))?;
            match check_extracted(&part, located, &mut failures) {
                Some(found) => verified = Some(found),
                None => return abandon(operation, &part, &failures),
            }
        }
        operation
            .done(2, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    // 3. place_build: release.json written into the part, the part renamed.
    if operation.journal().steps[2].state != StepState::Done {
        if !placed() {
            operation
                .start(3)
                .map_err(|cause| operation.interrupted(cause))?;
            let verified = match verified.take() {
                Some(verified) => verified,
                None => match check_extracted(&part, located, &mut failures) {
                    Some(verified) => verified,
                    None => return abandon(operation, &part, &failures),
                },
            };
            let record = ReleaseRecord {
                schema_version: RELEASE_RECORD_SCHEMA,
                verified: VerifiedRelease::of(located, &verified),
                local_build: inputs.local_build,
                facts: located.facts.as_ref(),
            };
            files::write_json_atomic(&part.join(RELEASE_RECORD_FILE), &record)
                .and_then(|()| files::rename(&part, &build))
                .map_err(|cause| operation.interrupted(cause))?;
        }
        operation
            .done(3, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    // 4. record_state
    if operation.journal().steps[3].state != StepState::Done {
        operation
            .start(4)
            .map_err(|cause| operation.interrupted(cause))?;
        let state =
            state::load_state_or_default(&paths).map_err(|cause| operation.interrupted(cause))?;
        let kept = KeptBuild {
            commit: located.commit.clone(),
            tag: located.facts.as_ref().map(|facts| facts.tag.clone()),
            origin: if located.facts.is_some() {
                BuildOrigin::Release
            } else {
                BuildOrigin::LocalBuild
            },
            operation_id: operation.id().to_owned(),
        };
        state::write_state(&paths, &state.with_build(kept))
            .map_err(|cause| operation.interrupted(cause))?;
        operation
            .done(4, None)
            .map_err(|cause| operation.interrupted(cause))?;
    }
    let operation_id = operation.id().to_owned();
    let outcome = Outcome::new(ActingctlCode::InstallBuildStaged)
        .commit_sha(located.commit.clone())
        .install_operation_id(operation_id.clone());
    operation.finish(&outcome)?;
    Answer::new(outcome)
        .with("commit", located.commit.as_str())?
        .with("unchanged", false)?
        .with("operation_id", operation_id)?
        .with("build", files::plain(&build))
}

/// A check of the extracted files failed: the part folder is renamed
/// `install\builds\<commit>.abandoned-<ms>`, the extraction recorded undone, and the operation
/// finished with the refusal.
fn abandon(
    mut operation: Operation<'_>,
    part: &Path,
    failures: &[Failure],
) -> Result<Answer, Outcome> {
    let answer = refused(failures)?;
    if part.exists() {
        let name = file_name(part);
        let base = name.split(".new-").next().unwrap_or(&name).to_owned();
        let abandoned = part.with_file_name(format!("{base}.abandoned-{}", operation.ms()));
        files::rename(part, &abandoned).map_err(|cause| operation.interrupted(cause))?;
        let evidence = serde_json::json!({ "abandoned": files::plain(&abandoned) });
        operation
            .undone(1, Some(evidence))
            .map_err(|cause| operation.interrupted(cause))?;
    }
    operation.finish(&answer.outcome)?;
    Ok(answer)
}
