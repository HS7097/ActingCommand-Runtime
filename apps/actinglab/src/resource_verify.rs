// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 RV-1 (`contracts/resource-verify.md`): `actinglab resource verify` checks one
//! commit of a resource repository the way the repository's CI does, in this process, with this
//! build's own checks, and writes a report bound to the commit, its tree, its base and the build.
//!
//! The steps run in a fixed order (`tools`, `checkout`, `declarations`, `pins`, `admission`,
//! `format`, `bundle`) and every step runs; only a failed `checkout` precondition leaves the
//! content steps `not_run`. A step lists every failure its check can go on to find. Failures and
//! warnings are outcome envelopes. Git runs read-only (`git.rs`); the content checks reuse the
//! library code of `resource validate`, `package preflight`, `package fmt --check`, `package
//! bundle` and `standard-pack check` (`steps.rs`, `bundle.rs`). The command writes only below
//! `--work` and the report, deletes nothing, opens no Runtime connection, touches no device,
//! writes no ledger and runs no repository code.

mod bundle;
mod git;
mod steps;

use crate::codes::{ActinglabCode, VerifyStep};
use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, OutcomeEnvelope};
use serde::{Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The report's schema, also the `report_schema` of the capability entry.
pub(crate) const REPORT_SCHEMA: &str = "actinglab.resource-verify.v1";
/// The six flags `resource verify` reads; every other flag is refused.
const FLAGS: [&str; 6] = [
    "--repo",
    "--source-repository",
    "--work",
    "--report",
    "--commit",
    "--base",
];
/// The key table's bounds of a path and of a JSON pointer value.
const PATH_VALUE_LIMIT: usize = 1024;
const POINTER_VALUE_LIMIT: usize = 512;

pub(crate) fn options() -> [&'static str; 6] {
    [
        "--repo <the top of a clean Git work tree whose HEAD is the commit to verify>",
        "--source-repository <owner/name, written into bundle.json source>",
        "--work <new directory whose parent exists: everything the command writes besides the report>",
        "--report <new file whose parent exists: the actinglab.resource-verify.v1 report>",
        "--commit <40-hex commit: must be HEAD>",
        "--base <commit-ish, an ancestor of HEAD: the Git diff from it joins the declaration selection>",
    ]
}

/// What the command was asked, its paths absolute.
struct VerifyRequest {
    repo: PathBuf,
    source_repository: String,
    work: PathBuf,
    report: PathBuf,
    commit: Option<String>,
    base: Option<String>,
}

/// `resource verify` (contracts/resource-verify.md, "The command").
pub(crate) fn run_resource_verify(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    verify_with_executable(global, flags, std::env::current_exe())
}

/// [`run_resource_verify`] with the executable the `tools` step reports and checks, which is the
/// running one in production (gate exec 103 line 10 gives another).
pub(crate) fn verify_with_executable(
    global: &GlobalOptions,
    flags: &FlagArgs,
    executable: io::Result<PathBuf>,
) -> CliOutcome<Value> {
    let request = parse_request(global, flags)?;
    crate::dry_run_gate::refuse(global, "resource verify", None, Value::Null)?;
    check_outputs(&request)?;
    let (report, top) = verify(&request, executable);
    let value = match serde_json::to_value(&report) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("ERROR actinglab: resource verify report does not serialize: {error}");
            let outcome = Outcome::new(ActinglabCode::ResourceVerifyReportWriteFailed)
                .path(path_value(&request.report));
            return Err(outcome_error(
                &outcome,
                "the resource verify report does not serialize".to_owned(),
            ));
        }
    };
    if let Err(outcome) = write_report(&request.report, &value) {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            outcome.code().as_str(),
            format!(
                "the report could not be written to {}; the details are the report",
                request.report.display()
            ),
            &[],
        )
        .with_details(value));
    }
    if top.is(ActinglabCode::ResourceVerifyPassed) {
        return Ok(value);
    }
    let message = if report.failed_steps.is_empty() {
        format!("resource verify did not pass ({top}); the details are the report")
    } else {
        format!(
            "resource verify failed in {}; the details are the report",
            report
                .failed_steps
                .iter()
                .map(|step| step.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Err(CliError::new(
        ErrorKind::UsageValidation,
        top.code().as_str(),
        message,
        &[],
    )
    .with_details(value))
}

/// The usage checks, before anything is read: the global options that name a device, a Runtime
/// or an instance, then the six flags and their values.
fn parse_request(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<VerifyRequest> {
    if global.run_root.is_some()
        || global.instance.is_some()
        || global.instances.is_some()
        || global.profile.is_some()
        || global.resource_root.is_some()
        || global.game.is_some()
        || global.server.is_some()
        || global.runtime_endpoint.is_some()
        || global.capture_backend.is_some()
        || global.touch_backend.is_some()
    {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            "offline_device_scope_forbidden",
            "resource verify accepts no global Runtime, device, instance, selector, backend, \
             resource or run-root flag",
            &[],
        ));
    }
    if let Some(positional) = flags.positionals.first() {
        return Err(CliError::usage(format!(
            "resource verify takes no positional argument: {positional}"
        )));
    }
    if let Some(name) = flags
        .flags
        .keys()
        .find(|name| !FLAGS.contains(&name.as_str()) && name.as_str() == "--never-given")
    {
        return Err(CliError::usage(format!(
            "resource verify does not accept {name}"
        )));
    }
    if let Some((name, _)) = flags.flags.iter().find(|(_, values)| values.len() > 1) {
        return Err(CliError::usage(format!(
            "resource verify takes {name} once"
        )));
    }
    let optional = |name: &str| -> CliOutcome<Option<String>> {
        if flags.flags.contains_key(name) {
            flags.required(name).map(Some)
        } else {
            Ok(None)
        }
    };
    let absolute = |name: &str| -> CliOutcome<PathBuf> {
        let given = flags.required_path(name)?;
        std::path::absolute(&given)
            .map_err(|error| CliError::usage(format!("{name} {}: {error}", given.display())))
    };
    let source_repository = flags.required("--source-repository")?;
    if !owner_name(&source_repository) {
        return Err(CliError::usage(format!(
            "--source-repository {source_repository} is not <owner>/<name>"
        )));
    }
    let commit = optional("--commit")?;
    if let Some(commit) = &commit
        && !is_commit(commit)
    {
        return Err(CliError::usage(format!(
            "--commit {commit} is not 40 lowercase hexadecimal characters"
        )));
    }
    let base = optional("--base")?;
    if let Some(base) = &base
        && (base.len() > 256 || base.starts_with('-') || base.chars().any(char::is_control))
    {
        return Err(CliError::usage(
            "--base must be a commit-ish of at most 256 bytes that does not start with '-'",
        ));
    }
    let report = absolute("--report")?;
    if report.file_name().is_none() {
        return Err(CliError::usage("--report names no file"));
    }
    Ok(VerifyRequest {
        repo: absolute("--repo")?,
        source_repository,
        work: absolute("--work")?,
        report,
        commit,
        base,
    })
}

/// `--work`, `--report` and `<report>.part` must not exist, and their parents must.
fn check_outputs(request: &VerifyRequest) -> CliOutcome<()> {
    for path in [&request.work, &request.report, &part_path(&request.report)] {
        let parent = path
            .parent()
            .is_some_and(|parent| fs::metadata(parent).is_ok_and(|metadata| metadata.is_dir()));
        if (fs::symlink_metadata(path).is_ok() && path != &request.report) || !parent {
            let outcome =
                Outcome::new(ActinglabCode::ResourceVerifyOutputExists).path(path_value(path));
            return Err(outcome_error(
                &outcome,
                format!(
                    "{} exists or has no parent directory; resource verify writes only a new \
                     work directory and a new report",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// A usage refusal whose details are its outcome.
fn outcome_error(outcome: &Outcome, message: String) -> CliError {
    let details = serde_json::to_value(outcome.envelope(Detail::Full)).unwrap_or(Value::Null);
    CliError::new(
        ErrorKind::UsageValidation,
        outcome.code().as_str(),
        message,
        &[],
    )
    .with_details(details)
}

fn part_path(report: &Path) -> PathBuf {
    let name = report
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    report.with_file_name(format!("{name}.part"))
}

/// Writes `<report>.part` and renames it to the report.
fn write_report(report: &Path, value: &Value) -> Result<(), Outcome> {
    let failed = |error: &io::Error, op: IoOp| {
        Outcome::new(ActinglabCode::ResourceVerifyReportWriteFailed)
            .path(path_value(report))
            .caused_by(Outcome::from_io_error(error, op).path(path_value(report)))
    };
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        eprintln!("ERROR actinglab: resource verify report does not serialize: {error}");
        Outcome::new(ActinglabCode::ResourceVerifyReportWriteFailed).path(path_value(report))
    })?;
    bytes.push(b'\n');
    let part = part_path(report);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .map_err(|error| failed(&error, IoOp::Write))?;
    fs::rename(part, report).map_err(|error| failed(&error, IoOp::Rename))
}

/// Runs every step in order and assembles the report and its top outcome.
fn verify(request: &VerifyRequest, executable: io::Result<PathBuf>) -> (Report, Outcome) {
    let started_at_unix_ms = unix_ms();
    let git = git::Git::new(&request.repo);
    let (tools, tools_run) = steps::tools(executable);
    let (checkout_facts, checkout, checkout_run) = steps::checkout(&git, request);
    let mut runs = vec![tools_run, checkout_run];
    let mut game = None;
    match &checkout {
        None => {
            for step in [
                VerifyStep::Declarations,
                VerifyStep::Pins,
                VerifyStep::Admission,
                VerifyStep::Format,
                VerifyStep::Bundle,
            ] {
                runs.push(StepRun::not_run(step));
            }
        }
        Some(checkout) => {
            let applications = steps::read_applications(&request.repo);
            game = applications
                .as_ref()
                .map(|applications| applications.game.clone());
            let layout = steps::read_layout(&request.repo);
            runs.push(steps::declarations(&git, &request.repo, checkout));
            runs.push(steps::pins(&git, &request.repo));
            let (identities, admission) =
                steps::admission(&request.repo, applications.as_ref(), &layout);
            runs.push(admission);
            runs.push(steps::format(&request.repo, &layout));
            runs.push(bundle::bundle(&bundle::BundleInputs {
                repo: &request.repo,
                work: &request.work,
                source_repository: &request.source_repository,
                commit: &checkout.commit,
                applications: applications.as_ref(),
                packs: &layout.directories,
                identities: &identities,
            }));
        }
    }
    let failed_steps = runs
        .iter()
        .filter(|run| run.failed())
        .map(|run| run.step)
        .collect::<Vec<_>>();
    let top = match (&checkout, runs.iter().find(|run| run.failed())) {
        (_, None) => Outcome::new(ActinglabCode::ResourceVerifyPassed).step_count(runs.len()),
        // The precondition's own outcome is the result.
        (None, Some(_)) if checkout_run_failed(&runs) => runs
            .iter()
            .find(|run| run.step == VerifyStep::Checkout)
            .and_then(StepRun::first_failure)
            .cloned()
            .unwrap_or_else(|| {
                Outcome::new(ActinglabCode::ResourceVerifyFailed)
                    .failed_step_count(failed_steps.len())
            }),
        _ => runs.iter().filter_map(StepRun::first_failure).fold(
            Outcome::new(ActinglabCode::ResourceVerifyFailed).failed_step_count(failed_steps.len()),
            |top, first| top.caused_by(first.clone()),
        ),
    };
    let report = Report {
        schema_version: REPORT_SCHEMA,
        result: if top.is(ActinglabCode::ResourceVerifyPassed) {
            VerifyResult::Passed
        } else {
            VerifyResult::Failed
        },
        outcome: top.envelope(Detail::Full),
        started_at_unix_ms,
        finished_at_unix_ms: unix_ms(),
        repository: RepositoryReport {
            root: request.repo.display().to_string(),
            commit: checkout_facts.commit,
            tree: checkout_facts.tree.map(|_| String::new()),
            base: request.base.as_ref().map(|rev| BaseReport {
                rev: rev.clone(),
                commit: checkout_facts.base,
            }),
            source_repository: request.source_repository.clone(),
            game,
        },
        tools,
        work: request.work.display().to_string(),
        steps: runs.iter().map(StepRun::report).collect(),
        failed_steps,
    };
    (report, top)
}

fn checkout_run_failed(runs: &[StepRun]) -> bool {
    runs.iter()
        .any(|run| run.step == VerifyStep::Checkout && run.failed())
}

/// One step as it runs: its failures and warnings as outcomes, and its counts.
pub(super) struct StepRun {
    pub(super) step: VerifyStep,
    started: Instant,
    duration_ms: Option<u64>,
    pub(super) counts: Counts,
    failures: Vec<Outcome>,
    warnings: Vec<Outcome>,
    ran: bool,
}

impl StepRun {
    pub(super) fn start(step: VerifyStep) -> Self {
        Self {
            step,
            started: Instant::now(),
            duration_ms: None,
            counts: Counts::Empty {},
            failures: Vec::new(),
            warnings: Vec::new(),
            ran: true,
        }
    }

    fn not_run(step: VerifyStep) -> Self {
        Self {
            ran: false,
            duration_ms: Some(0),
            ..Self::start(step)
        }
    }

    /// Records a failure, with this step as its `step` where its code declares the key.
    pub(super) fn fail(&mut self, outcome: Outcome) {
        let outcome = if declares(&outcome, "step") {
            outcome.step(self.step)
        } else {
            outcome
        };
        self.failures.push(outcome);
    }

    /// Records a warning once.
    pub(super) fn warn(&mut self, outcome: Outcome) {
        if !self.warnings.contains(&outcome) {
            self.warnings.push(outcome);
        }
    }

    pub(super) fn failed(&self) -> bool {
        !self.failures.is_empty()
    }

    fn first_failure(&self) -> Option<&Outcome> {
        self.failures.first()
    }

    /// Ends the step's clock.
    pub(super) fn finish(mut self) -> Self {
        let elapsed = self.started.elapsed().as_millis();
        self.duration_ms = Some(u64::try_from(elapsed).unwrap_or(u64::MAX));
        self
    }

    fn report(&self) -> StepReport {
        StepReport {
            step: self.step,
            result: if !self.ran {
                StepResult::NotRun
            } else if self.failed() {
                StepResult::Failed
            } else {
                StepResult::Passed
            },
            duration_ms: self.duration_ms.unwrap_or(0),
            counts: self.counts.clone(),
            failures: self
                .failures
                .iter()
                .map(|outcome| outcome.envelope(Detail::Full))
                .collect(),
            warnings: self
                .warnings
                .iter()
                .map(|outcome| outcome.envelope(Detail::Full))
                .collect(),
        }
    }
}

/// Whether the embedded catalog lists `key` for the outcome's code. A reused code carries only
/// the values its entry declares; this slice declares `step` and the others it attaches.
pub(super) fn declares(outcome: &Outcome, key: &str) -> bool {
    actingcommand_contract::outcome::catalog_json()
        .ok()
        .and_then(|catalog| {
            catalog
                .get("codes")?
                .get(outcome.code().as_str())?
                .get("values")?
                .get(key)
        })
        .is_some()
}

/// The lowercase hex SHA-256 of `bytes`.
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The wall clock in Unix milliseconds, for the report's two times.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// A path within the key table's bound, cut on a character boundary.
pub(super) fn path_value(path: impl AsRef<Path>) -> String {
    cut(&path.as_ref().to_string_lossy(), PATH_VALUE_LIMIT)
}

/// A JSON pointer within the key table's bound.
pub(super) fn pointer_value(pointer: &str) -> String {
    cut(pointer, POINTER_VALUE_LIMIT)
}

fn cut(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// `<owner>/<name>`, each one GitHub name segment.
fn owner_name(text: &str) -> bool {
    let segment = |value: &str| {
        !value.is_empty()
            && value.len() <= 100
            && value != "."
            && value != ".."
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    text.split_once('/')
        .is_some_and(|(owner, name)| segment(owner) && segment(name))
}

fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// What a step counts (contracts/resource-verify.md, "Report"): `{}` for `tools` and a step that
/// did not run.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(super) enum Counts {
    Empty {},
    Checkout {
        files_checked: usize,
    },
    Declarations {
        selected: usize,
        deleted: usize,
        valid: usize,
        excluded: usize,
        read_files: usize,
        read_bytes: u64,
        found_after_first: usize,
    },
    Pins {
        files: usize,
    },
    Admission {
        packs: usize,
        admitted: usize,
    },
    Format {
        packs: usize,
        json_files: usize,
        unformatted: usize,
    },
    Bundle {
        packs: usize,
        files: u64,
        bytes: u64,
        zip: Option<ZipCounts>,
        root_files: Vec<String>,
        package_revision: u32,
    },
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ZipCounts {
    pub(super) name: String,
    pub(super) sha256: String,
    pub(super) entries: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum VerifyResult {
    Passed,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum StepResult {
    Passed,
    Failed,
    NotRun,
}

/// `actinglab.resource-verify.v1`.
#[derive(Serialize)]
struct Report {
    schema_version: &'static str,
    result: VerifyResult,
    outcome: OutcomeEnvelope,
    started_at_unix_ms: u64,
    finished_at_unix_ms: u64,
    repository: RepositoryReport,
    tools: steps::ToolsReport,
    work: String,
    steps: Vec<StepReport>,
    #[serde(serialize_with = "step_tokens")]
    failed_steps: Vec<VerifyStep>,
}

#[derive(Serialize)]
struct RepositoryReport {
    root: String,
    commit: Option<String>,
    tree: Option<String>,
    base: Option<BaseReport>,
    source_repository: String,
    game: Option<String>,
}

#[derive(Serialize)]
struct BaseReport {
    rev: String,
    commit: Option<String>,
}

#[derive(Serialize)]
struct StepReport {
    #[serde(serialize_with = "step_token")]
    step: VerifyStep,
    result: StepResult,
    duration_ms: u64,
    counts: Counts,
    failures: Vec<OutcomeEnvelope>,
    warnings: Vec<OutcomeEnvelope>,
}

fn step_token<S: Serializer>(step: &VerifyStep, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(step.as_str())
}

fn step_tokens<S: Serializer>(steps: &[VerifyStep], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(steps.iter().map(|step| step.as_str()))
}
