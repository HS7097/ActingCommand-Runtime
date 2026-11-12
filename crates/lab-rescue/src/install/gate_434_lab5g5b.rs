// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G5b (register §1.287, "Slice LAB-5G5b"; contracts/install-cli.md, "The MCP
// entry"; coordinator work order `rd-1112/brief-lab5g5b.md`): the rescue module's MCP rows.
// - `mcp_plan_answers_the_checked_in_sample`: on a made-up root whose build carries the server in
//   its Tools member, `mcp plan` with a current build of another commit answers the checked-in
//   sample `tests/fixtures/mcp-plan-entry-protocol-2.json` (the temporary root written as the
//   sample's made-up `C:\ActingCommand`), exactly the members the second generation reads.
// - `mcp_plan_switch_is_false_for_the_selected_build`: `switch` false when `current_build` names
//   the installed build, and without one; the Runtime member's server before the Tools member's.
// - `mcp_plan_refuses_a_build_without_the_server`: a build whose manifests list no
//   `actinglab-mcp.exe` refuses `mcp_server_not_in_build` (`commit_sha`, `path`) with the tunables,
//   no child; a file in place that no manifest lists is not carried either.
// - `mcp_plan_refuses_entry_protocol_1`: protocol 1 (and 3) refused `--entry-protocol`
//   `not_recognised`, as the first generation's plan refuses protocol 2.
// - `mcp_plan_refuses_while_the_writer_lock_is_held`: the line the first generation's plan answers
//   (`writer_lock_failed` with the tunables).
// - `mcp_entries_lists_both_generations`: a status file of each generation in a temporary root,
//   both listed, each with its generation; a file stating none without one.
// - `mcp_config_gives_each_clients_snippet`: each client's shape, the second generation's
//   program, no `--tier`, `--list` after `--`, the warning while the program is not placed and none
//   once it is.
// - `mcp_config_refuses_a_list_word_the_server_refuses`: a word that is no module row's id, an
//   empty one, an empty list (`not_recognised`) and a word twice (`repeated`); every module id is
//   taken; an unknown client is refused.
// The machine is the fake host of `fixture.rs` (the rows ask nothing of it); every root is a
// temporary folder with neutral files; no Runtime, no device, no clock, no sleep.

use super::fixture::{BuildSpec, C1, C2, FakeHost, live_root, sha256_hex, word};
use super::lock::WriterLock;
use super::mcp_entry;
use super::state::Paths;
use super::{files, print};
use crate::agent_channel::Answer as Answered;
use crate::cli_line;
use crate::codes::{ActingctlCode, InstallLocation};
use crate::rescue::{self, Context};
use actingcommand_contract::outcome::Detail;
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_lab_ops::table::{ModuleRole, operation_table};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The checked-in sample of `mcp plan`'s answer: the contract between this row and the MCP entry's
/// second generation, whose test reads the same file with the entry's own reader.
const SAMPLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/mcp-plan-entry-protocol-2.json"
));
/// The made-up installation root the sample names.
const MADE_UP_ROOT: &str = r"C:\ActingCommand";
/// The sample root's Runtime manifest, byte for byte: its SHA-256 is the sample's
/// `manifest_sha256`. It names the build `C2` and lists no file.
const SAMPLE_RUNTIME_MANIFEST: &str = concat!(
    r#"{"repository":"neutral/runtime","#,
    r#""commit_sha":"2222222222222222222222222222222222222222","#,
    r#""files":[],"#,
    r#""runtime_payload_layout":"distribution-v1"}"#
);
/// The neutral bytes that stand for the server program in a build.
const SERVER_BYTES: &[u8] = b"neutral server";
/// The server program's name in a build's member.
const SERVER: &str = "actinglab-mcp.exe";
/// The initiator the tests run the module as: a Lab process the UI started (§4.1).
const WORD: Initiator = Initiator::LabRescue;

/// Writes `bytes` to `path`, its folders made first.
fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("the folder");
    fs::write(path, bytes).expect("the file");
}

/// The tunables the sample root's configuration names (`install.mcp_entry`).
fn tunables() -> Value {
    json!({"poll_ms": 1_000, "settle_ms": 5_000, "verify_timeout_ms": 30_000,
        "drain_max_ms": 60_000, "refused_retry_max_ms": 12_000})
}

