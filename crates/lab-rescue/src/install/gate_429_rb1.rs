// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 RB-1 (register §1.514; contracts/install-cli.md, "Update and rollback", the plan's
// "Subsystems the schedules require"; contracts/standard-pack-schedule.md, "`requires`"): the plan
// of an update or a rollback refuses `interface_edge` when the target's own check-config answer
// does not name a subsystem that a schedule a start of the target would read lists in `requires`
// (the `schedule.json` of each game in use, each fragment of the active catalogue generation): its
// one failed edge row is `requires` `next_names_required`, and its document's `requires_missing`
// names each game, file or generation and each missing name. When nothing is missing the plan
// answers as on main. The cases are the work order's tests 1 to 5:
// - `rb1_t1_...`: a rollback to a build that does not name `catalog_business`, which the game root
//   in use requires: refused, the game, its file and the name listed;
// - `rb1_t2_...`: the same for a deploy (an update) to an older build, with `pool_list_rows`; its
//   apply refuses the same and changes nothing;
// - `rb1_t3_...`: a game root that requires only what the target names while the active
//   generation's fragment requires `catalog_business`: refused, the generation named; once another
//   generation is active (the first one still on disk): planned, the gate passing;
// - `rb1_t4_...`: a target whose answer names no subsystems (no `config_manifest`): a game root
//   that requires `pool_lines` refuses it; with game roots that require nothing it is planned as on
//   main;
// - `rb1_t5_...`: ruling 5's golden, IT-5's update from C1 to C2 on the neutral builds: the answer's
//   members, the plan document's members, every edge row, the notice and no warning as on main; the
//   same for a root whose game root and active generation require only names the target names.
// Fix round 1 (the independent review of #1876 and the coordinator's rulings on it):
// - `rb1_fr1_a_target_that_refuses_...`: a target whose check-config refuses the configuration is
//   not judged: the rollback is refused in the gate's part `config` and the update
//   `config_refused_by_target`, as on main, with no `requires` row and no `requires_missing`;
// - `rb1_fr1_a_schedule_or_a_fragment_...`: a `schedule.json` and a fragment that are there and
//   cannot be read refuse, each entry saying why (`unread`);
// - `rb1_fr1_a_subsystem_listed_as_not_enabled_...`: a row `enabled: false` names nothing;
// - `rb1_fr1_two_answers_naming_...`: the target's and the live build's answers preview two active
//   generations: both are read.
// The machine is the fake host of `fixture.rs`; every root is a temporary folder with neutral files.

use super::fixture::{BuildSpec, C1, C2, FakeHost, code, live_root, run, word, write_json};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The game the configuration binds its one instance to.
const GAME: &str = "fixture-game-a";
const INSTANCE: &str = "neutral.a";

/// The answer's outcome value `key`, when it is a string.
fn text<'a>(answer: &'a super::Answer, key: &str) -> Option<&'a str> {
    answer.outcome.value(key).and_then(Value::as_str)
}

/// Asserts `answer` is `install_command_done` of the command `operation`.
fn assert_done(answer: &super::Answer, operation: &str) {
    assert_eq!(code(answer), "install_command_done", "{answer:?}");
    assert_eq!(text(answer, "operation"), Some(operation), "{answer:?}");
}

/// Asserts `answer` is `install_command_refused` of the command `operation` for `why`.
fn assert_refused(answer: &super::Answer, operation: &str, why: &str) {
    assert_eq!(code(answer), "install_command_refused", "{answer:?}");
    assert_eq!(text(answer, "operation"), Some(operation), "{answer:?}");
    assert_eq!(text(answer, "install_refusal"), Some(why), "{answer:?}");
}

