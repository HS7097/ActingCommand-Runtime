// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #464 P2 (contracts/resource-verify.md, "`standard-pack build`"; contracts/package-
// reference.md, "The built standard pack"; the design's section G, the row "P2 build"):
// `actinglab standard-pack build --repo <clean checkout> --source-repository <owner>/<name> --out
// <new or empty directory> [--commit <sha>]` runs `resource verify`'s steps on their one function
// and, only when every step passed, writes into `--out` the steps' zip, `SHA256SUMS` and
// `build.json`. The repositories are neutral fixtures (game `neutral`, server `test`, two packs
// built from the TOOL-1 content directory), made with git in a temporary directory, as RV-1's
// (`gate_rv1`); every git call, the fixture's and the command's, runs with no system configuration
// and an empty global one. No clock is read and nothing sleeps.

use super::*;
use crate::package_tool::gate_390_tool1::neutral_package;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const SOURCE_REPOSITORY: &str = "neutral-owner/neutral-resources";
const PACK_A: &str = "neutral.build.a";
const PACK_B: &str = "neutral.build.b";
const STEPS: [&str; 7] = [
    "tools",
    "checkout",
    "declarations",
    "pins",
    "admission",
    "format",
    "bundle",
];

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

/// A neutral resource repository `<temp>/repo`, the outputs of its runs beside it: the files of
/// RV-1's fixture, so that `resource verify` reports on it what RV-1 pins. The fields drop in
/// order, the environment lock last.
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

    /// `resource verify`'s work directory and report of the run `run`.
    fn work(&self, run: &str) -> PathBuf {
        self.temp.path().join(format!("{run}-work"))
    }

    fn report(&self, run: &str) -> PathBuf {
        self.temp.path().join(format!("{run}-report.json"))
    }

    /// `standard-pack build`'s `--out` of the run `run`.
    fn out(&self, run: &str) -> PathBuf {
        self.temp.path().join(format!("{run}-out"))
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

    /// `actinglab --json resource verify` of the run `run`; returns its result and, written
    /// whole, its report.
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

    /// `actinglab --json standard-pack build` with `out` as `--out`, then `extra`.
    fn build_into(&self, out: &Path, extra: &[&str]) -> CliResult {
        let mut arguments = vec![
            "--json".to_owned(),
            "standard-pack".to_owned(),
            "build".to_owned(),
            "--repo".to_owned(),
            self.repo().display().to_string(),
            "--source-repository".to_owned(),
            SOURCE_REPOSITORY.to_owned(),
            "--out".to_owned(),
            out.display().to_string(),
        ];
        arguments.extend(extra.iter().map(|argument| (*argument).to_owned()));
        run_cli(arguments, true)
    }

    /// `standard-pack build` of the run `run` into its `--out`.
    fn build(&self, run: &str, extra: &[&str]) -> CliResult {
        self.build_into(&self.out(run), extra)
    }

    /// The `build.json` the run `run` wrote.
    fn build_json(&self, run: &str) -> Value {
        serde_json::from_slice(&fs::read(self.out(run).join("build.json")).unwrap()).unwrap()
    }
}

/// The name the zip of the commit `head` takes.
fn zip_name(head: &str) -> String {
    format!("neutral-resources-{}.zip", &head[..7])
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
fn declared_package(entries: &BTreeMap<String, Vec<u8>>) -> Value {
    serde_json::from_slice::<Value>(&entries["interfaces.json"]).unwrap()["speaks"]["package"]
        .clone()
}

fn step<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["step"] == name)
        .unwrap_or_else(|| panic!("no step {name}: {report}"))
}

fn names(steps: &Value) -> Vec<&str> {
    steps
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["step"].as_str().unwrap())
        .collect()
}

fn error_code(result: &CliResult) -> String {
    result
        .envelope
        .error
        .as_ref()
        .map(|error| error.code.clone())
        .unwrap_or_default()
}

fn error_details(result: &CliResult) -> Value {
    result
        .envelope
        .error
        .as_ref()
        .and_then(|error| error.details.clone())
        .unwrap_or(Value::Null)
}

