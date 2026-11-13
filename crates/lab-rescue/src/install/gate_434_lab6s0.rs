// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-6S0 (register §1.287, "Slice LAB-6S0"; contracts lab-two-entries.md §4.2 and
// §5; coordinator work order `rd-1113/brief-lab6s0.md`): the rescue module's detached run carries
// `Lab-MCP`, so the Lab's MCP server serves `deploy` and `rollback`.
// - `detached_start_sets_the_marker_only_for_lab_mcp`: `deploy` and `rollback` with `detach`, run
//   as `Lab-MCP`, start their detached run with the word in `ACTINGCOMMAND_INITIATOR` and the
//   marker `ACTINGCOMMAND_DETACHED_RUN=rescue` beside it; as `Lab-制包` or `Lab-救急` with the word
//   alone and no marker; as `Lab-CLI` with neither; each answers at once, `detached`.
// - `mcp_deploy_and_rollback_run_detached_as_lab_mcp`: what the MCP server hands the module for a
//   call of `ac_deploy` and `ac_rollback` (the tool's arguments and the front's `detach` true),
//   with the server's initiator `Lab-MCP`: each answers at once (`install_command_done`,
//   `detached`, the run's `pid` and `log`, `next` naming `op status`), never
//   `mcp_operation_unsupported`; the detached run its start recorded takes `Lab-MCP` by the
//   initiator rule of an actinglab process from the recorded variable and marker, runs the row's
//   words without `--detach`, and every journal it writes says `Lab-MCP`; `op status` lists its
//   operation by its id, and with that id gives the journal, which says `Lab-MCP`.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock (no Runtime
// runs): no device, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{BuildSpec, C1, C2, FakeHost, TAG, code, live_root, run, word, write_json};
use super::host::Host;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::{DETACHED_RUN_MARK, Initiator};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A root whose live build is C1 (adopted), with the release folder of C2 beside it (its
/// release-facts file in the folder, as the download leaves it) and a start of C1 logged after
/// the adopt (the previous build's own open, which a rollback's gate reads), as `gate_434_lab3`
/// prepares one.
fn prepared(base: &Path, name: &str, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, name, &BuildSpec::new(C1));
    let adopted = run(host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    let release = base.join(format!("{name}-release"));
    let target = BuildSpec::new(C2);
    target.write_release(&release);
    write_json(&release.join("release-facts.json"), &target.facts(&release));
    let open = json!({"schema_version": "actingcommand.outcome.v1", "code": "ledger_opened",
        "category": "info", "values": {"sql_read_ms": 0, "verify_ms": 0, "material_ms": 0,
        "restore_ms": 0, "meta_ms": 10, "tail_ms": 20, "index_ms": 900, "import_ms": 0,
        "timeout_ms": 600_000}});
    let at = host.now_unix_ms();
    fs::write(
        root.join(format!("actingd-{at}.log")),
        format!("OUTCOME actingd: {open}\n"),
    )
    .expect("the previous build's start log");
    (root, release)
}

/// The rescue module's location of `root`: its configuration and the state root it names.
fn location(root: &Path) -> actingcommand_lab_ops::locate::Location {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    rescue::location_of(root, &config, &state_root)
}

/// The rescue row `name` with `params` on `root`, run as `who`; its one line and exit code.
fn rescue_line(
    host: &FakeHost,
    root: &Path,
    who: Initiator,
    name: &str,
    params: Value,
) -> (Value, u8) {
    let location = location(root);
    let context = Context {
        host,
        location: &location,
        initiator: who,
    };
    let Value::Object(params) = params else {
        panic!("{name}: parameters are an object");
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

/// Every journal of the root, by operation id.
fn journals(root: &Path) -> Vec<(String, Value)> {
    let directory = root.join("install").join("operations");
    let mut found = fs::read_dir(&directory)
        .expect("the journals")
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?.to_owned();
            id.starts_with("op-").then(|| {
                let journal = serde_json::from_slice(&fs::read(entry.path()).expect("a journal"))
                    .expect("journal JSON");
                (id, journal)
            })
        })
        .collect::<Vec<_>>();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The journals written since `before`.
fn new_journals(root: &Path, before: &[(String, Value)]) -> Vec<(String, Value)> {
    journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect()
}

/// The value the fake host recorded under `prefix` (`initiator=`, `marker=`) among a detached
/// start's environment.
fn recorded<'a>(environment: &'a [String], prefix: &str) -> Option<&'a OsStr> {
    environment
        .iter()
        .find_map(|entry| entry.strip_prefix(prefix))
        .map(OsStr::new)
}

/// The detached run the fake host started last: the word the initiator rule of an actinglab
/// process gives it from the variable and the marker its start set, and its words after
/// `--json`.
fn detached_run(host: &FakeHost) -> (Initiator, Vec<String>) {
    let line = host
        .detached
        .borrow()
        .last()
        .cloned()
        .expect("a detached run was started");
    let at = line
        .iter()
        .position(|entry| entry == "--json")
        .unwrap_or_else(|| panic!("the run's words: {line:?}"));
    let (environment, words) = line.split_at(at);
    let ruled = Initiator::of_lab_process_marked(
        recorded(environment, "initiator="),
        recorded(environment, "marker="),
    )
    .unwrap_or_else(|outcome| panic!("the detached run refuses its initiator: {outcome}"));
    (ruled, words[1..].to_vec())
}

