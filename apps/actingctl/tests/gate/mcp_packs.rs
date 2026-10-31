// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-11 (command table v1.1 §4.12, §2.3, ruling Q2): `actingctl packs` and
//! `ac_packs` read the selected configuration of an install root offline and answer the same
//! document. With the same parameters, the CLI's stdout JSON equals the MCP `result` with the
//! envelope's `warnings` as its top-level member (D3), and both are the rows §4.12 requires: id,
//! path and role, no package digest. The root is a temporary install root without slots
//! (`<root>\actingd.config.json`, as mcp-serve reads it); no Runtime runs, and the CLI gets no
//! `--state-root` (D5). Workflow #378 A5: the CLI line carries the outcome
//! `package_bindings_reported` beside the document, which is the rest of the line, and each
//! warning on either side is `{outcome}`.

use crate::mcp_harness::{McpServer, accepted};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// The ADB port the configuration gives the first instance (a neutral value).
const PORT: &str = "5601";

/// The selected configuration and the package folders it binds; `packages/startup-missing`
/// is bound but absent.
fn write_install_root(root: &Path) {
    let config = json!({
        "instances": [
            {
                "alias": "node-a",
                "instance_id": "neutral-a",
                "port": 5601,
                "startup_package": {"package": "packages/startup-a"},
                "resource_package": "resources/res-a",
            },
            {
                "alias": "node-b",
                "instance_id": "neutral-b",
                "startup_package": {"package": "packages/startup-missing"},
            },
        ],
        "policy": {
            "procedure_manifest": [
                {
                    "procedure_ref": "package.neutral.collect",
                    "operation_id": "neutral.collect",
                    "yield_points": [],
                    "scheduled_execution": {
                        "mode": "device_registry",
                        "package_path": "packages/collect",
                    },
                },
                {
                    "procedure_ref": "package.neutral.manual",
                    "operation_id": "neutral.manual",
                    "yield_points": [],
                },
            ],
        },
    });
    fs::write(root.join("actingd.config.json"), config.to_string()).expect("write configuration");
    for folder in [
        ["packages", "startup-a"],
        ["resources", "res-a"],
        ["packages", "collect"],
    ] {
        fs::create_dir_all(root.join(folder[0]).join(folder[1])).expect("create package folder");
    }
    fs::write(
        root.join("packages").join("startup-a").join("control.json"),
        json!({"package_id": "neutral.startup"}).to_string(),
    )
    .expect("write control.json");
}

/// `actingctl packs --root <root> <flags>`: its stdout JSON without the line's `outcome`, which is
/// `package_bindings_reported`.
fn cli_packs(root: &Path, flags: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .arg("packs")
        .arg("--root")
        .arg(root)
        .args(flags)
        .output()
        .expect("run actingctl packs");
    assert!(
        output.status.success(),
        "actingctl packs {flags:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl packs {flags:?} printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    let Some(outcome) = outcome else {
        panic!("actingctl packs {flags:?} printed no outcome: {line}")
    };
    assert_eq!(
        outcome["code"],
        json!("package_bindings_reported"),
        "actingctl packs {flags:?}: {outcome}"
    );
    line
}

/// The `result` of an accepted `ac_packs` call with the envelope's `warnings`, when there are
/// any, as its top-level member.
fn mcp_document(answer: &Value) -> Value {
    let mut document = accepted(answer, "ac_packs").clone();
    if let Some(warnings) = answer["result"]["structuredContent"].get("warnings") {
        document["warnings"] = warnings.clone();
    }
    document
}

