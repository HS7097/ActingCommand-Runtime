// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 §2.1, guard 1 (Batch 0a): the module jobs of `.github/workflows/ci.yml` drop
//! no test silently.
//! - Every workspace member is in exactly one module job's `CI_PACKAGES`.
//! - `gate` (check name `rust`) needs every job except itself and the non-gating `flaky` job.
//! - Every entry of the flaky-test mirror `ci/flaky-tests.toml` (board issue #71, D5) names a
//!   real `#[test]` function; its module job skips it (`CI_SKIP`), the `flaky` job runs it
//!   (`FLAKY_TESTS`), and no job skips a test the mirror does not hold.
//!
//! ci.yml keeps these as one-line forms, so the guard needs no YAML parser.

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
const ONE_LINE_KEYS: &[&str] = &["CI_PACKAGES", "CI_SKIP", "needs", "FLAKY_TESTS"];
const MIRROR_KEYS: &[&str] = &[
    "crate", "features", "id", "job", "name", "register", "status",
];

/// Job id → one-line key → every value the job declares for that key.
type CiJobs = BTreeMap<String, BTreeMap<String, Vec<String>>>;

#[test]
fn gate_ci_jobs_cover_every_member() {
    let root = workspace_root();
    let jobs = ci_jobs(&read(&root.join(CI_WORKFLOW)));
    let members = workspace_members(&root);
    let mut violations = Vec::new();

    for reserved in [LINT_JOB, GATE_JOB, FLAKY_JOB] {
        if !jobs.contains_key(reserved) {
            violations.push(format!("{CI_WORKFLOW} has no `{reserved}` job"));
        }
    }

    // Each job declares exactly the one-line forms of its kind.
    let mut module_packages: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut module_skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (job, lines) in &jobs {
        let expected: &[&str] = match job.as_str() {
            LINT_JOB => &[],
            GATE_JOB => &["needs"],
            FLAKY_JOB => &["FLAKY_TESTS"],
            _ => &["CI_PACKAGES", "CI_SKIP"],
        };
        for key in ONE_LINE_KEYS {
            let count = lines.get(*key).map_or(0, Vec::len);
            let wanted = usize::from(expected.contains(key));
            if count != wanted {
                violations.push(format!(
                    "job `{job}` declares `{key}:` {count} time(s); its kind declares it {wanted} time(s)"
                ));
            }
        }
        if !expected.contains(&"CI_PACKAGES") {
            continue;
        }
        if let Some([value]) = lines.get("CI_PACKAGES").map(Vec::as_slice) {
            match selected_packages(value) {
                Ok(packages) => {
                    module_packages.insert(job.clone(), packages);
                }
                Err(error) => violations.push(format!("job `{job}`: {error}")),
            }
        }
        if let Some([value]) = lines.get("CI_SKIP").map(Vec::as_slice) {
            module_skips.insert(
                job.clone(),
                value.split_whitespace().map(str::to_owned).collect(),
            );
        }
    }

    // Every member is in exactly one module job, and every selected package is a member.
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (job, packages) in &module_packages {
        for package in packages {
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
            Some(jobs) => violations.push(format!(
                "workspace member `{package}` is in more than one module job: {}",
                jobs.join(", ")
            )),
        }
    }
    for (package, jobs) in &owners {
        if !members.contains_key(*package) {
            violations.push(format!(
                "job(s) {} select `{package}`, which is no workspace member",
                jobs.join(", ")
            ));
        }
    }

    // `gate` needs every job but itself and `flaky`.
    if let Some([value]) = jobs
        .get(GATE_JOB)
        .and_then(|lines| lines.get("needs"))
        .map(Vec::as_slice)
    {
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

    // The flaky-test mirror: real tests, skipped by their module job, run by `flaky`.
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
            field("features"),
            field("register"),
            field("status"),
        )
        else {
            violations.push(format!(
                "{FLAKY_MIRROR}: every key of an entry is a string: {entry}"
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
        let Some(member) = members.get(package) else {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` crate `{package}` is no workspace member"
            ));
            continue;
        };
        if !module_packages
            .get(job)
            .is_some_and(|packages| packages.iter().any(|selected| selected == package))
        {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` job `{job}` is no module job that selects `{package}`"
            ));
        }
        for feature in features.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            if !declares_feature(&root.join(member), feature) {
                violations.push(format!(
                    "{FLAKY_MIRROR}: `{id}` feature `{feature}` is not declared by `{package}`"
                ));
            }
        }
        if let Err(error) = resolve_test(&root.join(member), target, path) {
            violations.push(format!(
                "{FLAKY_MIRROR}: `{id}` names no live test `{name}`: {error}"
            ));
        }
        expected_skips
            .entry(job.to_owned())
            .or_default()
            .insert(path.to_owned());
        expected_flaky.insert(if features.is_empty() {
            name.to_owned()
        } else {
            format!("{name}@{features}")
        });
    }
    for (job, skips) in &module_skips {
        let listed: BTreeSet<String> = skips.iter().cloned().collect();
        if listed.len() != skips.len() {
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
            "{FLAKY_MIRROR} registers {paths:?} for `{job}`, which declares no `CI_SKIP:`"
        ));
    }
    if let Some([value]) = jobs
        .get(FLAKY_JOB)
        .and_then(|lines| lines.get("FLAKY_TESTS"))
        .map(Vec::as_slice)
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