fn entries_of(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

// G, P2 first item, and the ruling "resource verify's report is unchanged for the same input":
// on one commit `resource verify` reports what RV-1 pins for this fixture (every step passed, the
// checkout's 11 files, the bundle's 2 packs, 6 files, no root file, package 3 and a zip of 9
// entries), and `standard-pack build` runs the same seven steps, every one passed, and writes into
// `--out` that zip byte for byte and the same `SHA256SUMS`, beside `build.json` and its work.
#[test]
fn p2_the_build_carries_resource_verifys_zip_and_verifys_report_is_as_before() {
    let fixture = Fixture::new();
    let head = fixture.commit("neutral fixture");
    let (verified, report) = fixture.verify("verify");
    assert_eq!(verified.exit_code(), 0, "{}", verified.envelope_json());
    assert_eq!(report["schema_version"], "actinglab.resource-verify.v1");
    assert_eq!(report["result"], "passed");
    assert_eq!(report["outcome"]["code"], "resource_verify_passed");
    assert_eq!(report["outcome"]["values"]["step_count"], 7);
    assert_eq!(names(&report["steps"]), STEPS);
    for name in STEPS {
        assert_eq!(step(&report, name)["result"], "passed", "{name}: {report}");
    }
    assert_eq!(report["failed_steps"], json!([]));
    assert_eq!(
        step(&report, "checkout")["counts"],
        json!({"files_checked": 11})
    );
    assert_eq!(step(&report, "pins")["counts"], json!({"files": 6}));
    assert_eq!(
        step(&report, "admission")["counts"],
        json!({"packs": 2, "admitted": 2})
    );
    let counts = &step(&report, "bundle")["counts"];
    assert_eq!(counts["packs"], 2);
    assert_eq!(counts["files"], 6);
    assert_eq!(counts["root_files"], json!([]));
    assert_eq!(counts["package_revision"], 3);
    assert_eq!(counts["zip"]["name"], zip_name(&head).as_str());
    assert_eq!(counts["zip"]["entries"], 9);
    let made = fixture.work("verify").join("out");
    let verified_zip = fs::read(made.join(zip_name(&head))).unwrap();
    assert_eq!(
        counts["zip"]["sha256"],
        crate::sha256::hex_sha256(&verified_zip).as_str()
    );

    let built = fixture.build("build", &[]);
    assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
    let answer = built.envelope.data.clone().unwrap();
    assert_eq!(names(&answer["steps"]), STEPS);
    for name in STEPS {
        assert_eq!(step(&answer, name)["result"], "passed", "{name}: {answer}");
    }
    let out = fixture.out("build");
    assert_eq!(
        fs::read(out.join(zip_name(&head))).unwrap(),
        verified_zip,
        "the build's zip is resource verify's, byte for byte"
    );
    assert_eq!(
        fs::read(out.join("SHA256SUMS")).unwrap(),
        fs::read(made.join("SHA256SUMS")).unwrap()
    );
    assert_eq!(
        entries_of(&out),
        BTreeSet::from([
            zip_name(&head),
            "SHA256SUMS".to_owned(),
            "build.json".to_owned(),
            "work".to_owned(),
        ])
    );
}

// G, P2 second item: a committed `notices/` makes the build's zip declare `package` [10, 10], and
// a committed `goals.json` (no entry) [12, 12], in its `interfaces.json` and in `build.json`; the
// zip carries `goals.json` byte for byte. `resource verify` of the commit with `goals.json` still
// leaves it out and declares 10, as before.
#[test]
fn p2_notices_give_package_10_and_goals_give_12() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let notice: &[u8] = b"Placeholder notice.\n";
    fixture.write("notices/NOTICE.md", notice);
    let head = fixture.commit("a notices directory");
    let built = fixture.build("notices", &[]);
    assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
    let entries = zip_entries(&fixture.out("notices").join(zip_name(&head)));
    assert_eq!(
        entries.get("notices/NOTICE.md").map(Vec::as_slice),
        Some(notice)
    );
    assert_eq!(declared_package(&entries), json!([10, 10]));
    let document = fixture.build_json("notices");
    assert_eq!(document["package"], json!([10, 10]));
    assert_eq!(document["root_directories"], json!(["notices"]));

    fixture.write_json(
        "goals.json",
        &json!({"schema_version": "actingcommand.goals.v1", "game": "neutral", "entries": []}),
    );
    let goals = fs::read(fixture.repo().join("goals.json")).unwrap();
    let head = fixture.commit("a goal declaration");
    let built = fixture.build("goals", &[]);
    assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
    let entries = zip_entries(&fixture.out("goals").join(zip_name(&head)));
    assert_eq!(entries.get("goals.json"), Some(&goals));
    assert_eq!(declared_package(&entries), json!([12, 12]));
    let document = fixture.build_json("goals");
    assert_eq!(document["package"], json!([12, 12]));
    assert!(
        document["root_files"]
            .as_array()
            .unwrap()
            .contains(&json!("goals.json")),
        "{document}"
    );

    let (verified, report) = fixture.verify("goals-verify");
    assert_eq!(verified.exit_code(), 0, "{}", verified.envelope_json());
    let counts = &step(&report, "bundle")["counts"];
    assert_eq!(counts["package_revision"], 10);
    assert_eq!(counts["root_files"], json!([]));
    let entries = zip_entries(
        &fixture
            .work("goals-verify")
            .join("out")
            .join(zip_name(&head)),
    );
    assert!(!entries.contains_key("goals.json"), "{:?}", entries.keys());
    assert_eq!(declared_package(&entries), json!([10, 10]));
}