/// The sample's made-up root in `base`: a single-location root whose live build `C2` carries the
/// server in its Tools member (the Runtime manifest [`SAMPLE_RUNTIME_MANIFEST`]), and a
/// configuration with a state root and the tunables.
fn sample_root(base: &Path) -> PathBuf {
    let root = base.join("root");
    write(
        &root.join("runtime").join("BUILD-MANIFEST.json"),
        SAMPLE_RUNTIME_MANIFEST.as_bytes(),
    );
    let tools = json!({
        "repository": "neutral/runtime",
        "commit_sha": C2,
        "files": [{"path": SERVER, "size_bytes": SERVER_BYTES.len(), "sha256": sha256_hex(SERVER_BYTES)}],
        "tools_payload_layout": "platform-tools-v3",
    });
    write(
        &root.join("tools").join("BUILD-MANIFEST.json"),
        &serde_json::to_vec_pretty(&tools).expect("JSON"),
    );
    write(&root.join("tools").join(SERVER), SERVER_BYTES);
    let config = json!({
        "state_root": base.join("state"),
        "instances": [],
        "install": {"mcp_entry": tunables()},
    });
    write(
        &root.join("actingd.config.json"),
        &serde_json::to_vec_pretty(&config).expect("JSON"),
    );
    root
}

/// The rescue row `name` with `params` on `root` as the module's own root, as [`WORD`]; its one
/// line and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = root.join("state");
    let location = rescue::location_of(root, &config, &state_root);
    let context = Context {
        host,
        location: &location,
        initiator: WORD,
    };
    let Value::Object(params) = params else {
        panic!("{name}: the parameters are an object");
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

/// `value` with the temporary root `plain` written as the sample's made-up root.
fn made_up(value: Value, plain: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(plain, MADE_UP_ROOT)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|item| made_up(item, plain)).collect())
        }
        Value::Object(members) => Value::Object(
            members
                .into_iter()
                .map(|(name, item)| (name, made_up(item, plain)))
                .collect(),
        ),
        other => other,
    }
}

/// The plain spelling of `root` once resolved, as the plan names its files.
fn plain_root(root: &Path) -> String {
    Paths::resolve(root).expect("the root resolves").plain
}

#[test]
fn mcp_plan_answers_the_checked_in_sample() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = sample_root(temp.path());
    let sample: Value = serde_json::from_str(SAMPLE).expect("the sample is JSON");

    let (answered, exit) = rescue_line(
        &host,
        &root,
        "mcp plan",
        json!({"entry_protocol": 2, "current_build": C1}),
    );
    assert_eq!(exit, 0, "{answered}");
    assert_eq!(
        made_up(answered, &plain_root(&root)),
        sample,
        "the answer is the checked-in sample"
    );

    // The sample holds what the second generation reads and nothing else: `entry_protocol` 2,
    // `switch`, `build {commit, manifest_sha256}`, a `child` of one element (no child words),
    // the five tunables and the outcome.
    let members = sample
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        members,
        BTreeSet::from([
            "build",
            "child",
            "entry_protocol",
            "outcome",
            "switch",
            "tunables"
        ])
    );
    assert_eq!(sample["entry_protocol"], 2);
    assert_eq!(sample["switch"], true, "the current build is another");
    assert_eq!(
        sample["build"],
        json!({"commit": C2, "manifest_sha256": sha256_hex(SAMPLE_RUNTIME_MANIFEST.as_bytes())})
    );
    assert_eq!(
        sample["child"],
        json!([format!(r"{MADE_UP_ROOT}\tools\{SERVER}")])
    );
    assert_eq!(sample["tunables"], tunables());
    assert_eq!(sample["outcome"]["code"], "install_command_done");
    assert_eq!(
        sample["outcome"]["values"]["operation"],
        "install_mcp_entry_plan"
    );
}