/// Reads the job ids under `jobs:` (two-space indent) and, inside each job, the one-line keys.
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
        if let Some((key, value)) = content.split_once(':')
            && ONE_LINE_KEYS.contains(&key)
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

fn declares_feature(member: &Path, feature: &str) -> bool {
    parse_toml(&member.join("Cargo.toml"))
        .get("features")
        .and_then(toml::Value::as_table)
        .is_some_and(|features| features.contains_key(feature))
}

/// Resolves `<libtest path>` of `target` (`lib` or an integration test) to a `#[test]` function
/// without `#[ignore]`, following `mod` declarations, `#[path]` and item-level `include!`.
fn resolve_test(member: &Path, target: &str, path: &str) -> Result<(), String> {
    let mut file = if target == "lib" {
        member.join("src").join("lib.rs")
    } else {
        let single = member.join("tests").join(format!("{target}.rs"));
        let directory = member.join("tests").join(target).join("main.rs");
        if single.is_file() { single } else { directory }
    };
    if !file.is_file() {
        return Err(format!(
            "target `{target}` has no root file {}",
            file.display()
        ));
    }
    let segments: Vec<&str> = path.split("::").collect();
    let Some((function, modules)) = segments.split_last() else {
        return Err("empty libtest path".to_owned());
    };
    let mut items = parse_items(&file)?;
    // Child modules of a crate root, a `mod.rs` or a `#[path]` file sit in its directory.
    let mut directory = parent(&file)?;
    let mut inline = false;
    for module in modules {
        let (content, declared_path) = {
            let declaration = items
                .iter()
                .find_map(|item| match item {
                    syn::Item::Mod(declaration) if declaration.ident == *module => {
                        Some(declaration)
                    }
                    _ => None,
                })
                .ok_or_else(|| format!("{} declares no `mod {module}`", file.display()))?;
            (
                declaration
                    .content
                    .as_ref()
                    .map(|(_, content)| content.clone()),
                path_attribute(&declaration.attrs),
            )
        };
        if let Some(content) = content {
            if declared_path.is_some() {
                return Err(format!(
                    "{}: `#[path]` on the inline `mod {module}` is not resolved by this guard",
                    file.display()
                ));
            }
            items = expand_includes(content, &file)?;
            directory = directory.join(module);
            inline = true;
            continue;
        }
        let next = match &declared_path {
            Some(relative) if inline => directory.join(relative),
            Some(relative) => parent(&file)?.join(relative),
            None => {
                let flat = directory.join(format!("{module}.rs"));
                if flat.is_file() {
                    flat
                } else {
                    directory.join(module).join("mod.rs")
                }
            }
        };
        if !next.is_file() {
            return Err(format!("`mod {module}` has no file {}", next.display()));
        }
        items = parse_items(&next)?;
        directory = if declared_path.is_some() || next.ends_with("mod.rs") {
            parent(&next)?
        } else {
            parent(&next)?.join(module)
        };
        file = next;
        inline = false;
    }
    let functions: Vec<&syn::ItemFn> = items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Fn(function_item) if function_item.sig.ident == *function => {
                Some(function_item)
            }
            _ => None,
        })
        .collect();
    if functions.is_empty() {
        return Err(format!("{} has no `fn {function}`", file.display()));
    }
    for function_item in functions {
        if !has_attribute(&function_item.attrs, "test") {
            return Err(format!(
                "{}: `fn {function}` is not a #[test]",
                file.display()
            ));
        }
        if has_attribute(&function_item.attrs, "ignore") {
            return Err(format!("{}: `fn {function}` is #[ignore]", file.display()));
        }
    }
    Ok(())
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
