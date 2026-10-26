// SPDX-License-Identifier: AGPL-3.0-only

//! #422 LN-7c C (`rd-1026/ln7c/final-query.md` §4 and §7 row C; Alice's ruling 70): confirming
//! that an evidence frame or a run was read. The Runtime is the c4 fake child process
//! (`tests/support/c4_runtime.rs`), whose ledger holds one neutral contained run with its capture
//! frames; the MCP server and the CLI are the real binaries.
//!
//! - MCP `ac_evidence_confirm_read` records one `client.action` with the agent's origin (Agent,
//!   Adapter), and `actingctl evidence confirm-read` one with the command line's (Cli, Cli) after
//!   its identity card `actingctl` in the same correlation. Each names a frame or a run with the
//!   controls R's frame index takes (`evidence_read_frame`, `evidence_read_run`), and each has its
//!   frame index row (`('artifact'|'run', id, 'read_confirmed')` at the record's sequence).
//! - An artifact or a run the ledger does not hold is refused on both sides
//!   (`client_action_invalid`), and nothing is recorded.
//! - Reading evidence through `ac_run_evidence` and `ac_observe` records no confirmation.
//!
//! The ledger is read after a clean stop: its records through `GlobalLedger::open_evidence`, the
//! frame index through the root's database.

use crate::c4_runtime::{RuntimeChild, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_artifact_store::ArtifactStore;
use actingcommand_contract::{
    ClientActionValue, ClientPayload, EventActor, EventPayload, EventQuery, EventSource,
    GovernanceIdentityVerdict, IdentifierIssuer,
};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerEvidenceConfig, PersistedEvent};
use actingcommand_runtime_database::{DatabaseLocation, RuntimeDatabase};
use serde_json::{Value, json};
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.evidence";
const TOOL: &str = "ac_evidence_confirm_read";
/// The controls as R's frame index (`crates/ledger/src/global/sqlite/frame_refs.rs`) spells them.
const FRAME_CONTROL: &str = "evidence_read_frame";
const RUN_CONTROL: &str = "evidence_read_run";

/// One evidence read confirmation as the ledger holds it.
#[derive(Debug, PartialEq, Eq)]
struct Confirmation {
    sequence: u64,
    event_id: String,
    actor: EventActor,
    source: EventSource,
    surface: String,
    control: String,
    value: String,
}

