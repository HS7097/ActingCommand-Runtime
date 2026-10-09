// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 §2.1, guard 1 (Batch 0a): the jobs of `.github/workflows/ci.yml` drop no test
//! silently.
//! - Every workspace member is in exactly one module job's `CI_PACKAGES`.
//! - `gate` (check name `rust`) runs `if: always()` and needs every job except itself and the
//!   non-gating `flaky` job.
//! - The apps job selects by target kind (`--bins`, `--test '*'`), so none of its packages has a
//!   lib target.
//! - Every entry of the flaky-test mirror `ci/flaky-tests.toml` (board issue #71, D5) names a
//!   real `#[test]` function without `#[ignore]`.
//! - Its module job skips it (`CI_SKIP`), and each skip path matches exactly one test of that
//!   job: libtest's `--skip` matches substrings.
//! - The `flaky` job runs it (`FLAKY_TESTS`) in every configuration its module job runs it in.
//! - No job skips a test the mirror does not hold.
//!
//! ci.yml keeps these as one-line forms, so the guard needs no YAML parser. Tests are found by
//! walking each target's module tree (`mod`, `#[path]`, item-level `include!`); a `#[test]`
//! written by a macro is not seen.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const CI_WORKFLOW: &str = ".github/workflows/ci.yml";
const FLAKY_MIRROR: &str = "ci/flaky-tests.toml";
const FLAKY_REGISTER: &str =
    "https://github.com/HS7097/ActingCommand-Workflow/issues/71#issuecomment-";
const LINT_JOB: &str = "lint";
const GATE_JOB: &str = "gate";
const FLAKY_JOB: &str = "flaky";
/// The module job that selects its tests by target kind (`--bins` and `--test '*'`).
const TARGET_KIND_JOB: &str = "apps";
/// One-line keys at the job level (four-space indent).
const JOB_KEYS: &[&str] = &["needs", "if"];
/// One-line keys in the job's `env:` (six-space indent).
const JOB_ENV_KEYS: &[&str] = &[
    "CI_PACKAGES",
    "CI_SKIP",
    "CI_FEATURES",
    "CI_EXTRA_FEATURES",
    "CI_EXTRA_FILTER",
    "FLAKY_TESTS",
];
const MIRROR_KEYS: &[&str] = &[
    "crate", "features", "id", "job", "name", "register", "status",
];

/// Job id → one-line key → every value the job declares for that key.
type CiJobs = BTreeMap<String, BTreeMap<String, Vec<String>>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Required,
    Optional,
    Forbidden,
}

struct ModuleJob {
    packages: Vec<String>,
    skips: Vec<String>,
    /// The `--features` value of the job's runs; empty for the default features.
    features: String,
    /// `CI_EXTRA_FEATURES` and `CI_EXTRA_FILTER`: one more `--lib` run under those features,
    /// limited to the filters.
    extra: Option<(String, Vec<String>)>,
}

#[derive(Clone)]
struct TestCase {
    /// `lib`, `bin:<name>` or the integration test's name.
    target: String,
    /// The libtest path inside the target.
    path: String,
    ignored: bool,
}

