// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #421 MCP-ENTRY, test row E-12: `actingctl mcp-config --entry` renders each of the
//! four clients' snippets pointing at `<root>\mcp-entry\g1\actingcommand-mcp-entry.exe`, with the
//! entry's `--root` and `--client` and the child's `mcp-serve --tier …` after `--` unchanged.
//! Fix round 1: E-15 (review M2) and E-17 (review M3), the handles `actingcommand/entryStatus`
//! counts.

use super::super::jobs::{Jobs, Outstanding, take_offered};
use super::super::operator::job_answer;
use super::super::runtime::RuntimeAccess;
use super::super::tools::ToolContext;
use super::super::{Client, entry_snippet};
use super::gate_mcpt1::unreached_deadline;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

/// Waits (a condition variable, no sleep) until the job ended.
fn ended(job: &super::super::jobs::Job) {
    assert!(
        job.wait_until(unreached_deadline(), &AtomicBool::new(false)),
        "the job ends"
    );
}

/// MCP-ENTRY fix round 1, test row E-15 (review M2): a job whose handle went out only in the
/// answer of a cancelled call counts while it runs and holds nothing back once it ended; a job
/// whose handle went out counts until its result is read, and `actingcommand/entryStatus` names
/// its handle.
#[test]
fn e15_a_cancelled_call_leaves_no_uncollectable_handle() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let runtime = RuntimeAccess::new(None, Some(root.path().to_path_buf()));
    let jobs = Jobs::new();
    let cancelled = AtomicBool::new(true);
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        session: 1,
        jobs: &jobs,
    };
    let (release, gate) = mpsc::channel::<()>();
    let job = jobs
        .start(
            "lab_job_1".to_owned(),
            "lab",
            None,
            "running",
            Vec::new(),
            move || {
                let _ = gate.recv();
                Ok(json!({"done": true}))
            },
        )
        .unwrap_or_else(|_| panic!("the job starts"));
    drop(take_offered());
    let Ok(answer) = job_answer(&context, &job, unreached_deadline()) else {
        panic!("a cancelled wait answers the handle");
    };
    assert_eq!(answer.result["handle"], "lab_job_1");
    assert_eq!(
        take_offered(),
        vec!["lab_job_1".to_owned()],
        "the answer offers the handle; the worker issues it only when the answer goes out"
    );
    // The call was cancelled: the worker sends no answer and issues nothing.
    assert_eq!(
        jobs.outstanding().handles,
        vec!["lab_job_1".to_owned()],
        "a running job counts"
    );
    release.send(()).expect("the job waits");
    ended(&job);
    assert_eq!(
        jobs.outstanding(),
        Outstanding::default(),
        "a finished job whose handle never went out holds no switch back"
    );

    // A handle that went out counts until its result is read.
    let issued = jobs
        .start(
            "lab_job_2".to_owned(),
            "lab",
            None,
            "running",
            Vec::new(),
            || Ok(json!({})),
        )
        .unwrap_or_else(|_| panic!("the job starts"));
    jobs.issue(&["lab_job_2".to_owned()]);
    ended(&issued);
    assert_eq!(
        jobs.outstanding(),
        Outstanding {
            handles: vec!["lab_job_2".to_owned()],
            kinds: vec!["lab"],
        }
    );
    let _ = issued.snapshot();
    assert_eq!(jobs.outstanding(), Outstanding::default());
}

/// MCP-ENTRY fix round 1, test row E-17 (review M3): a run job counts while its submission
/// runs, and not once it ended (its handle is a Runtime request_id).
#[test]
fn e17_a_run_pack_job_counts_while_it_submits() {
    let jobs = Jobs::new();
    let (release, gate) = mpsc::channel::<()>();
    let job = jobs
        .start(
            "request-1".to_owned(),
            "run_pack",
            Some("instance-a".to_owned()),
            "submitting",
            Vec::new(),
            move || {
                let _ = gate.recv();
                Ok(json!({"receipt_state": "accepted"}))
            },
        )
        .unwrap_or_else(|_| panic!("the job starts"));
    assert_eq!(
        jobs.outstanding(),
        Outstanding {
            handles: vec!["request-1".to_owned()],
            kinds: vec!["run_pack"],
        },
        "a submission still running counts"
    );
    release.send(()).expect("the job waits");
    ended(&job);
    assert_eq!(jobs.outstanding(), Outstanding::default());
}

const ROOT: &str = r"X:\TestRoot";
const ENTRY: &str = r"X:\TestRoot\mcp-entry\g1\actingcommand-mcp-entry.exe";

fn args(client: &str) -> Value {
    json!([
        "--root",
        ROOT,
        "--client",
        client,
        "--",
        "mcp-serve",
        "--tier",
        "observer,operator,author"
    ])
}

fn server(client: Client) -> Value {
    let text = entry_snippet(client, ROOT, "observer,operator,author");
    let snippet: Value = serde_json::from_str(&text).expect("a JSON snippet");
    snippet["mcpServers"]["actingcommand"].clone()
}

#[test]
fn e12_mcp_config_entry_renders_each_clients_snippet() {
    assert_eq!(
        server(Client::Claude),
        json!({"type": "stdio", "command": ENTRY, "args": args("claude"), "env": {}})
    );
    assert_eq!(
        server(Client::Antigravity),
        json!({"command": ENTRY, "args": args("antigravity"), "timeoutSeconds": 60})
    );
    assert_eq!(
        server(Client::Dsh),
        json!({"command": ENTRY, "args": args("dsh")})
    );
    assert_eq!(
        entry_snippet(Client::Codex, ROOT, "observer,operator,author"),
        "[mcp_servers.actingcommand]\n\
         command = 'X:\\TestRoot\\mcp-entry\\g1\\actingcommand-mcp-entry.exe'\n\
         args = ['--root', 'X:\\TestRoot', '--client', 'codex', '--', 'mcp-serve', '--tier', 'observer,operator,author']\n\
         startup_timeout_sec = 10\n\
         tool_timeout_sec = 60\n"
    );
    // A trailing separator on --root does not double the entry's path separator.
    let text = entry_snippet(Client::Dsh, r"X:\TestRoot\", "observer");
    assert!(text.contains(r#""X:\\TestRoot\\mcp-entry\\g1\\actingcommand-mcp-entry.exe""#));
}
