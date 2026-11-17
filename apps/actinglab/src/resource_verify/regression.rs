// SPDX-License-Identifier: AGPL-3.0-only

//! #463 RC-1 (`contracts/resource-regression-case.md`, "The operation"): `actinglab resource
//! regression --repo <checkout> [--game <game>] [--case <case>] [--package-id <id>]` runs the
//! regression cases of a resource repository's checkout offline, each paired with its package
//! under the checkout's `packs/`, through the Lab's runner (`actingcommand_lab`'s
//! `read_regression_case` and `RegressionPackage`, the functions `record stop` will call too), and
//! answers every case's verdict and the counts.
//!
//! - **The cases** are the directories `regression/<game>/<case>/`, in the order of their names;
//!   `--game` (actinglab's global option) keeps one game's, `--case` the directories of one name,
//!   `--package-id` the cases whose `case.json` names that package. A directory without a
//!   readable `case.json` is a case of its own, `failed` (`case_missing`), unless `--package-id`
//!   is given, since its package is not known. No case selected is refused.
//! - **The package** of a case is admitted once per run, however many cases name it.
//! - **The answer** (fix round 1: it always fits and says which cases failed first): `repo` and
//!   `counts`; a short row of a case ([`CaseRow`]) is its `path`, `id`, `package_id`, verdict, the
//!   checks that did not pass (each with its word and what it names, without its `detail`) and its
//!   warnings. When a case failed, the answer is the details of the refusal
//!   `resource_regression_failed` (exit 1 on actinglab's own line, 2 on the `--via` line) and its
//!   `cases` are the rows of the cases that did not pass, the failed ones first, at most
//!   [`ROWS_LISTED_MAX`], with `failed_total` (how many did not pass) and `listed`. Otherwise it is
//!   the result, with `cases` only when `--game`, `--case` or `--package-id` asked for them: every
//!   selected case's row. Partial cases are said aloud: the warning `resource_regression_partial`
//!   with their count, in `warnings`. A case's warnings are also written to stderr as developer
//!   lines.
//! - It reads only: nothing is written, no Runtime is reached, no device is touched.