#[test]
fn mcp_plan_switch_is_false_for_the_selected_build() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = sample_root(temp.path());
    for params in [
        json!({"entry_protocol": 2, "current_build": C2}),
        json!({"entry_protocol": 2}),
    ] {
        let (answered, exit) = rescue_line(&host, &root, "mcp plan", params.clone());
        assert_eq!(exit, 0, "{params}: {answered}");
        assert_eq!(answered["switch"], false, "{params}: {answered}");
        assert_eq!(answered["build"]["commit"], C2, "{answered}");
        assert_eq!(answered["tunables"], tunables(), "{answered}");
    }

    // A build that carries the server in both members: the Runtime member's, where the entry
    // looks first.
    let mut build = BuildSpec::new(C2);
    build
        .runtime
        .files
        .push((SERVER.to_owned(), b"neutral server in runtime".to_vec()));
    build
        .tools
        .files
        .push((SERVER.to_owned(), SERVER_BYTES.to_vec()));
    let both = live_root(temp.path(), "both", &build);
    let (answered, exit) = rescue_line(
        &host,
        &both,
        "mcp plan",
        json!({"entry_protocol": 2, "current_build": C2}),
    );
    assert_eq!(exit, 0, "{answered}");
    assert_eq!(answered["switch"], false, "{answered}");
    assert_eq!(
        answered["child"],
        json!([format!(r"{}\runtime\{SERVER}", plain_root(&both))]),
        "{answered}"
    );
}

#[test]
fn mcp_plan_refuses_a_build_without_the_server() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    // Every build before the server is packaged: neither member lists it.
    let root = live_root(temp.path(), "before", &BuildSpec::new(C2));
    let params = json!({"entry_protocol": 2, "current_build": C1});
    let (answered, exit) = rescue_line(&host, &root, "mcp plan", params.clone());
    assert_eq!(exit, 1, "{answered}");
    let values = &answered["outcome"]["values"];
    assert_eq!(
        answered["outcome"]["code"], "install_command_refused",
        "{answered}"
    );
    assert_eq!(values["operation"], "install_mcp_entry_plan", "{answered}");
    assert_eq!(
        values["install_refusal"], "mcp_server_not_in_build",
        "{answered}"
    );
    assert_eq!(values["commit_sha"], C2, "{answered}");
    let path = values["path"].as_str().expect("the last place looked");
    assert!(path.ends_with(r"\tools\actinglab-mcp.exe"), "{path}");
    assert!(
        !Path::new(path).exists(),
        "no path that does not exist is a child"
    );
    // The refusal carries the tunables, as every refusal after the configuration read; no
    // child, no build, no switch.
    assert!(answered["tunables"].is_object(), "{answered}");
    for member in ["child", "build", "switch", "entry_protocol"] {
        assert!(answered.get(member).is_none(), "{member}: {answered}");
    }

    // A file in place that no manifest lists is not carried by the build either.
    write(&root.join("tools").join(SERVER), SERVER_BYTES);
    let (answered, exit) = rescue_line(&host, &root, "mcp plan", params);
    assert_eq!(exit, 1, "{answered}");
    assert_eq!(
        answered["outcome"]["values"]["install_refusal"], "mcp_server_not_in_build",
        "{answered}"
    );
}

#[test]
fn mcp_plan_refuses_entry_protocol_1() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = sample_root(temp.path());
    // The first generation's plan refuses the second generation's protocol.
    let first = mcp_entry::plan(&["--root", word(&root), "--entry-protocol", "2"])
        .expect_err("protocol 2 is not the first generation's");
    assert!(first.is(ActingctlCode::ActingctlUsageInvalid), "{first}");
    let first = cli_line::outcome_value(&first, Detail::Full);
    assert_eq!(first["values"]["argument"], "--entry-protocol", "{first}");
    assert_eq!(
        first["values"]["argument_problem"], "not_recognised",
        "{first}"
    );
    // The second generation's row refuses protocol 1, and any other, alike.
    for protocol in [1, 3] {
        let (answered, exit) = rescue_line(
            &host,
            &root,
            "mcp plan",
            json!({"entry_protocol": protocol}),
        );
        assert_eq!(exit, 1, "{protocol}: {answered}");
        assert_eq!(
            answered,
            json!({"outcome": first}),
            "{protocol}: refused as the first generation refuses protocol 2, nothing read"
        );
    }
}

#[test]
fn mcp_plan_refuses_while_the_writer_lock_is_held() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = sample_root(temp.path());
    let paths = Paths::resolve(&root).expect("the root resolves");
    let _held = WriterLock::take(&paths, 1_000, InstallLocation::Deploy).expect("the writer lock");

    let (answered, exit) = rescue_line(&host, &root, "mcp plan", json!({"entry_protocol": 2}));
    assert_eq!(exit, 1, "{answered}");
    assert_eq!(
        answered["outcome"]["values"]["install_refusal"], "writer_lock_failed",
        "{answered}"
    );
    assert_eq!(answered["tunables"], tunables(), "{answered}");
    // As the first generation's plan answers it: the same refusal, the same tunables.
    let first = mcp_entry::plan(&["--root", word(&root), "--entry-protocol", "1"])
        .expect("the first generation's answer");
    let first = print(first).line.expect("its line");
    assert_eq!(answered, first);
}