/// A root whose live build is `live` (adopted) and that keeps `target` (staged from its release
/// folder as a local build), as IT-5 prepares one.
fn prepared(
    base: &Path,
    name: &str,
    live: &BuildSpec,
    target: &BuildSpec,
    host: &FakeHost,
) -> PathBuf {
    let root = live_root(base, name, live);
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_done(&adopted, "install_adopt");
    let release = base.join(format!("{name}-release"));
    target.write_release(&release);
    let staged = run(
        host,
        &[
            "release",
            "stage",
            "--root",
            word(&root),
            "--dir",
            word(&release),
            "--local-build",
        ],
    );
    assert_done(&staged, "install_release_stage");
    root
}

fn plan_update(host: &FakeHost, root: &Path) -> super::Answer {
    run(
        host,
        &["update", "plan", "--root", word(root), "--build", C2],
    )
}

fn plan_rollback(host: &FakeHost, root: &Path) -> super::Answer {
    run(
        host,
        &["rollback", "plan", "--root", word(root), "--to", C1],
    )
}

/// A root updated from C1 to C2 (S0 = 100, the head 150 after it), keeping C1 for a rollback, as
/// IT-12 prepares one.
fn updated(base: &Path, host: &FakeHost, name: &str) -> PathBuf {
    host.programs.ledger_head.set(100);
    let root = prepared(base, name, &BuildSpec::new(C1), &BuildSpec::new(C2), host);
    let planned = plan_update(host, &root);
    assert_done(&planned, "install_update_plan");
    let given = planned.members["plan_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a plan id: {planned:?}"))
        .to_owned();
    let applied = run(
        host,
        &[
            "update",
            "apply",
            "--root",
            word(&root),
            "--build",
            C2,
            "--plan",
            given.as_str(),
        ],
    );
    assert_done(&applied, "install_update_apply");
    host.programs.ledger_head.set(150);
    root
}

/// A start of the kept build C1 on `root` after its adopt whose ledger open took 8 000 ms: the
/// target's own open, so the rollback gate's open-time part passes (IT-12's passing case).
fn own_start_log(root: &Path) {
    let state: Value = serde_json::from_slice(
        &fs::read(root.join("install").join("state.json")).expect("state.json"),
    )
    .expect("state JSON");
    let adopted = state["builds"]
        .as_array()
        .expect("builds")
        .iter()
        .find(|build| build["origin"] == "adopted")
        .and_then(|build| build["operation_id"].as_str())
        .and_then(|id| id.strip_prefix("op-"))
        .and_then(|digits| digits.parse::<u64>().ok())
        .expect("the adopt");
    let line = json!({
        "schema_version": "actingcommand.outcome.v1",
        "code": "ledger_opened",
        "category": "info",
        "values": {
            "event_count": 4096, "artifact_count": 0, "artifact_bytes": 0, "worker_count": 1,
            "sql_read_ms": 1000, "verify_ms": 1000, "material_ms": 1000, "restore_ms": 1000,
            "meta_ms": 1000, "tail_ms": 1000, "index_ms": 1000, "import_ms": 1000,
            "timeout_ms": 600_000
        }
    });
    fs::write(
        root.join(format!("actingd-{}.log", adopted + 1)),
        format!("INFO actingd: opened\nOUTCOME actingd: {line}\n"),
    )
    .expect("the start log");
}

/// The root's configuration names `packs` as `standard_packs.root` and binds one instance to
/// [`GAME`], so a start reads that game's root.
fn configure(root: &Path, packs: &Path) {
    let path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    config["policy"] = json!({
        "facts": {"instances": [{"instance_id": INSTANCE, "game_id": GAME}]}
    });
    write_json(&path, &config);
}

/// The state root the root's configuration names.
fn state_root_of(root: &Path) -> PathBuf {
    let config: Value = serde_json::from_slice(
        &fs::read(root.join("actingd.config.json")).expect("the configuration"),
    )
    .expect("JSON");
    PathBuf::from(config["state_root"].as_str().expect("the state root"))
}

/// A `schedule.json` of no rule whose `requires` lists `names` (none: no such member).
fn schedule(names: &[&str]) -> Value {
    let mut document = json!({"schema_version": "actingcommand.schedule.v1"});
    if !names.is_empty() {
        document["requires"] = json!(names);
    }
    document
}

