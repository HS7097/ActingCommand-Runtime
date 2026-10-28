// SPDX-License-Identifier: AGPL-3.0-only

// #411 P49 (brief rd-1028/brief-wave1.md W1-10; board #411 6444748093, #421 6436231390; review
// rd-1027/mcp-entry/review.md L4; production 10-27: the status file of an entry that ended at
// 17:35 still listed LIVE after 18:54, when a console host took its pid): `actingctl mcp-entry
// list` judges an entry's status file by its pid and the process's creation time.
// - An entry that recorded its creation time (`process_created_at_unix_ms`) is live only while
//   the process with its pid has that creation time; a reused pid is stale.
// - A file without it (an entry before v0.12.25, or one whose own probe failed) is live only
//   while the process with its pid was created no later than the file's `started_at_unix_ms`.
// - A pid no process runs with is stale, as before; stale files are listed only with `--all`.
// - Fix round 1 (brief rd-1028/brief-w1-10-fix1.md, review rd-1028/review-w1-10.md M1): a pid
//   the probe cannot judge (a non-elevated session is refused the query for a process of
//   another account: 174 of 868 locally, console hosts among them) is that row's answer, never
//   the whole list's: `stale: null` and `probe_failed`, listed with or without `--all`, and one
//   warning of the answer names it.
// The probe is a fake answering for each pid; the root is a temporary folder.

use super::fixture::{code, word};
use super::mcp_entry::list_with;
use crate::process_probe::created_at_of;
use actingcommand_contract::outcome::Outcome;
use actingcommand_host_metrics::{HostMetricsCode, ProcessProbe};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Writes the status file of an entry with `pid`.
fn status_file(root: &Path, pid: u32, status: &Value) {
    let directory = root.join("mcp-entry").join("state");
    fs::create_dir_all(&directory).expect("the state folder");
    fs::write(
        directory.join(format!("entry-{pid}.json")),
        serde_json::to_vec_pretty(status).expect("a status"),
    )
    .expect("a status file");
}

/// The fake probe: the creation time of the process now running with each pid.
fn created_at(pid: u32) -> Result<Option<u64>, Outcome> {
    Ok(match pid {
        // The entry itself, and an entry before the field that runs still.
        101 => Some(5_000),
        104 => Some(5_990),
        // Processes created after the entries that had these pids ended.
        102 | 103 => Some(9_000),
        // No process runs with this pid.
        _ => None,
    })
}

/// The `file` and `stale` members of the listed entries.
fn listed(root: &Path, all: bool) -> Vec<(String, bool)> {
    let mut words = vec!["--root", word(root)];
    if all {
        words.push("--all");
    }
    let answer = list_with(&words, &created_at).expect("the list");
    assert_eq!(code(&answer), "install_command_done", "{answer:?}");
    answer.members["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["file"].as_str().expect("a file").to_owned(),
                entry["stale"].as_bool().expect("stale"),
            )
        })
        .collect()
}

#[test]
fn p49_l4_the_list_tells_a_reused_pid_from_the_entry_by_its_creation_time() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    // Files with the creation time the entry recorded.
    let recorded = |pid: u32| {
        json!({
            "pid": pid,
            "process_created_at_unix_ms": 5_000,
            "started_at_unix_ms": 5_010,
        })
    };
    status_file(root, 101, &recorded(101));
    status_file(root, 102, &recorded(102));
    // Files without the creation time.
    let unrecorded = |pid: u32| json!({"pid": pid, "started_at_unix_ms": 6_000});
    status_file(root, 103, &unrecorded(103));
    status_file(root, 104, &unrecorded(104));
    status_file(root, 105, &recorded(105));

    assert_eq!(
        listed(root, true),
        vec![
            ("entry-101.json".to_owned(), false),
            ("entry-102.json".to_owned(), true),
            ("entry-103.json".to_owned(), true),
            ("entry-104.json".to_owned(), false),
            ("entry-105.json".to_owned(), true),
        ]
    );
    assert_eq!(
        listed(root, false),
        vec![
            ("entry-101.json".to_owned(), false),
            ("entry-104.json".to_owned(), false),
        ],
        "without --all only the live entries"
    );

    // The operating system's probe names this test's own process with its creation time.
    if cfg!(windows) {
        assert!(
            crate::process_probe::pid_created_at(std::process::id())
                .expect("the probe")
                .is_some(),
            "a running process has a creation time"
        );
    }
}

#[test]
fn p49_m1_a_pid_the_probe_cannot_judge_marks_its_row_and_the_list_still_answers() {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path();
    let recorded = |pid: u32| {
        json!({
            "pid": pid,
            "process_created_at_unix_ms": 5_000,
            "started_at_unix_ms": 5_010,
        })
    };
    // The entry itself, a pid another process took, and a pid whose process the operating
    // system refuses to describe.
    status_file(root, 101, &recorded(101));
    status_file(root, 102, &recorded(102));
    status_file(root, 106, &recorded(106));
    let probe = |pid: u32| match pid {
        106 => created_at_of(
            pid,
            ProcessProbe::Unknown(HostMetricsCode::ProcessQueryRefused.code()),
        ),
        other => created_at(other),
    };

    for all in [true, false] {
        let mut words = vec!["--root", word(root)];
        if all {
            words.push("--all");
        }
        let answer = list_with(&words, &probe).expect("the list answers");
        assert_eq!(code(&answer), "install_command_done", "{answer:?}");
        let entries = answer.members["entries"].as_array().expect("entries");
        let rows = entries
            .iter()
            .map(|entry| {
                (
                    entry["file"].as_str().expect("a file"),
                    entry["stale"].clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut expected = vec![("entry-101.json", json!(false))];
        if all {
            expected.push(("entry-102.json", json!(true)));
        }
        expected.push(("entry-106.json", Value::Null));
        assert_eq!(rows, expected, "--all: {all}");

        // The row whose probe failed says so, with its pid and the probe's reason.
        let failed = entries.last().expect("the row of pid 106");
        let probe_failed = &failed["probe_failed"]["outcome"];
        assert_eq!(probe_failed["code"], "process_probe_failed", "{failed}");
        assert_eq!(probe_failed["values"]["pid"], 106, "{failed}");
        assert_eq!(
            probe_failed["causes"][0]["code"], "process_query_refused",
            "{failed}"
        );
        assert_eq!(failed["status"]["pid"], 106, "{failed}");
        for judged in &entries[..entries.len() - 1] {
            assert!(judged.get("probe_failed").is_none(), "{judged}");
        }

        // The answer carries one warning, which names that row by its pid.
        let warnings = answer.members["warnings"].as_array().expect("warnings");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let warning = &warnings[0]["outcome"];
        assert_eq!(warning["code"], "process_probe_failed", "{warning}");
        let named = warning["causes"]
            .as_array()
            .expect("the rows")
            .iter()
            .map(|row| (row["code"].clone(), row["values"]["pid"].clone()))
            .collect::<Vec<_>>();
        assert_eq!(named, vec![(json!("process_probe_failed"), json!(106))]);
    }

    // A list whose every probe answers carries no warnings.
    let answer = list_with(&["--root", word(root)], &created_at).expect("the list");
    assert!(!answer.members.contains_key("warnings"), "{answer:?}");
}
