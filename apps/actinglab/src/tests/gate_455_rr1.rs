// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR1 (0.12.92; register §1.535; contracts/standard-pack-goal-tables.md, "Who reads
// it"; contracts/component-interfaces.md, the `package` row 28; coordinator work order
// `rd-1123/brief-455-rr1.md`), the Lab: `resource verify` checks a repository's `goal-tables.json`
// with the one check `standard-pack check` and `package bundle --goal-tables` run, its `bundle`
// step carries an accepted file beside `bundle.json` byte for byte, and the zip declares `package`
// 28; a refused file fails the step and `standard-pack check` names it with its pointer, exiting as
// for a refused `goals.json`; the revision table counts both files as 28, and a bundle that
// declares less is refused by the contract crate's rule the installer calls. The repositories are
// neutral fixtures made with git in a temporary directory as `gate_455_goals_pack`'s (game
// `neutral`, server `test`, two packs built from the TOOL-1 content directory, no
// `schedule.json`); every git call runs with no system configuration and an empty global one. No
// clock is read and nothing sleeps.

use super::*;
use crate::package_tool::gate_390_tool1::neutral_package;
use actingcommand_contract::{
    BundleRevisionNeed, GOAL_TABLES_PACKAGE_REVISION, GOALS_PACKAGE_REVISION, StandardPackProblem,
    package_edge_holds,
};
use actingcommand_resource_tooling::{
    PackageBundleRequest, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackRootFiles, check_standard_pack_root, package_bundle,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const SOURCE_REPOSITORY: &str = "neutral-owner/neutral-resources";
const PACK_A: &str = "neutral.tables.a";
const PACK_B: &str = "neutral.tables.b";
const GOALS: &str = "goals.json";
const TABLES: &str = "goal-tables.json";
const ROOT_INVALID: &str = "standard_pack_root_invalid";

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

/// A neutral resource repository `<temp>/repo`, the outputs of its runs beside it. The fields
/// drop in order, the environment lock last.
struct Fixture {
    temp: TempDir,
    _git: GitConfiguration,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    /// The repository with its root inputs and packs A and B, not yet committed.
    fn new() -> Self {
        let guard = env_lock();
        let temp = TempDir::new().unwrap();
        let global = temp.path().join("empty.gitconfig");
        fs::write(&global, b"").unwrap();
        let git = GitConfiguration::isolate(&global);
        let fixture = Self {
            temp,
            _git: git,
            _guard: guard,
        };
        fs::create_dir(fixture.repo()).unwrap();
        fixture.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        fixture.write(".gitattributes", "* -text\n");
        fixture.write(".gitignore", "*.tmp\n");
        fixture.write(
            "manifest.yaml",
            "game: neutral\nsupported_servers:\n  - test\n",
        );
        fixture.write_json(
            "applications.json",
            &json!({
                "schema_version": "actingcommand.applications.v1",
                "game": "neutral",
                "servers": {"test": {"application_id": "org.neutral.client", "label": "Test"}}
            }),
        );
        fixture.write("maintenance.json", "[]\n");
        fixture.write_json(
            "declaration-exclusions.json",
            &json!({
                "schema_version": "actingcommand.declaration-exclusions.v1",
                "directories": ["notes", format!("packs/{PACK_A}/resources/notes")]
            }),
        );
        fixture.write("packs/README.md", "Neutral fixture packs.\n");
        fixture.pack(PACK_A);
        fixture.pack(PACK_B);
        fixture
    }

    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }

    fn work(&self, run: &str) -> PathBuf {
        self.temp.path().join(format!("{run}-work"))
    }

    fn report(&self, run: &str) -> PathBuf {
        self.temp.path().join(format!("{run}-report.json"))
    }

    /// The game root the run `run` installed and checked, `<work>/installed/neutral`.
    fn installed(&self, run: &str) -> PathBuf {
        self.work(run).join("installed").join("neutral")
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
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// Commits the whole work tree; returns the commit.
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = self.repo().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn write_json(&self, relative: &str, value: &Value) {
        self.write(
            relative,
            format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
        );
    }

    /// The TOOL-1 content directory as `packs/<package_id>/`, its control naming it.
    fn pack(&self, package_id: &str) {
        neutral_package(&self.repo().join("packs").join(package_id));
        let control_path = format!("packs/{package_id}/control.json");
        let mut control: Value =
            serde_json::from_slice(&fs::read(self.repo().join(&control_path)).unwrap()).unwrap();
        control["package_id"] = json!(package_id);
        self.write_json(&control_path, &control);
    }

    /// `actinglab --json resource verify` of the run `run`; its result and its report.
    fn verify(&self, run: &str) -> (CliResult, Value) {
        let result = run_cli(
            [
                "--json".to_owned(),
                "resource".to_owned(),
                "verify".to_owned(),
                "--repo".to_owned(),
                self.repo().display().to_string(),
                "--source-repository".to_owned(),
                SOURCE_REPOSITORY.to_owned(),
                "--work".to_owned(),
                self.work(run).display().to_string(),
                "--report".to_owned(),
                self.report(run).display().to_string(),
            ],
            true,
        );
        let report = serde_json::from_slice(&fs::read(self.report(run)).unwrap())
            .unwrap_or_else(|error| panic!("the report of {run}: {error}"));
        (result, report)
    }

    /// The zip the run `run` wrote for the commit `head`.
    fn zip(&self, run: &str, head: &str) -> PathBuf {
        self.work(run)
            .join("out")
            .join(format!("neutral-resources-{}.zip", &head[..7]))
    }
}

/// A `goals.json` of version 3: a heading and a records entry on the table `heroes`.
fn records_goals() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v3",
        "game": "neutral",
        "entries": [
            {"name": "stock", "type": "heading", "title": {"en": "Stock"}},
            {"name": "raise", "type": "records", "table": "heroes", "max_records": 4,
             "title": {"en": "Whom to raise"}}
        ]
    })
}