/// Writes the `schedule.json` of [`GAME`]'s root below `packs`, requiring `names`; its path.
fn root_requires(packs: &Path, names: &[&str]) -> PathBuf {
    let game_root = packs.join(GAME);
    fs::create_dir_all(&game_root).expect("the game root");
    let path = game_root.join("schedule.json");
    write_json(&path, &schedule(names));
    path
}

/// A generation's catalogue hash: `sha256:` and 64 times `digit`.
fn hash_of(digit: char) -> String {
    format!("sha256:{}", String::from(digit).repeat(64))
}

/// The folder of the fragments of the generation `catalog_hash` below `state_root`, where the
/// Runtime's catalogue store keeps a composed generation's fragments.
fn fragments_dir(state_root: &Path, catalog_hash: &str) -> PathBuf {
    let hex = catalog_hash
        .strip_prefix("sha256:")
        .expect("a catalog hash");
    actingcommand_contract::CATALOG_STORE_PATH
        .iter()
        .fold(state_root.to_path_buf(), |path, part| path.join(part))
        .join(actingcommand_contract::CATALOG_GENERATIONS_DIRECTORY)
        .join(hex)
        .join("fragments")
}

/// Writes the fragment of [`GAME`] of the generation `catalog_hash` below `state_root`, requiring
/// `names`; its path.
fn write_generation(state_root: &Path, catalog_hash: &str, names: &[&str]) -> PathBuf {
    let folder = fragments_dir(state_root, catalog_hash);
    fs::create_dir_all(&folder).expect("the generation's fragments");
    let path = folder.join(format!("{GAME}.json"));
    write_json(&path, &schedule(names));
    path
}

/// The check-config of the build `build` (`live`, or a commit) lists `rows` (a name, and whether
/// it is enabled) as its report's `config_manifest.subsystems`.
fn answers_rows(host: &FakeHost, build: &str, rows: &[(&str, bool)]) {
    let listed = rows
        .iter()
        .map(|(name, enabled)| {
            json!({"name": name, "enabled": enabled, "outcome": {
                "code": "config_subsystem_always_on", "category": "info", "values": {}}})
        })
        .collect::<Vec<_>>();
    host.programs.check_config_report_of.borrow_mut().insert(
        build.to_owned(),
        json!({"config_manifest": {"subsystems": listed, "parameters": []}}),
    );
}

/// The check-config of the build `build` (`live`, or a commit) names `subsystems`, each enabled.
fn answers_names(host: &FakeHost, build: &str, subsystems: &[&str]) {
    let rows = subsystems
        .iter()
        .map(|name| (*name, true))
        .collect::<Vec<_>>();
    answers_rows(host, build, &rows);
}

/// The check-config of the build `build` (`live`, or a commit) alone previews the generation
/// `catalog_hash` as the active one (the other members of its report kept).
fn previews_active(host: &FakeHost, build: &str, catalog_hash: &str) {
    let mut answers = host.programs.check_config_report_of.borrow_mut();
    // A build without an entry gets `null`, which the index below makes an object.
    let members = answers.entry(build.to_owned()).or_default();
    members["policy_plan"] = json!({"plan": null, "approvals": null, "active": {
        "catalog_id": "standard-packs", "catalog_version": 2, "catalog_hash": catalog_hash}});
}

/// Every check-config previews the generation `catalog_hash` as the active one.
fn set_active(host: &FakeHost, catalog_hash: &str) {
    host.programs.check_config_report.borrow_mut().insert(
        "policy_plan",
        json!({"plan": null, "approvals": null, "active": {"catalog_id": "standard-packs",
            "catalog_version": 2, "catalog_hash": catalog_hash}}),
    );
}

/// The plan document's `requires_missing` of a plan's answer (`null` without one).
fn requires_missing(answer: &super::Answer) -> &Value {
    &answer.members["plan"]["requires_missing"]
}