// G, P2 third item, and the ruling on `--out`: a tree with a tracked change, and one with an
// untracked file, are refused `resource_build_failed`, caused by the checkout precondition's
// `resource_verify_checkout_dirty`, with the steps' report in the details; fix round 1 (adapted:
// the work order's point 4 changes what this pinned, an empty `--out`): the refused call leaves
// nothing behind, `--out` itself not made. An `--out` that holds a file, and one inside the
// repository, are refused `resource_build_output_invalid` before anything is read, the file kept
// and nothing made.
#[test]
fn p2_a_dirty_tree_or_an_output_in_use_is_refused_and_nothing_is_built() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let cases: [(&str, &str, &str); 2] = [
        ("tracked", "tracked_change", "maintenance.json"),
        ("untracked", "untracked", "scratch.txt"),
    ];
    for (run, problem, path) in cases {
        match run {
            "tracked" => fixture.write("maintenance.json", "[ ]\n"),
            _ => {
                fixture.write("maintenance.json", "[]\n");
                fixture.write("scratch.txt", "scratch\n");
            }
        }
        let built = fixture.build(run, &[]);
        assert_eq!(built.exit_code(), 2, "{run}: {}", built.envelope_json());
        assert_eq!(error_code(&built), "resource_build_failed", "{run}");
        let details = error_details(&built);
        let outcome = &details["outcome"];
        assert_eq!(outcome["code"], "resource_build_failed", "{details}");
        assert_eq!(outcome["values"]["failed_step_count"], 1, "{run}");
        let cause = &outcome["causes"][0];
        assert_eq!(cause["code"], "resource_verify_checkout_dirty", "{run}");
        assert_eq!(cause["values"]["dirty_problem"], problem, "{run}");
        assert_eq!(cause["values"]["first_path"], path, "{run}");
        assert_eq!(details["report"]["failed_steps"], json!(["checkout"]));
        for name in ["declarations", "pins", "admission", "format", "bundle"] {
            assert_eq!(
                step(&details["report"], name)["result"],
                "not_run",
                "{run} {name}"
            );
        }
        assert!(
            !fixture.out(run).exists(),
            "{run}: nothing is left behind, --out included"
        );
    }

    let used = fixture.out("used");
    fs::create_dir(&used).unwrap();
    fs::write(used.join("kept.txt"), b"kept").unwrap();
    let inside = fixture.repo().join("built");
    for out in [&used, &inside] {
        let built = fixture.build_into(out, &[]);
        assert_eq!(built.exit_code(), 2, "{}", built.envelope_json());
        assert_eq!(error_code(&built), "resource_build_output_invalid");
        assert_eq!(
            error_details(&built)["values"]["path"],
            out.display().to_string()
        );
    }
    assert_eq!(entries_of(&used), BTreeSet::from(["kept.txt".to_owned()]));
    assert_eq!(fs::read(used.join("kept.txt")).unwrap(), b"kept");
    assert!(!inside.exists());
}

// G, P2 fourth item: `build.json` is the zip's: its schema, game, source repository, commit and
// tree (git's), the Tools' head and executable SHA-256, the `package` range the zip's
// `interfaces.json` declares, the zip's root files and root directories, its `bundle.json` packs,
// and the zip's name, SHA-256 (also `SHA256SUMS`'s) and size; it has no other member (no
// readiness), and the answer carries the same document.
#[test]
fn p2_build_json_names_the_commit_and_matches_the_zip() {
    let fixture = Fixture::new();
    let head = fixture.commit("neutral fixture");
    let tree = fixture.git(&["rev-parse", "HEAD^{tree}"]);
    let built = fixture.build("build", &["--commit", &head]);
    assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
    let out = fixture.out("build");
    let document = fixture.build_json("build");
    let zip_path = out.join(zip_name(&head));
    let zip = fs::read(&zip_path).unwrap();
    let zip_sha256 = crate::sha256::hex_sha256(&zip);
    let entries = zip_entries(&zip_path);
    let index: Value = serde_json::from_slice(&entries["bundle.json"]).unwrap();

    let members = document
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        members,
        BTreeSet::from([
            "schema_version",
            "game",
            "source_repository",
            "commit",
            "tree",
            "tools",
            "package",
            "root_files",
            "root_directories",
            "packs",
            "zip",
        ])
    );
    assert_eq!(
        document["schema_version"],
        "actingcommand.standard-pack-build.v1"
    );
    assert_eq!(document["game"], "neutral");
    assert_eq!(document["game"], index["game"]);
    assert_eq!(document["source_repository"], SOURCE_REPOSITORY);
    assert_eq!(
        index["source"],
        json!({"repository": SOURCE_REPOSITORY, "commit": head})
    );
    assert_eq!(document["commit"], head.as_str());
    assert_eq!(document["tree"], tree.as_str());
    assert_eq!(
        document["tools"],
        json!({
            "runtime_head": env!("ACTINGCOMMAND_RUNTIME_HEAD"),
            "executable_sha256": crate::sha256::hex_sha256(
                &fs::read(std::env::current_exe().unwrap()).unwrap()
            ),
        })
    );
    assert_eq!(document["package"], declared_package(&entries));
    assert_eq!(document["package"], json!([3, 3]));
    assert_eq!(
        document["root_files"],
        json!(["applications.json", "bundle.json", "interfaces.json"])
    );
    assert_eq!(document["root_directories"], json!([]));
    let packs = index["packs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pack| {
            json!({
                "package_id": pack["package_id"],
                "server": pack["server"],
                "entry_task_id": pack["entry_task_id"],
                "file_count": pack["file_count"],
                "byte_count": pack["byte_count"],
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(document["packs"], json!(packs));
    assert_eq!(packs.len(), 2);
    assert_eq!(
        document["zip"],
        json!({"name": zip_name(&head), "sha256": zip_sha256, "bytes": zip.len()})
    );
    assert_eq!(
        fs::read_to_string(out.join("SHA256SUMS")).unwrap(),
        format!("{zip_sha256}  {}\n", zip_name(&head))
    );
    let answer = built.envelope.data.clone().unwrap();
    assert_eq!(answer["build"], document);
    assert_eq!(answer["out"], out.display().to_string());
    assert_eq!(
        answer["files"]["build"],
        out.join("build.json").display().to_string()
    );
}

/// The bytes of a table `neutral_scores` of the fixture's game (`actingcommand.table.v1`).
fn neutral_table() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.table.v1",
        "game": "neutral",
        "table_id": "neutral_scores",
        "row_key": ["unit"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "score_milli", "type": "integer", "min": 0, "max": 100000}
        ],
        "rows": [["unit.a", 500]]
    }))
    .unwrap()
}

