// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 GOALS-PACK-1 (register §1.498; contracts/standard-pack-goals.md, "Who reads it";
// contracts/resource-verify.md, the `bundle` step; coordinator work order
// `rd-1121/brief-455-goals-pack.md`): `resource verify` checks a repository's `goals.json` with the
// one goal check `standard-pack check` and `package bundle --goals` run, its `bundle` step carries
// an accepted file beside `bundle.json` byte for byte, and its `interfaces.json` declares the
// `package` revision a zip with the file needs. The repositories are neutral fixtures (game
// `neutral`, server `test`, two packs built from the TOOL-1 content directory, no
// `schedule.json`), made with git in a temporary directory as RV-1's (`gate_rv1`); every git call,
// the fixture's and the command's, runs with no system configuration and an empty global one. No
// clock is read and nothing sleeps.

use super::*;
use crate::package_tool::gate_390_tool1::neutral_package;
use actingcommand_contract::{BundleRevisionNeed, GOALS_PACKAGE_REVISION, package_edge_holds};
use actingcommand_resource_tooling::{
    PackageBundleRequest, StandardPackCheckRequest, StandardPackCheckResponse,
    StandardPackRootFiles, check_standard_pack_root, package_bundle,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const SOURCE_REPOSITORY: &str = "neutral-owner/neutral-resources";
const PACK_A: &str = "neutral.goals.a";
const PACK_B: &str = "neutral.goals.b";
const GOALS: &str = "goals.json";
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

/// A neutral resource repository `<temp>/repo`, the outputs of its runs beside it: RV-1's files,
/// so that a run without `goals.json` reports what RV-1 pins. The fields drop in order, the
/// environment lock last.
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

    /// The work directory and the report of the run `run`.
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

    /// `actinglab --json resource verify` of the run `run`; its result and, written whole, its
    /// report.
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

/// An accepted `goals.json` of the neutral game: a heading and a note, which name nothing of the
/// root.
fn accepted_goals() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v2",
        "game": "neutral",
        "entries": [
            {"name": "stock", "type": "heading", "title": {"en": "Stock"}},
            {"name": "about", "type": "note", "title": {"en": "About"},
             "help": {"en": "Neutral text."}}
        ]
    })
}