/// A `goal-tables.json` of the neutral game whose one key names `class`: its items name no pool
/// (the root has no `schedule.json`), so with `common` every rule holds.
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

/// Every entry of the zip at `path`, by name, with its bytes.
fn zip_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        entries.insert(entry.name().to_owned(), bytes);
    }
    entries
}

/// The `package` range the zip's `interfaces.json` declares.
fn declared_package(entries: &BTreeMap<String, Vec<u8>>) -> [u64; 2] {
    let interfaces: Value = serde_json::from_slice(&entries["interfaces.json"]).unwrap();
    serde_json::from_value(interfaces["speaks"]["package"].clone()).unwrap()
}

/// The `package` range this build declares (`distribution/windows/component-interfaces.json`,
/// which the rescue module embeds).
fn own_package() -> [u64; 2] {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../distribution/windows/component-interfaces.json");
    let declaration: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    serde_json::from_value(declaration["speaks"]["package"].clone()).unwrap()
}

fn step<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["step"] == name)
        .unwrap_or_else(|| panic!("no step {name}: {report}"))
}

/// `standard-pack check` of the game root `root`.
fn checked(root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: root.to_owned(),
    })
    .unwrap_or_else(|error| panic!("{}: {}: {}", root.display(), error.code, error.message))
}

/// The word, the pointer and the path of each `standard_pack_root_invalid` among `outcomes`, in
/// order.
fn root_problems<'a>(
    outcomes: impl IntoIterator<Item = &'a Value>,
) -> Vec<(String, String, String)> {
    outcomes
        .into_iter()
        .filter(|outcome| outcome["code"] == ROOT_INVALID)
        .map(|outcome| {
            let text = |key: &str| {
                outcome["values"][key]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            };
            (
                text("standard_pack_problem"),
                text("json_pointer"),
                text("path"),
            )
        })
        .collect()
}

/// The warnings of a `standard-pack check` answer as the report writes outcomes.
fn warnings_of(answer: &StandardPackCheckResponse) -> Vec<Value> {
    answer
        .warnings
        .iter()
        .map(|warning| json!({"code": warning.code().as_str(), "values": warning.values()}))
        .collect()
}

/// The exit status of `actinglab --json standard-pack check --root <root>`.
fn check_exit(root: &Path) -> i32 {
    run_cli(
        [
            "--json".to_owned(),
            "standard-pack".to_owned(),
            "check".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
        ],
        true,
    )
    .exit_code()
}

