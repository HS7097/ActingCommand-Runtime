// SPDX-License-Identifier: AGPL-3.0-only

// #463 S3-7 (register §1.414; contracts/resource-verify.md, "`standard-pack build`", the answer;
// the design rd-1115/pack-stage3-design.md §4.2 "S3-7" with the coordinator's rulings in the work
// order rd-1115/brief-463-s37-build-readiness.md): besides what it answered, `standard-pack build`
// answers `readiness`, the readiness of the installed root its steps laid out and kept,
// `<out>/work/installed/<game>/`, as `standard-pack ready --root` answers it for that root, and
// `next.place`, the words of the command line of the placement's plan of the files it wrote;
// `build.json` does not change. The repositories are neutral fixtures as `gate_464_p2`'s (game `neutral`, server
// `test`, two packs built from the TOOL-1 content directory) with a `schedule.json` whose one pool
// has a valuation and whose rules are written as `gate_464_p3` writes a rule with a complete
// value, made with git in a temporary directory, every git call with no system configuration and
// an empty global one. No clock is read and nothing sleeps.
// - `s37_a_held_pack_in_the_commit_answers_waiting_held`: pack A held (a held entry, no rule) and
//   pack B ruled with a value: A `waiting`, its S2 failing `held`, B `ready_to_place`; the answer
//   of `standard-pack ready --root` for the kept root; `build.json` the answer's `build`, v1, its
//   eleven members;
// - `s37_a_commit_whose_packs_are_all_valued_answers_ready_to_place`: both packs ruled with a
//   value: both `ready_to_place`, their installation items `not_judged` `installation_not_read`;
// - `s37_next_place_is_the_plan_line_of_the_files_written`: `next` is the plan's line of the zip
//   and `SHA256SUMS` written, with `--plan`; fix round 2: its five words after the program's name;
// - `s37_a_refused_readiness_is_a_warning_of_the_answer` (fix round 2): what the answer says of a
//   root the readiness function refuses: no `readiness`, the refusal in `readiness_refused`, and in
//   `warnings` one `resource_build_readiness_refused` caused by the refusal's code.

use super::*;
use crate::package_tool::gate_390_tool1::neutral_package;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

const SOURCE_REPOSITORY: &str = "neutral-owner/neutral-resources";
const PACK_A: &str = "neutral.build.a";
const PACK_B: &str = "neutral.build.b";
/// The pool every rule produces into, with a valuation.
const VALUED: &str = "pool.valued";
/// The one `requires` word of the rules (as `gate_464_p3` writes it).
const SUBSYSTEM: &str = "fixture_subsystem";

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

/// A neutral resource repository `<temp>/repo` and the build's `--out` `<temp>/out` beside it.
/// The fields drop in order, the environment lock last.
struct Fixture {
    temp: TempDir,
    _git: GitConfiguration,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    /// The repository with `gate_464_p2`'s root inputs, packs A and B and `schedule` as its
    /// `schedule.json`, committed; the fixture and the commit.
    fn committed(schedule: &Value) -> (Self, String) {
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
        fixture.add_pack(PACK_A);
        fixture.add_pack(PACK_B);
        fixture.write_json("schedule.json", schedule);
        let head = fixture.commit("neutral fixture with a schedule");
        (fixture, head)
    }

    fn repo(&self) -> PathBuf {
        self.temp.path().join("repo")
    }

