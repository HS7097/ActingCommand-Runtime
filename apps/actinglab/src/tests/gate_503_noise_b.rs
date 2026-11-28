// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 NOISE-B (0.12.102; register §1.553; contracts/resource-declarations.md, the rows
// `goals.json`, `goal-tables.json` and `pack-catalog.json`): `resource validate` reads the three
// root files of a resource repository by their readers, for the game of the root's `pages.json`:
// each well formed is valid in its family (the catalog of either version, by the contract
// crate's check of the file alone, its package ids and pools judged by no registry and no
// schedule); each its reader refuses is `resource_declaration_invalid` with the first issue's
// `standard_pack_problem` and `json_pointer` and every issue in the message; a root without
// `pages.json` is refused naming it; a Git deletion of each is a removal; the same names outside
// the root are still unmapped; and `resource verify`'s declarations step takes a diff that adds
// and then removes `goals.json`. The repositories are neutral fixtures (game `neutral`) in a
// temporary directory; the verification's is made with git, every git call with no system
// configuration and an empty global one. No clock is read and nothing sleeps.

use super::*;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const PAGES: &str = "pages.json";
const GOALS: &str = "goals.json";
const TABLES: &str = "goal-tables.json";
const CATALOG: &str = "pack-catalog.json";
const INVALID: &str = "resource_declaration_invalid";

/// The issues of one refused file, each its `standard_pack_problem` and `json_pointer`, the first
/// first.
type Issues = &'static [(&'static str, &'static str)];

/// The root's page list of the game `neutral`.
fn pages() -> Value {
    json!({
        "schema_version": "actingcommand.pages.v1",
        "game": "neutral",
        "features": ["neutral.feature"],
        "pages": [{"id": "home", "kind": "page"}]
    })
}

/// A `goals.json` of version 3 for `game`: a heading and a note, which name nothing of the root.
fn goals(game: &str) -> Value {
    json!({
        "schema_version": "actingcommand.goals.v3",
        "game": game,
        "entries": [
            {"name": "stock", "type": "heading", "title": {"en": "Stock"}},
            {"name": "about", "type": "note", "title": {"en": "About"},
             "help": {"en": "Neutral text."}}
        ]
    })
}

/// A `goal-tables.json` of the game `neutral` whose one key names `class`: its items name no pool,
/// so with `common` every rule of the file holds.
fn tables(class: &str) -> Value {
    json!({
        "schema_version": "actingcommand.goal-tables.v1",
        "game": "neutral",
        "items": {"coin": {}, "book": {}},
        "tables": [{
            "name": "heroes",
            "progress": [{"fact_key": "inventory.list.neutral_heroes", "key_column": "hero",
                          "held_by": true, "columns": ["grade"]}],
            "columns": [{"name": "grade", "read": ["grade"], "per": "class"}],
            "classes": {"common": {"grade": {"positions": [[0], [1], [2]],
                                              "steps": [{"coin": 5}, {"coin": 9, "book": 2}]}}},
            "keys": {"hero_a": {"class": class}}
        }]
    })
}

/// A `pack-catalog.json` of version 1 whose one entry names a package no registry here holds and
/// a pool no `schedule.json` here declares: what the file alone says holds.
fn catalog_v1() -> Value {
    json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": "neutral",
        "packs": [{
            "package_id": "neutral.unlisted",
            "consumes": [],
            "produces": [{"pool_id": "pool.neutral.points",
                          "quantity": {"kind": "constant", "amount": 3}}]
        }]
    })
}

/// A `pack-catalog.json` of version 2: one entry with its own part and a fixed value.
fn catalog_v2() -> Value {
    json!({
        "schema_version": "actingcommand.pack-catalog.v2",
        "game": "neutral",
        "packs": [{"package_id": "neutral.unlisted", "consumes": [], "produces": [],
                   "value": {"milli": 1000}}]
    })
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("the parent directory");
    }
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).expect("JSON")),
    )
    .expect("write the fixture");
}

