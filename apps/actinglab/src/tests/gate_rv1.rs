// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 RV-1 (plan-rv §11, RV-1 lines 1-12; register §1.126; exec gate key 103):
// `actinglab resource verify --repo <clean checkout> --source-repository <owner>/<name> --work
// <new dir> --report <new file> [--commit <sha>] [--base <rev>]` checks one commit of a resource
// repository as its CI does, in one process, and writes a report bound to the commit, its tree,
// its base and the build. Every step runs (`tools`, `checkout`, `declarations`, `pins`,
// `admission`, `format`, `bundle`); only a failed `checkout` precondition leaves the content steps
// `not_run`. Every expected value comes from plan-rv §4 (the command), §5 (the steps), §8 (the
// codes) and §9 (the report). The repositories are neutral fixtures (game `neutral`, server
// `test`, two or three packs built from the TOOL-1 content directory), made with git in a
// temporary directory; every git call, the fixture's and the command's, runs with no system
// configuration and an empty global one, so the runner's configuration cannot change a result.

use super::*;
use crate::package_tool::gate_390_tool1::neutral_package;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const SOURCE_REPOSITORY: &str = "neutral-owner/neutral-resources";
const REPORT_SCHEMA: &str = "actinglab.resource-verify.v1";
const PACK_A: &str = "neutral.verify.a";
const PACK_B: &str = "neutral.verify.b";
const PACK_C: &str = "neutral.verify.c";
const STEPS: [&str; 7] = [
    "tools",
    "checkout",
    "declarations",
    "pins",
    "admission",
    "format",
    "bundle",
];
const CONTENT_STEPS: [&str; 5] = ["declarations", "pins", "admission", "format", "bundle"];
const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

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

/// A neutral resource repository `<temp>/repo` with the outputs of its runs beside it. The fields
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
        fixture.write_json("applications.json", &applications(None));
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
        self.write(relative, canonical(value));
    }

    fn read_json(&self, relative: &str) -> Value {
        serde_json::from_slice(&fs::read(self.repo().join(relative)).unwrap()).unwrap()
    }

    /// The TOOL-1 content directory as `packs/<package_id>/`, its control naming it; returns its
    /// task document.
    fn pack(&self, package_id: &str) -> Value {
        let task = neutral_package(&self.repo().join("packs").join(package_id));
        let control_path = format!("packs/{package_id}/control.json");
        let mut control = self.read_json(&control_path);
        control["package_id"] = json!(package_id);
        self.write_json(&control_path, &control);
        task
    }

    /// The arguments of one run: the repository, the source repository, the run's work
    /// directory and report, then `extra`.
    fn arguments(&self, run: &str, extra: &[&str]) -> Vec<String> {
        let mut arguments = vec![
            "--repo".to_owned(),
            self.repo().display().to_string(),
            "--source-repository".to_owned(),
            SOURCE_REPOSITORY.to_owned(),
            "--work".to_owned(),
            self.work(run).display().to_string(),
            "--report".to_owned(),
            self.report(run).display().to_string(),
        ];
        arguments.extend(extra.iter().map(|argument| (*argument).to_owned()));
        arguments
    }

    /// `actinglab --json resource verify` with the run's arguments, `before` going first.
    fn verify_with(&self, run: &str, before: &[&str], extra: &[&str]) -> CliResult {
        let mut arguments = vec!["--json".to_owned()];
        arguments.extend(before.iter().map(|argument| (*argument).to_owned()));
        arguments.extend(["resource".to_owned(), "verify".to_owned()]);
        arguments.extend(self.arguments(run, extra));
        run_cli(arguments, true)
    }

    fn verify(&self, run: &str, extra: &[&str]) -> CliResult {
        self.verify_with(run, &[], extra)
    }

    /// The report of a run: the answer's data on success and its details on failure, which must
    /// be the report file, byte for byte as JSON.
    fn report_of(&self, run: &str, result: &CliResult) -> Value {
        let envelope: Value = serde_json::from_str(&result.envelope_json()).unwrap();
        let answered = if envelope["ok"] == json!(true) {
            envelope["data"].clone()
        } else {
            envelope["error"]["details"].clone()
        };
        let written: Value = serde_json::from_slice(&fs::read(self.report(run)).unwrap())
            .unwrap_or_else(|error| panic!("the report of {run}: {error}"));
        assert_eq!(answered, written, "{}", result.envelope_json());
        written
    }

    /// The repository's files outside `.git`, `/`-separated.
    fn files(&self) -> BTreeSet<String> {
        let mut files = BTreeSet::new();
        let mut directories = vec![(self.repo(), String::new())];
        while let Some((directory, prefix)) = directories.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                if prefix.is_empty() && name == ".git" {
                    continue;
                }
                let relative = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                if entry.file_type().unwrap().is_dir() {
                    directories.push((entry.path(), relative));
                } else {
                    files.insert(relative);
                }
            }
        }
        files
    }
}