    /// The build's `--out`.
    fn out(&self) -> PathBuf {
        self.temp.path().join("out")
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
    fn add_pack(&self, package_id: &str) {
        neutral_package(&self.repo().join("packs").join(package_id));
        let control_path = format!("packs/{package_id}/control.json");
        let mut control: Value =
            serde_json::from_slice(&fs::read(self.repo().join(&control_path)).unwrap()).unwrap();
        control["package_id"] = json!(package_id);
        self.write_json(&control_path, &control);
    }

    /// `actinglab --json standard-pack build` of the commit into [`Self::out`], which must pass;
    /// its answer.
    fn build(&self) -> Value {
        let built = run_cli(
            [
                "--json".to_owned(),
                "standard-pack".to_owned(),
                "build".to_owned(),
                "--repo".to_owned(),
                self.repo().display().to_string(),
                "--source-repository".to_owned(),
                SOURCE_REPOSITORY.to_owned(),
                "--out".to_owned(),
                self.out().display().to_string(),
            ],
            true,
        );
        assert_eq!(built.exit_code(), 0, "{}", built.envelope_json());
        built.envelope.data.clone().unwrap()
    }
}

/// `actinglab --json standard-pack ready --root <root>`, which must pass: its answer.
fn ready(root: &Path) -> Value {
    let judged = run_cli(
        [
            "--json".to_owned(),
            "standard-pack".to_owned(),
            "ready".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
        ],
        true,
    );
    assert_eq!(judged.exit_code(), 0, "{}", judged.envelope_json());
    judged.envelope.data.clone().unwrap()
}

/// The installed root the build's steps laid out, as the answer's `files.work` names its work
/// directory: `<work>/installed/neutral`.
fn installed(answer: &Value) -> PathBuf {
    PathBuf::from(answer["files"]["work"].as_str().unwrap())
        .join("installed")
        .join("neutral")
}

/// The rule `<package_id>.rule` of `package_id` on an hourly clock, producing one unit into the
/// valued pool: a rule with a complete value (as `gate_464_p3` writes it).
fn rule(package_id: &str) -> Value {
    let id = format!("{package_id}.rule");
    json!({
        "id": id.as_str(),
        "package_id": package_id,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3600000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id.as_str(),
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": [],
        "produces": [{
            "pool_id": VALUED,
            "direction": "produce",
            "amount": 1,
            "observation_source": "scan_verified",
            "confidence_milli": 1000
        }],
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86400000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}

/// `schedule.json` with `rules`, the held entries `held` and the valued pool (as `gate_464_p3`
/// writes it).
fn schedule(rules: &[Value], held: &[Value]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [SUBSYSTEM],
        "rules": rules,
        "pools": [{
            "id": VALUED,
            "capacity": 1000,
            "projection": {"kind": "none"},
            "observation": {"kind": "fact", "fact_key": "resource.valued"},
            "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
            "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
            "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000}
        }],
        "activity": [],
        "timeline": [],
        "held": held
    })
}

/// The readiness row of the pack `package_id`.
fn row<'a>(readiness: &'a Value, package_id: &str) -> &'a Value {
    readiness["packs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pack| pack["package_id"] == package_id)
        .unwrap_or_else(|| panic!("no pack {package_id}: {readiness}"))
}

/// The item `id` (`S1` to `S7`, `I1` to `I6`) of a pack's readiness row.
fn item<'a>(pack: &'a Value, id: &str) -> &'a Value {
    pack["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|answer| answer["id"] == id)
        .unwrap_or_else(|| panic!("no item {id}: {pack}"))
}

// The work order's first test, with the coordinator's ruling 1: a commit whose pack A is held (a
// held entry and no rule) and whose pack B has a rule with a complete value builds, and the
// answer's readiness lists A `waiting`, its S2 `classified` failing with `package_dispatch_waiting`
// (`readiness_problem` `held`), and B `ready_to_place`: one of each, nothing dispatchable, the
// installation `not_read`, no refusal. It is the answer `standard-pack ready --root` gives for the
// installed root the build kept, `<files.work>/installed/neutral`, which still exists.
// `build.json` is the answer's `build` and is as before: schema v1, its eleven members, no
// readiness.
#[test]
fn s37_a_held_pack_in_the_commit_answers_waiting_held() {
    let (fixture, _) = Fixture::committed(&schedule(
        &[rule(PACK_B)],
        &[json!({"package_id": PACK_A, "reason": "the fixture holds it"})],
    ));
    let answer = fixture.build();
    assert!(answer.get("readiness_refused").is_none(), "{answer}");
    let readiness = &answer["readiness"];
    let root = installed(&answer);
    assert!(
        root.is_dir(),
        "the build keeps its installed root: {answer}"
    );
    assert_eq!(readiness["root"], root.display().to_string());
    assert_eq!(readiness["game"], "neutral");
    assert_eq!(readiness["installation"], "not_read");
    assert_eq!(readiness["dispatchable"], 0, "{readiness}");
    assert_eq!(readiness["ready_to_place"], 1, "{readiness}");
    assert_eq!(readiness["waiting"], 1, "{readiness}");
    assert_eq!(readiness["not_scheduled"], 0, "{readiness}");

    let held = row(readiness, PACK_A);
    assert_eq!(held["server"], "test");
    assert_eq!(held["readiness"], "waiting", "{held}");
    let classified = item(held, "S2");
    assert_eq!(classified["item"], "classified");
    assert_eq!(classified["state"], "fails", "{classified}");
    let outcome = &classified["outcome"];
    assert_eq!(outcome["code"], "package_dispatch_waiting", "{outcome}");
    assert_eq!(outcome["values"]["package_id"], PACK_A, "{outcome}");
    assert_eq!(
        outcome["values"]["readiness_item"], "classified",
        "{outcome}"
    );
    assert_eq!(outcome["values"]["readiness_problem"], "held", "{outcome}");
    let valued = row(readiness, PACK_B);
    assert_eq!(valued["readiness"], "ready_to_place", "{valued}");

    assert_eq!(
        *readiness,
        ready(&root),
        "the answer of standard-pack ready --root for the kept root"
    );

    let document: Value =
        serde_json::from_slice(&fs::read(fixture.out().join("build.json")).unwrap()).unwrap();
    assert_eq!(answer["build"], document);
    assert_eq!(
        document["schema_version"],
        "actingcommand.standard-pack-build.v1"
    );
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
}