#[test]
fn gate_ci_jobs_cover_every_member() {
    let root = workspace_root();
    let jobs = ci_jobs(&read(&root.join(CI_WORKFLOW)));
    let members = workspace_members(&root);
    let mut violations = Vec::new();

    for reserved in [LINT_JOB, GATE_JOB, FLAKY_JOB, TARGET_KIND_JOB] {
        if !jobs.contains_key(reserved) {
            violations.push(format!("{CI_WORKFLOW} has no `{reserved}` job"));
        }
    }

    // Each job declares exactly the one-line forms of its kind.
    let mut modules: BTreeMap<String, ModuleJob> = BTreeMap::new();
    for (job, lines) in &jobs {
        for key in JOB_KEYS.iter().chain(JOB_ENV_KEYS) {
            let count = lines.get(*key).map_or(0, Vec::len);
            let allowed = match presence(job, key) {
                Presence::Required => count == 1,
                Presence::Optional => count <= 1,
                Presence::Forbidden => count == 0,
            };
            if !allowed {
                violations.push(format!(
                    "job `{job}` declares `{key}:` {count} time(s), which its kind does not allow"
                ));
            }
        }
        if presence(job, "CI_PACKAGES") != Presence::Required {
            continue;
        }
        match module_job(lines) {
            Ok(module) => {
                modules.insert(job.clone(), module);
            }
            Err(error) => violations.push(format!("job `{job}`: {error}")),
        }
    }

    // Every member is in exactly one module job, and every selected package is a member.
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (job, module) in &modules {
        for package in &module.packages {
            owners
                .entry(package.as_str())
                .or_default()
                .push(job.as_str());
        }
    }
    for package in members.keys() {
        match owners.get(package.as_str()).map(Vec::as_slice) {
            None | Some([]) => {
                violations.push(format!("workspace member `{package}` is in no module job"));
            }
            Some([_]) => {}
            Some(owning) => violations.push(format!(
                "workspace member `{package}` is in more than one module job: {}",
                owning.join(", ")
            )),
        }
    }
    for (package, owning) in &owners {
        if !members.contains_key(*package) {
            violations.push(format!(
                "job(s) {} select `{package}`, which is no workspace member",
                owning.join(", ")
            ));
        }
    }

    // `gate` runs whatever its needs end in, and needs every job but itself and `flaky`.
    let gate = jobs.get(GATE_JOB);
    match gate.and_then(|lines| single(lines, "if")) {
        Some(condition) if condition.contains("always()") && !condition.contains("!always()") => {}
        other => violations.push(format!(
            "`gate` must run after any result of its needs (`if:` with always()), found {other:?}"
        )),
    }
    if let Some(value) = gate.and_then(|lines| single(lines, "needs")) {
        match needs_list(value) {
            Ok(needs) => {
                let listed: BTreeSet<&str> = needs.iter().map(String::as_str).collect();
                if listed.len() != needs.len() {
                    violations.push(format!("`gate` needs a job twice: {value}"));
                }
                let expected: BTreeSet<&str> = jobs
                    .keys()
                    .map(String::as_str)
                    .filter(|job| *job != GATE_JOB && *job != FLAKY_JOB)
                    .collect();
                for missing in expected.difference(&listed) {
                    violations.push(format!("`gate` does not need job `{missing}`"));
                }
                for extra in listed.difference(&expected) {
                    violations.push(format!(
                        "`gate` needs `{extra}`, which is no gating job of {CI_WORKFLOW}"
                    ));
                }
            }
            Err(error) => violations.push(format!("`gate`: {error}")),
        }
    }

    // The target-kind job runs `--bins` and `--test '*'`: a lib target would run nowhere.
    if let Some(module) = modules.get(TARGET_KIND_JOB) {
        for package in &module.packages {
            if let Some(member) = members.get(package)
                && let Some(lib) = lib_root(&root.join(member))
            {
                violations.push(format!(
                    "`{package}` in job `{TARGET_KIND_JOB}` has a lib target ({}), which its \
                     `--bins` and `--test '*'` calls do not run: add `--lib` and `--doc` there",
                    lib.display()
                ));
            }
        }
    }

    // Every skip path matches exactly one test of its job.
    let mut tests: BTreeMap<String, Result<Vec<TestCase>, String>> = BTreeMap::new();
    for (job, module) in &modules {
        if module.skips.is_empty() {
            continue;
        }
        let mut cases = BTreeSet::new();
        for package in &module.packages {
            match tests_of(&mut tests, &root, &members, package) {
                Ok(found) => cases.extend(
                    found
                        .into_iter()
                        .map(|case| (format!("{package}::{}", case.target), case.path)),
                ),
                Err(error) => violations.push(format!("job `{job}`: {error}")),
            }
        }
        for skip in &module.skips {
            let matched: Vec<String> = cases
                .iter()
                .filter(|(_, path)| path.contains(skip.as_str()))
                .map(|(target, path)| format!("{target}::{path}"))
                .collect();
            if matched.len() != 1 {
                violations.push(format!(
                    "job `{job}`: `--skip {skip}` matches {} tests, not exactly one: {matched:?}",
                    matched.len()
                ));
            }
        }
    }

    // The flaky-test mirror: real tests, skipped by their module job, run by `flaky` in every
    // configuration of that job.
    let mut expected_skips: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut expected_flaky = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for entry in flaky_mirror(&root) {
        let Some(table) = entry.as_table() else {
            violations.push(format!("{FLAKY_MIRROR}: an entry is not a table: {entry}"));
            continue;
        };
        let keys: BTreeSet<&str> = table.keys().map(String::as_str).collect();
        let wanted: BTreeSet<&str> = MIRROR_KEYS.iter().copied().collect();
        if keys != wanted {
            violations.push(format!(
                "{FLAKY_MIRROR}: entry keys {keys:?} are not exactly {wanted:?}"
            ));
            continue;
        }
        let field = |key: &str| table.get(key).and_then(toml::Value::as_str);
        let features: Option<Vec<String>> = table
            .get("features")
            .and_then(toml::Value::as_array)
            .and_then(|values| {
                values
                    .iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect()
            });
        let (
            Some(id),
            Some(name),
            Some(package),
            Some(job),
            Some(features),
            Some(register),
            Some(status),
        ) = (
            field("id"),
            field("name"),
            field("crate"),
            field("job"),
            features,
            field("register"),
            field("status"),
        )
        else {
            violations.push(format!(
                "{FLAKY_MIRROR}: `features` is a list of strings and every other key a string: \
                 {entry}"
            ));
            continue;
        };
        if !ids.insert(id.to_owned()) {
            violations.push(format!("{FLAKY_MIRROR}: `{id}` appears twice"));
        }
        if !id
            .strip_prefix("FLK-")
            .is_some_and(|number| number.len() == 2 && number.bytes().all(|b| b.is_ascii_digit()))
        {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` is not a register number FLK-NN"
            ));
        }
        if !register
            .strip_prefix(FLAKY_REGISTER)
            .is_some_and(|comment| {
                !comment.is_empty() && comment.bytes().all(|b| b.is_ascii_digit())
            })
        {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` register `{register}` is no comment of board issue #71"
            ));
        }
        if status.trim().is_empty() {
            violations.push(format!("{FLAKY_MIRROR}: `{id}` has no status"));
        }
        let Some((name_package, target, path)) = name.split_once("::").and_then(|(owner, rest)| {
            rest.split_once("::")
                .map(|(target, path)| (owner, target, path))
        }) else {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` name `{name}` is not <package>::<target>::<libtest path>"
            ));
            continue;
        };
        if name_package != package {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` name `{name}` belongs to `{name_package}`, not crate `{package}`"
            ));
        }
        if !members.contains_key(package) {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` crate `{package}` is no workspace member"
            ));
            continue;
        }
        match tests_of(&mut tests, &root, &members, package) {
            Ok(found) => match found
                .iter()
                .find(|case| case.target == target && case.path == path)
            {
                Some(case) if case.ignored => {
                    violations.push(format!("{FLAKY_MIRROR}: `{id}` test `{name}` is #[ignore]"))
                }
                Some(_) => {}
                None => {
                    violations.push(format!("{FLAKY_MIRROR}: `{id}` names no #[test] `{name}`"))
                }
            },
            Err(error) => violations.push(format!("{FLAKY_MIRROR}: `{id}`: {error}")),
        }
        let configurations: BTreeSet<&str> = features.iter().map(String::as_str).collect();
        if configurations.len() != features.len() {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` lists a configuration twice: {features:?}"
            ));
        }
        for configuration in &configurations {
            for feature in configuration
                .split(',')
                .map(str::trim)
                .filter(|f| !f.is_empty())
            {
                if !declares_feature(&root, &members, package, feature) {
                    violations.push(format!(
                        "{FLAKY_MIRROR}: `{id}` feature `{feature}` is declared by no package it names"
                    ));
                }
            }
        }
        match modules.get(job) {
            Some(module) if module.packages.iter().any(|selected| selected == package) => {
                let mut runs = BTreeSet::from([module.features.as_str()]);
                if let Some((extra_features, filter)) = &module.extra
                    && target == "lib"
                    && filter.iter().any(|token| path.contains(token.as_str()))
                {
                    runs.insert(extra_features.as_str());
                }
                if configurations != runs {
                    violations.push(format!(
                        "{FLAKY_MIRROR}: `{id}` lists configurations {configurations:?}, but job \
                         `{job}` runs the test in {runs:?}"
                    ));
                }
            }
            _ => violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` job `{job}` is no module job that selects `{package}`"
            )),
        }
        expected_skips
            .entry(job.to_owned())
            .or_default()
            .insert(path.to_owned());
        for configuration in &configurations {
            expected_flaky.insert(if configuration.is_empty() {
                name.to_owned()
            } else {
                format!("{name}@{configuration}")
            });
        }
    }
    for (job, module) in &modules {
        let listed: BTreeSet<String> = module.skips.iter().cloned().collect();
        if listed.len() != module.skips.len() {
            violations.push(format!("job `{job}` skips a test twice"));
        }
        let expected = expected_skips.remove(job).unwrap_or_default();
        for missing in expected.difference(&listed) {
            violations.push(format!(
                "job `{job}` does not skip `{missing}`, which {FLAKY_MIRROR} registers for it"
            ));
        }
        for extra in listed.difference(&expected) {
            violations.push(format!(
                "job `{job}` skips `{extra}`, which {FLAKY_MIRROR} does not register for it"
            ));
        }
    }
    for (job, paths) in &expected_skips {
        violations.push(format!(
            "{FLAKY_MIRROR} registers {paths:?} for `{job}`, which is no module job"
        ));
    }
    if let Some(value) = jobs
        .get(FLAKY_JOB)
        .and_then(|lines| single(lines, "FLAKY_TESTS"))
    {
        let selected: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
        let listed: BTreeSet<String> = selected.iter().cloned().collect();
        if listed.len() != selected.len() {
            violations.push("`flaky` runs a test twice".to_owned());
        }
        for missing in expected_flaky.difference(&listed) {
            violations.push(format!(
                "`flaky` does not run `{missing}`, which {FLAKY_MIRROR} registers"
            ));
        }
        for extra in listed.difference(&expected_flaky) {
            violations.push(format!(
                "`flaky` runs `{extra}`, which {FLAKY_MIRROR} does not register"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "CI job coverage violations (test plan v2.2 §2.1 guard 1):\n{}",
        violations.join("\n")
    );
}

fn presence(job: &str, key: &str) -> Presence {
    let module = !matches!(job, LINT_JOB | GATE_JOB | FLAKY_JOB);
    match key {
        "needs" | "if" if job == GATE_JOB => Presence::Required,
        "if" => Presence::Optional,
        "FLAKY_TESTS" if job == FLAKY_JOB => Presence::Required,
        "CI_PACKAGES" | "CI_SKIP" if module => Presence::Required,
        "CI_FEATURES" | "CI_EXTRA_FEATURES" | "CI_EXTRA_FILTER" if module => Presence::Optional,
        _ => Presence::Forbidden,
    }
}

fn single<'a>(lines: &'a BTreeMap<String, Vec<String>>, key: &str) -> Option<&'a str> {
    match lines.get(key).map(Vec::as_slice) {
        Some([value]) => Some(value.as_str()),
        _ => None,
    }
}

fn module_job(lines: &BTreeMap<String, Vec<String>>) -> Result<ModuleJob, String> {
    let packages =
        selected_packages(single(lines, "CI_PACKAGES").ok_or("no single `CI_PACKAGES:` line")?)?;
    let skips = single(lines, "CI_SKIP")
        .ok_or("no single `CI_SKIP:` line")?
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let features = match single(lines, "CI_FEATURES") {
        None => String::new(),
        Some(value) => match value.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["--features", features] => (*features).to_owned(),
            _ => {
                return Err(format!(
                    "`CI_FEATURES:` is not `--features <features>`: {value}"
                ));
            }
        },
    };
    let extra = match (
        single(lines, "CI_EXTRA_FEATURES").map(str::trim),
        single(lines, "CI_EXTRA_FILTER"),
    ) {
        (None, None) => None,
        (Some(extra_features), Some(filter)) => {
            let filter: Vec<String> = filter.split_whitespace().map(str::to_owned).collect();
            if extra_features.is_empty() || filter.is_empty() {
                return Err(
                    "`CI_EXTRA_FEATURES:` and `CI_EXTRA_FILTER:` are both non-empty".to_owned(),
                );
            }
            if !features.is_empty() {
                return Err("`CI_EXTRA_FEATURES:` runs without the job's `CI_FEATURES:`; a job declares one of them".to_owned());
            }
            Some((extra_features.to_owned(), filter))
        }
        _ => {
            return Err("`CI_EXTRA_FEATURES:` and `CI_EXTRA_FILTER:` come together".to_owned());
        }
    };
    Ok(ModuleJob {
        packages,
        skips,
        features,
        extra,
    })
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("architecture tool must live at tools/<name>")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn parse_toml(path: &Path) -> toml::Value {
    toml::from_str::<toml::Value>(&read(path))
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

/// Package name → member directory (relative to the workspace root), from the root
/// `Cargo.toml` and each member's own manifest.
fn workspace_members(root: &Path) -> BTreeMap<String, PathBuf> {
    let manifest = parse_toml(&root.join("Cargo.toml"));
    let declared = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .expect("root Cargo.toml declares workspace.members");
    let mut members = BTreeMap::new();
    for member in declared {
        let member = member.as_str().expect("workspace member is a path");
        assert!(
            !member.contains(['*', '?', '[']),
            "workspace member `{member}` is a glob; this guard reads explicit members only"
        );
        let package = parse_toml(&root.join(member).join("Cargo.toml"));
        let name = package
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("{member}/Cargo.toml declares package.name"));
        assert!(
            members
                .insert(name.to_owned(), PathBuf::from(member))
                .is_none(),
            "two workspace members are named `{name}`"
        );
    }
    assert!(!members.is_empty(), "the workspace has no members");
    members
}

/// Reads the job ids under `jobs:` (two-space indent) and, inside each job, the job-level keys
/// (four spaces) and the job `env:` keys (six spaces).
fn ci_jobs(workflow: &str) -> CiJobs {
    let mut jobs = CiJobs::new();
    let mut in_jobs = false;
    let mut current: Option<String> = None;
    for line in workflow.lines() {
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let indent = line.len() - content.len();
        if indent == 0 {
            in_jobs = line.trim_end() == "jobs:";
            current = None;
            continue;
        }
        if !in_jobs {
            continue;
        }
        if indent == 2 {
            let id = content
                .trim_end()
                .strip_suffix(':')
                .unwrap_or_else(|| panic!("{CI_WORKFLOW}: a job header is not `<id>:`: {line}"));
            assert!(
                jobs.insert(id.to_owned(), BTreeMap::new()).is_none(),
                "{CI_WORKFLOW}: job `{id}` appears twice"
            );
            current = Some(id.to_owned());
            continue;
        }
        let Some(job) = current.as_ref() else {
            panic!("{CI_WORKFLOW}: a line under `jobs:` belongs to no job: {line}");
        };
        let keys = match indent {
            4 => JOB_KEYS,
            6 => JOB_ENV_KEYS,
            _ => continue,
        };
        if let Some((key, value)) = content.split_once(':')
            && keys.contains(&key)
        {
            jobs.get_mut(job)
                .expect("the current job was inserted")
                .entry(key.to_owned())
                .or_default()
                .push(unquote(value.trim()).to_owned());
        }
    }
    assert!(!jobs.is_empty(), "{CI_WORKFLOW} declares no jobs");
    jobs
}

fn unquote(value: &str) -> &str {
    for quote in ['\'', '"'] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// `-p <package> -p <package> …`
fn selected_packages(value: &str) -> Result<Vec<String>, String> {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("`CI_PACKAGES:` selects no package".to_owned());
    }
    tokens
        .chunks(2)
        .map(|pair| match pair {
            ["-p", package] => Ok((*package).to_owned()),
            _ => Err(format!(
                "`CI_PACKAGES:` is not `-p <package> …` at {pair:?}"
            )),
        })
        .collect()
}

/// `[<job>, <job>, …]`
fn needs_list(value: &str) -> Result<Vec<String>, String> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| format!("`needs:` is not a one-line list `[<job>, …]`: {value}"))?;
    let needs: Vec<String> = inner
        .split(',')
        .map(str::trim)
        .filter(|job| !job.is_empty())
        .map(str::to_owned)
        .collect();
    if needs.is_empty() {
        return Err("`needs:` lists no job".to_owned());
    }
    Ok(needs)
}

fn flaky_mirror(root: &Path) -> Vec<toml::Value> {
    let mirror = parse_toml(&root.join(FLAKY_MIRROR));
    let table = mirror
        .as_table()
        .unwrap_or_else(|| panic!("{FLAKY_MIRROR} is a table"));
    let keys: Vec<&str> = table.keys().map(String::as_str).collect();
    assert!(
        keys.iter().all(|key| *key == "test"),
        "{FLAKY_MIRROR} holds only `[[test]]` entries, found keys {keys:?}"
    );
    table
        .get("test")
        .map(|entries| {
            entries
                .as_array()
                .unwrap_or_else(|| panic!("{FLAKY_MIRROR}: `test` is an array of tables"))
                .clone()
        })
        .unwrap_or_default()
}

/// `feature` of `package`, or `<member>/<feature>` of another workspace member.
fn declares_feature(
    root: &Path,
    members: &BTreeMap<String, PathBuf>,
    package: &str,
    feature: &str,
) -> bool {
    let (owner, feature) = feature.split_once('/').unwrap_or((package, feature));
    members.get(owner).is_some_and(|member| {
        parse_toml(&root.join(member).join("Cargo.toml"))
            .get("features")
            .and_then(toml::Value::as_table)
            .is_some_and(|features| features.contains_key(feature))
    })
}

fn tests_of(
    cache: &mut BTreeMap<String, Result<Vec<TestCase>, String>>,
    root: &Path,
    members: &BTreeMap<String, PathBuf>,
    package: &str,
) -> Result<Vec<TestCase>, String> {
    cache
        .entry(package.to_owned())
        .or_insert_with(|| match members.get(package) {
            Some(member) => package_tests(&root.join(member))
                .map_err(|error| format!("tests of `{package}`: {error}")),
            None => Err(format!("`{package}` is no workspace member")),
        })
        .clone()
}

/// Every `#[test]` function of every lib, bin and integration-test target of a package.
fn package_tests(member: &Path) -> Result<Vec<TestCase>, String> {
    let mut cases = Vec::new();
    for (target, root_file) in target_roots(member)? {
        let items = parse_items(&root_file)?;
        let mut found = Vec::new();
        collect_tests(
            &items,
            &root_file,
            &parent(&root_file)?,
            false,
            "",
            &mut found,
        )?;
        cases.extend(found.into_iter().map(|(path, ignored)| TestCase {
            target: target.clone(),
            path,
            ignored,
        }));
    }
    Ok(cases)
}

fn lib_root(member: &Path) -> Option<PathBuf> {
    let manifest = parse_toml(&member.join("Cargo.toml"));
    if let Some(path) = manifest
        .get("lib")
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
    {
        return Some(member.join(path));
    }
    let default = member.join("src").join("lib.rs");
    default.is_file().then_some(default)
}

/// Target label → crate root file: the lib, every bin (`bin:<name>`) and every integration test,
/// explicit in the manifest or discovered the way cargo does.
fn target_roots(member: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let manifest = parse_toml(&member.join("Cargo.toml"));
    let package = manifest.get("package");
    let package_name = package
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("{} declares no package.name", member.display()))?;
    let discovers = |key: &str| {
        package
            .and_then(|package| package.get(key))
            .and_then(toml::Value::as_bool)
            != Some(false)
    };
    let mut roots = Vec::new();
    if let Some(lib) = lib_root(member) {
        roots.push(("lib".to_owned(), lib));
    }

    let mut bins: BTreeMap<PathBuf, String> = BTreeMap::new();
    for bin in manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = bin
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("{}: a [[bin]] has no name", member.display()))?;
        let file = match bin.get("path").and_then(toml::Value::as_str) {
            Some(path) => member.join(path),
            None if name == package_name => member.join("src").join("main.rs"),
            None => member.join("src").join("bin").join(format!("{name}.rs")),
        };
        bins.insert(file, name.to_owned());
    }
    if discovers("autobins") {
        let main = member.join("src").join("main.rs");
        if main.is_file() {
            bins.entry(main).or_insert_with(|| package_name.to_owned());
        }
        for (name, file) in discovered(&member.join("src").join("bin"))? {
            bins.entry(file).or_insert(name);
        }
    }
    roots.extend(
        bins.into_iter()
            .map(|(file, name)| (format!("bin:{name}"), file)),
    );

    let mut tests: BTreeMap<PathBuf, String> = BTreeMap::new();
    for test in manifest
        .get("test")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = test
            .get("name")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("{}: a [[test]] has no name", member.display()))?;
        let file = match test.get("path").and_then(toml::Value::as_str) {
            Some(path) => member.join(path),
            None => member.join("tests").join(format!("{name}.rs")),
        };
        tests.insert(file, name.to_owned());
    }
    if discovers("autotests") {
        for (name, file) in discovered(&member.join("tests"))? {
            tests.entry(file).or_insert(name);
        }
    }
    roots.extend(tests.into_iter().map(|(file, name)| (name, file)));

    for (target, file) in &roots {
        if !file.is_file() {
            return Err(format!(
                "target `{target}` has no root file {}",
                file.display()
            ));
        }
    }
    Ok(roots)
}