/// `actinglab --json resource validate --repo <repo>` with `--changed-path` for each of `paths`.
fn validate(repo: &Path, paths: &[&str]) -> CliResult {
    let mut arguments = vec![
        "--json".to_owned(),
        "resource".to_owned(),
        "validate".to_owned(),
        "--repo".to_owned(),
        repo.display().to_string(),
    ];
    for path in paths {
        arguments.extend(["--changed-path".to_owned(), (*path).to_owned()]);
    }
    run_cli(arguments, true)
}

/// The refusal of `result`: its exit status is 2 and its code `resource_declaration_invalid`.
fn refusal(result: &CliResult) -> (&str, Value) {
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().expect("a refusal");
    assert_eq!(error.code, INVALID, "{}", result.envelope_json());
    (
        error.message.as_str(),
        error.details.clone().expect("its details"),
    )
}

// Test 1: with the root's `pages.json`, each of the three files well formed is valid in its
// family, with no warning, the page list read beside them; the catalog of version 1 names a
// package and a pool nothing here declares, which the file alone does not judge, and one of
// version 2 is valid too.
#[test]
fn noise_b_each_root_file_well_formed_is_valid_in_its_family() {
    let temp = TempDir::new().expect("a temporary directory");
    write_json(&temp.path().join(PAGES), &pages());
    write_json(&temp.path().join(GOALS), &goals("neutral"));
    write_json(&temp.path().join(TABLES), &tables("common"));
    write_json(&temp.path().join(CATALOG), &catalog_v1());

    let result = validate(temp.path(), &[GOALS, TABLES, CATALOG]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the validation");
    assert_eq!(data["status"], "valid");
    assert_eq!(
        data["entries"],
        json!([
            {"path": TABLES, "status": "valid", "family": "standard_pack_goal_tables"},
            {"path": GOALS, "status": "valid", "family": "standard_pack_goals"},
            {"path": CATALOG, "status": "valid", "family": "standard_pack_catalog"}
        ])
    );
    assert_eq!(data["warnings"], json!([]));
    let files_read = data["read_files"].as_array().expect("the files read");
    assert!(files_read.contains(&json!(PAGES)), "{data}");

    write_json(&temp.path().join(CATALOG), &catalog_v2());
    let result = validate(temp.path(), &[CATALOG]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the validation");
    assert_eq!(
        data["entries"],
        json!([{"path": CATALOG, "status": "valid", "family": "standard_pack_catalog"}])
    );
}

// Test 2: each of the three files its reader refuses is `resource_declaration_invalid` naming
// the file, with the first issue's `standard_pack_problem` and `json_pointer` in the details and
// every issue in the message: `goals.json` of another game with a name twice (`game_mismatch` at
// `/game`, then `goal_name_duplicate` at `/entries/1/name`), `goal-tables.json` whose key names a
// class the table does not have (`table_class_unknown`), and a catalog of version 2 with a value
// beside the realization mark and a package listed twice (`catalog_value_conflict` at
// `/packs/0/value`, then `catalog_duplicate` at `/packs/1/package_id`), both of the file alone.
#[test]
fn noise_b_each_root_file_its_reader_refuses_is_refused_with_its_problem_and_pointer() {
    let temp = TempDir::new().expect("a temporary directory");
    write_json(&temp.path().join(PAGES), &pages());
    let mut twice = goals("other");
    twice["entries"][1] = json!({"name": "stock", "type": "heading", "title": {"en": "Again"}});
    write_json(&temp.path().join(GOALS), &twice);
    write_json(&temp.path().join(TABLES), &tables("rare"));
    write_json(
        &temp.path().join(CATALOG),
        &json!({
            "schema_version": "actingcommand.pack-catalog.v2",
            "game": "neutral",
            "packs": [
                {"package_id": "neutral.unlisted", "consumes": [], "produces": [],
                 "value": {"milli": 1000}, "realization": true},
                {"package_id": "neutral.unlisted", "consumes": [], "produces": []}
            ]
        }),
    );
    let rows: [(&str, Issues); 3] = [
        (
            GOALS,
            &[
                ("game_mismatch", "/game"),
                ("goal_name_duplicate", "/entries/1/name"),
            ],
        ),
        (
            TABLES,
            &[("table_class_unknown", "/tables/0/keys/hero_a/class")],
        ),
        (
            CATALOG,
            &[
                ("catalog_value_conflict", "/packs/0/value"),
                ("catalog_duplicate", "/packs/1/package_id"),
            ],
        ),
    ];
    for (file, issues) in rows {
        let result = validate(temp.path(), &[file]);
        let (message, details) = refusal(&result);
        assert_eq!(details["declaration_file"], file, "{details}");
        assert_eq!(details["standard_pack_problem"], issues[0].0, "{details}");
        assert_eq!(details["json_pointer"], issues[0].1, "{details}");
        for (problem, pointer) in issues {
            let issue = format!("standard_pack_problem={problem} json_pointer={pointer}");
            assert!(message.contains(&issue), "{file}: {message}");
        }
    }
}

// Test 3: a root without `pages.json` refuses each of the three files, well formed as they are,
// naming `pages.json`, as a root `interrupts.json` is refused without it.
#[test]
fn noise_b_a_root_without_its_page_list_refuses_each_file_naming_the_page_list() {
    let temp = TempDir::new().expect("a temporary directory");
    write_json(&temp.path().join(GOALS), &goals("neutral"));
    write_json(&temp.path().join(TABLES), &tables("common"));
    write_json(&temp.path().join(CATALOG), &catalog_v1());
    for file in [GOALS, TABLES, CATALOG] {
        let result = validate(temp.path(), &[file]);
        let (message, details) = refusal(&result);
        assert_eq!(details["declaration_file"], PAGES, "{file}: {details}");
        assert!(message.starts_with("pages.json: "), "{file}: {message}");
    }
}

// Test 4: a Git deletion of each of the three files is a removal with the local reference scope
// and no declaration checked, on a root that holds nothing else, so needs no other document.
#[test]
fn noise_b_a_git_deletion_of_each_root_file_is_a_removal() {
    let temp = TempDir::new().expect("a temporary directory");
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).expect("the repository");
    let list = temp.path().join("changes.bin");
    fs::write(
        &list,
        b"D\0goals.json\0D\0goal-tables.json\0D\0pack-catalog.json\0",
    )
    .expect("the Git list");
    let result = run_cli(
        [
            "--json".to_owned(),
            "resource".to_owned(),
            "validate".to_owned(),
            "--repo".to_owned(),
            repo.display().to_string(),
            "--changed-paths-file".to_owned(),
            list.display().to_string(),
        ],
        true,
    );
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the validation");
    let removed = |path: &str| {
        json!({"path": path, "status": "removed",
               "reference_scope": "current_local_declaration_dependencies",
               "checked_declarations": []})
    };
    assert_eq!(
        data["entries"],
        json!([removed(TABLES), removed(GOALS), removed(CATALOG)])
    );
}

// Test 5: the same three names anywhere but at the root are still a family no parser is mapped
// to, refused naming the file.
#[test]
fn noise_b_the_same_names_outside_the_root_are_still_unmapped() {
    let temp = TempDir::new().expect("a temporary directory");
    write_json(&temp.path().join(PAGES), &pages());
    for (file, value) in [
        ("drafts/goals.json", goals("neutral")),
        ("drafts/goal-tables.json", tables("common")),
        ("drafts/pack-catalog.json", catalog_v1()),
    ] {
        write_json(&temp.path().join(file), &value);
        let result = validate(temp.path(), &[file]);
        let (message, details) = refusal(&result);
        assert_eq!(details["declaration_file"], file, "{details}");
        assert!(
            message.contains("program declaration family has no mapped parser"),
            "{file}: {message}"
        );
    }
}

/// The process's git configuration for the life of a fixture: no system file and an empty global
/// file; the previous values come back when it drops.
struct GitConfiguration {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl GitConfiguration {
    fn isolate(global: &Path) -> Self {
        let previous = ["GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL"]
            .into_iter()
            .map(|name| (name, env::var_os(name)))
            .collect();
        unsafe {
            env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            env::set_var("GIT_CONFIG_GLOBAL", global);
        }
        Self { previous }
    }
}

impl Drop for GitConfiguration {
    fn drop(&mut self) {
        for (name, value) in &self.previous {
            unsafe {
                match value {
                    Some(value) => env::set_var(name, value),
                    None => env::remove_var(name),
                }
            }
        }
    }
}

/// A neutral resource repository `<temp>/repo` holding its page list, the outputs of its runs
/// beside it. The fields drop in order, the environment lock last.
struct Repository {
    temp: TempDir,
    _git: GitConfiguration,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Repository {
    fn new() -> Self {
        let guard = env_lock();
        let temp = TempDir::new().expect("a temporary directory");
        let global = temp.path().join("empty.gitconfig");
        fs::write(&global, b"").expect("the empty global configuration");
        let git = GitConfiguration::isolate(&global);
        let repository = Self {
            temp,
            _git: git,
            _guard: guard,
        };
        fs::create_dir(repository.repo()).expect("the repository");
        repository.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        fs::write(repository.repo().join(".gitattributes"), "* -text\n").expect("attributes");
        write_json(&repository.repo().join(PAGES), &pages());
        repository
    }

    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }

    /// Runs git in the repository with a fixed author, committer and date; returns its stdout.
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(self.repo())
            .args(args)
            .env("GIT_AUTHOR_NAME", "Neutral Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00")
            .env("GIT_COMMITTER_NAME", "Neutral Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00")
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git answers text")
            .trim()
            .to_owned()
    }

    /// Commits the whole work tree; returns the commit.
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// The report of `actinglab --json resource verify` of the run `run` over the diff from
    /// `base`.
    fn verify(&self, run: &str, base: &str) -> Value {
        let report = self.temp.path().join(format!("{run}-report.json"));
        run_cli(
            [
                "--json".to_owned(),
                "resource".to_owned(),
                "verify".to_owned(),
                "--repo".to_owned(),
                self.repo().display().to_string(),
                "--source-repository".to_owned(),
                "neutral-owner/neutral-resources".to_owned(),
                "--work".to_owned(),
                self.temp
                    .path()
                    .join(format!("{run}-work"))
                    .display()
                    .to_string(),
                "--report".to_owned(),
                report.display().to_string(),
                "--base".to_owned(),
                base.to_owned(),
            ],
            true,
        );
        serde_json::from_slice(&fs::read(&report).expect("the report is written"))
            .unwrap_or_else(|error| panic!("the report of {run}: {error}"))
    }
}

/// The step `name` of `report`.
fn step<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["steps"]
        .as_array()
        .expect("the steps")
        .iter()
        .find(|step| step["step"] == name)
        .unwrap_or_else(|| panic!("no step {name}: {report}"))
}