// Fix round 1, the review's second point (`carry_tables`): a commit's `tables/neutral_scores.json`
// goes into the zip byte for byte, the zip's `bundle.json` lists it in `tables`, `build.json`
// names the directory, and the build declares no revision for it (the `package` row 6: no member
// of a pack reads a game root's tables); a table its reader refuses fails the build, the reader's
// refusal the bundle step's failure and the cause of the steps' outcome, and no product is
// written.
#[test]
fn p2_tables_are_carried_and_listed_and_a_refused_table_fails_the_build() {
    let fixture = Fixture::new();
    let table = neutral_table();
    fixture.write("tables/neutral_scores.json", &table);
    let head = fixture.commit("a table");
    let built = fixture.build("tables", &[]);
    assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
    let entries = zip_entries(&fixture.out("tables").join(zip_name(&head)));
    assert_eq!(entries.get("tables/neutral_scores.json"), Some(&table));
    let index: Value = serde_json::from_slice(&entries["bundle.json"]).unwrap();
    assert_eq!(index["tables"], json!(["neutral_scores"]));
    assert_eq!(declared_package(&entries), json!([3, 3]));
    let document = fixture.build_json("tables");
    assert_eq!(document["root_directories"], json!(["tables"]));
    assert_eq!(document["package"], json!([3, 3]));

    fixture.write("tables/neutral_scores.json", "{}\n");
    fixture.commit("a table its reader refuses");
    let built = fixture.build("refused-table", &[]);
    assert_eq!(built.exit_code(), 2, "{}", built.envelope_json());
    assert_eq!(error_code(&built), "resource_build_failed");
    let details = error_details(&built);
    assert_eq!(
        details["outcome"]["causes"][0]["code"], "resource_verify_failed",
        "{details}"
    );
    let report = &details["report"];
    assert_eq!(report["failed_steps"], json!(["bundle"]), "{report}");
    assert_eq!(
        report["outcome"]["causes"][0]["code"], "standard_pack_table_invalid",
        "{report}"
    );
    assert_eq!(
        step(report, "bundle")["failures"][0]["code"],
        "standard_pack_table_invalid"
    );
    assert_eq!(
        entries_of(&fixture.out("refused-table")),
        BTreeSet::from(["work".to_owned()]),
        "no product"
    );
}