use crate::cli_module::PROGRAM;
use crate::codes::ActinglabCode;
use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab::{
    LabOcr, REGRESSION_CASE_FILE, RegressionCaseVerdict, RegressionCheck, RegressionCounts,
    RegressionPackage, RegressionProblem, RegressionResult, RegressionVerdict, RegressionWarning,
    read_regression_case,
};
use actingcommand_lab_ops::cli_line;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The three flags `resource regression` reads (its game is actinglab's global `--game`); every
/// other flag is refused.
pub(crate) const REGRESSION_FLAGS: [&str; 3] = ["--repo", "--case", "--package-id"];
/// The directory of a checkout that holds the cases, by game, and the one that holds the
/// packages, by `package_id`.
const REGRESSION_DIRECTORY: &str = "regression";
const PACKS_DIRECTORY: &str = "packs";
/// Fix round 1: the most rows of cases that did not pass a failing run lists, the failed ones
/// first; the rest are counted in `failed_total` and left out, so that the refusal fits the Lab's
/// own line and the MCP server's answer budget whatever the number of cases.
pub(crate) const ROWS_LISTED_MAX: usize = 10;

pub(crate) fn regression_options() -> [&'static str; 4] {
    [
        "--repo <a resource repository's checkout>",
        "--game <game> (global option: only the cases under regression/<game>/)",
        "--case <the name of a case's directory>",
        "--package-id <the package_id a case names>",
    ]
}

/// What the command was asked, the checkout's path absolute.
struct RegressionRequest {
    repo: PathBuf,
    game: Option<String>,
    case: Option<String>,
    package_id: Option<String>,
}

/// A case's directory, `regression/<game>/<case>/`.
struct CaseDirectory {
    game: String,
    case: String,
    path: PathBuf,
}

/// One case run: where it is (`regression/<game>/<case>`, relative to the checkout) and its
/// verdict. #463 AC-1 fix round 1: adoption lists its cases' runs so too
/// (`standard_pack_cli::LabAdoption`), each by the directory it would write.
pub(crate) struct CaseRun {
    pub(crate) path: String,
    pub(crate) verdict: RegressionCaseVerdict,
}

/// Fix round 1: one case of the answer, short: where it is, its `id` and `package_id` (`null`
/// when its `case.json` gives none as text), its verdict, the checks that did not pass (failed or
/// not run, each with its word and what it names: its place `at`, the page, the target, the two
/// values compared; without the `detail` text) and its warnings. Never the case's document.
#[derive(Serialize)]
pub(crate) struct CaseRow {
    path: String,
    id: Option<String>,
    package_id: Option<String>,
    verdict: &'static str,
    checks: Vec<RegressionCheck>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<RegressionWarning>,
}

/// The answer: the counts; when a case failed, how many did not pass and how many rows are listed;
/// the rows (see the module header); the run's warnings.
#[derive(Serialize)]
struct RegressionAnswer {
    repo: String,
    counts: RegressionCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    listed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cases: Option<Vec<CaseRow>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<Value>,
}

/// The packages a run admitted, by `package_id`, each once.
type Packages = BTreeMap<String, Result<RegressionPackage, RegressionCheck>>;

/// `resource regression` (`contracts/resource-regression-case.md`, "The operation").
pub(crate) fn run_resource_regression(
    global: &GlobalOptions,
    flags: &FlagArgs,
) -> CliOutcome<Value> {
    let request = parse_request(global, flags)?;
    let ocr = crate::offline_ocr::lab_ocr();
    let mut packages = Packages::new();
    let mut runs = Vec::new();
    for directory in case_directories(&request)? {
        if let Some(run) = run_case(&request, directory, &mut packages, ocr) {
            runs.push(run);
        }
    }
    if runs.is_empty() {
        return Err(CliError::usage(format!(
            "resource regression found no case under {} that its filters select",
            request.repo.join(REGRESSION_DIRECTORY).display()
        )));
    }
    for run in &runs {
        for warning in &run.verdict.warnings {
            eprintln!(
                "{}",
                cli_line::developer_text(PROGRAM, &warning.outcome, Some(run.path.as_str()))
            );
        }
    }
    let counts = RegressionCounts::of(runs.iter().map(|run| &run.verdict));
    // Fix round 1: partial cases are said aloud, never passed silently.
    let mut warnings = Vec::new();
    if counts.partial > 0 {
        let partial = Outcome::new(ActinglabCode::ResourceRegressionPartial).count(counts.partial);
        eprintln!("{}", cli_line::developer_text(PROGRAM, &partial, None));
        warnings.push(cli_line::warning(&partial));
    }
    let repo = request.repo.display().to_string();
    if counts.failed == 0 {
        let asked =
            request.game.is_some() || request.case.is_some() || request.package_id.is_some();
        let answer = RegressionAnswer {
            repo,
            counts,
            failed_total: None,
            listed: None,
            cases: asked.then(|| runs.iter().map(CaseRow::of).collect()),
            warnings,
        };
        return answer_value(&answer);
    }
    // Fix round 1: the cases that did not pass, the failed ones first, each group in the order of
    // their paths; the first `ROWS_LISTED_MAX` of them are listed.
    let NotPassed { total, rows } = not_passed(&runs);
    let message = format!(
        "{} of {} regression cases failed, the first {}; the details list the first {} of the {} \
         cases that did not pass, the failed ones first",
        counts.failed,
        counts.cases,
        rows.first().map_or("", |row| row.path.as_str()),
        rows.len(),
        total
    );
    let answer = RegressionAnswer {
        repo,
        counts,
        failed_total: Some(total),
        listed: Some(rows.len()),
        cases: Some(rows),
        warnings,
    };
    Err(CliError::new(
        ErrorKind::UsageValidation,
        ActinglabCode::ResourceRegressionFailed.as_str(),
        message,
        &[],
    )
    .with_details(answer_value(&answer)?))
}

/// `answer` as the command's JSON value.
fn answer_value(answer: &RegressionAnswer) -> CliOutcome<Value> {
    serde_json::to_value(answer).map_err(|error| {
        CliError::device(format!(
            "the answer of resource regression does not serialize: {error}"
        ))
    })
}

/// Fix round 1: the cases of a run that did not pass, as a failing run lists them.
pub(crate) struct NotPassed {
    /// How many did not pass (`failed` and `partial`), the answer's `failed_total`.
    pub(crate) total: usize,
    /// The rows of the first [`ROWS_LISTED_MAX`] of them, the failed ones first.
    pub(crate) rows: Vec<CaseRow>,
}

/// Fix round 1: the cases of `runs` that did not pass, the failed ones first, each group in the
/// order of `runs` (the order of their paths), the rows of the first [`ROWS_LISTED_MAX`] of them
/// short ([`CaseRow`]), so that the refusal fits the Lab's own line and the MCP server's answer
/// budget whatever the number of cases. #463 AC-1 fix round 1: the one rule, which `standard-pack
/// adopt`'s `case_failed` lists its cases by too (`standard_pack_cli::LabAdoption`).
pub(crate) fn not_passed(runs: &[CaseRun]) -> NotPassed {
    let did_not_pass = [RegressionVerdict::Failed, RegressionVerdict::Partial]
        .into_iter()
        .flat_map(|verdict| {
            runs.iter()
                .filter(move |run| run.verdict.verdict == verdict)
        })
        .collect::<Vec<_>>();
    NotPassed {
        total: did_not_pass.len(),
        rows: did_not_pass
            .into_iter()
            .take(ROWS_LISTED_MAX)
            .map(CaseRow::of)
            .collect(),
    }
}

impl CaseRow {
    /// The short row of the case run `run`.
    fn of(run: &CaseRun) -> Self {
        let verdict = &run.verdict;
        Self {
            path: run.path.clone(),
            id: verdict.id.clone(),
            package_id: verdict.package_id.clone(),
            verdict: verdict.verdict.as_str(),
            checks: verdict
                .checks
                .iter()
                .filter(|check| check.result != RegressionResult::Passed)
                .map(|check| RegressionCheck {
                    detail: None,
                    ..check.clone()
                })
                .collect(),
            warnings: verdict.warnings.clone(),
        }
    }
}

/// The usage checks, before anything is read: the global options that name a device, a Runtime,
/// an instance or a server, then the three flags and their values.
fn parse_request(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<RegressionRequest> {
    if global.run_root.is_some()
        || global.instance.is_some()
        || global.instances.is_some()
        || global.profile.is_some()
        || global.resource_root.is_some()
        || global.server.is_some()
        || global.runtime_endpoint.is_some()
        || global.capture_backend.is_some()
        || global.touch_backend.is_some()
    {
        return Err(CliError::new(
            ErrorKind::UsageValidation,
            "offline_device_scope_forbidden",
            "resource regression accepts no global Runtime, device, instance, server, backend, \
             resource or run-root flag; of the global options it reads --game",
            &[],
        ));
    }
    if let Some(positional) = flags.positionals.first() {
        return Err(CliError::usage(format!(
            "resource regression takes no positional argument: {positional}"
        )));
    }
    if let Some(name) = flags
        .flags
        .keys()
        .find(|name| !REGRESSION_FLAGS.contains(&name.as_str()))
    {
        return Err(CliError::usage(format!(
            "resource regression does not accept {name}"
        )));
    }
    if let Some((name, _)) = flags.flags.iter().find(|(_, values)| values.len() > 1) {
        return Err(CliError::usage(format!(
            "resource regression takes {name} once"
        )));
    }
    let optional = |name: &str| -> CliOutcome<Option<String>> {
        if flags.flags.contains_key(name) {
            flags.required(name).map(Some)
        } else {
            Ok(None)
        }
    };
    let given = flags.required_path("--repo")?;
    let repo = std::path::absolute(&given)
        .map_err(|error| CliError::usage(format!("--repo {}: {error}", given.display())))?;
    Ok(RegressionRequest {
        repo,
        game: global.game.clone(),
        case: optional("--case")?,
        package_id: optional("--package-id")?,
    })
}

/// The case directories `request` selects, by game and case name in the order of their names.
fn case_directories(request: &RegressionRequest) -> CliOutcome<Vec<CaseDirectory>> {
    let mut found = Vec::new();
    for (game, game_path) in directories(&request.repo.join(REGRESSION_DIRECTORY))? {
        if request.game.as_ref().is_some_and(|wanted| *wanted != game) {
            continue;
        }
        for (case, path) in directories(&game_path)? {
            if request.case.as_ref().is_some_and(|wanted| *wanted != case) {
                continue;
            }
            found.push(CaseDirectory {
                game: game.clone(),
                case,
                path,
            });
        }
    }
    Ok(found)
}

/// The directories directly in `parent`, each with its name, in the order of their names; none
/// when `parent` is not there. A directory that cannot be listed is refused.
fn directories(parent: &Path) -> CliOutcome<Vec<(String, PathBuf)>> {
    let unlisted = |error: std::io::Error| {
        CliError::usage(format!("{} cannot be listed: {error}", parent.display()))
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unlisted(error)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.map_err(unlisted)?.path();
        if fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            found.push((name, path));
        }
    }
    found.sort();
    Ok(found)
}

/// The answer of one case directory, or none when `--package-id` does not select it
/// ([`case_of_directory`], its package `packs/<package_id>/` of the checkout admitted once per
/// run, kept in `packages`).
fn run_case(
    request: &RegressionRequest,
    directory: CaseDirectory,
    packages: &mut Packages,
    ocr: &LabOcr,
) -> Option<CaseRun> {
    let path = format!(
        "{REGRESSION_DIRECTORY}/{}/{}",
        directory.game, directory.case
    );
    let mut checkout = CheckoutPackages {
        request,
        admitted: packages,
        ocr,
    };
    let read = case_of_directory(&directory.path, &mut checkout)?;
    Some(CaseRun {
        path,
        verdict: read.verdict,
    })
}

/// #463 AC-1 fix round 1: which cases a reading of case directories takes and the package each is
/// run on ([`case_of_directory`]).
pub(crate) trait CasePackages {
    /// Whether the case whose `case.json` names the package `package_id` (`None` when it names
    /// none as text, or cannot be read) is taken; every case is, unless a filter says otherwise.
    fn selects(&self, _package_id: Option<&str>) -> bool {
        true
    }

    /// The package the case of `package_id` is run on, as the runner admitted it
    /// (`RegressionPackage::load`), or the check that refused it.
    fn package(&mut self, package_id: &str) -> &Result<RegressionPackage, RegressionCheck>;
}

/// The packages of a checkout `resource regression` runs its cases on: `packs/<package_id>/` of
/// `--repo`, each admitted once per run (kept in `admitted`); `--package-id` selects.
struct CheckoutPackages<'a> {
    request: &'a RegressionRequest,
    admitted: &'a mut Packages,
    ocr: &'a LabOcr,
}