/// The canonical serialization `package fmt` writes.
fn canonical(value: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(value).unwrap())
}

/// The neutral applications table: game `neutral`, server `test`, with its default package.
fn applications(default_package_id: Option<&str>) -> Value {
    let mut server = json!({"application_id": "org.neutral.client", "label": "Test"});
    if let Some(package_id) = default_package_id {
        server["default_package_id"] = json!(package_id);
    }
    json!({
        "schema_version": "actingcommand.applications.v1",
        "game": "neutral",
        "servers": {"test": server}
    })
}

fn error_code(result: &CliResult) -> String {
    result
        .envelope
        .error
        .as_ref()
        .map(|error| error.code.clone())
        .unwrap_or_default()
}

fn step<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["step"] == name)
        .unwrap_or_else(|| panic!("no step {name}: {report}"))
}

fn first_failure<'a>(report: &'a Value, name: &str) -> &'a Value {
    &step(report, name)["failures"][0]
}

/// The report names exactly `failed` as failed steps; every other step ran and passed.
fn assert_failed_steps(report: &Value, failed: &[&str]) {
    assert_eq!(report["failed_steps"], json!(failed), "{report}");
    for name in STEPS {
        let expected = if failed.contains(&name) {
            "failed"
        } else {
            "passed"
        };
        assert_eq!(step(report, name)["result"], expected, "{name}: {report}");
    }
}

/// A precondition failure: the precondition's code tops the report, `tools` ran, `checkout`
/// failed and no content step ran.
fn assert_precondition(result: &CliResult, report: &Value, code: &str) {
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(error_code(result), code);
    assert_eq!(report["result"], "failed");
    assert_eq!(report["outcome"]["code"], code, "{report}");
    assert_eq!(report["failed_steps"], json!(["checkout"]), "{report}");
    assert_eq!(step(report, "tools")["result"], "passed");
    assert_eq!(step(report, "checkout")["result"], "failed");
    for name in CONTENT_STEPS {
        assert_eq!(step(report, name)["result"], "not_run", "{name}: {report}");
        assert_eq!(step(report, name)["failures"], json!([]), "{name}");
    }
}

/// One seeded commit: the step it fails, the seed, the steps that fail and the step's code.
type SeedCase = (
    &'static str,
    fn(&Fixture),
    &'static [&'static str],
    &'static str,
);

/// A fixture committed, changed by `seed` and committed again, then verified at that commit.
fn verify_seeded(run: &str, seed: impl FnOnce(&Fixture)) -> (i32, String, Value) {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    seed(&fixture);
    fixture.commit(run);
    let result = fixture.verify(run, &[]);
    let report = fixture.report_of(run, &result);
    (result.exit_code(), error_code(&result), report)
}

fn seed_invalid_declaration(fixture: &Fixture) {
    let mut exclusions = fixture.read_json("declaration-exclusions.json");
    exclusions["unexpected"] = json!(true);
    fixture.write_json("declaration-exclusions.json", &exclusions);
}