// Fix round 1, the review's third point: `ui.json` is a root file of the standard package root
// (contracts/package-reference.md), which `place` places and sets aside when a zip carries none,
// so the build carries a commit's `ui.json` byte for byte, `build.json` lists it among the root
// files and it needs no revision; a `ui.json` its reader (`parse_standard_pack_ui`) refuses
// against the zip's `bundle.json` (a pack the bundle does not list) fails the build with the
// reader's refusal, `standard_pack_root_invalid` naming the file and `package_not_listed`.
#[test]
fn p2_ui_json_is_carried_and_a_refused_one_fails_the_build() {
    let fixture = Fixture::new();
    let ui_of = |package_id: &str| {
        json!({
            "schema_version": "actingcommand.ui.v1",
            "game": "neutral",
            "groups": [{"id": "daily", "title": {"en": "Daily"}}],
            "packs": [{
                "package_id": package_id,
                "title": {"en": "Pack"},
                "summary": {"en": "A neutral pack"},
                "group": "daily"
            }]
        })
    };
    fixture.write_json("ui.json", &ui_of(PACK_A));
    let ui = fs::read(fixture.repo().join("ui.json")).unwrap();
    let head = fixture.commit("display metadata");
    let built = fixture.build("ui", &[]);
    assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
    let entries = zip_entries(&fixture.out("ui").join(zip_name(&head)));
    assert_eq!(entries.get("ui.json"), Some(&ui));
    assert_eq!(declared_package(&entries), json!([3, 3]));
    let document = fixture.build_json("ui");
    assert_eq!(
        document["root_files"],
        json!([
            "applications.json",
            "bundle.json",
            "interfaces.json",
            "ui.json"
        ])
    );

    fixture.write_json("ui.json", &ui_of("neutral.build.unlisted"));
    fixture.commit("a pack the bundle does not list");
    let built = fixture.build("refused-ui", &[]);
    assert_eq!(built.exit_code(), 2, "{}", built.envelope_json());
    assert_eq!(error_code(&built), "resource_build_failed");
    let details = error_details(&built);
    let report = &details["report"];
    assert_eq!(report["failed_steps"], json!(["bundle"]), "{report}");
    let refusal = &step(report, "bundle")["failures"][0];
    assert_eq!(refusal["code"], "standard_pack_root_invalid", "{report}");
    assert_eq!(refusal["values"]["game"], "neutral");
    assert_eq!(
        refusal["values"]["path"],
        fixture.repo().join("ui.json").display().to_string()
    );
    assert_eq!(
        refusal["values"]["standard_pack_problem"],
        "package_not_listed"
    );
    assert_eq!(
        report["outcome"]["causes"][0]["code"],
        "standard_pack_root_invalid"
    );
    assert_eq!(
        entries_of(&fixture.out("refused-ui")),
        BTreeSet::from(["work".to_owned()]),
        "no product"
    );
}

// Fix round 1, the review's first and second points: one test per row of the revision table
// (`apps/actinglab/src/resource_verify/revision.rs`; contracts/resource-verify.md, "The `package`
// revision"). Each builds the zip entries `content_revision` reads: a base whose content needs
// the row's revision minus one (one member of a row below, in another pack), then the fixture
// with the row's member, which needs the row's revision, and the same fixture without it, which
// stays one lower. A conditional row's fixture without it is the same member where the condition
// does not hold.

/// The pack that holds a row's fixture, and the pack of the base.
const ROW_PACK: &str = "neutral.row";
const BASE_PACK: &str = "neutral.base";
/// A pack's task file, a selection-policy document of that task and its navigation projection.
const TASK_FILE: &str = "resources/operations/task/task.json";
const POLICY_FILE: &str = "resources/operations/task/policies/policy.json";
const NAVIGATION_FILE: &str = "resources/navigation/neutral.test.projection.json";

type Entry = (String, Vec<u8>);

fn json_entry(name: String, value: &Value) -> Entry {
    (name, serde_json::to_vec(value).unwrap())
}

/// The row pack's file `path` holding `value`.
fn in_row(path: &str, value: &Value) -> Entry {
    json_entry(format!("packs/{ROW_PACK}/{path}"), value)
}

fn task(value: &Value) -> Entry {
    in_row(TASK_FILE, value)
}

fn control(value: &Value) -> Entry {
    in_row("control.json", value)
}

fn policy(value: &Value) -> Entry {
    in_row(POLICY_FILE, value)
}

/// A task whose one operation is `operation`.
fn operation(operation: &Value) -> Entry {
    task(&json!({"operations": [operation]}))
}

/// The game root's `interrupts.json` with one entry `entry`.
fn interrupt(entry: &Value) -> Entry {
    json_entry(
        "interrupts.json".to_owned(),
        &json!({"interrupts": [entry]}),
    )
}

/// Entries whose content needs exactly `revision`, 3 to 11: nothing for 3, else one member of a
/// row of that revision in [`BASE_PACK`] (11 and 10 by root and pack files the rows name).
fn base(revision: u64) -> Vec<Entry> {
    let pack = |path: &str| format!("packs/{BASE_PACK}/{path}");
    let base_task = |value: Value| json_entry(pack(TASK_FILE), &value);
    match revision {
        3 => vec![],
        4 => vec![json_entry(pack("control.json"), &json!({"provides": []}))],
        5 => vec![base_task(json!({
            "operations": [{"click": {"kind": "single_touch_path_drag_v1"}}]
        }))],
        6 => vec![(pack("resources/loading.json"), b"{}\n".to_vec())],
        7 => vec![base_task(
            json!({"operations": [{"select": {"track_px": 4}}]}),
        )],
        8 => vec![base_task(json!({"operations": [{"read_list": {}}]}))],
        9 => vec![json_entry(
            pack(POLICY_FILE),
            &json!({"lower_bound_column": "floor"}),
        )],
        10 => vec![("notices/NOTICE.md".to_owned(), b"Notice.\n".to_vec())],
        11 => vec![base_task(json!({"field_lists": []}))],
        _ => panic!("no base for revision {revision}"),
    }
}