#[test]
fn detached_start_sets_the_marker_only_for_lab_mcp() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "marker", &host);
    let marker = format!("marker={DETACHED_RUN_MARK}");
    // Each case: the initiator the module runs as, and the environment its detached run is
    // started with.
    let cases = [
        (
            Initiator::LabMcp,
            vec!["initiator=Lab-MCP".to_owned(), marker],
        ),
        (
            Initiator::LabAuthoring,
            vec!["initiator=Lab-制包".to_owned()],
        ),
        (Initiator::LabRescue, vec!["initiator=Lab-救急".to_owned()]),
        (Initiator::LabCli, Vec::new()),
    ];
    // Each row: its parameters with `detach`, and the words its detached run runs.
    let rows = [
        (
            "deploy",
            json!({"target": TAG, "dir": word(&release), "detach": true}),
            vec!["deploy", TAG, "--dir", word(&release)],
        ),
        ("rollback", json!({"detach": true}), vec!["rollback"]),
    ];
    for (who, environment) in cases {
        for (name, params, words) in &rows {
            let (answered, exit) = rescue_line(&host, &root, who, name, params.clone());
            assert_eq!(exit, 0, "{who} {name}: {answered}");
            assert_eq!(
                answered["detached"],
                json!(true),
                "{who} {name}: {answered}"
            );
            let mut expected = environment.clone();
            expected.push("--json".to_owned());
            expected.extend(words.iter().map(|part| (*part).to_owned()));
            assert_eq!(
                host.detached.borrow().last(),
                Some(&expected),
                "{who} {name}: the detached start"
            );
        }
    }
}

#[test]
fn mcp_deploy_and_rollback_run_detached_as_lab_mcp() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "mcp", &host);
    // Each row: the arguments a client gives its tool, the words its detached run runs, and the
    // kind of the operation that run journals.
    let rows = [
        (
            "deploy",
            json!({"target": TAG, "dir": word(&release)}),
            vec!["deploy", TAG, "--dir", word(&release)],
            "update",
        ),
        ("rollback", json!({}), vec!["rollback"], "rollback"),
    ];
    for (name, given, words, kind) in rows {
        // What the server hands the module: the tool's arguments and the front's `detach` true
        // (`tools::SUPPLIED` of actinglab-mcp), with the server's initiator.
        let mut handed = given.clone();
        handed["detach"] = json!(true);
        let (answered, exit) = rescue_line(&host, &root, Initiator::LabMcp, name, handed);
        assert_eq!(exit, 0, "{name}: {answered}");
        let outcome = &answered["outcome"];
        assert_eq!(
            outcome["code"], "install_command_done",
            "{name}: {answered}"
        );
        assert_ne!(
            outcome["code"], "mcp_operation_unsupported",
            "{name}: {answered}"
        );
        assert_eq!(answered["detached"], json!(true), "{name}: {answered}");
        assert_eq!(
            answered["next"],
            json!({"status": ["op", "status"]}),
            "{name}: {answered}"
        );
        assert_eq!(outcome["values"]["pid"], json!(9_999), "{name}: {answered}");
        assert!(
            answered["log"]
                .as_str()
                .is_some_and(|log| log.contains("detached-")),
            "{name}: {answered}"
        );

        // The detached run: its word read from what its start set, its words the row's.
        let (ruled, run_words) = detached_run(&host);
        assert_eq!(ruled, Initiator::LabMcp, "{name}");
        assert_eq!(run_words, words, "{name}");
        let before = journals(&root);
        let (ran, exit) = rescue_line(&host, &root, ruled, name, given);
        assert_eq!(exit, 0, "{name}: the detached run: {ran}");
        let written = new_journals(&root, &before);
        assert!(!written.is_empty(), "{name}: {ran}");
        for (id, journal) in &written {
            assert_eq!(journal["initiator"], "Lab-MCP", "{name}: {id}");
        }
        let operation_id = ran["outcome"]["values"]["install_operation_id"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: the run's operation: {ran}"))
            .to_owned();
        assert!(
            written
                .iter()
                .any(|(id, journal)| *id == operation_id && journal["kind"] == kind),
            "{name}: {written:?}"
        );

        // `op status`, which the answer's `next` names, follows the operation by its id.
        let (status, exit) = rescue_line(&host, &root, Initiator::LabMcp, "op status", json!({}));
        assert_eq!(exit, 0, "{name}: {status}");
        assert!(
            status["last_operations"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["operation_id"] == operation_id && row["kind"] == kind)
            }),
            "{name}: {status}"
        );
        let (followed, exit) = rescue_line(
            &host,
            &root,
            Initiator::LabMcp,
            "op status",
            json!({"op": operation_id}),
        );
        assert_eq!(exit, 0, "{name}: {followed}");
        assert_eq!(
            followed["journal"]["initiator"], "Lab-MCP",
            "{name}: {followed}"
        );
    }
}
