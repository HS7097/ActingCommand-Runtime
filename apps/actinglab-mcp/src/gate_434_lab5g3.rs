// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #434 LAB-5G3 (register §1.287, "Slice LAB-5G3"; contracts/lab-two-entries.md §9.3; the
//! coordinator's work order `rd-1112/brief-lab5g3.md`, points 2 to 4): the start options of
//! the jobs, what happens at each cap, and the members the front adds to the generated tools, in
//! the server's own code. No Runtime, no clock, no sleep: the jobs here are made-up bodies, ended
//! by the test.
//!
//! - `mcp_job_start_options`: without the options, today's values (a call budget of 25 s, 8 jobs at
//!   once, 64 kept); the values given, at both ends of their ranges; 0, one past the range and -1
//!   refused `out_of_range` with the range, a word that is no whole number `wrong_type`, the option
//!   without its value `value_missing`, twice `repeated`, and `--jobs` and `--kept-jobs` beside
//!   `--list-tools` `conflicting`, each `actingctl_usage_invalid` naming the option, with the usage
//!   line; `--call-budget` beside `--list-tools` is taken. At each cap what today's server does:
//!   past `--jobs` a job is refused `job_capacity_exhausted` with `limit_count`; past `--kept-jobs`
//!   the oldest finished job is dropped and a running one never; a finished job whose handle never
//!   went out counts for nothing, one whose handle went out counts until its line was read. The
//!   front's added members are one table (`tools::ADDED`), none of them a parameter of its row or
//!   one the front gives; for a call budget given, `ac_run_show`'s and `ac_run_stop`'s `wait_s`
//!   take 0 to it, their defaults (0 and today's 20) never more than it, their descriptions name
//!   it; the instructions name the budget given where they named 25 s.

use crate::calls::Finished;
use crate::jobs::{JOB_PREFIX, Jobs, Limits, Outstanding};
use crate::tools::{self, Catalog, Listing};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_lab_ops::cli_line;
use actingcommand_lab_ops::locate::AuthoringModule;
use actingcommand_lab_ops::table::{self, OperationModule};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Bounds a broken run only.
const GATE_WAIT: Duration = Duration::from_secs(120);
const EVERY_MODULE: [OperationModule; 4] = [
    OperationModule::Core,
    OperationModule::Debugging,
    OperationModule::Authoring,
    OperationModule::Rescue,
];

/// The command line `words`, with the jobs' options, as the program reads its arguments.
fn parse_words(words: &[&str]) -> Result<(crate::Invocation, crate::JobOptions), crate::Ended> {
    let arguments = words
        .iter()
        .map(|word| OsString::from(*word))
        .collect::<Vec<_>>();
    crate::parse_all(&arguments)
}

/// The job options of a call budget of `seconds`, `running` jobs at once and `kept` kept.
fn options(seconds: u64, running: usize, kept: usize) -> crate::JobOptions {
    crate::JobOptions {
        call_budget: Duration::from_secs(seconds),
        limits: Limits { running, kept },
    }
}

/// A made-up job's line.
fn finished() -> Finished {
    Finished {
        line: json!({"outcome": {"code": "lab_operation_completed"}}),
        is_error: false,
    }
}

/// A place outside any installation root, where every module is listed.
fn unrooted() -> Result<AuthoringModule, Outcome> {
    Ok(AuthoringModule::Unrooted)
}

/// Each tool's input properties, for a call budget of `budget_s` seconds.
fn properties(budget_s: u64) -> BTreeMap<String, Value> {
    let table = table::operation_table().expect("the operation table this build embeds reads");
    Catalog::of(&table)
        .and_then(|catalog| catalog.with_call_budget(budget_s))
        .expect("the table's tools are generated")
        .listed(&Listing::Named(EVERY_MODULE.to_vec()), &unrooted)
        .into_iter()
        .map(|definition| {
            let name = definition["name"].as_str().unwrap_or_default().to_owned();
            (name, definition["inputSchema"]["properties"].clone())
        })
        .collect()
}

