// SPDX-License-Identifier: AGPL-3.0-only

//! Test helper of `tests/serve_calls.rs` (Workflow #434 LAB-5G4b, `mcp_serve_children_pool`): a
//! made-up `actinglab` for the server's `serve` children. Copied beside a copy of the server as
//! its `actinglab` program, it is started as `actinglab serve --stdio --front mcp` and answers
//! each request line `{"op", "params"}` with the envelope `serve` answers under that option,
//! `{exit_code, line}`, its line naming this process and the request's row. Each process logs to
//! the folder `ACTINGCOMMAND_TEST_CHILD_LOG` names, in a file of its own, one JSON line per event:
//! its start (its words, whether `ACTINGCOMMAND_INITIATOR` is set, the state root variable), each
//! request it reads, and its end at its input's end. One row (`watch`) is held until the file
//! `release` appears in that folder; another (`record draft`) ends the process with a line on
//! stderr and exit 3, without an answer. Never shipped.

use serde_json::{Map, Value, json};
use std::fs::OpenOptions;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// The variable that names the folder this helper logs to.
const LOG_VARIABLE: &str = "ACTINGCOMMAND_TEST_CHILD_LOG";
/// The file whose presence in that folder releases a held request.
const RELEASE: &str = "release";
/// The row whose request is held until released, and the row whose request ends the process.
const HELD: &str = "watch";
const FAILED: &str = "record draft";
/// What the process writes on stderr before it ends without an answer, and its exit code.
const FAILURE_TEXT: &str = "neutral child failure text";
const FAILURE_EXIT: i32 = 3;
/// How often a held request looks for the release, and for how long at most.
const POLL: Duration = Duration::from_millis(20);
const HOLD_LIMIT: Duration = Duration::from_secs(120);

fn main() {
    let log = std::env::var_os(LOG_VARIABLE).map(PathBuf::from);
    let log = log.as_deref();
    let pid = std::process::id();
    let words = std::env::args().skip(1).collect::<Vec<_>>();
    record(
        log,
        pid,
        &json!({
            "event": "started",
            "pid": pid,
            "arguments": words,
            "initiator": std::env::var_os("ACTINGCOMMAND_INITIATOR").is_some(),
            "state_root": std::env::var("ACTINGCOMMAND_RUNTIME_STATE_ROOT").ok(),
        }),
    );
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        let request = serde_json::from_str::<Value>(&line).unwrap_or(Value::Null);
        let row = request
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        record(
            log,
            pid,
            &json!({"event": "request", "pid": pid, "op": row}),
        );
        if row == FAILED {
            eprintln!("{FAILURE_TEXT}");
            std::process::exit(FAILURE_EXIT);
        }
        if row == HELD {
            held(log);
        }
        let mut answer = Map::new();
        answer.insert("exit_code".to_owned(), json!(0));
        answer.insert("line".to_owned(), json!({"child_pid": pid, "op": row}));
        if writeln!(stdout, "{}", Value::Object(answer))
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break;
        }
    }
    record(log, pid, &json!({"event": "ended", "pid": pid}));
}

/// Waits until the release file appears in `log`, for at most [`HOLD_LIMIT`].
fn held(log: Option<&Path>) {
    let Some(log) = log else {
        return;
    };
    let release = log.join(RELEASE);
    let until = Instant::now() + HOLD_LIMIT;
    while !release.exists() && Instant::now() < until {
        thread::sleep(POLL);
    }
}

/// Appends `event` as one line to this process's file in `log`; a write that fails is said on
/// stderr.
fn record(log: Option<&Path>, pid: u32, event: &Value) {
    let Some(log) = log else {
        return;
    };
    let path = log.join(format!("child-{pid}.jsonl"));
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{event}"));
    if let Err(error) = written {
        eprintln!("cannot write {}: {error}", path.display());
    }
}
