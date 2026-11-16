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
//! - **The answer**: `repo`, `counts`, `failed` (the paths of the failed cases) and `cases`, each
//!   with its `path`, `game` and `case` beside its verdict; a case's warnings are also written to
//!   stderr as developer lines. When a case failed, the answer is the details of the refusal
//!   `resource_regression_failed` (exit 1 on actinglab's own line); otherwise it is the result.
//! - It reads only: nothing is written, no Runtime is reached, no device is touched.

use crate::cli_module::PROGRAM;
use crate::codes::ActinglabCode;
use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_lab::{
    LabOcr, REGRESSION_CASE_FILE, RegressionCase, RegressionCaseVerdict, RegressionCheck,
    RegressionCounts, RegressionPackage, RegressionProblem, RegressionVerdict,
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

/// One case of the answer: where it is, and its verdict.
#[derive(Serialize)]
struct CaseAnswer {
    /// `regression/<game>/<case>`, relative to the checkout.
    path: String,
    game: String,
    case: String,
    #[serde(flatten)]
    verdict: RegressionCaseVerdict,
}

/// The answer: the counts, the failed cases' paths, then every case.
#[derive(Serialize)]
struct RegressionAnswer {
    repo: String,
    counts: RegressionCounts,
    failed: Vec<String>,
    cases: Vec<CaseAnswer>,
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
    let mut cases = Vec::new();
    for directory in case_directories(&request)? {
        if let Some(case) = run_case(&request, directory, &mut packages, ocr) {
            cases.push(case);
        }
    }
    if cases.is_empty() {
        return Err(CliError::usage(format!(
            "resource regression found no case under {} that its filters select",
            request.repo.join(REGRESSION_DIRECTORY).display()
        )));
    }
    for case in &cases {
        for warning in &case.verdict.warnings {
            eprintln!(
                "{}",
                cli_line::developer_text(PROGRAM, &warning.outcome, Some(&case.path))
            );
        }
    }
    let counts = RegressionCounts::of(cases.iter().map(|case| &case.verdict));
    let failed = cases
        .iter()
        .filter(|case| case.verdict.verdict == RegressionVerdict::Failed)
        .map(|case| case.path.clone())
        .collect::<Vec<_>>();
    let message = format!(
        "{} of {} regression cases failed ({}); the details are the answer",
        failed.len(),
        counts.cases,
        failed.join(", ")
    );
    let answer = RegressionAnswer {
        repo: request.repo.display().to_string(),
        counts,
        failed,
        cases,
    };
    let value = serde_json::to_value(&answer).map_err(|error| {
        CliError::device(format!(
            "the answer of resource regression does not serialize: {error}"
        ))
    })?;
    if counts.failed == 0 {
        return Ok(value);
    }
    Err(CliError::new(
        ErrorKind::UsageValidation,
        ActinglabCode::ResourceRegressionFailed.as_str(),
        message,
        &[],
    )
    .with_details(value))
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

/// The answer of one case directory, or none when `--package-id` does not select it: its
/// `case.json` read, its scene read, its package admitted (once per run, kept in `packages`) and
/// the case run on it.
fn run_case(
    request: &RegressionRequest,
    directory: CaseDirectory,
    packages: &mut Packages,
    ocr: &LabOcr,
) -> Option<CaseAnswer> {
    let path = format!(
        "{REGRESSION_DIRECTORY}/{}/{}",
        directory.game, directory.case
    );
    let selects = |package_id: Option<&str>| {
        request
            .package_id
            .as_deref()
            .is_none_or(|wanted| package_id == Some(wanted))
    };
    let verdict = match fs::read(directory.path.join(REGRESSION_CASE_FILE)) {
        Err(error) => {
            if !selects(None) {
                return None;
            }
            RegressionCaseVerdict::refused(
                None,
                None,
                RegressionCheck::failed(
                    "",
                    RegressionProblem::CaseMissing,
                    Some(format!("{REGRESSION_CASE_FILE} cannot be read: {error}")),
                ),
            )
        }
        Ok(bytes) => match read_regression_case(&bytes) {
            Err(verdict) => {
                if !selects(verdict.package_id.as_deref()) {
                    return None;
                }
                verdict
            }
            Ok(case) => {
                if !selects(Some(case.package_id())) {
                    return None;
                }
                run_read_case(&request.repo, &directory.path, &case, packages, ocr)
            }
        },
    };
    Some(CaseAnswer {
        path,
        game: directory.game,
        case: directory.case,
        verdict,
    })
}

/// The verdict of the read case `case` of the directory `directory`: its scene read, its package
/// `packs/<package_id>/` of the checkout `repo` admitted (once per run, kept in `packages`) and
/// the case run on it.
fn run_read_case(
    repo: &Path,
    directory: &Path,
    case: &RegressionCase,
    packages: &mut Packages,
    ocr: &LabOcr,
) -> RegressionCaseVerdict {
    let scene = match fs::read(directory.join(case.scene())) {
        Ok(scene) => scene,
        Err(error) => {
            return case.refused(RegressionCheck::failed(
                "/scene",
                RegressionProblem::SceneMissing,
                Some(format!("{} cannot be read: {error}", case.scene())),
            ));
        }
    };
    let package_dir = repo.join(PACKS_DIRECTORY).join(case.package_id());
    let package = packages
        .entry(case.package_id().to_owned())
        .or_insert_with(|| RegressionPackage::load(&package_dir, ocr));
    match package {
        Ok(package) => package.run(case, &scene),
        Err(check) => case.refused(check.clone()),
    }
}