/// The revision `content_revision` gives the base of `below` with `entries`.
fn revision_of(below: u64, entries: &[Entry]) -> u64 {
    let zip = base(below)
        .into_iter()
        .chain(entries.iter().cloned())
        .collect::<BTreeMap<_, _>>();
    crate::resource_verify::revision::content_revision(&zip)
}

fn names_of(entries: &[Entry]) -> Vec<(&str, String)> {
    entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), String::from_utf8_lossy(bytes).into_owned()))
        .collect()
}

/// The row of `revision`: `with` needs it over a base one lower, and `without` stays one lower.
fn assert_row(revision: u64, with: &[Entry], without: &[Entry]) {
    let below = revision - 1;
    assert_eq!(revision_of(below, &[]), below, "the base of {below}");
    assert_eq!(
        revision_of(below, with),
        revision,
        "with: {:?}",
        names_of(with)
    );
    assert_eq!(
        revision_of(below, without),
        below,
        "without: {:?}",
        names_of(without)
    );
}

#[test]
fn p2_revision_4_remedy_of_a_mapping() {
    assert_row(
        4,
        &[task(
            &json!({"scheduling_outcome": {"mappings": [{"remedy": {}}]}}),
        )],
        &[task(&json!({"scheduling_outcome": {"mappings": [{}]}}))],
    );
}

#[test]
fn p2_revision_4_provides_of_control() {
    assert_row(
        4,
        &[control(&json!({"provides": []}))],
        &[control(&json!({}))],
    );
}

#[test]
fn p2_revision_5_path_drag_click() {
    let drag = json!({"kind": "single_touch_path_drag_v1"});
    let point = json!({"kind": "point"});
    assert_row(
        5,
        &[operation(&json!({"click": drag}))],
        &[operation(&json!({"click": point}))],
    );
    assert_row(
        5,
        &[in_row(
            NAVIGATION_FILE,
            &json!({"navigation": [{"click": drag}]}),
        )],
        &[in_row(
            NAVIGATION_FILE,
            &json!({"navigation": [{"click": point}]}),
        )],
    );
}

#[test]
fn p2_revision_6_tables_of_a_policy() {
    assert_row(6, &[policy(&json!({"tables": []}))], &[policy(&json!({}))]);
}

#[test]
fn p2_revision_6_table_value_source() {
    assert_row(
        6,
        &[policy(&json!({"gates": [{"value": {"source": "table"}}]}))],
        &[policy(&json!({"gates": [{"value": {"source": "field"}}]}))],
    );
}

#[test]
fn p2_revision_6_post_admission_ocr_of_a_schema_0_9_task_only() {
    let with_ocr =
        |schema: &str| task(&json!({"schema_version": schema, "post_admission_ocr": {}}));
    assert_row(6, &[with_ocr("0.9")], &[with_ocr("0.8")]);
    assert_row(6, &[with_ocr("0.9")], &[with_ocr("0.7")]);
    assert_row(
        6,
        &[with_ocr("0.9")],
        &[task(&json!({"schema_version": "0.9"}))],
    );
}

#[test]
fn p2_revision_6_expect_after_target_present() {
    assert_row(
        6,
        &[operation(
            &json!({"expect_after": {"target_present": ["t"]}}),
        )],
        &[operation(&json!({"expect_after": {}}))],
    );
}

#[test]
fn p2_revision_6_expect_after_target_gone() {
    assert_row(
        6,
        &[operation(&json!({"expect_after": {"target_gone": ["t"]}}))],
        &[operation(&json!({"expect_after": {}}))],
    );
}

#[test]
fn p2_revision_6_expect_after_loading() {
    assert_row(
        6,
        &[operation(&json!({"expect_after": {"loading": "off"}}))],
        &[operation(&json!({"expect_after": {}}))],
    );
}

#[test]
fn p2_revision_6_expect_after_wait_page() {
    assert_row(
        6,
        &[operation(
            &json!({"expect_after": {"wait_page": {"page_id": "p", "max_ms": 1}}}),
        )],
        &[operation(&json!({"expect_after": {}}))],
    );
}

#[test]
fn p2_revision_6_integer_compare_predicate() {
    assert_row(
        6,
        &[policy(
            &json!({"gates": [{"predicate": {"kind": "integer_compare"}}]}),
        )],
        &[policy(
            &json!({"gates": [{"predicate": {"kind": "string_in"}}]}),
        )],
    );
}

#[test]
fn p2_revision_6_part_of_an_ocr_integer_feature() {
    let feature = |integer: Value| {
        task(&json!({"candidate_layouts": [{"features": [
            {"value": "ocr_integer", "integer": integer}
        ]}]}))
    };
    assert_row(
        6,
        &[feature(
            json!({"format": "current_capacity", "part": "capacity"}),
        )],
        &[feature(json!({"format": "current_capacity"}))],
    );
}