impl CasePackages for CheckoutPackages<'_> {
    fn selects(&self, package_id: Option<&str>) -> bool {
        self.request
            .package_id
            .as_deref()
            .is_none_or(|wanted| package_id == Some(wanted))
    }

    fn package(&mut self, package_id: &str) -> &Result<RegressionPackage, RegressionCheck> {
        let package_dir = self.request.repo.join(PACKS_DIRECTORY).join(package_id);
        let ocr = self.ocr;
        self.admitted
            .entry(package_id.to_owned())
            .or_insert_with(|| RegressionPackage::load(&package_dir, ocr))
    }
}

/// #463 AC-1 fix round 1: a case directory as it was read and run ([`case_of_directory`]).
pub(crate) struct CaseRead {
    pub(crate) verdict: RegressionCaseVerdict,
    /// The case's files as they were read, each by its name in the directory: its `case.json`
    /// when it could be read, and the scene it names when that could be read too.
    pub(crate) files: Vec<(String, Vec<u8>)>,
}

/// #463 RC-1 (`contracts/resource-regression-case.md`, "The operation"); #463 AC-1 fix round 1:
/// the one reading of a case directory, which `resource regression` and `standard-pack adopt`
/// (`standard_pack_cli::LabAdoption`) both call. Its `case.json` read by the one reader (a file
/// that cannot be read is `failed` with `case_missing`; the reader's refusal is its verdict); none
/// when `packages` does not select the package the case names; the scene it names read
/// (`scene_missing` when it cannot be); and the case run on the package `packages` gives for it
/// (refused by the package's own check when the runner refused the package). With the files read.
pub(crate) fn case_of_directory(
    directory: &Path,
    packages: &mut dyn CasePackages,
) -> Option<CaseRead> {
    let case_json = match fs::read(directory.join(REGRESSION_CASE_FILE)) {
        Ok(bytes) => bytes,
        Err(error) => {
            if !packages.selects(None) {
                return None;
            }
            let verdict = RegressionCaseVerdict::refused(
                None,
                None,
                RegressionCheck::failed(
                    "",
                    RegressionProblem::CaseMissing,
                    Some(format!("{REGRESSION_CASE_FILE} cannot be read: {error}")),
                ),
            );
            return Some(CaseRead {
                verdict,
                files: Vec::new(),
            });
        }
    };
    let case = match read_regression_case(&case_json) {
        Ok(case) => case,
        Err(verdict) => {
            if !packages.selects(verdict.package_id.as_deref()) {
                return None;
            }
            return Some(CaseRead {
                verdict,
                files: vec![(REGRESSION_CASE_FILE.to_owned(), case_json)],
            });
        }
    };
    if !packages.selects(Some(case.package_id())) {
        return None;
    }
    let scene = match fs::read(directory.join(case.scene())) {
        Ok(scene) => scene,
        Err(error) => {
            let verdict = case.refused(RegressionCheck::failed(
                "/scene",
                RegressionProblem::SceneMissing,
                Some(format!("{} cannot be read: {error}", case.scene())),
            ));
            return Some(CaseRead {
                verdict,
                files: vec![(REGRESSION_CASE_FILE.to_owned(), case_json)],
            });
        }
    };
    let verdict = match packages.package(case.package_id()) {
        Ok(package) => package.run(&case, &scene),
        Err(check) => case.refused(check.clone()),
    };
    Some(CaseRead {
        verdict,
        files: vec![
            (REGRESSION_CASE_FILE.to_owned(), case_json),
            (case.scene().to_owned(), scene),
        ],
    })
}