fn seed_committed_sha256(fixture: &Fixture) {
    fixture.write_json(
        &format!("packs/{PACK_A}/resources/notes/meta.json"),
        &json!({"sha256": ZERO_SHA256}),
    );
}

fn seed_refused_pack(fixture: &Fixture) {
    let path = format!("packs/{PACK_B}/control.json");
    let mut control = fixture.read_json(&path);
    control["entry_task_id"] = json!("absent_task");
    fixture.write_json(&path, &control);
}

fn seed_unformatted_json(fixture: &Fixture) {
    fixture.write(
        &format!("packs/{PACK_A}/resources/operations/resources.json"),
        r#"{"schema_version":"1.0","resources":[],"resource_count":0}"#,
    );
}

fn seed_unmatched_default(fixture: &Fixture) {
    fixture.write_json(
        "applications.json",
        &applications(Some("neutral.verify.absent")),
    );
}

// Line 1: a clean fixture at a commit passes with exit 0; the report carries the commit, its tree,
// the base, the build's head and executable SHA-256, and every step passed with its counts.
#[test]
fn rv1_a_clean_fixture_passes_with_its_bindings() {
    let fixture = Fixture::new();
    let head = fixture.commit("neutral fixture");
    let tree = fixture.git(&["rev-parse", "HEAD^{tree}"]);
    let result = fixture.verify("clean", &["--commit", &head]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = fixture.report_of("clean", &result);
    assert_eq!(report["schema_version"], REPORT_SCHEMA);
    assert_eq!(report["result"], "passed");
    assert_eq!(report["outcome"]["code"], "resource_verify_passed");
    assert_eq!(report["outcome"]["values"]["step_count"], 7);
    assert_eq!(report["repository"]["commit"], head.as_str());
    assert_eq!(report["repository"]["tree"], tree.as_str());
    assert_eq!(report["repository"]["base"], Value::Null);
    assert_eq!(report["repository"]["source_repository"], SOURCE_REPOSITORY);
    assert_eq!(report["repository"]["game"], "neutral");
    assert_eq!(
        report["tools"]["runtime_head"],
        env!("ACTINGCOMMAND_RUNTIME_HEAD")
    );
    let executable = std::env::current_exe().unwrap();
    assert_eq!(
        report["tools"]["executable_sha256"],
        crate::sha256::hex_sha256(&fs::read(executable).unwrap()).as_str()
    );
    assert_eq!(report["tools"]["build_manifest"], Value::Null);
    let steps = report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["step"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(steps, STEPS);
    assert_failed_steps(&report, &[]);
    for name in STEPS {
        assert_eq!(step(&report, name)["failures"], json!([]), "{name}");
    }
    // The counts of §9 for two packs of three JSON files each, the pack README and four root
    // inputs (applications, maintenance, declaration exclusions, manifest).
    assert_eq!(
        step(&report, "checkout")["counts"],
        json!({"files_checked": 11})
    );
    let declarations = &step(&report, "declarations")["counts"];
    assert_eq!(declarations["selected"], 9);
    assert_eq!(declarations["deleted"], 0);
    assert_eq!(declarations["valid"], 9);
    assert_eq!(declarations["excluded"], 0);
    assert_eq!(declarations["found_after_first"], 0);
    assert!(declarations["read_files"].as_u64().unwrap() > 0);
    assert!(declarations["read_bytes"].as_u64().unwrap() > 0);
    assert_eq!(step(&report, "pins")["counts"], json!({"files": 6}));
    assert_eq!(
        step(&report, "admission")["counts"],
        json!({"packs": 2, "admitted": 2})
    );
    assert_eq!(
        step(&report, "format")["counts"],
        json!({"packs": 2, "json_files": 6, "unformatted": 0})
    );
    let bundle = &step(&report, "bundle")["counts"];
    let pack_bytes = [PACK_A, PACK_B]
        .iter()
        .flat_map(|pack| {
            [
                "control.json",
                "resources/operations/resources.json",
                "resources/operations/task/task.json",
            ]
            .map(|file| format!("packs/{pack}/{file}"))
        })
        .map(|file| fs::metadata(fixture.repo().join(file)).unwrap().len())
        .sum::<u64>();
    let zip_name = format!("neutral-resources-{}.zip", &head[..7]);
    let zip = fixture.work("clean").join("out").join(&zip_name);
    let zip_sha256 = crate::sha256::hex_sha256(&fs::read(zip).unwrap());
    assert_eq!(bundle["packs"], 2);
    assert_eq!(bundle["files"], 6);
    assert_eq!(bundle["bytes"], pack_bytes);
    assert_eq!(bundle["root_files"], json!([]));
    assert_eq!(bundle["package_revision"], 3);
    assert_eq!(
        bundle["zip"],
        json!({"name": zip_name, "sha256": zip_sha256, "entries": 9})
    );
    assert_eq!(
        fs::read_to_string(fixture.work("clean").join("out").join("SHA256SUMS")).unwrap(),
        format!("{zip_sha256}  {zip_name}\n")
    );
}

// Line 2: one commit per step seeds one failure; each gives `resource_verify_failed` (exit 2)
// naming the step and its registered code, the step's first failure a cause of the top outcome
// with `step`. A refused pack also fails `bundle`, whose `package bundle` admits every copy.
#[test]
fn rv1_one_seeded_failure_per_step_names_the_step_and_its_code() {
    let cases: [SeedCase; 5] = [
        (
            "declarations",
            seed_invalid_declaration,
            &["declarations"],
            "resource_declaration_invalid",
        ),
        (
            "pins",
            seed_committed_sha256,
            &["pins"],
            "resource_verify_pin_committed",
        ),
        (
            "admission",
            seed_refused_pack,
            &["admission", "bundle"],
            "package_invalid",
        ),
        (
            "format",
            seed_unformatted_json,
            &["format"],
            "package_fmt_check_failed",
        ),
        (
            "bundle",
            seed_unmatched_default,
            &["bundle"],
            "package_bundle_default_package_missing",
        ),
    ];
    for (seeded, seed, failed, code) in cases {
        let (exit, error, report) = verify_seeded(seeded, seed);
        assert_eq!(exit, 2, "{seeded}: {report}");
        assert_eq!(error, "resource_verify_failed", "{seeded}");
        assert_eq!(report["result"], "failed");
        assert_eq!(report["outcome"]["code"], "resource_verify_failed");
        assert_eq!(
            report["outcome"]["values"]["failed_step_count"],
            failed.len(),
            "{seeded}"
        );
        assert_failed_steps(&report, failed);
        let first = first_failure(&report, seeded);
        assert_eq!(first["code"], code, "{seeded}: {report}");
        assert_eq!(first["values"]["step"], seeded, "{seeded}: {first}");
        let cause = &report["outcome"]["causes"][0];
        assert_eq!(cause["code"], code, "{seeded}");
        assert_eq!(cause["values"]["step"], seeded, "{seeded}");
        match seeded {
            "declarations" => {
                assert_eq!(first["values"]["path"], "declaration-exclusions.json");
                assert_eq!(first["values"]["json_pointer"], "/unexpected");
            }
            "pins" => {
                assert_eq!(
                    first["values"]["path"],
                    format!("packs/{PACK_A}/resources/notes/meta.json")
                );
                assert_eq!(first["values"]["json_pointer"], "/sha256");
                assert_eq!(first["values"]["member"], "sha256");
            }
            "admission" => {
                assert_eq!(first["values"]["path"], format!("packs/{PACK_B}"));
                assert_eq!(first_failure(&report, "bundle")["code"], "package_invalid");
            }
            "format" => {
                assert_eq!(first["values"]["path"], format!("packs/{PACK_A}"));
                assert_eq!(
                    first["values"]["unformatted"],
                    json!(["resources/operations/resources.json"])
                );
            }
            _ => {}
        }
    }
}

// Line 3: failures in two steps at once are both named, each a cause with its step; two invalid
// declarations are both named, the second with `found_after_first: true`.
#[test]
fn rv1_failures_in_two_steps_and_two_declarations_are_all_named() {
    let (exit, _, report) = verify_seeded("two-steps", |fixture| {
        seed_unformatted_json(fixture);
        seed_unmatched_default(fixture);
    });
    assert_eq!(exit, 2);
    assert_failed_steps(&report, &["format", "bundle"]);
    assert_eq!(report["outcome"]["values"]["failed_step_count"], 2);
    let causes = report["outcome"]["causes"].as_array().unwrap();
    assert_eq!(causes.len(), 2, "{report}");
    assert_eq!(causes[0]["code"], "package_fmt_check_failed");
    assert_eq!(causes[0]["values"]["step"], "format");
    assert_eq!(causes[1]["code"], "package_bundle_default_package_missing");
    assert_eq!(causes[1]["values"]["step"], "bundle");

    let fixture = Fixture::new();
    let base = fixture.commit("neutral fixture");
    fixture.write("drafts/one.json", "{}\n");
    fixture.write("drafts/two.json", "{}\n");
    fixture.commit("two undeclared documents");
    let result = fixture.verify("two-declarations", &["--base", &base]);
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let report = fixture.report_of("two-declarations", &result);
    assert_failed_steps(&report, &["declarations"]);
    let failures = step(&report, "declarations")["failures"]
        .as_array()
        .unwrap();
    assert_eq!(failures.len(), 2, "{report}");
    assert_eq!(failures[0]["code"], "resource_declaration_invalid");
    assert_eq!(failures[0]["values"]["path"], "drafts/one.json");
    assert_eq!(failures[0]["values"].get("found_after_first"), None);
    assert_eq!(failures[1]["code"], "resource_declaration_invalid");
    assert_eq!(failures[1]["values"]["path"], "drafts/two.json");
    assert_eq!(failures[1]["values"]["found_after_first"], true);
    assert_eq!(
        step(&report, "declarations")["counts"]["found_after_first"],
        1
    );
}

// Line 4: a modified tracked file, an untracked file, an ignored file under packs/ and a pack file
// whose bytes differ from its blob without git noticing are each `resource_verify_checkout_dirty`
// with the content steps `not_run`, and the report is written.
#[test]
fn rv1_a_dirty_checkout_is_refused_before_any_content_step() {
    let ignored = format!("packs/{PACK_A}/build.tmp");
    let control = format!("packs/{PACK_A}/control.json");
    let cases: [(&str, &str, &str); 4] = [
        ("tracked", "tracked_change", "maintenance.json"),
        ("untracked", "untracked", "scratch.txt"),
        ("ignored", "ignored_in_read_path", &ignored),
        ("blob", "blob_mismatch", &control),
    ];
    for (run, problem, path) in cases {
        let fixture = Fixture::new();
        fixture.commit("neutral fixture");
        match run {
            "tracked" => fixture.write("maintenance.json", "[ ]\n"),
            "untracked" => fixture.write("scratch.txt", "scratch\n"),
            "ignored" => fixture.write(&ignored, "build output\n"),
            _ => {
                fixture.write(&control, "{}\n");
                fixture.git(&["update-index", "--assume-unchanged", &control]);
            }
        }
        let result = fixture.verify(run, &[]);
        let report = fixture.report_of(run, &result);
        assert_precondition(&result, &report, "resource_verify_checkout_dirty");
        let values = &report["outcome"]["values"];
        assert_eq!(values["dirty_problem"], problem, "{run}: {report}");
        assert_eq!(values["first_path"], path, "{run}");
        assert_eq!(values["path_count"], 1, "{run}");
    }
}

// Line 5: `--commit` other than HEAD gives `resource_verify_checkout_invalid`; a `--base` that is
// not an ancestor of HEAD gives `resource_verify_base_invalid`.
#[test]
fn rv1_a_commit_other_than_head_or_a_base_off_its_history_is_refused() {
    let fixture = Fixture::new();
    let head = fixture.commit("neutral fixture");
    let other = "1111111111111111111111111111111111111111";
    let result = fixture.verify("commit", &["--commit", other]);
    let report = fixture.report_of("commit", &result);
    assert_precondition(&result, &report, "resource_verify_checkout_invalid");
    assert_eq!(
        report["outcome"]["values"]["checkout_problem"],
        "commit_mismatch"
    );
    assert_eq!(report["repository"]["commit"], head.as_str());

    let side = fixture.git(&["commit-tree", "HEAD^{tree}", "-m", "unrelated history"]);
    let result = fixture.verify("base", &["--base", &side]);
    let report = fixture.report_of("base", &result);
    assert_precondition(&result, &report, "resource_verify_base_invalid");
    assert_eq!(report["outcome"]["values"]["base_problem"], "not_ancestor");
    assert_eq!(report["outcome"]["values"]["base"], side.as_str());
    assert_eq!(
        report["repository"]["base"],
        json!({"rev": side, "commit": side})
    );
}

// Line 6: an existing `--report` or `--work` gives `resource_verify_output_exists` before anything
// is read (the repository given does not even exist); nothing is overwritten.
#[test]
fn rv1_an_existing_output_is_refused_before_anything_is_read() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let missing_repo = fixture.temp.path().join("no-such-repository");
    let arguments = |run: &str| {
        let mut arguments = fixture.arguments(run, &[]);
        arguments[1] = missing_repo.display().to_string();
        let mut all = vec![
            "--json".to_owned(),
            "resource".to_owned(),
            "verify".to_owned(),
        ];
        all.extend(arguments);
        all
    };

    fs::write(fixture.report("report"), b"kept").unwrap();
    let result = run_cli(arguments("report"), true);
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(error_code(&result), "resource_verify_output_exists");
    let details = result
        .envelope
        .error
        .as_ref()
        .unwrap()
        .details
        .clone()
        .unwrap();
    assert_eq!(
        details["values"]["path"],
        fixture.report("report").display().to_string()
    );
    assert_eq!(fs::read(fixture.report("report")).unwrap(), b"kept");
    assert!(!fixture.work("report").exists());

    fs::create_dir(fixture.work("work")).unwrap();
    fs::write(fixture.work("work").join("kept.txt"), b"kept").unwrap();
    let result = run_cli(arguments("work"), true);
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    assert_eq!(error_code(&result), "resource_verify_output_exists");
    assert_eq!(
        fs::read_dir(fixture.work("work")).unwrap().count(),
        1,
        "the work directory is unchanged"
    );
    assert_eq!(
        fs::read(fixture.work("work").join("kept.txt")).unwrap(),
        b"kept"
    );
    assert!(!fixture.report("work").exists());
}

// Line 7: global `--dry-run` gives `dry_run_unsupported`, `--instance` gives
// `offline_device_scope_forbidden` and an unknown flag `validation_failed`; nothing is written.
#[test]
fn rv1_dry_run_device_options_and_unknown_flags_are_refused() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let cases: [(&str, &[&str], &[&str], &str); 3] = [
        ("dry", &["--dry-run"], &[], "dry_run_unsupported"),
        (
            "instance",
            &["--instance", "neutral"],
            &[],
            "offline_device_scope_forbidden",
        ),
        (
            "unknown",
            &[],
            &["--unknown-flag", "value"],
            "validation_failed",
        ),
    ];
    for (run, before, extra, code) in cases {
        let result = fixture.verify_with(run, before, extra);
        assert_eq!(result.exit_code(), 2, "{run}: {}", result.envelope_json());
        assert_eq!(error_code(&result), code, "{run}");
        assert!(!fixture.report(run).exists(), "{run}");
        assert!(!fixture.work(run).exists(), "{run}");
    }
}