/// A `goals.json` the reader accepts and whose references are refused twice: a `resource` entry
/// naming a pool the root does not offer (it holds no `schedule.json`) and the placement of a
/// variable no pack declares.
fn refused_goals() -> Value {
    json!({
        "schema_version": "actingcommand.goals.v2",
        "game": "neutral",
        "entries": [
            {"name": "points", "type": "resource", "governs": {"pool": "pool.neutral.points"},
             "title": {"en": "Points to hold"}},
            {"name": "route", "type": "variable"}
        ]
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

/// The `package` range this build's installer speaks: the declaration its `place` reads
/// (`distribution/windows/component-interfaces.json`, which the rescue module embeds).
fn installer_package() -> [u64; 2] {
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

fn error_code(result: &CliResult) -> String {
    result
        .envelope
        .error
        .as_ref()
        .map(|error| error.code.clone())
        .unwrap_or_default()
}

/// `standard-pack check` of the game root `root`.
fn checked(root: &Path) -> StandardPackCheckResponse {
    check_standard_pack_root(StandardPackCheckRequest {
        root: root.to_owned(),
    })
    .unwrap_or_else(|error| panic!("{}: {}: {}", root.display(), error.code, error.message))
}

/// The word and the pointer of each `standard_pack_root_invalid` among `outcomes`, in order.
fn root_problems<'a>(outcomes: impl IntoIterator<Item = &'a Value>) -> Vec<(String, String)> {
    outcomes
        .into_iter()
        .filter(|outcome| outcome["code"] == ROOT_INVALID)
        .map(|outcome| {
            (
                outcome["values"]["standard_pack_problem"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                outcome["values"]["json_pointer"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
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

/// The names the zip of a repository without `goals.json` holds: `applications.json`,
/// `bundle.json`, `interfaces.json` and the three files of each pack.
fn plain_entries() -> BTreeSet<String> {
    let mut names = BTreeSet::from([
        "applications.json".to_owned(),
        "bundle.json".to_owned(),
        "interfaces.json".to_owned(),
    ]);
    for pack in [PACK_A, PACK_B] {
        for file in [
            "control.json",
            "resources/operations/resources.json",
            "resources/operations/task/task.json",
        ] {
            names.insert(format!("packs/{pack}/{file}"));
        }
    }
    names
}

// Test 1: a repository without `goals.json` is verified, bundled and declared as before: the
// checkout's 11 files, the bundle step's counts exactly RV-1's members (no `goals`, no root file,
// package 3, a zip of 9 entries), the zip's member list RV-1's, its `interfaces.json` `package`
// [3, 3] validated with this build's head, and no `goals.json` in the zip or the installed root.
#[test]
fn goals_pack_a_repository_without_the_file_is_verified_as_before() {
    let fixture = Fixture::new();
    let head = fixture.commit("neutral fixture");
    let (result, report) = fixture.verify("plain");
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert_eq!(report["result"], "passed");
    assert_eq!(report["failed_steps"], json!([]));
    assert_eq!(
        step(&report, "checkout")["counts"],
        json!({"files_checked": 11})
    );
    let zip = fixture.zip("plain", &head);
    let counts = &step(&report, "bundle")["counts"];
    assert_eq!(
        *counts,
        json!({
            "packs": 2,
            "files": 6,
            "bytes": counts["bytes"].clone(),
            "zip": {
                "name": zip.file_name().unwrap().to_string_lossy(),
                "sha256": crate::sha256::hex_sha256(&fs::read(&zip).unwrap()),
                "entries": 9
            },
            "root_files": [],
            "package_revision": 3
        })
    );
    let entries = zip_entries(&zip);
    assert_eq!(
        entries.keys().cloned().collect::<BTreeSet<_>>(),
        plain_entries()
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&entries["interfaces.json"]).unwrap(),
        json!({
            "schema_version": "actingcommand.component-interfaces.v1",
            "speaks": {"package": [3, 3]},
            "validated_with": {
                "repository": "HS7097/ActingCommand-Runtime",
                "commit": env!("ACTINGCOMMAND_RUNTIME_HEAD")
            }
        })
    );
    assert!(!fixture.installed("plain").join(GOALS).exists());
}

// Test 2: an accepted `goals.json` (a heading and a note): the verification passes; the checkout
// binds the file to the commit (12 files); the bundle step names it (the game, its path, one
// heading and one note, every other type 0) among its root files; the zip carries it byte for byte
// beside `bundle.json` and its `interfaces.json` declares `package` [14, 14]
// (`GOALS_PACKAGE_REVISION`); the installer's plan takes such a zip: this build's installer range
// holds 14 (`package_edge_holds`) and 14 declares what the file asks (`BundleRevisionNeed::Goals`,
// which [3, 3], the declaration before this slice, does not: `goal_undeclared`); the installed root
// holds the file and `standard-pack check` reads it without a problem.
#[test]
fn goals_pack_an_accepted_file_is_named_carried_and_declared() {
    let fixture = Fixture::new();
    fixture.write_json(GOALS, &accepted_goals());
    let goals = fs::read(fixture.repo().join(GOALS)).unwrap();
    let head = fixture.commit("a goal declaration");
    let (result, report) = fixture.verify("accepted");
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert_eq!(report["outcome"]["code"], "resource_verify_passed");
    assert_eq!(report["failed_steps"], json!([]));
    assert_eq!(
        step(&report, "checkout")["counts"],
        json!({"files_checked": 12})
    );
    let bundle = step(&report, "bundle");
    assert_eq!(bundle["result"], "passed");
    assert!(
        root_problems(bundle["warnings"].as_array().unwrap()).is_empty(),
        "{bundle}"
    );
    let counts = &bundle["counts"];
    assert_eq!(
        counts["goals"],
        json!({
            "game": "neutral",
            "path": GOALS,
            "entries": {"heading": 1, "note": 1, "resource": 0, "tier": 0, "variable": 0}
        })
    );
    assert_eq!(counts["root_files"], json!([GOALS]));
    assert_eq!(counts["package_revision"], GOALS_PACKAGE_REVISION);
    assert_eq!(counts["zip"]["entries"], 10);

    let entries = zip_entries(&fixture.zip("accepted", &head));
    let mut expected = plain_entries();
    expected.insert(GOALS.to_owned());
    assert_eq!(entries.keys().cloned().collect::<BTreeSet<_>>(), expected);
    assert_eq!(entries.get(GOALS), Some(&goals));
    let declared = declared_package(&entries);
    assert_eq!(declared, [GOALS_PACKAGE_REVISION, GOALS_PACKAGE_REVISION]);
    assert!(
        package_edge_holds(installer_package(), declared),
        "{:?} {declared:?}",
        installer_package()
    );
    assert!(BundleRevisionNeed::Goals.declared_by(declared));
    assert!(!BundleRevisionNeed::Goals.declared_by([3, 3]));

    let installed = fixture.installed("accepted");
    assert_eq!(fs::read(installed.join(GOALS)).unwrap(), goals);
    let answer = checked(&installed);
    assert!(
        answer.root_files.contains(&GOALS),
        "{:?}",
        answer.root_files
    );
    let warnings = warnings_of(&answer);
    assert!(root_problems(&warnings).is_empty(), "{warnings:?}");
}

// Test 3: a refused `goals.json` (a `resource` entry naming a pool the root does not offer, and the
// placement of a variable no pack declares): the verification fails in the `bundle` step alone with
// both problems, in file order, each `standard_pack_root_invalid` with the game, the repository's
// file, its word and its pointer, and the step; the first is the top outcome's cause; no zip is
// written and the counts name no file. Fix round 1: the step's index and copy checks still run in
// the same run: pack B's directory also holds a file the bundle does not copy (a pack is copied as
// `control.json` and `resources/` only), and the same failures name it, `pack_copy_differs`.
// `standard-pack check` of a root that holds the same file gives the same words at the same
// pointers in the same order: one check.
#[test]
fn goals_pack_a_refused_file_fails_with_every_problem_and_no_zip() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let (plain, _) = fixture.verify("plain");
    assert_eq!(plain.exit_code(), 0, "{}", plain.envelope_json());
    fixture.write_json(GOALS, &refused_goals());
    fixture.write(
        &format!("packs/{PACK_B}/NOTES.txt"),
        "A note the bundle does not copy.\n",
    );
    fixture.commit("a refused goal declaration");
    let (result, report) = fixture.verify("refused");
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(error_code(&result), "resource_verify_failed");
    assert_eq!(report["failed_steps"], json!(["bundle"]), "{report}");
    let bundle = step(&report, "bundle");
    let failures = bundle["failures"].as_array().unwrap();
    let expected = [
        (
            "goal_pool_unknown".to_owned(),
            "/entries/0/governs/pool".to_owned(),
        ),
        (
            "goal_variable_unknown".to_owned(),
            "/entries/1/name".to_owned(),
        ),
    ];
    assert_eq!(root_problems(failures), expected, "{bundle}");
    assert_eq!(failures.len(), 3, "{bundle}");
    let copied = format!("packs/{PACK_B}");
    assert!(
        failures.iter().any(|failure| {
            failure["code"] == "resource_verify_bundle_mismatch"
                && failure["values"]["path"] == copied.as_str()
                && failure["values"]["bundle_problem"] == "pack_copy_differs"
        }),
        "{bundle}"
    );
    let path = fixture.repo().join(GOALS).display().to_string();
    for failure in failures
        .iter()
        .filter(|failure| failure["code"] == ROOT_INVALID)
    {
        assert_eq!(failure["values"]["game"], "neutral", "{failure}");
        assert_eq!(failure["values"]["path"], path.as_str(), "{failure}");
        assert_eq!(failure["values"]["step"], "bundle", "{failure}");
    }
    let cause = &report["outcome"]["causes"][0];
    assert_eq!(cause["code"], ROOT_INVALID, "{report}");
    assert_eq!(
        cause["values"]["standard_pack_problem"],
        "goal_pool_unknown"
    );
    assert!(!fixture.work("refused").join("out").exists());
    assert_eq!(bundle["counts"]["zip"], Value::Null);
    assert_eq!(bundle["counts"].get("goals"), None);

    let installed = fixture.installed("plain");
    fs::copy(fixture.repo().join(GOALS), installed.join(GOALS)).unwrap();
    assert_eq!(root_problems(&warnings_of(&checked(&installed))), expected);
}

// Test 4: a version 1 `goals.json` fails the verification with its one problem,
// `goal_version_retired` at `/schema_version`, and no zip; `package bundle --goals` refuses the same
// file with the same word at the same pointer, `--out` not written.
#[test]
fn goals_pack_a_version_1_file_is_refused_as_the_bundle_refuses_it() {
    let fixture = Fixture::new();
    fixture.write_json(
        GOALS,
        &json!({"schema_version": "actingcommand.goals.v1", "game": "neutral", "entries": []}),
    );
    fixture.commit("a version 1 goal declaration");
    let (result, report) = fixture.verify("retired");
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(report["failed_steps"], json!(["bundle"]), "{report}");
    let failures = step(&report, "bundle")["failures"].as_array().unwrap();
    let expected = [(
        "goal_version_retired".to_owned(),
        "/schema_version".to_owned(),
    )];
    assert_eq!(root_problems(failures), expected, "{report}");
    assert_eq!(failures.len(), 1, "{report}");
    assert!(!fixture.work("retired").join("out").exists());

    let out = fixture.temp.path().join("bundle-with-goals");
    let error = package_bundle(PackageBundleRequest {
        applications: fixture.repo().join("applications.json"),
        packs_root: fixture.work("retired").join("packs-root"),
        out: out.clone(),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles {
            goals: Some(fixture.repo().join(GOALS)),
            ..StandardPackRootFiles::default()
        },
    })
    .expect_err("package bundle --goals refuses a version 1 file");
    assert_eq!(error.code, ROOT_INVALID, "{}", error.message);
    let refused = &error.details.as_ref().unwrap()["outcome"];
    assert_eq!(root_problems([refused]), expected, "{refused}");
    assert!(!out.exists());
}

// Test 5: a repository is one game root (`applications.json`'s game, its root files at the top),
// so two game roots in one repository cannot be verified; the nearest case: one repository's two
// commits, the first with an accepted `goals.json`, the second without it, each verified as its
// own case: the first names, carries and declares the file; the second, after it, is the
// repository without the file (11 files, no `goals`, no root file, package 3, a zip of RV-1's
// members declaring [3, 3]).
#[test]
fn goals_pack_each_commit_is_verified_as_its_own_case() {
    let fixture = Fixture::new();
    fixture.write_json(GOALS, &accepted_goals());
    let with = fixture.commit("with the goal declaration");
    let (result, report) = fixture.verify("with");
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let counts = &step(&report, "bundle")["counts"];
    assert_eq!(counts["goals"]["entries"]["heading"], 1, "{counts}");
    assert_eq!(counts["package_revision"], GOALS_PACKAGE_REVISION);
    let entries = zip_entries(&fixture.zip("with", &with));
    assert!(entries.contains_key(GOALS));
    assert_eq!(
        declared_package(&entries),
        [GOALS_PACKAGE_REVISION, GOALS_PACKAGE_REVISION]
    );

    fixture.git(&["rm", "-q", GOALS]);
    let without = fixture.commit("without the goal declaration");
    let (result, report) = fixture.verify("without");
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert_eq!(
        step(&report, "checkout")["counts"],
        json!({"files_checked": 11})
    );
    let counts = &step(&report, "bundle")["counts"];
    assert_eq!(counts.get("goals"), None, "{counts}");
    assert_eq!(counts["root_files"], json!([]));
    assert_eq!(counts["package_revision"], 3);
    assert_eq!(counts["zip"]["entries"], 9);
    let entries = zip_entries(&fixture.zip("without", &without));
    assert_eq!(
        entries.keys().cloned().collect::<BTreeSet<_>>(),
        plain_entries()
    );
    assert_eq!(declared_package(&entries), [3, 3]);
    assert!(!fixture.installed("without").join(GOALS).exists());
}
