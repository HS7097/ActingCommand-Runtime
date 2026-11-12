// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G6 (register §1.287, "Slice LAB-5G6"; coordinator work order
// `rd-1112/brief-lab5g6.md`; contracts/install-cli.md, "Running images"; the coordinator's ruling 5
// on board #434, 6775192250, not the owner's words): the register's named test
// `deploy_not_blocked_by_mcp`. A running image of the Lab's MCP server `actinglab-mcp.exe` is
// treated as a running `actingctl.exe` is: a deploy that replaces its file is planned and applied,
// the plan listing it under `running.renamed`; its file is renamed aside (`<file>.old-<ms>`, still
// read by the process that holds it) and the new build's file takes its place; the apply lists it
// under `running_renamed`, noted by the caveat `mcp_clients_on_previous_build`; in `tools\` and in
// `runtime\` alike. A running `tools\actinglab.exe` that is not the planning process still blocks
// (`running_image_blocks`). The machine is the fake host of `fixture.rs` with its fake Runtime (not
// running) and fake clock: no device, no sleep; every root is a temporary folder with neutral
// files.

use super::fixture::{BuildSpec, C1, C2, FakeHost, code, live_root, run, word, write_json};
use super::host::RunningImage;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the test runs the module as: a Lab process the UI started.
const WORD: Initiator = Initiator::LabRescue;
/// The file name of the Lab's MCP server.
const SERVER: &str = "actinglab-mcp.exe";

fn assert_done(answer: &super::Answer, operation: &str) {
    assert_eq!(code(answer), "install_command_done", "{answer:?}");
    assert_eq!(
        answer.outcome.value("operation").and_then(Value::as_str),
        Some(operation),
        "{answer:?}"
    );
}

/// The server's bytes in the neutral build `commit`.
fn server_bytes(commit: &str) -> Vec<u8> {
    format!("neutral mcp server {commit}").into_bytes()
}

/// A neutral build of `commit` whose `tools\` also holds `actinglab.exe` and whose `tools\` (or,
/// with `in_runtime`, `runtime\`) holds the server, each with its commit's bytes.
fn with_server(commit: &str, in_runtime: bool) -> BuildSpec {
    let mut build = BuildSpec::new(commit);
    build.tools.files.push((
        "actinglab.exe".to_owned(),
        format!("neutral lab {commit}").into_bytes(),
    ));
    let server = (SERVER.to_owned(), server_bytes(commit));
    if in_runtime {
        build.runtime.files.push(server);
    } else {
        build.tools.files.push(server);
    }
    build
}

/// A root whose live build is C1 (adopted), with the release of C2 staged as a kept build (its
/// release-facts file beside it, as the download leaves it).
fn prepared(base: &Path, name: &str, host: &FakeHost, in_runtime: bool) -> PathBuf {
    let root = live_root(base, name, &with_server(C1, in_runtime));
    assert_done(
        &run(host, &["adopt", "--root", word(&root)]),
        "install_adopt",
    );
    let release = base.join(format!("{name}-release"));
    let target = with_server(C2, in_runtime);
    target.write_release(&release);
    let facts = release.join("release-facts.json");
    write_json(&facts, &target.facts(&release));
    assert_done(
        &run(
            host,
            &[
                "release",
                "stage",
                "--root",
                word(&root),
                "--dir",
                word(&release),
                "--facts",
                word(&facts),
            ],
        ),
        "install_release_stage",
    );
    root
}

/// The process `pid` whose image is `<root>\<half>\<name>`, as the process probe names it. The
/// probe marks `mcp_serve` only for a command line that holds `mcp-serve`, and the server's holds
/// none.
fn image(root: &Path, half: &str, name: &str, pid: u32) -> RunningImage {
    RunningImage {
        pid,
        path: super::files::plain(
            &fs::canonicalize(root)
                .expect("the canonical root")
                .join(half)
                .join(name),
        ),
        mcp_serve: false,
        parent_pid: None,
    }
}

/// The rescue row `name` with `params` on `root`, as [`WORD`]; its one line and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
    let context = Context {
        host,
        location: &location,
        initiator: WORD,
    };
    let params = match params {
        Value::Object(params) => params,
        _ => Map::new(),
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

/// The step `name` of a composite's line.
fn step<'a>(line: &'a Value, name: &str) -> &'a Value {
    line["steps"]
        .as_array()
        .and_then(|steps| {
            steps
                .iter()
                .find(|found| found["step"].as_str() == Some(name))
        })
        .unwrap_or_else(|| panic!("no step {name}: {line}"))
}