#[test]
fn ln7c_c_mcp_and_the_cli_each_record_one_client_action_with_its_frame_index_row() {
    let (root, mut runtime) = ready_runtime();
    let run_id = run_neutral_task(root.path());
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();
    let artifact_id = first_frame(&mut mcp, 2, &run_id);

    let mut answers = Vec::new();
    for (id, argument, value) in [(3, "artifact_id", &artifact_id), (4, "run_id", &run_id)] {
        let mut arguments = serde_json::Map::new();
        arguments.insert(argument.to_owned(), json!(value));
        let answer = mcp.call_tool(id, TOOL, Value::Object(arguments));
        let result = accepted(&answer, TOOL).clone();
        assert_eq!(result["target"], json!({argument: value}), "{answer}");
        answers.push(result);
    }
    for (flag, member, value) in [
        ("--artifact", "artifact_id", &artifact_id),
        ("--run", "run_id", &run_id),
    ] {
        let (succeeded, line) = actingctl(
            root.path(),
            &["evidence", "confirm-read", flag, value.as_str()],
        );
        assert!(
            succeeded,
            "actingctl evidence confirm-read {flag} failed: {line}"
        );
        assert_eq!(
            line["outcome"]["code"],
            json!("request_record_client_action_completed"),
            "{line}"
        );
        assert_eq!(line["target"], json!({member: value}), "{line}");
        answers.push(line);
    }
    mcp.close_input();
    drop(mcp);
    runtime.stop_clean();

    let recorded =
        |index: usize, actor, source, surface: &str, control: &str, value: &str| Confirmation {
            sequence: answers[index]["sequence"]
                .as_u64()
                .unwrap_or_else(|| panic!("an answer without its sequence: {}", answers[index])),
            event_id: answers[index]["event_id"]
                .as_str()
                .unwrap_or_else(|| panic!("an answer without its event_id: {}", answers[index]))
                .to_owned(),
            actor,
            source,
            surface: surface.to_owned(),
            control: control.to_owned(),
            value: value.to_owned(),
        };
    let expected = vec![
        recorded(
            0,
            EventActor::Agent,
            EventSource::Adapter,
            "mcp",
            FRAME_CONTROL,
            &artifact_id,
        ),
        recorded(
            1,
            EventActor::Agent,
            EventSource::Adapter,
            "mcp",
            RUN_CONTROL,
            &run_id,
        ),
        recorded(
            2,
            EventActor::Cli,
            EventSource::Cli,
            "cli",
            FRAME_CONTROL,
            &artifact_id,
        ),
        recorded(
            3,
            EventActor::Cli,
            EventSource::Cli,
            "cli",
            RUN_CONTROL,
            &run_id,
        ),
    ];
    let events = closed_ledger_events(root.path());
    assert_eq!(
        confirmations(&events),
        expected,
        "each call records exactly its one client.action, with the caller's origin"
    );

    // The CLI declared its identity card `actingctl` in the correlation of each confirmation.
    for confirmation in &expected[2..] {
        let correlation = events
            .iter()
            .find(|event| event.sequence() == confirmation.sequence)
            .and_then(|event| event.links().correlation_id().copied())
            .unwrap_or_else(|| panic!("the confirmation has no correlation: {confirmation:?}"));
        let declared = events.iter().any(|event| {
            event.links().correlation_id() == Some(&correlation)
                && event.sequence() < confirmation.sequence
                && (event.origin().actor(), event.origin().source())
                    == (EventActor::Cli, EventSource::Cli)
                && matches!(
                    event.payload(),
                    EventPayload::Client(ClientPayload::GovernanceIdentityDeclared(payload))
                        if payload.card().client == "actingctl"
                            && payload.verdict() == GovernanceIdentityVerdict::Accepted
                )
        });
        assert!(
            declared,
            "no accepted identity card actingctl before {confirmation:?}"
        );
    }

    // R's frame index links each confirmation to its target at the record's sequence.
    assert_eq!(
        read_confirmed_rows(root.path()),
        expected
            .iter()
            .map(|confirmation| (
                confirmation.sequence,
                if confirmation.control == FRAME_CONTROL {
                    "artifact".to_owned()
                } else {
                    "run".to_owned()
                },
                confirmation.value.clone(),
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
fn ln7c_c_an_unknown_artifact_or_run_is_refused_and_nothing_is_recorded() {
    let (root, mut runtime) = ready_runtime();
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let unknown_artifact = text(issuer.mint_artifact_id().expect("artifact id").transport());
    let unknown_run = text(issuer.mint_run_id().expect("run id").transport());
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();

    for (id, arguments) in [
        (2, json!({"artifact_id": unknown_artifact})),
        (3, json!({"run_id": unknown_run})),
    ] {
        let answer = mcp.call_tool(id, TOOL, arguments.clone());
        let result = &answer["result"];
        assert_eq!(result["isError"], json!(true), "{arguments}: {answer}");
        assert_eq!(
            result["structuredContent"]["error"]["outcome"]["code"],
            json!("client_action_invalid"),
            "{arguments}: {answer}"
        );
    }
    for (flag, value) in [("--artifact", &unknown_artifact), ("--run", &unknown_run)] {
        let (succeeded, line) = actingctl(
            root.path(),
            &["evidence", "confirm-read", flag, value.as_str()],
        );
        assert!(!succeeded, "{flag} {value} was not refused: {line}");
        assert_eq!(
            (&line["outcome"]["code"], &line["receipt"]["state"]),
            (&json!("client_action_invalid"), &json!("denied")),
            "{line}"
        );
    }
    mcp.close_input();
    drop(mcp);
    runtime.stop_clean();

    assert_eq!(
        confirmations(&closed_ledger_events(root.path())),
        Vec::new()
    );
    assert_eq!(read_confirmed_rows(root.path()), Vec::new());
}

#[test]
fn ln7c_c_ac_run_evidence_and_ac_observe_record_no_confirmation() {
    let (root, mut runtime) = ready_runtime();
    let run_id = run_neutral_task(root.path());
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();
    // The run has frames, and ac_run_evidence answers them.
    first_frame(&mut mcp, 2, &run_id);
    let answer = mcp.call_tool(3, "ac_observe", json!({"instance": ALIAS}));
    let observed = accepted(&answer, "ac_observe");
    assert!(observed["frame"]["ref"].is_string(), "{answer}");
    mcp.close_input();
    drop(mcp);
    runtime.stop_clean();

    assert_eq!(
        confirmations(&closed_ledger_events(root.path())),
        Vec::new()
    );
    assert_eq!(read_confirmed_rows(root.path()), Vec::new());
}

/// A c4 Runtime child on a new state root with one instance, `ALIAS`, ready.
fn ready_runtime() -> (TempDir, RuntimeChild) {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    (root, runtime)
}

/// Runs the neutral package on `ALIAS` with `actingctl task-run`: the run's id.
fn run_neutral_task(root: &Path) -> String {
    let package = root.join("neutral-task.zip");
    write_neutral_task_package(&package);
    let (succeeded, line) = actingctl(
        root,
        &[
            "task-run",
            "--instance",
            ALIAS,
            "--package",
            package.to_str().expect("package path"),
        ],
    );
    assert!(succeeded, "actingctl task-run failed: {line}");
    assert_eq!(
        line["receipt"]["result"]["kind"],
        json!("contained_task_completed"),
        "{line}"
    );
    line["receipt"]["result"]["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a run without its run_id: {line}"))
        .to_owned()
}

/// The artifact id of the first capture frame `ac_run_evidence` answers for the run.
fn first_frame(mcp: &mut McpServer, id: u64, run_id: &str) -> String {
    let answer = mcp.call_tool(id, "ac_run_evidence", json!({"run_id": run_id}));
    let evidence = accepted(&answer, "ac_run_evidence");
    let reference = evidence["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|step| step["frames"].as_array().into_iter().flatten())
        .filter_map(|frame| frame["ref"].as_str())
        .chain(evidence["final"]["frame_ref"].as_str())
        .next()
        .unwrap_or_else(|| panic!("the run has no frame: {answer}"));
    // A material ref is `<seq>:<event_id>:<artifact_id>`.
    reference
        .rsplit(':')
        .next()
        .unwrap_or_else(|| panic!("a frame ref without its artifact id: {reference}"))
        .to_owned()
}

/// `actingctl <words> --state-root <root>`: whether it exited 0, and its one stdout line.
fn actingctl(root: &Path, words: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words)
        .arg("--state-root")
        .arg(root)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl");
    let line = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl {words:?} printed no JSON line ({error}): {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), line)
}

/// The records of the closed ledger at `root`, authenticated.
fn closed_ledger_events(root: &Path) -> Vec<PersistedEvent> {
    let artifacts = ArtifactStore::open(root).expect("open the artifact store");
    GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root), |reference| {
        artifacts.verify_recovery_reference(reference).ok()
    })
    .expect("read the closed ledger")
    .query(&EventQuery::default())
}