#[test]
fn gate_packs_and_ac_packs_answer_the_same_bindings_of_the_selected_configuration() {
    let root = TempDir::new().expect("tempdir");
    let root = root.path();
    write_install_root(root);
    let path = |parts: [&str; 2]| root.join(parts[0]).join(parts[1]).display().to_string();
    let source = json!({
        "kind": "selected_config",
        "config": root.join("actingd.config.json").display().to_string(),
    });
    let startup_a = json!({
        "role": "startup",
        "package_id": "neutral.startup",
        "path": path(["packages", "startup-a"]),
        "readable": true,
    });
    let missing = path(["packages", "startup-missing"]);
    let every_binding = json!({
        "source": source,
        "instances": [
            {
                "alias": "node-a",
                "instance_id": "neutral-a",
                "packs": [
                    startup_a,
                    {
                        "role": "resource_package",
                        "path": path(["resources", "res-a"]),
                        "readable": true,
                    },
                ],
            },
            {
                "alias": "node-b",
                "instance_id": "neutral-b",
                "packs": [{"role": "startup", "path": missing, "readable": false}],
            },
        ],
        "installation": [
            {
                "role": "procedure",
                "package_id": "neutral.collect",
                "path": path(["packages", "collect"]),
                "readable": true,
                "procedure_ref": "package.neutral.collect",
                "operation_id": "neutral.collect",
                "scheduled": true,
            },
            {
                "role": "procedure",
                "package_id": "neutral.manual",
                "procedure_ref": "package.neutral.manual",
                "operation_id": "neutral.manual",
                "scheduled": false,
            },
        ],
    });
    let one_instance_startup = json!({
        "source": source,
        "instances": [{"alias": "node-a", "instance_id": "neutral-a", "packs": [startup_a]}],
        "installation": [],
    });

    let mut mcp = McpServer::spawn_at_root(root);
    mcp.initialize();
    let cases = [
        (2, json!({}), Vec::new(), every_binding, true),
        (
            3,
            json!({"instance": PORT, "role": "startup"}),
            vec!["--instance", PORT, "--role", "startup"],
            one_instance_startup,
            false,
        ),
    ];
    for (id, arguments, flags, expected, warned) in cases {
        let cli = cli_packs(root, &flags);
        let answer = mcp.call_tool(id, "ac_packs", arguments.clone());
        assert_eq!(
            cli,
            mcp_document(&answer),
            "actingctl packs {flags:?} and ac_packs {arguments} answer differently: {answer}"
        );
        let mut rows = cli;
        let warnings = rows
            .as_object_mut()
            .and_then(|document| document.remove("warnings"));
        assert_eq!(rows, expected, "actingctl packs {flags:?}");
        if warned {
            // One unreadable binding: its row says readable false and one warning names it, its
            // outcome's values the path and the instance that binds it.
            let warnings = warnings.unwrap_or_else(|| panic!("no warnings for {missing}"));
            let named = warnings
                .as_array()
                .map(|warnings| {
                    warnings
                        .iter()
                        .map(|warning| {
                            (
                                warning["outcome"]["code"].clone(),
                                warning["outcome"]["values"].clone(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(
                named,
                vec![(
                    json!("package_binding_unreadable"),
                    json!({"path": missing, "alias": "node-b"}),
                )],
                "warnings: {warnings}"
            );
        } else {
            assert_eq!(warnings, None, "actingctl packs {flags:?}");
        }
    }
}

/// Workflow #357 DB-1 (model M-357a v1.1 §8, the DB-1 test list; register §1.65 TB4): with
/// `standard_packs.root` set, `actingctl packs` and `ac_packs` answer the same document, and it
/// lists the tables of each sound game root as check-config does: file name order, a read
/// table's row count and expired columns, a refused table's problem. Neither `instance` nor
/// `role` narrows it.
#[test]
fn gate_packs_and_ac_packs_list_the_standard_pack_tables() {
    let root = TempDir::new().expect("tempdir");
    let root = root.path();
    fs::write(
        root.join("actingd.config.json"),
        json!({"instances": [], "standard_packs": {"root": "packages"}}).to_string(),
    )
    .expect("write configuration");
    let game = root.join("packages").join("neutral");
    fs::create_dir_all(game.join("tables")).expect("create the tables directory");
    fs::write(
        game.join("bundle.json"),
        json!({
            "schema_version": "actingcommand.bundle.v4",
            "game": "neutral",
            "packs": [{"package_id": "neutral.test.task", "server": "test",
                       "entry_task_id": "task", "path": "packs/neutral.test.task",
                       "file_count": 1, "byte_count": 1}]
        })
        .to_string(),
    )
    .expect("write bundle.json");
    // `secondary_milli` expired in 2001; `primary_milli` expires in 2096.
    let table = |table_id: &str, rows: Value| {
        json!({
            "schema_version": "actingcommand.table.v1",
            "game": "neutral",
            "table_id": table_id,
            "row_key": ["unit"],
            "columns": [
                {"name": "unit", "type": "key"},
                {"name": "primary_milli", "type": "integer", "min": 0, "max": 100000,
                 "nullable": true, "valid_until_unix_s": 4_000_000_000_i64},
                {"name": "secondary_milli", "type": "integer", "min": 0, "max": 100000,
                 "nullable": true, "valid_until_unix_s": 1_000_000_000_i64}
            ],
            "rows": rows
        })
        .to_string()
    };
    fs::write(
        game.join("tables").join("unit_scores.json"),
        table(
            "unit_scores",
            json!([["unit.a", 87500, 21300], ["unit.b", null, null]]),
        ),
    )
    .expect("write unit_scores.json");
    fs::write(
        game.join("tables").join("unit_ranks.json"),
        table("unit_ranks", json!([["unit.a", 1, 2], ["unit.a", 3, 4]])),
    )
    .expect("write unit_ranks.json");

    let mut mcp = McpServer::spawn_at_root(root);
    mcp.initialize();
    for (id, arguments, flags) in [
        (2, json!({}), Vec::new()),
        (3, json!({"role": "startup"}), vec!["--role", "startup"]),
    ] {
        let cli = cli_packs(root, &flags);
        let answer = mcp.call_tool(id, "ac_packs", arguments.clone());
        assert_eq!(
            cli,
            mcp_document(&answer),
            "actingctl packs {flags:?} and ac_packs {arguments} answer differently: {answer}"
        );
        let packs = &cli["standard_packs"];
        assert_eq!(
            packs["root"],
            json!(root.join("packages").display().to_string()),
            "{cli}"
        );
        let games = packs["games"].as_array().expect("the game roots");
        assert_eq!(games.len(), 1, "{cli}");
        assert_eq!(games[0]["game"], "neutral");
        let tables = games[0]["tables"].as_array().expect("the root's tables");
        assert_eq!(tables.len(), 2, "{cli}");
        assert_eq!(tables[0]["table_id"], "unit_ranks");
        assert_eq!(
            tables[0]["path"],
            json!(
                game.join("tables")
                    .join("unit_ranks.json")
                    .display()
                    .to_string()
            )
        );
        assert_eq!(tables[0].get("row_count"), None);
        assert_eq!(tables[0]["problem"]["code"], "standard_pack_table_invalid");
        assert_eq!(
            tables[0]["problem"]["values"]["standard_pack_problem"],
            "table_row_duplicate"
        );
        assert_eq!(tables[1]["table_id"], "unit_scores");
        assert_eq!(tables[1]["row_count"], 2);
        assert_eq!(tables[1].get("problem"), None);
        assert_eq!(tables[1]["expired_columns"], json!(["secondary_milli"]));
    }
}

/// Workflow #429 R-11 (register §1.240; work order `rd-1031/brief-r11.md`, test 6): with a
/// composer-mode configuration, `actingctl packs` and `ac_packs` answer the same rows and the same
/// warnings. A game root's valid `ui.json` is its row's `ui` as written with its `display`; each
/// composed row carries the tasks that run its package (`task_ids`), whether the package is
/// irreversible as its `control.json` `provides` says, and its game's `ui.json` entry (`ui`) and
/// `display`. A refused `ui.json` adds one `standard_pack_root_invalid` warning per violation, at
/// its pointer, and its game's rows are listed as without the file.
#[test]
fn gate_r11_packs_and_ac_packs_answer_the_same_ui_irreversible_and_task_ids() {
    use crate::standard_pack_schedule::{bundle, rule, schedule, write_json};
    const GAME_A: &str = "fixture-game-a";
    const GAME_B: &str = "fixture-game-b";
    const SERVER: &str = "fixture-server-a";
    const ALPHA: &str = "fixture.alpha";
    const BETA: &str = "fixture.beta";
    const GAMMA: &str = "fixture.gamma";
    let root = TempDir::new().expect("tempdir");
    let root = root.path();
    let packages = root.join("packages");
    let game_root = |game: &str, packs: &[(&str, &str, Value)]| {
        let ids = packs.iter().map(|(_, id, _)| *id).collect::<Vec<_>>();
        write_json(
            &packages.join(game).join("bundle.json"),
            &bundle(game, SERVER, &ids),
        );
        let rules = packs
            .iter()
            .map(|(rule_id, id, _)| rule(rule_id, id, true))
            .collect::<Vec<_>>();
        write_json(
            &packages.join(game).join("schedule.json"),
            &schedule(&rules),
        );
        for (_, id, control) in packs {
            write_json(&packages.join(game).join(id).join("control.json"), control);
        }
    };
    let irreversible = json!({"package_id": ALPHA, "provides": [
        {"capability": "cap_a", "irreversible": true, "parameters": [
            {"name": "param_a", "options": ["opt_a", "opt_b"], "default": "opt_a"}
        ]}
    ]});
    game_root(
        GAME_A,
        &[
            ("fixture.one", ALPHA, irreversible),
            ("fixture.two", BETA, json!({"package_id": BETA})),
        ],
    );
    game_root(
        GAME_B,
        &[("fixture.three", GAMMA, json!({"package_id": GAMMA}))],
    );
    let ui_a = json!({
        "schema_version": "actingcommand.ui.v1",
        "game": GAME_A,
        "groups": [{"id": "group_a", "title": {"en": "Group A"}}],
        "packs": [{"package_id": ALPHA, "title": {"en": "Alpha"},
                   "summary": {"en": "The first neutral package."},
                   "group": "group_a", "risk": ["word_one"]}],
        "display": {"aspect_ratios": ["16:9"], "safe_area": "declared_word"}
    });
    write_json(&packages.join(GAME_A).join("ui.json"), &ui_a);
    // An unregistered package with a parameter no package declares.
    write_json(
        &packages.join(GAME_B).join("ui.json"),
        &json!({
            "schema_version": "actingcommand.ui.v1",
            "game": GAME_B,
            "groups": [],
            "packs": [{"package_id": "fixture.absent", "title": {"en": "Absent"},
                       "summary": {"en": "Not in the bundle."},
                       "parameters": {"level": {"label": {"en": "Level"}}}}]
        }),
    );
    fs::write(
        root.join("actingd.config.json"),
        json!({
            "instances": [
                {"alias": "node-a", "instance_id": "neutral-a"},
                {"alias": "node-b", "instance_id": "neutral-b"},
                {"alias": "node-c", "instance_id": "neutral-c"}
            ],
            "standard_packs": {"root": "packages"},
            "policy": {"facts": {"instances": [
                {"instance_id": "node-a", "game_id": GAME_A, "server_id": SERVER},
                {"instance_id": "node-b", "game_id": GAME_B, "server_id": SERVER},
                {"instance_id": "node-c", "game_id": GAME_B, "server_id": SERVER}
            ]}}
        })
        .to_string(),
    )
    .expect("write configuration");

    let mut mcp = McpServer::spawn_at_root(root);
    mcp.initialize();
    let cli = cli_packs(root, &[]);
    let answer = mcp.call_tool(2, "ac_packs", json!({}));
    assert_eq!(
        cli,
        mcp_document(&answer),
        "actingctl packs and ac_packs answer differently: {answer}"
    );

    let place = |game: &str, package: &str| packages.join(game).join(package).display().to_string();
    let row = |game: &str, package: &str, tasks: Value, irreversible: bool| {
        json!({
            "role": "procedure",
            "package_id": package,
            "path": place(game, package),
            "readable": true,
            "procedure_ref": format!("package.{package}"),
            "operation_id": "operation.package.run",
            "scheduled": true,
            "task_ids": tasks,
            "irreversible": irreversible,
        })
    };
    let mut alpha = row(GAME_A, ALPHA, json!(["fixture.one"]), true);
    alpha["ui"] = ui_a["packs"][0].clone();
    alpha["display"] = ui_a["display"].clone();
    let mut beta = row(GAME_A, BETA, json!(["fixture.two"]), false);
    beta["display"] = ui_a["display"].clone();
    let gamma = row(
        GAME_B,
        GAMMA,
        json!(["fixture.three@node-b", "fixture.three@node-c"]),
        false,
    );
    assert_eq!(cli["installation"], json!([alpha, beta, gamma]), "{cli}");
    assert_eq!(
        cli["standard_packs"]["games"],
        json!([
            {"game": GAME_A, "tables": [], "ui": ui_a, "display": ui_a["display"]},
            {"game": GAME_B, "tables": []}
        ]),
        "{cli}"
    );
    let refused = packages.join(GAME_B).join("ui.json").display().to_string();
    let warning = |problem: &str, pointer: &str| {
        json!({
            "game": GAME_B,
            "path": refused,
            "standard_pack_problem": problem,
            "json_pointer": pointer,
        })
    };
    let warnings = cli["warnings"]
        .as_array()
        .map(|warnings| {
            warnings
                .iter()
                .map(|warning| {
                    (
                        warning["outcome"]["code"].clone(),
                        warning["outcome"]["values"].clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(
        warnings,
        vec![
            (
                json!("standard_pack_root_invalid"),
                warning("package_not_listed", "/packs/0/package_id"),
            ),
            (
                json!("standard_pack_root_invalid"),
                warning("ui_parameter_undeclared", "/packs/0/parameters/level"),
            ),
        ],
        "{cli}"
    );
}