// The work order's second test: a commit whose two packs each have a rule with a complete value
// answers both `ready_to_place`, two, none waiting, none dispatchable, none `not_scheduled`: no
// installation is read, so the five installation items of each pack are `not_judged` with
// `installation_not_read`. It is the answer `standard-pack ready --root` gives for the installed
// root the build kept.
#[test]
fn s37_a_commit_whose_packs_are_all_valued_answers_ready_to_place() {
    let (fixture, _) = Fixture::committed(&schedule(&[rule(PACK_A), rule(PACK_B)], &[]));
    let answer = fixture.build();
    assert!(answer.get("readiness_refused").is_none(), "{answer}");
    let readiness = &answer["readiness"];
    assert_eq!(readiness["ready_to_place"], 2, "{readiness}");
    assert_eq!(readiness["waiting"], 0, "{readiness}");
    assert_eq!(readiness["dispatchable"], 0, "{readiness}");
    assert_eq!(readiness["not_scheduled"], 0, "{readiness}");
    for package_id in [PACK_A, PACK_B] {
        let pack = row(readiness, package_id);
        assert_eq!(pack["readiness"], "ready_to_place", "{pack}");
        for id in ["I1", "I2", "I4", "I5", "I6"] {
            let answered = item(pack, id);
            assert_eq!(answered["state"], "not_judged", "{answered}");
            assert_eq!(
                answered["outcome"]["values"]["readiness_unjudged"], "installation_not_read",
                "{answered}"
            );
        }
    }
    assert_eq!(*readiness, ready(&installed(&answer)));
}

// The work order's third test, with the coordinator's ruling 4: `next` is `{place}` alone, the
// command line of the placement's plan of the zip and `SHA256SUMS` the build wrote into `--out`,
// the two paths those of `files`, and `--plan`: never the line that places. Fix round 2: the line
// is the five words of the Lab's command line after the program's name, as every `next` of the
// Lab holds a command, each path as the build wrote it, unquoted.
#[test]
fn s37_next_place_is_the_plan_line_of_the_files_written() {
    let (fixture, head) = Fixture::committed(&schedule(&[rule(PACK_A), rule(PACK_B)], &[]));
    let answer = fixture.build();
    let zip = fixture
        .out()
        .join(format!("neutral-resources-{}.zip", &head[..7]));
    let sums = fixture.out().join("SHA256SUMS");
    assert!(zip.is_file(), "{answer}");
    assert!(sums.is_file(), "{answer}");
    assert_eq!(answer["files"]["zip"], zip.display().to_string());
    assert_eq!(answer["files"]["sums"], sums.display().to_string());
    assert_eq!(
        answer["next"],
        json!({"place": [
            "place",
            zip.display().to_string(),
            "--sums",
            sums.display().to_string(),
            "--plan"
        ]})
    );
}

// Fix round 2 (the review's point C): when the readiness function refuses the installed root, the
// answer says so in its warnings too. No fixture makes the function refuse the root after the
// `bundle` step passed: the step lays the root out and checks it with `standard-pack check`, whose
// structure check the function runs, and nothing of the build runs between the step and the
// judgement. So the function that builds the answer's readiness members is driven directly, on two
// roots the readiness function itself refuses: one that is not there (`validation_failed`) and an
// empty one, which holds no `bundle.json` (`standard_pack_root_invalid`). Each answers no
// `readiness`, the refusal with the function's own code in `readiness_refused`, and in `warnings`
// exactly one `{outcome}`: `resource_build_readiness_refused`, a warning, `path` the root, caused
// by the refusal's code.
#[test]
fn s37_a_refused_readiness_is_a_warning_of_the_answer() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let gone = temp.path().join("gone").join("neutral");
    let empty = temp.path().join("empty").join("neutral");
    fs::create_dir_all(&empty).unwrap();
    for (root, code) in [
        (gone, "validation_failed"),
        (empty, "standard_pack_root_invalid"),
    ] {
        let members =
            serde_json::to_value(crate::resource_verify::readiness_members(root.clone())).unwrap();
        assert!(members.get("readiness").is_none(), "{members}");
        assert_eq!(members["readiness_refused"]["code"], code, "{members}");
        let warnings = members["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 1, "{members}");
        let warning = &warnings[0]["outcome"];
        assert_eq!(
            warning["code"], "resource_build_readiness_refused",
            "{warning}"
        );
        assert_eq!(warning["category"], "warning", "{warning}");
        assert_eq!(
            warning["values"]["path"],
            root.to_string_lossy().into_owned(),
            "{warning}"
        );
        assert_eq!(warning["causes"][0]["code"], code, "{warning}");
        assert_eq!(warning["causes"][0]["relation"], "caused_by", "{warning}");
    }
}
