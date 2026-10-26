// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-T (#386; contracts/standard-pack-schedule.md "Tools",
// contracts/resource-declarations.md): the two Tools defects RL-D found in v0.12.13.
// - `standard-pack compose --root <standard_packs.root> --instance <alias>=<game>/<server>...
//   --out <new directory>` receives every `--instance` of its command line, so two bound
//   instances of one game and server compose one copy of each rule and item per instance.
// - `resource validate` reads a repository root's `schedule.json` by the RL-1a schedule reader:
//   a valid file is valid with no warning, an invalid one is refused naming its problem and its
//   JSON pointer.
//
// The fixture is the neutral catalog `contracts/scheduling/examples/catalog-a` as the
// `schedule.json` of the game root `game-a` (the as-is rules of RL-1a's T1), whose registry holds
// the rule's package, a return-home package and one held package, all on `server-a`.

use super::*;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const INSTANCE_A: &str = "fixture-instance-a";
const INSTANCE_B: &str = "fixture-instance-b";
// catalog-a's task id; the package its rule runs has the same name.
const RULE: &str = "fixture.observe";
const PACKAGE: &str = "fixture.observe";
const RETURN_PACKAGE: &str = "fixture.return";
const HELD_PACKAGE: &str = "fixture.extra";

/// Removes every `scope` member at any depth of `value`.
fn without_scopes(value: &mut Value) {
    match value {
        Value::Object(members) => {
            members.shift_remove("scope");
            members.values_mut().for_each(without_scopes);
        }
        Value::Array(items) => items.iter_mut().for_each(without_scopes),
        _ => {}
    }
}

/// The element list `member` of one catalog-a document, without its scopes.
fn catalog_a(text: &str, member: &str) -> Value {
    let mut document: Value = serde_json::from_str(text).expect("a catalog-a document");
    let mut items = document[member].take();
    without_scopes(&mut items);
    items
}

/// catalog-a as `schedule.json`: no scope; the task without its entrypoint, procedure and
/// instance overrides, naming its package; each event valid from the epoch; the package no rule
/// runs held.
fn schedule() -> Value {
    let mut rules = catalog_a(
        include_str!("../../../../contracts/scheduling/examples/catalog-a/tasks.json"),
        "tasks",
    );
    for rule in rules.as_array_mut().expect("the tasks") {
        let fields = rule.as_object_mut().expect("a task");
        for filled in ["entrypoint", "procedure_ref", "instance_overrides"] {
            assert!(
                fields.shift_remove(filled).is_some(),
                "catalog-a's {filled}"
            );
        }
        fields.insert("package_id".to_owned(), json!(PACKAGE));
    }
    let mut timeline = catalog_a(
        include_str!("../../../../contracts/scheduling/examples/catalog-a/timeline.json"),
        "events",
    );
    for event in timeline.as_array_mut().expect("the events") {
        event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
    }
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": [],
        "rules": rules,
        "pools": catalog_a(
            include_str!("../../../../contracts/scheduling/examples/catalog-a/pools.json"),
            "pools",
        ),
        "activity": catalog_a(
            include_str!("../../../../contracts/scheduling/examples/catalog-a/activity.json"),
            "profiles",
        ),
        "timeline": timeline,
        "held": [{"package_id": HELD_PACKAGE, "reason": "fixture: kept out of the routine"}],
    })
}

/// The game root's registry: three packages on `server-a`, the return-home one in maintenance.
fn bundle() -> Value {
    json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [PACKAGE, RETURN_PACKAGE, HELD_PACKAGE]
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>(),
        "maintenance": [{"package_id": RETURN_PACKAGE, "server": SERVER, "uses": ["return_home"]}]
    })
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
    fs::write(path, serde_json::to_vec_pretty(value).expect("JSON")).expect("the file");
}

fn copy_id(id: &str, alias: &str) -> String {
    format!("{id}@{alias}")
}