/// cargo's target discovery in one directory: `<name>.rs` and `<name>/main.rs`.
fn discovered(directory: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    let mut targets = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("read {}: {error}", directory.display()))?
            .path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            targets.push((stem.to_owned(), path.clone()));
        } else if path.is_dir() && path.join("main.rs").is_file() {
            targets.push((stem.to_owned(), path.join("main.rs")));
        }
    }
    targets.sort();
    Ok(targets)
}

/// Collects `(libtest path, ignored)` of every `#[test]` function in `items` and the modules
/// they declare. `directory` holds the files of child modules; `inline` says whether `items`
/// sit in an inline `mod { … }`, which changes what a `#[path]` is relative to.
fn collect_tests(
    items: &[syn::Item],
    file: &Path,
    directory: &Path,
    inline: bool,
    prefix: &str,
    found: &mut Vec<(String, bool)>,
) -> Result<(), String> {
    for item in items {
        match item {
            syn::Item::Fn(function) if has_attribute(&function.attrs, "test") => {
                found.push((
                    qualified(prefix, &function.sig.ident.to_string()),
                    has_attribute(&function.attrs, "ignore"),
                ));
            }
            syn::Item::Mod(module) => {
                let name = module.ident.to_string();
                let path = qualified(prefix, &name);
                let declared = path_attribute(&module.attrs);
                if let Some((_, content)) = &module.content {
                    if declared.is_some() {
                        return Err(format!(
                            "{}: `#[path]` on the inline `mod {name}` is not resolved by this guard",
                            file.display()
                        ));
                    }
                    let content = expand_includes(content.clone(), file)?;
                    collect_tests(&content, file, &directory.join(&name), true, &path, found)?;
                    continue;
                }
                let next = match &declared {
                    Some(relative) if inline => directory.join(relative),
                    Some(relative) => parent(file)?.join(relative),
                    None => {
                        let flat = directory.join(format!("{name}.rs"));
                        if flat.is_file() {
                            flat
                        } else {
                            directory.join(&name).join("mod.rs")
                        }
                    }
                };
                if !next.is_file() {
                    return Err(format!(
                        "{}: `mod {name}` has no file {}",
                        file.display(),
                        next.display()
                    ));
                }
                let child_items = parse_items(&next)?;
                // Child modules of a `mod.rs` or a `#[path]` file sit in its directory.
                let child_directory = if declared.is_some() || next.ends_with("mod.rs") {
                    parent(&next)?
                } else {
                    parent(&next)?.join(&name)
                };
                collect_tests(&child_items, &next, &child_directory, false, &path, found)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn qualified(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}::{name}")
    }
}

fn parent(file: &Path) -> Result<PathBuf, String> {
    file.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", file.display()))
}

fn parse_items(file: &Path) -> Result<Vec<syn::Item>, String> {
    let source =
        fs::read_to_string(file).map_err(|error| format!("read {}: {error}", file.display()))?;
    let parsed =
        syn::parse_file(&source).map_err(|error| format!("parse {}: {error}", file.display()))?;
    expand_includes(parsed.items, file)
}

/// Replaces each item-level `include!("<file>")` with the items of that file, which is relative
/// to the file that holds the macro.
fn expand_includes(items: Vec<syn::Item>, file: &Path) -> Result<Vec<syn::Item>, String> {
    let mut expanded = Vec::with_capacity(items.len());
    for item in items {
        match item {
            syn::Item::Macro(included) if included.mac.path.is_ident("include") => {
                let relative = included
                    .mac
                    .parse_body::<syn::LitStr>()
                    .map_err(|error| format!("{}: include!: {error}", file.display()))?
                    .value();
                expanded.extend(parse_items(&parent(file)?.join(relative))?);
            }
            other => expanded.push(other),
        }
    }
    Ok(expanded)
}

fn path_attribute(attributes: &[syn::Attribute]) -> Option<String> {
    attributes.iter().find_map(|attribute| {
        if !attribute.path().is_ident("path") {
            return None;
        }
        match &attribute.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(value),
                        ..
                    }),
                ..
            }) => Some(value.value()),
            _ => None,
        }
    })
}

fn has_attribute(attributes: &[syn::Attribute], name: &str) -> bool {
    attributes.iter().any(|attribute| {
        attribute
            .path()
            .segments
            .last()
            .is_some_and(|segment| segment.ident == name)
    })
}