/// The `(interface, check)` of every failed edge of a plan's answer.
fn failed_edges(answer: &super::Answer) -> Vec<(String, String)> {
    answer.members["plan"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("the plan's edges: {answer:?}"))
        .iter()
        .filter(|edge| edge["result"] == "fail")
        .map(|edge| {
            (
                edge["interface"].as_str().unwrap_or_default().to_owned(),
                edge["check"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The one failed row of a refusal for a subsystem the target does not name.
fn requires_row() -> Vec<(String, String)> {
    vec![("requires".to_owned(), "next_names_required".to_owned())]
}

/// The failed parts of a refused rollback gate.
fn gate_parts(answer: &super::Answer) -> Vec<String> {
    answer
        .outcome
        .value("rollback_gate_parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The names of the members of `value`, an object, sorted.
fn member_names(value: &Value) -> Vec<String> {
    let mut names = value
        .as_object()
        .map(|members| members.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    names.sort();
    names
}

/// The answer of a planned update from C1 to C2 on the neutral builds, as on main (the golden of
/// IT-5's update): its result members, its plan document's members, every edge row, the notice and
/// no warning; no edge row and no member of this slice.
fn assert_as_on_main(answer: &super::Answer) {
    assert_done(answer, "install_update_plan");
    let mut answered = answer.members.keys().cloned().collect::<Vec<_>>();
    answered.sort();
    assert_eq!(
        answered,
        [
            "extra", "kind", "notice", "plan", "plan_id", "running", "warnings"
        ],
        "{answer:?}"
    );
    let document = &answer.members["plan"];
    assert_eq!(
        member_names(document),
        [
            "check_config",
            "config_sha256",
            "defaults_diff",
            "edges",
            "extra",
            "files",
            "inputs",
            "installer",
            "kind",
            "ledger_verify",
            "live",
            "schema_version",
            "target",
        ],
        "{answer:?}"
    );
    let pair = |range: [u64; 2]| {
        json!([
            {"party": "Rp", "source": "manifest", "range": range},
            {"party": "R*", "source": "manifest", "range": range},
        ])
    };
    let installer = |range: [u64; 2], other: &str| {
        json!([
            {"party": "I", "source": "declaration", "range": range},
            {"party": other, "source": "manifest", "range": range},
        ])
    };
    assert_eq!(
        document["edges"],
        json!([
            {"interface": "ledger", "check": "previous_max_in_next", "result": "pass",
                "parties": pair([3, 4])},
            {"interface": "ledger", "check": "one_way", "result": "pass", "parties": pair([3, 4])},
            {"interface": "runtime-outcomes", "check": "next_max_in_installer", "result": "pass",
                "parties": [
                    {"party": "R*", "source": "manifest", "range": [1, 1]},
                    {"party": "I", "source": "declaration", "range": [1, 1]},
                ]},
            {"interface": "install-control", "check": "installer_meets_previous",
                "result": "pass", "parties": installer([0, 1], "Rp")},
            {"interface": "install-control", "check": "installer_meets_next", "result": "pass",
                "parties": installer([0, 1], "R*")},
            {"interface": "runtime-client", "check": "installer_meets_previous", "result": "pass",
                "parties": installer([2, 2], "Rp")},
            {"interface": "runtime-client", "check": "installer_meets_next", "result": "pass",
                "parties": installer([2, 2], "R*")},
            {"interface": "speaks", "check": "runtime_equals_tools", "result": "pass",
                "parties": [{"party": "Rp", "source": "manifest", "range": null}]},
            {"interface": "speaks", "check": "runtime_equals_tools", "result": "pass",
                "parties": [{"party": "R*", "source": "manifest", "range": null}]},
            {"interface": "actingd-config", "check": "installer_knows_keys", "result": "pass",
                "parties": [
                    {"party": "C", "source": "file", "range": null},
                    {"party": "I", "source": "declaration", "range": [5, 18]},
                ]},
            {"interface": "actingd-config", "check": "next_check_config", "result": "pass",
                "parties": [
                    {"party": "C", "source": "file", "range": null},
                    {"party": "R*", "source": "manifest", "range": [5, 18]},
                ]},
        ]),
        "{answer:?}"
    );
    assert_eq!(
        answer.members["notice"],
        json!({"from_commit": C1, "to_commit": C2, "replaced": 5, "added": 0,
            "runtime_running": false, "mcp_clients_reconnect": false,
            "rollback_window_bounded": false}),
        "{answer:?}"
    );
    assert_eq!(answer.members["warnings"], json!([]), "{answer:?}");
}

// Test 1: a rollback to a build that does not name a subsystem the game root in use requires.
#[test]
fn rb1_t1_a_rollback_to_a_build_that_does_not_name_a_roots_requirement_is_refused_naming_both() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = updated(temp.path(), &host, "t1");
    let packs = temp.path().join("t1-packs");
    configure(&root, &packs);
    let path = root_requires(&packs, &["pool_lines", "catalog_business"]);
    answers_names(&host, "live", &["pool_lines", "catalog_business"]);
    answers_names(&host, C1, &["pool_lines", "pool_list_rows"]);

    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "interface_edge");
    assert_eq!(
        refused.outcome.value("count"),
        Some(&json!(1)),
        "{refused:?}"
    );
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    assert_eq!(
        requires_missing(&refused),
        &json!([{"game": GAME, "source": "root", "path": super::files::plain(&path),
            "missing": ["catalog_business"]}]),
        "{refused:?}"
    );
}

// Test 2: the same for a deploy of an older build; its apply refuses the same.
#[test]
fn rb1_t2_a_deploy_of_an_older_build_is_refused_the_same_way() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "t2",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let packs = temp.path().join("t2-packs");
    configure(&root, &packs);
    let path = root_requires(&packs, &["pool_lines", "pool_list_rows"]);
    answers_names(&host, "live", &["pool_lines", "pool_list_rows"]);
    answers_names(&host, C2, &["pool_lines"]);

    let refused = plan_update(&host, &root);
    assert_refused(&refused, "install_update_plan", "interface_edge");
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    assert_eq!(
        requires_missing(&refused),
        &json!([{"game": GAME, "source": "root", "path": super::files::plain(&path),
            "missing": ["pool_list_rows"]}]),
        "{refused:?}"
    );

    let applied = run(
        &host,
        &[
            "update",
            "apply",
            "--root",
            word(&root),
            "--build",
            C2,
            "--plan",
            "plan-0000000000000000",
        ],
    );
    assert_refused(&applied, "install_update_apply", "interface_edge");
    let manifest: Value = serde_json::from_slice(
        &fs::read(root.join("runtime").join("BUILD-MANIFEST.json")).expect("the live manifest"),
    )
    .expect("JSON");
    assert_eq!(manifest["commit_sha"], C1, "nothing changed");
}

// Test 3: a game root that no longer requires the name while the active generation still does:
// refused, the generation named; once a generation without it is active: planned.
#[test]
fn rb1_t3_a_kept_generation_that_requires_the_name_refuses_until_another_is_active() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = updated(temp.path(), &host, "t3");
    own_start_log(&root);
    let packs = temp.path().join("t3-packs");
    configure(&root, &packs);
    root_requires(&packs, &["pool_lines"]);
    let state_root = state_root_of(&root);
    let first = hash_of('1');
    let fragment = write_generation(&state_root, &first, &["pool_lines", "catalog_business"]);
    set_active(&host, &first);
    answers_names(&host, "live", &["pool_lines", "catalog_business"]);
    answers_names(&host, C1, &["pool_lines"]);

    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "interface_edge");
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    assert_eq!(
        requires_missing(&refused),
        &json!([{"game": GAME, "source": "generation", "catalog_hash": first,
            "path": super::files::plain(&fragment), "missing": ["catalog_business"]}]),
        "{refused:?}"
    );

    // The Runtime activated a generation composed without it; the first stays on disk.
    let second = hash_of('2');
    write_generation(&state_root, &second, &["pool_lines"]);
    set_active(&host, &second);
    let planned = plan_rollback(&host, &root);
    assert_done(&planned, "install_rollback_plan");
    assert!(fragment.is_file(), "the first generation is kept");
    assert!(failed_edges(&planned).is_empty(), "{planned:?}");
    assert!(requires_missing(&planned).is_null(), "{planned:?}");
}

// Test 4: a target whose answer names no subsystems refuses any requirement; with game roots that
// require nothing it is planned as on main.
#[test]
fn rb1_t4_a_target_whose_answer_names_no_subsystems_is_refused_any_requirement() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "t4",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let packs = temp.path().join("t4-packs");
    configure(&root, &packs);
    let path = root_requires(&packs, &["pool_lines"]);

    let refused = plan_update(&host, &root);
    assert_refused(&refused, "install_update_plan", "interface_edge");
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    assert_eq!(
        requires_missing(&refused),
        &json!([{"game": GAME, "source": "root", "path": super::files::plain(&path),
            "missing": ["pool_lines"]}]),
        "{refused:?}"
    );

    root_requires(&packs, &[]);
    let planned = plan_update(&host, &root);
    assert_as_on_main(&planned);
}

// Test 5: ruling 5's golden. IT-5's update on the neutral builds answers as on main, and so does the
// same update on a root whose schedules require only names the target names.
#[test]
fn rb1_t5_a_plan_whose_schedules_require_only_named_subsystems_answers_as_on_main() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let plain_root = prepared(
        temp.path(),
        "t5-plain",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    assert_as_on_main(&plan_update(&host, &plain_root));

    let named_root = prepared(
        temp.path(),
        "t5-named",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let packs = temp.path().join("t5-packs");
    configure(&named_root, &packs);
    root_requires(&packs, &["pool_lines", "catalog_business"]);
    let first = hash_of('1');
    write_generation(&state_root_of(&named_root), &first, &["catalog_business"]);
    set_active(&host, &first);
    answers_names(&host, C2, &["pool_lines", "catalog_business"]);
    assert_as_on_main(&plan_update(&host, &named_root));
}

// Fix round 1, the must-fix: a target whose check-config refuses the configuration is not judged.
// The rollback is refused in the gate's part `config` and the update `config_refused_by_target`,
// as on main, though the game root in use requires a name: no `requires` row, no member.
#[test]
fn rb1_fr1_a_target_that_refuses_the_configuration_is_refused_as_on_main_without_the_check() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = updated(temp.path(), &host, "fr1-config");
    own_start_log(&root);
    let packs = temp.path().join("fr1-config-packs");
    configure(&root, &packs);
    root_requires(&packs, &["pool_lines"]);
    host.programs
        .refuse_check_config
        .replace(vec![C1.to_owned()]);

    let refused = plan_rollback(&host, &root);
    assert_refused(&refused, "install_rollback_plan", "rollback_gate_failed");
    assert_eq!(gate_parts(&refused), ["config"], "{refused:?}");
    assert_eq!(
        failed_edges(&refused),
        [("actingd-config".to_owned(), "next_check_config".to_owned())],
        "{refused:?}"
    );
    assert!(requires_missing(&refused).is_null(), "{refused:?}");

    let other = prepared(
        temp.path(),
        "fr1-update",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let other_packs = temp.path().join("fr1-update-packs");
    configure(&other, &other_packs);
    root_requires(&other_packs, &["pool_lines"]);
    host.programs
        .refuse_check_config
        .replace(vec![C2.to_owned()]);
    let refused_update = plan_update(&host, &other);
    assert_refused(
        &refused_update,
        "install_update_plan",
        "config_refused_by_target",
    );
    assert_eq!(
        failed_edges(&refused_update),
        [("actingd-config".to_owned(), "next_check_config".to_owned())],
        "{refused_update:?}"
    );
    assert!(
        requires_missing(&refused_update).is_null(),
        "{refused_update:?}"
    );
}

// Fix round 1: a `schedule.json` and a fragment that are there and cannot be read (folders in the
// files' places) refuse the plan, each entry saying why.
#[test]
fn rb1_fr1_a_schedule_or_a_fragment_that_cannot_be_read_refuses_and_says_why() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "fr1-unread",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let packs = temp.path().join("fr1-unread-packs");
    configure(&root, &packs);
    let schedule_path = packs.join(GAME).join("schedule.json");
    fs::create_dir_all(&schedule_path).expect("a folder in the schedule's place");
    let first = hash_of('1');
    let fragment = fragments_dir(&state_root_of(&root), &first).join(format!("{GAME}.json"));
    fs::create_dir_all(&fragment).expect("a folder in the fragment's place");
    set_active(&host, &first);
    answers_names(&host, C2, &["pool_lines", "catalog_business"]);

    let refused = plan_update(&host, &root);
    assert_refused(&refused, "install_update_plan", "interface_edge");
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    let entries = requires_missing(&refused)
        .as_array()
        .unwrap_or_else(|| panic!("the entries: {refused:?}"));
    assert_eq!(entries.len(), 2, "{refused:?}");
    for (entry, source, path) in [
        (&entries[0], "root", &schedule_path),
        (&entries[1], "generation", &fragment),
    ] {
        assert_eq!(entry["game"], GAME, "{entry}");
        assert_eq!(entry["source"], source, "{entry}");
        assert_eq!(entry["path"], super::files::plain(path), "{entry}");
        assert_eq!(entry["missing"], json!([]), "{entry}");
        assert!(entry["unread"]["outcome"]["code"].is_string(), "{entry}");
    }
    assert_eq!(entries[0]["catalog_hash"], Value::Null, "{refused:?}");
    assert_eq!(entries[1]["catalog_hash"], first.as_str(), "{refused:?}");
}

// Fix round 1: a subsystem the target lists with `enabled: false` is not named.
#[test]
fn rb1_fr1_a_subsystem_listed_as_not_enabled_is_not_named() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "fr1-disabled",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let packs = temp.path().join("fr1-disabled-packs");
    configure(&root, &packs);
    let path = root_requires(&packs, &["pool_lines", "catalog_business"]);
    answers_rows(
        &host,
        C2,
        &[("pool_lines", true), ("catalog_business", false)],
    );

    let refused = plan_update(&host, &root);
    assert_refused(&refused, "install_update_plan", "interface_edge");
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    assert_eq!(
        requires_missing(&refused),
        &json!([{"game": GAME, "source": "root", "path": super::files::plain(&path),
            "missing": ["catalog_business"]}]),
        "{refused:?}"
    );
}

// Fix round 1: the target's answer previews one active generation and the live build's another:
// both are read, each a fragment's entry, by generation.
#[test]
fn rb1_fr1_two_answers_naming_different_active_generations_are_both_read() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "fr1-active",
        &BuildSpec::new(C1),
        &BuildSpec::new(C2),
        &host,
    );
    let state_root = state_root_of(&root);
    let first = hash_of('1');
    let second = hash_of('2');
    let first_fragment = write_generation(&state_root, &first, &["catalog_business"]);
    let second_fragment = write_generation(&state_root, &second, &["pool_list_rows"]);
    answers_names(&host, C2, &["pool_lines"]);
    previews_active(&host, C2, &first);
    previews_active(&host, "live", &second);

    let refused = plan_update(&host, &root);
    assert_refused(&refused, "install_update_plan", "interface_edge");
    assert_eq!(failed_edges(&refused), requires_row(), "{refused:?}");
    assert_eq!(
        requires_missing(&refused),
        &json!([
            {"game": GAME, "source": "generation", "catalog_hash": first,
                "path": super::files::plain(&first_fragment), "missing": ["catalog_business"]},
            {"game": GAME, "source": "generation", "catalog_hash": second,
                "path": super::files::plain(&second_fragment), "missing": ["pool_list_rows"]},
        ]),
        "{refused:?}"
    );
}
