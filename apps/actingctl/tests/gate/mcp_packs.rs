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
        "prerequisite_packages": [
            {"package_id": "neutral.home", "package_path": "packages/home"},
        ],
        "return_home_packages": [
            {"game": "neutral-game", "server": "neutral-server", "package_id": "neutral.home"},
        ],
    });
    fs::write(root.join("actingd.config.json"), config.to_string()).expect("write configuration");
    for folder in [
        ["packages", "startup-a"],
        ["resources", "res-a"],
        ["packages", "collect"],
        ["packages", "home"],
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
            {
                "role": "prerequisite",
                "package_id": "neutral.home",
                "path": path(["packages", "home"]),
                "readable": true,
                "scheduled": false,
            },
            {
                "role": "return_home",
                "package_id": "neutral.home",
                "path": path(["packages", "home"]),
                "readable": true,
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