#[test]
fn mcp_job_start_options() {
    // Without the options, today's values; the values given, at both ends of their ranges.
    for (words, expected) in [
        (vec![], options(25, 8, 64)),
        (
            vec!["--call-budget", "1", "--jobs", "1", "--kept-jobs", "1"],
            options(1, 1, 1),
        ),
        (
            vec![
                "--call-budget",
                "300",
                "--jobs",
                "64",
                "--kept-jobs",
                "1024",
            ],
            options(300, 64, 1024),
        ),
        (
            vec!["--kept-jobs", "5", "--list", "core", "--call-budget", "7"],
            options(7, 8, 5),
        ),
        (
            vec!["--list-tools", "--call-budget", "7"],
            options(7, 8, 64),
        ),
    ] {
        let (_, given) = parse_words(&words).expect("the line parses");
        assert_eq!(given, expected, "{words:?}");
    }
    // Refused as `--workers` is refused.
    for (option, max) in [
        ("--call-budget", 300),
        ("--jobs", 64),
        ("--kept-jobs", 1024),
    ] {
        let past = (max + 1).to_string();
        for (words, problem, ranged) in [
            (vec![option, "0"], "out_of_range", true),
            (vec![option, past.as_str()], "out_of_range", true),
            (vec![option, "-1"], "out_of_range", true),
            (vec![option, "many"], "wrong_type", false),
            (vec![option, "2.5"], "wrong_type", false),
            (vec![option], "value_missing", false),
            (vec![option, "2", option, "3"], "repeated", false),
        ] {
            let ended = parse_words(&words).expect_err("the line is refused");
            let outcome = cli_line::outcome_value(&ended.outcome, Detail::Full);
            assert_eq!(outcome["code"], "actingctl_usage_invalid", "{words:?}");
            assert_eq!(outcome["values"]["argument"], option, "{words:?}");
            assert_eq!(outcome["values"]["argument_problem"], problem, "{words:?}");
            let range = (
                &outcome["values"]["range_min"],
                &outcome["values"]["range_max"],
            );
            if ranged {
                assert_eq!(range, (&json!(1), &json!(max)), "{words:?}: the range");
            } else {
                assert_eq!(range, (&Value::Null, &Value::Null), "{words:?}: no range");
            }
            assert_eq!(ended.text, Some(crate::USAGE), "{words:?}: the usage line");
        }
    }
    for option in ["--jobs", "--kept-jobs"] {
        let ended = parse_words(&["--list-tools", option, "2"]).expect_err("the line is refused");
        let outcome = cli_line::outcome_value(&ended.outcome, Detail::Full);
        assert_eq!(
            (
                &outcome["code"],
                &outcome["values"]["argument"],
                &outcome["values"]["argument_problem"],
            ),
            (
                &json!("actingctl_usage_invalid"),
                &json!(option),
                &json!("conflicting"),
            ),
            "{option} beside --list-tools"
        );
    }

    // At `--jobs`: the next job is refused, as today's server refuses it past its 8.
    let never = AtomicBool::new(false);
    let jobs = Jobs::new(
        Limits {
            running: 1,
            kept: 2,
        },
        0x5eed,
    );
    let (release, released) = mpsc::channel::<()>();
    let first = jobs
        .start(jobs.handle(), "ac_neutral", false, move || {
            released.recv().expect("the test releases the job");
            finished()
        })
        .expect("the first job starts");
    assert!(first.handle.starts_with(JOB_PREFIX), "{}", first.handle);
    let refused = jobs
        .start(jobs.handle(), "ac_neutral", false, finished)
        .expect_err("a job past --jobs is refused");
    let refusal = cli_line::outcome_value(&refused, Detail::Full);
    assert_eq!(
        (&refusal["code"], &refusal["values"]["limit_count"]),
        (&json!("job_capacity_exhausted"), &json!(1)),
        "{refusal}"
    );
    release.send(()).expect("release the first job");
    assert!(
        first.wait_until(Instant::now() + GATE_WAIT, &never),
        "the first job ends"
    );
    // At `--kept-jobs`: the oldest finished job goes first.
    let second = jobs
        .start(jobs.handle(), "ac_neutral", false, finished)
        .expect("a job starts once the first ended");
    assert!(second.wait_until(Instant::now() + GATE_WAIT, &never));
    let third = jobs
        .start(jobs.handle(), "ac_neutral", false, finished)
        .expect("a job starts once the second ended");
    assert!(third.wait_until(Instant::now() + GATE_WAIT, &never));
    assert!(
        jobs.find(&first.handle).is_none(),
        "past --kept-jobs the oldest finished job is dropped"
    );
    assert!(jobs.find(&second.handle).is_some() && jobs.find(&third.handle).is_some());
    assert_ne!(second.handle, third.handle, "a handle is never given twice");
    // What the MCP entry asks: a finished job whose handle never went out counts for nothing; one
    // whose handle went out counts until its line was read.
    assert_eq!(jobs.outstanding(), Outstanding::default());
    jobs.issue(&second.handle);
    assert_eq!(
        jobs.outstanding(),
        Outstanding {
            handles: vec![second.handle.clone()],
            kinds: vec!["ac_neutral".to_owned()],
        }
    );
    second.collect();
    assert_eq!(jobs.outstanding(), Outstanding::default());
    // A running job is never dropped, past `--kept-jobs` too.
    let running = Jobs::new(
        Limits {
            running: 2,
            kept: 1,
        },
        0x5eed,
    );
    let (release, released) = mpsc::channel::<()>();
    let (release_too, released_too) = mpsc::channel::<()>();
    let held = running
        .start(running.handle(), "ac_neutral", false, move || {
            released.recv().expect("the test releases the job");
            finished()
        })
        .expect("a job starts");
    let held_too = running
        .start(running.handle(), "ac_neutral", false, move || {
            released_too.recv().expect("the test releases the job");
            finished()
        })
        .expect("a second job starts");
    assert!(running.find(&held.handle).is_some() && running.find(&held_too.handle).is_some());
    assert_eq!(running.outstanding().handles.len(), 2, "running jobs count");
    release.send(()).expect("release the job");
    release_too.send(()).expect("release the job");
    assert!(held.wait_until(Instant::now() + GATE_WAIT, &never));
    assert!(held_too.wait_until(Instant::now() + GATE_WAIT, &never));

    // The front's added members: one table, none of them a parameter of its row or one the front
    // gives itself.
    assert_eq!(
        tools::ADDED.map(|(row, member, _)| (row, member)),
        [("run show", "wait_s"), ("run stop", "wait_s")]
    );
    let table = table::operation_table().expect("the operation table this build embeds reads");
    for (row, member, _) in tools::ADDED {
        let operation = table
            .operation(row)
            .unwrap_or_else(|| panic!("the table has no row {row}"));
        assert!(
            operation.params.iter().all(|param| param.name != member)
                && tools::supplied(row, member).is_none(),
            "{row}: {member} is the front's own"
        );
    }
    // Their properties name the call budget in effect.
    for (budget_s, stop_default) in [(25_u64, 20_u64), (7, 7)] {
        let schemas = properties(budget_s);
        let show = &schemas["ac_run_show"]["wait_s"];
        let stop = &schemas["ac_run_stop"]["wait_s"];
        for (wait, default) in [(show, 0), (stop, stop_default)] {
            assert_eq!(
                (
                    &wait["type"],
                    &wait["minimum"],
                    &wait["maximum"],
                    &wait["default"],
                ),
                (
                    &json!("integer"),
                    &json!(0),
                    &json!(budget_s),
                    &json!(default),
                ),
                "{wait}"
            );
            assert!(
                wait["description"]
                    .as_str()
                    .is_some_and(|text| text.contains(&format!("from 0 to {budget_s},"))),
                "the description names the budget: {wait}"
            );
        }
        let instructions = tools::instructions(budget_s);
        assert!(
            instructions.contains(&format!("within {budget_s} s"))
                && !instructions.contains("{call_budget_s}"),
            "the instructions name the budget: {instructions}"
        );
    }
    assert!(
        !tools::instructions(7).contains("25 s"),
        "no literal 25 s beside another budget"
    );
}