// Test 1: a repository with `goals.json` of version 3 (a heading and a records entry on the table
// `heroes`) and an accepted `goal-tables.json` (the table, its items without a pool): the
// verification passes; the checkout binds both files (13 files); the bundle step names both among
// its root files, counts the goal entries by type (`records` among them) and the tables file's
// tables and items, and declares 28; the zip carries both byte for byte beside `bundle.json` and
// its `interfaces.json` declares [28, 28], which this build's range holds and which declares what
// both files ask (`BundleRevisionNeed::Goals` and `::GoalTables`); the installed root holds both,
// and `standard-pack check` reads them without a problem.
#[test]
fn rr1_lab_a_repository_with_both_files_is_verified_carried_and_declared_28() {
    let fixture = Fixture::new();
    fixture.write_json(GOALS, &records_goals());
    fixture.write_json(TABLES, &tables("common"));
    let goals = fs::read(fixture.repo().join(GOALS)).unwrap();
    let goal_tables = fs::read(fixture.repo().join(TABLES)).unwrap();
    let head = fixture.commit("goals and their tables");
    let (result, report) = fixture.verify("both");
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert_eq!(report["failed_steps"], json!([]), "{report}");
    assert_eq!(
        step(&report, "checkout")["counts"],
        json!({"files_checked": 13})
    );
    let counts = &step(&report, "bundle")["counts"];
    assert_eq!(counts["root_files"], json!([GOALS, TABLES]), "{counts}");
    assert_eq!(
        counts["goals"]["entries"],
        json!({"heading": 1, "note": 0, "resource": 0, "tier": 0, "variable": 0, "records": 1})
    );
    assert_eq!(
        counts["goal_tables"],
        json!({"game": "neutral", "path": TABLES, "entries": {"items": 2, "tables": 1}})
    );
    assert_eq!(counts["package_revision"], GOAL_TABLES_PACKAGE_REVISION);
    assert_eq!(GOALS_PACKAGE_REVISION, GOAL_TABLES_PACKAGE_REVISION);

    let entries = zip_entries(&fixture.zip("both", &head));
    assert_eq!(entries.get(GOALS), Some(&goals));
    assert_eq!(entries.get(TABLES), Some(&goal_tables));
    let declared = declared_package(&entries);
    assert_eq!(declared, [28, 28]);
    assert!(package_edge_holds(own_package(), declared));
    assert!(BundleRevisionNeed::Goals.declared_by(declared));
    assert!(BundleRevisionNeed::GoalTables.declared_by(declared));

    let installed = fixture.installed("both");
    assert_eq!(fs::read(installed.join(TABLES)).unwrap(), goal_tables);
    let answer = checked(&installed);
    assert!(
        answer.root_files.contains(&GOALS) && answer.root_files.contains(&TABLES),
        "{:?}",
        answer.root_files
    );
    let warnings = warnings_of(&answer);
    assert!(root_problems(&warnings).is_empty(), "{warnings:?}");
}

// Test 2: the same repository with a `goal-tables.json` the reader refuses (its key names a class
// the table does not have): the verification fails in the `bundle` step with that one problem,
// `standard_pack_root_invalid` with `table_class_unknown` at `/tables/0/keys/hero_a/class` naming
// the repository's file (the records entry is not judged against a refused file, so `goals.json`
// has none), and no zip is written. `standard-pack check` of a root that holds the same two files
// names the same problem at the same pointer with the root's file, and exits as it exits for a
// root whose `goals.json` is refused (version 2, `goal_version_retired`). `package bundle
// --goal-tables` refuses the same file with the same word at the same pointer, `--out` not
// written.
#[test]
fn rr1_lab_a_refused_tables_file_is_named_at_its_pointer_and_fails_the_bundle() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let (plain, _) = fixture.verify("plain");
    assert_eq!(plain.exit_code(), 0, "{}", plain.envelope_json());
    fixture.write_json(GOALS, &records_goals());
    fixture.write_json(TABLES, &tables("rare"));
    fixture.commit("a refused table of goals");
    let (result, report) = fixture.verify("refused");
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(report["failed_steps"], json!(["bundle"]), "{report}");
    let failures = step(&report, "bundle")["failures"].as_array().unwrap();
    let repository_file = fixture.repo().join(TABLES).display().to_string();
    assert_eq!(
        root_problems(failures),
        [(
            StandardPackProblem::TableClassUnknown.as_str().to_owned(),
            "/tables/0/keys/hero_a/class".to_owned(),
            repository_file
        )],
        "{report}"
    );
    assert!(!fixture.work("refused").join("out").exists());

    let installed = fixture.installed("plain");
    fs::copy(fixture.repo().join(GOALS), installed.join(GOALS)).unwrap();
    fs::copy(fixture.repo().join(TABLES), installed.join(TABLES)).unwrap();
    let refused_tables = root_problems(&warnings_of(&checked(&installed)));
    assert_eq!(
        refused_tables
            .iter()
            .map(|(word, pointer, path)| (word.as_str(), pointer.as_str(), path.ends_with(TABLES)))
            .collect::<Vec<_>>(),
        [("table_class_unknown", "/tables/0/keys/hero_a/class", true)]
    );
    let refused_tables_exit = check_exit(&installed);
    fs::write(
        installed.join(TABLES),
        serde_json::to_vec(&tables("common")).unwrap(),
    )
    .unwrap();
    let mut version_2 = records_goals();
    version_2["schema_version"] = json!("actingcommand.goals.v2");
    fs::write(
        installed.join(GOALS),
        serde_json::to_vec(&version_2).unwrap(),
    )
    .unwrap();
    let refused_goals = root_problems(&warnings_of(&checked(&installed)));
    assert_eq!(
        refused_goals
            .iter()
            .map(|(word, pointer, _)| (word.as_str(), pointer.as_str()))
            .collect::<Vec<_>>(),
        [("goal_version_retired", "/schema_version")]
    );
    assert_eq!(
        refused_tables_exit,
        check_exit(&installed),
        "the check exits as for a refused goals.json"
    );

    let out = fixture.temp.path().join("bundle-with-tables");
    let error = package_bundle(PackageBundleRequest {
        applications: fixture.repo().join("applications.json"),
        packs_root: fixture.work("plain").join("packs-root"),
        out: out.clone(),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles {
            goal_tables: Some(fixture.repo().join(TABLES)),
            ..StandardPackRootFiles::default()
        },
    })
    .expect_err("package bundle --goal-tables refuses a refused file");
    assert_eq!(error.code, ROOT_INVALID, "{}", error.message);
    let refused = &error.details.as_ref().unwrap()["outcome"];
    assert_eq!(
        root_problems([refused])
            .iter()
            .map(|(word, pointer, _)| (word.as_str(), pointer.as_str()))
            .collect::<Vec<_>>(),
        [("table_class_unknown", "/tables/0/keys/hero_a/class")],
        "{refused}"
    );
    assert!(!out.exists());
}