#[test]
fn p2_revision_6_alternatives_of_a_color_probe() {
    assert_row(
        6,
        &[task(
            &json!({"color_probes": [{"id": "p", "alternatives": [[0, 0, 0]]}]}),
        )],
        &[task(&json!({"color_probes": [{"id": "p"}]}))],
    );
}

#[test]
fn p2_revision_6_alternatives_of_a_color_check() {
    assert_row(
        6,
        &[task(
            &json!({"anchors": [{"id": "a", "color_check": {"alternatives": [[0, 0, 0]]}}]}),
        )],
        &[task(&json!({"anchors": [{"id": "a", "color_check": {}}]}))],
    );
}

#[test]
fn p2_revision_6_busy_of_a_mapping() {
    assert_row(
        6,
        &[task(
            &json!({"scheduling_outcome": {"mappings": [{"busy": true}]}}),
        )],
        &[task(&json!({"scheduling_outcome": {"mappings": [{}]}}))],
    );
}

#[test]
fn p2_revision_6_end_pages_of_a_stage() {
    let with = json!({"phases": [{"end_pages": ["p"]}]});
    let without = json!({"phases": [{}]});
    assert_row(6, &[task(&with)], &[task(&without)]);
    assert_row(6, &[control(&with)], &[control(&without)]);
}

#[test]
fn p2_revision_6_entry_pages_of_a_stage() {
    let with = json!({"phases": [{"entry_pages": ["p"]}]});
    let without = json!({"phases": [{}]});
    assert_row(6, &[task(&with)], &[task(&without)]);
    assert_row(6, &[control(&with)], &[control(&without)]);
}

#[test]
fn p2_revision_6_still_frames_input_recheck() {
    assert_row(
        6,
        &[operation(
            &json!({"input_recheck": {"kind": "still_frames"}}),
        )],
        &[operation(
            &json!({"input_recheck": {"kind": "fresh_frame"}}),
        )],
    );
}

#[test]
fn p2_revision_6_input_recheck_of_a_linear_steps_pack_only() {
    let recheck = operation(&json!({"input_recheck": {"kind": "fresh_frame"}}));
    assert_row(
        6,
        &[
            control(&json!({"execution_mode": "linear_steps"})),
            recheck.clone(),
        ],
        &[
            control(&json!({"execution_mode": "navigable_route"})),
            recheck,
        ],
    );
}

#[test]
fn p2_revision_6_dictionary_of_an_unsigned_integer_value_only() {
    let reading = |value: Value| task(&json!({"resource_readings": [{"value": value}]}));
    assert_row(
        6,
        &[reading(
            json!({"type": "unsigned_integer", "dictionary": {}}),
        )],
        &[reading(
            json!({"type": "dictionary_entry", "dictionary": {"path": "d.json"}}),
        )],
    );
}

#[test]
fn p2_revision_6_agreement_of_a_left_side() {
    assert_row(
        6,
        &[task(
            &json!({"checks": [{"value_test": {"left": {"agreement": {}}}}]}),
        )],
        &[task(&json!({"checks": [{"value_test": {"left": {}}}]}))],
    );
}

#[test]
fn p2_revision_6_agreement_of_a_right_side() {
    assert_row(
        6,
        &[task(
            &json!({"checks": [{"value_test": {"right": {"agreement": {}}}}]}),
        )],
        &[task(&json!({"checks": [{"value_test": {"right": {}}}]}))],
    );
}

#[test]
fn p2_revision_6_agreement_of_a_reading_mark() {
    assert_row(
        6,
        &[task(&json!({"resource_readings": [{"agreement": {}}]}))],
        &[task(&json!({"resource_readings": [{}]}))],
    );
}

#[test]
fn p2_revision_6_max_runs_of_an_operation() {
    assert_row(
        6,
        &[operation(&json!({"max_runs": 3}))],
        &[operation(&json!({}))],
    );
}

#[test]
fn p2_revision_6_on_max_runs_of_an_operation() {
    assert_row(
        6,
        &[operation(&json!({"on_max_runs": "next_candidate"}))],
        &[operation(&json!({}))],
    );
}

#[test]
fn p2_revision_6_keyed_on_guard_refused_only() {
    assert_row(
        6,
        &[operation(
            &json!({"on_guard_refused": {"outcome_key": "k"}}),
        )],
        &[operation(&json!({"on_guard_refused": "next_candidate"}))],
    );
}

#[test]
fn p2_revision_6_luma_floor_of_an_anchor() {
    assert_row(
        6,
        &[task(&json!({"anchors": [{"id": "a", "luma_floor": {}}]}))],
        &[task(&json!({"anchors": [{"id": "a"}]}))],
    );
}

#[test]
fn p2_revision_6_luma_floor_of_a_verify_template() {
    assert_row(
        6,
        &[task(
            &json!({"verify_templates": [{"id": "v", "luma_floor": {}}]}),
        )],
        &[task(&json!({"verify_templates": [{"id": "v"}]}))],
    );
}