// Line 8: with `--base`, the declaration selection adds `git diff --no-renames --name-status -z
// base HEAD` over `*.json` / `*.yaml` / `*.yml`, and a deleted declaration is validated as a
// removal; without it, the selection is every tracked `packs/**/*.json` and the root declaration
// files present.
#[test]
fn rv1_the_declaration_selection_follows_git_with_and_without_a_base() {
    let fixture = Fixture::new();
    fixture.pack(PACK_C);
    let base = fixture.commit("three packs");
    fixture.git(&["rm", "-r", "-q", &format!("packs/{PACK_C}")]);
    fixture.write("notes/added.json", "{}\n");
    fixture.commit("one pack removed, one note added");

    let result = fixture.verify("without-base", &[]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = fixture.report_of("without-base", &result);
    let counts = &step(&report, "declarations")["counts"];
    assert_eq!(counts["selected"], 9, "{report}");
    assert_eq!(counts["deleted"], 0);
    assert_eq!(counts["valid"], 9);
    assert_eq!(counts["excluded"], 0);

    let result = fixture.verify("with-base", &["--base", &base]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = fixture.report_of("with-base", &result);
    assert_eq!(report["repository"]["base"]["commit"], base.as_str());
    let counts = &step(&report, "declarations")["counts"];
    assert_eq!(counts["selected"], 13, "{report}");
    assert_eq!(counts["deleted"], 3);
    assert_eq!(counts["valid"], 9);
    assert_eq!(counts["excluded"], 1);
    assert_eq!(step(&report, "declarations")["result"], "passed");
}

// Line 9: the bundle step checks the installed root `<work>/installed/<game>/<package_id>/`; a pack
// whose remedy names a capability no pack provides yields a `package_bundle_remedy_invalid`
// warning, and the step stays passed.
#[test]
fn rv1_the_bundle_step_checks_the_installed_root() {
    let fixture = Fixture::new();
    let mut task = fixture.pack(PACK_A);
    task["schema_version"] = json!("0.9");
    task["scheduling_outcome"] = json!({"mappings": [{
        "outcome_key": "blocked_k",
        "effect": "no_designated_effect",
        "terminal_pages": ["terminal"],
        "remedy": {"need": "need_neutral", "capabilities": ["capability_neutral"]}
    }]});
    fixture.write_json(
        &format!("packs/{PACK_A}/resources/operations/task/task.json"),
        &task,
    );
    let control_path = format!("packs/{PACK_A}/control.json");
    let mut control = fixture.read_json(&control_path);
    control["schema_version"] = json!("Lab-1y.control.v2");
    fixture.write_json(&control_path, &control);
    fixture.commit("a remedy no pack provides");
    let result = fixture.verify("remedy", &[]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = fixture.report_of("remedy", &result);
    let bundle = step(&report, "bundle");
    assert_eq!(bundle["result"], "passed");
    let remedy = bundle["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|warning| warning["code"] == "package_bundle_remedy_invalid")
        .collect::<Vec<_>>();
    assert_eq!(remedy.len(), 1, "{bundle}");
    assert_eq!(remedy[0]["values"]["package_id"], PACK_A);
    assert_eq!(
        remedy[0]["values"]["remedy_problem"],
        "capability_unprovided"
    );
    let installed = fixture.work("remedy").join("installed").join("neutral");
    assert!(installed.join("bundle.json").is_file());
    for pack in [PACK_A, PACK_B] {
        assert!(
            installed.join(pack).join("control.json").is_file(),
            "{pack}"
        );
    }
}

// Line 10: a `BUILD-MANIFEST.json` beside the executable naming other bytes gives
// `resource_verify_tools_mismatch` (the executable given through the test seam).
#[test]
fn rv1_a_build_manifest_naming_other_bytes_is_a_tools_mismatch() {
    let fixture = Fixture::new();
    fixture.commit("neutral fixture");
    let tools = fixture.temp.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let executable = tools.join("actinglab.exe");
    fs::write(&executable, b"neutral tools build\n").unwrap();
    let manifest = tools.join("BUILD-MANIFEST.json");
    fs::write(
        &manifest,
        canonical(&json!({
            "repository": "HS7097/ActingCommand-Runtime",
            "commit_sha": env!("ACTINGCOMMAND_RUNTIME_HEAD"),
            "files": [{"path": "actinglab.exe", "size_bytes": 20, "sha256": ZERO_SHA256}]
        })),
    )
    .unwrap();
    let global = GlobalOptions {
        json: true,
        ..GlobalOptions::default()
    };
    let flags = FlagArgs::parse(&fixture.arguments("tools", &[])).unwrap();
    let error = crate::resource_verify::verify_with_executable(&global, &flags, Ok(executable))
        .expect_err("a tools mismatch fails the run");
    assert_eq!(error.code, "resource_verify_failed");
    let report = error.details.unwrap();
    assert_failed_steps(&report, &["tools"]);
    let first = first_failure(&report, "tools");
    assert_eq!(first["code"], "resource_verify_tools_mismatch");
    assert_eq!(first["values"]["tools_problem"], "executable_sha256");
    assert_eq!(
        first["values"]["manifest_path"],
        manifest.display().to_string()
    );
    assert_eq!(
        report["tools"]["executable_sha256"],
        crate::sha256::hex_sha256(b"neutral tools build\n").as_str()
    );
    assert_eq!(
        report["tools"]["build_manifest"]["actinglab_sha256"],
        ZERO_SHA256
    );
}

// Line 11: the capability, help and dry-run tables list `resource verify`; its capability entry
// carries `report_schema: "actinglab.resource-verify.v1"`.
#[test]
fn rv1_capability_help_and_dry_run_tables_list_resource_verify() {
    let capability = command_capabilities()
        .into_iter()
        .find(|entry| entry["command"] == "resource verify")
        .expect("resource verify capability");
    assert_eq!(capability["status"], "available");
    assert_eq!(capability["needs"], json!(["offline"]));
    assert_eq!(capability["report_schema"], REPORT_SCHEMA);
    assert_eq!(capability["dry_run_mode"], "refused");
    assert_eq!(capability["dry_run_refusal_code"], "dry_run_unsupported");
    let options = help_data()["command_options"]["resource verify"].clone();
    for flag in [
        "--repo",
        "--source-repository",
        "--work",
        "--report",
        "--commit",
        "--base",
    ] {
        assert!(
            options
                .as_array()
                .unwrap()
                .iter()
                .any(|option| option.as_str().unwrap().starts_with(flag)),
            "{flag}: {options}"
        );
    }
    let listed = run_cli(["--json", "list", "commands"], true);
    assert_eq!(listed.exit_code(), 0);
    assert!(
        listed.envelope.data.as_ref().unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["command"] == "resource verify")
    );
    let contract = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/actinglab-dry-run.md"),
    )
    .unwrap();
    assert!(
        contract.contains("| `resource verify` | refused | `dry_run_unsupported` |"),
        "contracts/actinglab-dry-run.md lists resource verify"
    );
}

// Line 12: no list file is written into `--repo`: after a run with `--base`, `git status
// --porcelain --untracked-files=all --ignored` and the files of the fixture are unchanged.
#[test]
fn rv1_no_list_file_is_written_into_the_repository() {
    let fixture = Fixture::new();
    let base = fixture.commit("neutral fixture");
    fixture.write("notes/added.json", "{}\n");
    fixture.commit("one note added");
    let status = || {
        fixture.git(&[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored",
        ])
    };
    let status_before = status();
    let files_before = fixture.files();
    let result = fixture.verify("listless", &["--base", &base]);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    assert_eq!(status(), status_before);
    assert_eq!(fixture.files(), files_before);
    let git_dir = fixture.repo().join(".git");
    assert!(!git_dir.join("resource-changed-paths").exists());
    assert!(
        !fixture
            .repo()
            .join(".localci-resource-changed-paths")
            .exists()
    );
}