/// `standard-pack compose` with two `--instance` bindings of one game and server, as the RL-1a
/// contract writes its command line: the command receives both, and the composition holds one
/// copy of the rule and of each item per instance, each in that instance's scope.
#[test]
fn rlt_1_standard_pack_compose_receives_every_instance_of_its_command_line() {
    let temp = TempDir::new().expect("a temporary directory");
    let root = temp.path().join("standard-packs");
    write_json(&root.join(GAME).join("bundle.json"), &bundle());
    write_json(&root.join(GAME).join("schedule.json"), &schedule());
    let out = temp.path().join("composed");
    let root_text = root.to_str().expect("a UTF-8 path").to_owned();
    let out_text = out.to_str().expect("a UTF-8 path").to_owned();
    let binding_a = format!("{INSTANCE_A}={GAME}/{SERVER}");
    let binding_b = format!("{INSTANCE_B}={GAME}/{SERVER}");
    let command_args = [
        "--root",
        root_text.as_str(),
        "--instance",
        binding_a.as_str(),
        "--instance",
        binding_b.as_str(),
        "--out",
        out_text.as_str(),
    ]
    .map(str::to_owned);
    let args = ["--json", "standard-pack", "compose"]
        .map(str::to_owned)
        .into_iter()
        .chain(command_args.iter().cloned())
        .collect::<Vec<_>>();

    let Ok(invocation) = parse_invocation(args.clone(), true) else {
        panic!("{args:?} parses");
    };
    assert_eq!(invocation.command, ["standard-pack", "compose"]);
    assert_eq!(invocation.args, command_args);
    assert!(
        invocation.global.instance.is_none(),
        "the global --instance takes none: {:?}",
        invocation.global.instance
    );

    let result = run_invocation(parse_invocation(args, true));
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the composition");
    assert_eq!(data["status"], "composed");
    assert_eq!(data["tasks"], 2);
    assert_eq!(data["pools"], 2);
    assert_eq!(data["profiles"], 4);
    assert_eq!(data["events"], 2);
    let games = data["games"].as_array().expect("the bound games");
    assert_eq!(games.len(), 1, "{games:?}");
    assert_eq!(games[0]["game_id"], GAME);
    assert_eq!(games[0]["instances"], json!([INSTANCE_A, INSTANCE_B]));
    assert_eq!(games[0]["tasks"], 2);
    assert_eq!(games[0]["scheduled"], json!([PACKAGE]));
    assert_eq!(games[0]["held"], json!([HELD_PACKAGE]));
    assert_eq!(games[0]["maintenance"], json!([RETURN_PACKAGE]));
    assert_eq!(games[0]["unclassified"], json!([]));
    let procedures = json!([{
        "procedure_ref": format!("package.{PACKAGE}"),
        "game_id": GAME,
        "package_id": PACKAGE,
        "directory": PACKAGE
    }]);
    assert_eq!(data["manifest"], procedures);
    assert_eq!(data["warnings"], json!([]));

    let read = |name: &str| -> Value {
        serde_json::from_slice(&fs::read(out.join(name)).expect("a composed file"))
            .expect("a composed JSON document")
    };
    let tasks = read("tasks.json");
    let tasks = tasks["tasks"].as_array().expect("the composed tasks");
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    for (task, alias) in tasks.iter().zip([INSTANCE_A, INSTANCE_B]) {
        assert_eq!(task["id"], copy_id(RULE, alias));
        assert_eq!(
            task["scope"],
            json!({"kind": "instance", "instance_id": alias})
        );
        assert_eq!(task["procedure_ref"], format!("package.{PACKAGE}"));
        assert_eq!(task["entrypoint"]["operation_id"], "operation.package.run");
    }
    let pools = read("pools.json");
    let pool_ids = pools["pools"]
        .as_array()
        .expect("the composed pools")
        .iter()
        .map(|pool| pool["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        pool_ids,
        [
            copy_id("fixture-pool-a", INSTANCE_A),
            copy_id("fixture-pool-a", INSTANCE_B)
        ]
    );
    let manifest = read("manifest.json");
    assert_eq!(manifest["procedures"], procedures);
    assert_eq!(manifest["rules_digest"], data["rules_digest"]);
}

fn validate_schedule(repo: &Path) -> CliResult {
    run_cli(
        [
            "--json",
            "resource",
            "validate",
            "--repo",
            repo.to_str().expect("a UTF-8 path"),
            "--changed-path",
            "schedule.json",
        ],
        true,
    )
}

/// A repository root's valid `schedule.json` validates as the family `standard_pack_schedule`,
/// with no warning.
#[test]
fn rlt_2_resource_validate_accepts_a_valid_root_schedule_json() {
    let temp = TempDir::new().expect("a temporary directory");
    write_json(&temp.path().join("schedule.json"), &schedule());
    let result = validate_schedule(temp.path());
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.as_ref().expect("the validation");
    assert_eq!(data["status"], "valid");
    assert_eq!(
        data["entries"],
        json!([{"path": "schedule.json", "status": "valid", "family": "standard_pack_schedule"}])
    );
    assert_eq!(data["warnings"], json!([]));
}

/// A repository root's `schedule.json` with a scope in a rule's predicate is refused as
/// `resource_declaration_invalid` naming the file, `schedule_invalid` and the scope's pointer.
#[test]
fn rlt_2_resource_validate_refuses_an_invalid_root_schedule_json_with_its_pointer() {
    let temp = TempDir::new().expect("a temporary directory");
    let mut invalid = schedule();
    invalid["rules"][0]["trigger"]["predicates"][1]["scope"] =
        json!({"kind": "instance", "instance_id": INSTANCE_A});
    write_json(&temp.path().join("schedule.json"), &invalid);
    let result = validate_schedule(temp.path());
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().expect("a refusal");
    assert_eq!(error.code, "resource_declaration_invalid");
    let details = error.details.as_ref().expect("its details");
    assert_eq!(details["declaration_file"], "schedule.json");
    assert_eq!(details["standard_pack_problem"], "schedule_invalid");
    assert_eq!(
        details["json_pointer"],
        "/rules/0/trigger/predicates/1/scope"
    );
}