#[test]
fn p2_revision_6_confirm_of_a_select() {
    assert_row(
        6,
        &[operation(&json!({"select": {"confirm": "decision"}}))],
        &[operation(&json!({"select": {}}))],
    );
}

#[test]
fn p2_revision_6_wait_of_an_operation() {
    assert_row(
        6,
        &[operation(&json!({"wait": {"max_ms": 1}}))],
        &[operation(&json!({}))],
    );
}

#[test]
fn p2_revision_6_timestamp_ms_value_of_a_reading_mark() {
    let reading = |kind: &str| task(&json!({"resource_readings": [{"value": {"type": kind}}]}));
    assert_row(
        6,
        &[reading("timestamp_ms")],
        &[reading("unsigned_integer")],
    );
}

#[test]
fn p2_revision_6_limit_cause_of_an_interrupt() {
    assert_row(
        6,
        &[interrupt(&json!({"limit_cause": "environment"}))],
        &[interrupt(&json!({}))],
    );
}

#[test]
fn p2_revision_6_store_update_of_control() {
    assert_row(
        6,
        &[control(&json!({"store_update": {}}))],
        &[control(&json!({}))],
    );
}

#[test]
fn p2_revision_6_preprocess_of_an_ocr_target() {
    assert_row(
        6,
        &[task(
            &json!({"ocr_targets": [{"id": "o", "preprocess": {}}]}),
        )],
        &[task(&json!({"ocr_targets": [{"id": "o"}]}))],
    );
}

#[test]
fn p2_revision_6_preprocess_of_a_shared_read() {
    assert_row(
        6,
        &[task(
            &json!({"candidate_layouts": [{"shared_ocr": {"preprocess": {}}}]}),
        )],
        &[task(&json!({"candidate_layouts": [{"shared_ocr": {}}]}))],
    );
}

#[test]
fn p2_revision_6_result_of_an_interrupt() {
    assert_row(
        6,
        &[interrupt(&json!({"result": "application_update_required"}))],
        &[interrupt(&json!({}))],
    );
}

#[test]
fn p2_revision_6_loading_copy_of_a_pack() {
    let copy = |path: &str| (format!("packs/{ROW_PACK}/{path}"), b"{}\n".to_vec());
    assert_row(6, &[copy("resources/loading.json")], &[]);
    assert_row(6, &[copy("resources/loading/page.png")], &[]);
}

#[test]
fn p2_revision_7_anchor_milli_feature() {
    let feature =
        |value: &str| task(&json!({"candidate_layouts": [{"features": [{"value": value}]}]}));
    assert_row(7, &[feature("anchor_milli")], &[feature("ocr_integer")]);
}

#[test]
fn p2_revision_7_confirm_track_of_a_select() {
    assert_row(
        7,
        &[operation(&json!({"select": {"confirm": "track"}}))],
        &[operation(&json!({"select": {"confirm": "decision"}}))],
    );
}

#[test]
fn p2_revision_7_track_px_of_a_select() {
    assert_row(
        7,
        &[operation(&json!({"select": {"track_px": 4}}))],
        &[operation(&json!({"select": {}}))],
    );
}

#[test]
fn p2_revision_7_masked_anchor_of_a_repeated_anchor_layout() {
    let layout = |anchor: Value| {
        task(&json!({
            "candidate_layouts": [{"kind": "repeated_anchor", "anchor": "a"}],
            "anchors": [anchor]
        }))
    };
    assert_row(
        7,
        &[layout(json!({"id": "a", "mask": {}}))],
        &[layout(json!({"id": "a"}))],
    );
    assert_row(
        7,
        &[layout(json!({"id": "a", "scales": [1000]}))],
        &[layout(json!({"id": "a"}))],
    );
}

#[test]
fn p2_revision_8_read_list_of_an_operation() {
    assert_row(
        8,
        &[operation(&json!({"read_list": {}}))],
        &[operation(&json!({}))],
    );
}

#[test]
fn p2_revision_8_table_max_over_list_value_source() {
    assert_row(
        8,
        &[policy(
            &json!({"gates": [{"value": {"source": "table_max_over_list"}}]}),
        )],
        &[policy(&json!({"gates": [{"value": {"source": "field"}}]}))],
    );
}

#[test]
fn p2_revision_9_lower_bound_column() {
    assert_row(
        9,
        &[policy(
            &json!({"gates": [{"value": {"lower_bound_column": "floor"}}]}),
        )],
        &[policy(&json!({"gates": [{"value": {}}]}))],
    );
}

#[test]
fn p2_revision_10_notices_directory() {
    assert_row(
        10,
        &[("notices/NOTICE.md".to_owned(), b"Notice.\n".to_vec())],
        &[],
    );
}

#[test]
fn p2_revision_11_field_lists_of_a_task() {
    assert_row(
        11,
        &[task(&json!({"field_lists": []}))],
        &[task(&json!({}))],
    );
}

#[test]
fn p2_revision_12_goals_json() {
    assert_row(12, &[json_entry("goals.json".to_owned(), &json!({}))], &[]);
}