/// The evidence read confirmations among `events`: the `client.action` records with either
/// control, in ledger order.
fn confirmations(events: &[PersistedEvent]) -> Vec<Confirmation> {
    events
        .iter()
        .filter_map(|event| {
            let EventPayload::Client(ClientPayload::Action(payload)) = event.payload() else {
                return None;
            };
            let record = payload.record();
            if record.control_id() != FRAME_CONTROL && record.control_id() != RUN_CONTROL {
                return None;
            }
            let Some(ClientActionValue::PathSafeString(value)) = record.value() else {
                panic!("a confirmation without its path-safe value: {record:?}");
            };
            Some(Confirmation {
                sequence: event.sequence(),
                event_id: text(event.event_id()),
                actor: event.origin().actor(),
                source: event.origin().source(),
                surface: record.surface_id().to_owned(),
                control: record.control_id().to_owned(),
                value: value.clone(),
            })
        })
        .collect()
}

/// The frame index rows of read confirmations at `root`: (sequence, target kind, target id), in
/// sequence order.
fn read_confirmed_rows(root: &Path) -> Vec<(u64, String, String)> {
    let database = RuntimeDatabase::open_existing(root, false).expect("the root's database");
    let connection = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("database connection");
    let mut statement = connection
        .prepare(
            "SELECT sequence,target_kind,target_id FROM ledger_frame_refs WHERE pointer='read_confirmed' ORDER BY sequence,target_kind,target_id",
        )
        .expect("the frame index");
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("the frame index rows")
        .map(|row| row.expect("a frame index row"))
        // The ledger's ordered integer encoding.
        .map(|(sequence, kind, target)| ((sequence as u64) ^ (1_u64 << 63), kind, target))
        .collect()
}

/// An identifier's canonical text.
fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

/// The neutral contained task package of `c4_process.rs`: from the home page (red pixel), one
/// tap opens the terminal page (blue pixel).
fn write_neutral_task_package(path: &Path) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: &[(&str, &[u8])] = &[
        (
            "control.json",
            br#"{
                "schema_version":"Lab-1y.control.v1",
                "package_id":"neutral.semantic.evidence",
                "execution_mode":"navigable_route",
                "game":"neutral",
                "server":"test",
                "resolution":{"width":16,"height":9},
                "entry_task_id":"task",
                "capture_interval_ms":1,
                "step_timeout_ms":50,
                "timeout_ms":3000,
                "max_steps":2
            }"#,
        ),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        (
            "resources/operations/task/task.json",
            br#"{
                "schema_version":"0.6",
                "task_id":"task",
                "game":"neutral",
                "server_scope":["test"],
                "coordinate_space":{"width":16,"height":9},
                "entry_page":"home",
                "target_page":"terminal",
                "operations":[{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0},
                    "unguarded_trusted_coordinate":true
                }]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{
                "schema_version":"0.3",
                "game":"neutral",
                "server":"test",
                "coordinate_space":{"width":16,"height":9},
                "defaults":{"color_max_distance":0.0},
                "targets":[
                    {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                ]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                    {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                ]
            }"#,
        ),
    ];
    for (entry, contents) in files {
        zip.start_file(*entry, options).expect("zip entry");
        zip.write_all(contents).expect("zip contents");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    fs::write(path, bytes).expect("write the neutral package");
}