/// The pids of a list of running images.
fn pids(images: &Value) -> Vec<u64> {
    images
        .as_array()
        .map(|listed| {
            listed
                .iter()
                .filter_map(|listed_image| listed_image["pid"].as_u64())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Whether `line` carries, as its top or as a link at any depth, an outcome `wanted` with every
/// one of `values`.
fn carries(line: &Value, wanted: &str, values: &[(&str, Value)]) -> bool {
    match line {
        Value::Object(members) => {
            (members.get("code").and_then(Value::as_str) == Some(wanted)
                && values.iter().all(|(key, value)| {
                    members.get("values").and_then(|found| found.get(*key)) == Some(value)
                }))
                || members.values().any(|inner| carries(inner, wanted, values))
        }
        Value::Array(items) => items.iter().any(|inner| carries(inner, wanted, values)),
        _ => false,
    }
}

// The register's `deploy_not_blocked_by_mcp`: a running `actinglab-mcp.exe` never blocks a deploy
// that replaces its file, in `tools\` or `runtime\`; a running `actinglab.exe` still does.
#[test]
fn deploy_not_blocked_by_mcp() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "tools-server", &host, false);
    let tools = root.join("tools");
    // A client's server (5201) and another actinglab (5202: a recording, say) run the live files
    // of `tools\`, which C2 replaces.
    *host.processes.borrow_mut() = vec![
        image(&root, "tools", SERVER, 5201),
        image(&root, "tools", "actinglab.exe", 5202),
    ];

    // The counter-case: the other actinglab blocks; the server is renamed, not blocked.
    let (planned, exit) = rescue_line(&host, &root, "deploy", json!({"target": C2, "plan": true}));
    assert_eq!(exit, 1, "{planned}");
    assert_eq!(
        planned["outcome"]["values"]["install_refusal"],
        json!("running_image_blocks"),
        "{planned}"
    );
    assert_eq!(
        planned["outcome"]["values"]["pid"],
        json!(5202),
        "{planned}"
    );
    assert_eq!(pids(&planned["running"]["renamed"]), [5201], "{planned}");
    assert_eq!(pids(&planned["running"]["blocked"]), [5202], "{planned}");

    // With only the server running, holding its file open, the deploy is planned and applied.
    host.processes
        .borrow_mut()
        .retain(|running| running.pid == 5201);
    let mut open = File::open(tools.join(SERVER)).expect("an open image");
    let (planned, exit) = rescue_line(&host, &root, "deploy", json!({"target": C2, "plan": true}));
    assert_eq!(exit, 0, "{planned}");
    let plan = &step(&planned, "plan")["result"];
    assert_eq!(pids(&plan["running"]["renamed"]), [5201], "{planned}");
    assert!(pids(&plan["running"]["blocked"]).is_empty(), "{planned}");

    let (deployed, exit) = rescue_line(&host, &root, "deploy", json!({"target": C2}));
    assert_eq!(exit, 0, "{deployed}");
    assert_eq!(
        deployed["outcome"]["code"],
        json!("install_command_done"),
        "{deployed}"
    );
    // What the apply reports about the process still on the old build.
    let apply = step(&deployed, "apply");
    assert_eq!(
        pids(&apply["result"]["running_renamed"]),
        [5201],
        "{deployed}"
    );
    assert!(
        carries(
            &apply["outcome"],
            "install_caveat_noted",
            &[
                ("install_caveat", json!("mcp_clients_on_previous_build")),
                ("count", json!(1)),
            ],
        ),
        "{deployed}"
    );
    // The new build's server is in place, the old one kept beside it and still read by the
    // process that holds it.
    assert_eq!(
        fs::read(tools.join(SERVER)).expect("the new server"),
        server_bytes(C2)
    );
    let kept = fs::read_dir(tools.as_path())
        .expect("the tools folder")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("actinglab-mcp.exe.old-"))
        })
        .expect("the kept old server");
    assert_eq!(
        fs::read(kept).expect("the kept old server"),
        server_bytes(C1)
    );
    let mut read = Vec::new();
    open.read_to_end(&mut read).expect("read the open image");
    assert_eq!(read, server_bytes(C1));
    drop(open);
    host.processes.borrow_mut().clear();

    // Should a build carry the server in `runtime\`, it is treated the same there.
    let root = prepared(temp.path(), "runtime-server", &host, true);
    *host.processes.borrow_mut() = vec![image(&root, "runtime", SERVER, 5301)];
    let (deployed, exit) = rescue_line(&host, &root, "deploy", json!({"target": C2}));
    assert_eq!(exit, 0, "{deployed}");
    assert_eq!(
        pids(&step(&deployed, "plan")["result"]["running"]["renamed"]),
        [5301],
        "{deployed}"
    );
    assert_eq!(
        pids(&step(&deployed, "apply")["result"]["running_renamed"]),
        [5301],
        "{deployed}"
    );
    assert_eq!(
        fs::read(root.join("runtime").join(SERVER)).expect("the new server"),
        server_bytes(C2)
    );
    host.processes.borrow_mut().clear();
}