// Test 6: `resource verify` over a diff that adds `goals.json` beside the page list passes its
// declarations step, both files valid; over the next diff, which removes it, the step passes
// with the removal. Until this slice the step refused the added file as unmapped. Only the
// declarations step is judged: the fixture holds no package.
#[test]
fn noise_b_resource_verify_takes_a_diff_that_adds_and_removes_goals_json() {
    let repository = Repository::new();
    let base = repository.commit("the page list");
    write_json(&repository.repo().join(GOALS), &goals("neutral"));
    let added = repository.commit("the goals");

    let report = repository.verify("added", &base);
    let declarations = step(&report, "declarations");
    assert_eq!(declarations["result"], "passed", "{report}");
    assert_eq!(declarations["failures"], json!([]), "{report}");
    assert_eq!(declarations["counts"]["selected"], 2, "{declarations}");
    assert_eq!(declarations["counts"]["deleted"], 0, "{declarations}");
    assert_eq!(declarations["counts"]["valid"], 2, "{declarations}");

    repository.git(&["rm", "-q", GOALS]);
    repository.commit("no goals");
    let report = repository.verify("removed", &added);
    let declarations = step(&report, "declarations");
    assert_eq!(declarations["result"], "passed", "{report}");
    assert_eq!(declarations["failures"], json!([]), "{report}");
    assert_eq!(declarations["counts"]["selected"], 2, "{declarations}");
    assert_eq!(declarations["counts"]["deleted"], 1, "{declarations}");
    assert_eq!(declarations["counts"]["valid"], 1, "{declarations}");
}