// Test 3, the revision table (`standard-pack build`'s) and the installer's rule: a zip whose root
// holds `goal-tables.json`, or `goals.json` of version 3, needs 28, whatever the file holds; this
// build declares [3, 28] (`package` 27 is another slice's) and places such a zip; an installer of
// [3, 27] refuses it at its `package` edge; and a zip that declares less than 28 does not declare
// what either file asks (`goal_tables_undeclared`, `goal_undeclared`), the rule the installer's
// `place` and the readiness item I2 call.
#[test]
fn rr1_lab_both_files_need_package_28_and_a_bundle_below_it_is_refused() {
    let revision_of = |name: &str, value: &Value| {
        let zip = BTreeMap::from([(name.to_owned(), serde_json::to_vec(value).unwrap())]);
        crate::resource_verify::revision::content_revision(&zip)
    };
    assert_eq!(revision_of(TABLES, &tables("common")), 28);
    assert_eq!(
        revision_of(TABLES, &json!({"anything": true})),
        GOAL_TABLES_PACKAGE_REVISION,
        "whatever the file holds"
    );
    assert_eq!(revision_of(GOALS, &records_goals()), GOALS_PACKAGE_REVISION);
    assert_eq!(GOALS_PACKAGE_REVISION, 28);

    // Workflow #503 LIST-B (register §1.536) raises the build's range to [3, 29].
    let own = own_package();
    assert_eq!(own, [3, 29]);
    let needed = [GOAL_TABLES_PACKAGE_REVISION, GOAL_TABLES_PACKAGE_REVISION];
    assert!(package_edge_holds(own, needed));
    assert!(
        !package_edge_holds([3, GOAL_TABLES_PACKAGE_REVISION - 1], needed),
        "an installer of the earlier range refuses the zip at its package edge"
    );
    for below in [[3, 27], [14, 14], [3, 3]] {
        assert!(
            !BundleRevisionNeed::GoalTables.declared_by(below),
            "{below:?}"
        );
        assert!(!BundleRevisionNeed::Goals.declared_by(below), "{below:?}");
    }
    assert_eq!(
        BundleRevisionNeed::GoalTables.problem(),
        StandardPackProblem::GoalTablesUndeclared
    );
    assert_eq!(
        BundleRevisionNeed::Goals.problem(),
        StandardPackProblem::GoalUndeclared
    );
    let names = BTreeSet::from([
        StandardPackProblem::GoalTablesUndeclared.as_str(),
        StandardPackProblem::GoalUndeclared.as_str(),
    ]);
    assert_eq!(names.len(), 2, "one word for each file");
}