#[test]
fn mcp_entries_lists_both_generations() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = temp.path().join("root");
    // Each file names this test's process with its creation time, so each is live.
    let pid = std::process::id();
    let created = crate::process_probe::pid_created_at(pid)
        .expect("the probe answers")
        .expect("this process runs");
    let state = root.join("mcp-entry").join("state");
    let mut written = Vec::new();
    for (file, generation) in [
        ("entry-1.json", Some(1)),
        ("entry-2.json", Some(2)),
        ("entry-3.json", None),
    ] {
        let mut status = json!({
            "schema_version": "actingcommand.mcp-entry-status.v1",
            "pid": pid,
            "process_created_at_unix_ms": created,
            "started_at_unix_ms": created + 10,
        });
        if let Some(generation) = generation {
            status["entry_generation"] = json!(generation);
            status["entry_protocol"] = json!(generation);
        }
        write(
            &state.join(file),
            &serde_json::to_vec_pretty(&status).expect("JSON"),
        );
        written.push(status);
    }

    let (answered, exit) = rescue_line(&host, &root, "mcp entries", json!({}));
    assert_eq!(exit, 0, "{answered}");
    assert_eq!(
        answered["outcome"]["values"]["operation"], "install_mcp_entry_list",
        "{answered}"
    );
    let rows = answered["entries"].as_array().expect("the entries");
    let listed = rows
        .iter()
        .map(|row| {
            (
                row["file"].clone(),
                row["stale"].clone(),
                row.get("generation").cloned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        listed,
        vec![
            (json!("entry-1.json"), json!(false), Some(json!(1))),
            (json!("entry-2.json"), json!(false), Some(json!(2))),
            (json!("entry-3.json"), json!(false), None),
        ],
        "{answered}"
    );
    for (row, status) in rows.iter().zip(&written) {
        assert_eq!(&row["status"], status, "each file as written");
    }
}

#[test]
fn mcp_config_gives_each_clients_snippet() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("the root");
    let root_word = files::plain(&root);
    let entry = format!(r"{root_word}\mcp-entry\g2\actingcommand-mcp-entry-g2.exe");
    let args = |client: &str| json!(["--root", root_word, "--client", client, "--"]);

    for client in ["claude", "codex", "antigravity", "dsh"] {
        let (answered, exit) = rescue_line(&host, &root, "mcp config", json!({"client": client}));
        assert_eq!(exit, 0, "{client}: {answered}");
        assert_eq!(answered["client"], client, "{answered}");
        assert_eq!(answered["entry"], json!(entry), "{answered}");
        let snippet = answered["snippet"].as_str().expect("the snippet");
        assert!(
            !snippet.contains("--tier") && !snippet.contains("mcp-serve"),
            "{client}: no tier, no first-generation words: {snippet}"
        );
        if client == "codex" {
            assert_eq!(
                snippet,
                format!(
                    "[mcp_servers.actingcommand]\ncommand = '{entry}'\nargs = ['--root', \
                     '{root_word}', '--client', 'codex', '--']\nstartup_timeout_sec = 10\n\
                     tool_timeout_sec = 60\n"
                )
            );
        } else {
            let server = match client {
                "claude" => {
                    json!({"type": "stdio", "command": entry, "args": args(client), "env": {}})
                }
                "antigravity" => {
                    json!({"command": entry, "args": args(client), "timeoutSeconds": 60})
                }
                _ => json!({"command": entry, "args": args(client)}),
            };
            let parsed: Value = serde_json::from_str(snippet).expect("the snippet is JSON");
            assert_eq!(
                parsed,
                json!({"mcpServers": {"actingcommand": server}}),
                "{client}"
            );
        }
        // The program is not placed: the snippet all the same, and the warning that says so.
        assert_eq!(
            answered["outcome"]["code"], "install_command_done",
            "{answered}"
        );
        assert_eq!(
            answered["outcome"]["values"]["operation"], "install_mcp_config",
            "{answered}"
        );
        let warnings = answered["warnings"].as_array().expect("the warning");
        assert_eq!(warnings.len(), 1, "{answered}");
        let warning = &warnings[0]["outcome"];
        assert_eq!(warning["code"], "install_caveat_noted", "{answered}");
        assert_eq!(
            warning["values"]["install_caveat"], "mcp_entry_not_placed",
            "{answered}"
        );
        assert_eq!(warning["values"]["path"], json!(entry), "{answered}");
        let note = &answered["outcome"]["causes"][0];
        assert_eq!(note["relation"], "note", "{answered}");
        assert_eq!(note["code"], "install_caveat_noted", "{answered}");
    }

    // `list` reaches the server after `--`, its words in their order.
    let (answered, exit) = rescue_line(
        &host,
        &root,
        "mcp config",
        json!({"client": "claude", "list": ["core", "rescue"]}),
    );
    assert_eq!(exit, 0, "{answered}");
    let parsed: Value = serde_json::from_str(answered["snippet"].as_str().expect("the snippet"))
        .expect("the snippet is JSON");
    assert_eq!(
        parsed["mcpServers"]["actingcommand"]["args"],
        json!([
            "--root",
            root_word,
            "--client",
            "claude",
            "--",
            "--list",
            "core,rescue"
        ])
    );
    let (answered, exit) = rescue_line(
        &host,
        &root,
        "mcp config",
        json!({"client": "codex", "list": ["debugging"]}),
    );
    assert_eq!(exit, 0, "{answered}");
    let snippet = answered["snippet"].as_str().expect("the snippet");
    assert!(
        snippet.contains("'--', '--list', 'debugging']"),
        "{snippet}"
    );

    // Placed: no warning. Nothing else is in the root: the row wrote no file.
    write(Path::new(&entry), b"neutral entry");
    let (answered, exit) = rescue_line(&host, &root, "mcp config", json!({"client": "dsh"}));
    assert_eq!(exit, 0, "{answered}");
    assert!(answered.get("warnings").is_none(), "{answered}");
    assert!(answered["outcome"].get("causes").is_none(), "{answered}");
    let in_root = fs::read_dir(root.as_path())
        .expect("the root")
        .map(|item| {
            item.expect("an item")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(in_root, ["mcp-entry"], "{in_root:?}");
}

#[test]
fn mcp_config_refuses_a_list_word_the_server_refuses() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = temp.path().join("root");
    fs::create_dir_all(&root).expect("the root");

    let refused = |answered: &Value, argument: &str, problem: &str| {
        assert_eq!(
            answered.as_object().map(Map::len),
            Some(1),
            "only the refusal: {answered}"
        );
        assert_eq!(
            answered["outcome"]["code"], "actingctl_usage_invalid",
            "{answered}"
        );
        assert_eq!(
            answered["outcome"]["values"]["argument"], argument,
            "{answered}"
        );
        assert_eq!(
            answered["outcome"]["values"]["argument_problem"], problem,
            "{answered}"
        );
    };
    for (list, problem) in [
        (json!(["core", "neutral"]), "not_recognised"),
        (json!(["core", ""]), "not_recognised"),
        (json!([]), "not_recognised"),
        (json!(["rescue", "core", "rescue"]), "repeated"),
    ] {
        let (answered, exit) = rescue_line(
            &host,
            &root,
            "mcp config",
            json!({"client": "codex", "list": list}),
        );
        assert_eq!(exit, 1, "{answered}");
        refused(&answered, "--list", problem);
    }

    // Every id of the table's module rows of role `operations` is taken, the authoring module's
    // too, as the server's `--list` takes it.
    let table = operation_table().expect("the operation table");
    let modules = table
        .modules
        .iter()
        .filter(|module| module.role == ModuleRole::Operations)
        .map(|module| module.id.clone())
        .collect::<Vec<_>>();
    assert!(modules.iter().any(|module| module == "authoring"));
    let (answered, exit) = rescue_line(
        &host,
        &root,
        "mcp config",
        json!({"client": "codex", "list": modules}),
    );
    assert_eq!(exit, 0, "{answered}");
    let snippet = answered["snippet"].as_str().expect("the snippet");
    assert!(
        snippet.contains(&format!("'--list', '{}'", modules.join(","))),
        "{snippet}"
    );

    // A client `actingctl mcp-config` does not know.
    let (answered, exit) = rescue_line(&host, &root, "mcp config", json!({"client": "neutral"}));
    assert_eq!(exit, 1, "{answered}");
    refused(&answered, "--client", "not_recognised");
}
