// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-12 (register §1.264; contracts/runtime-interface.md, the command and tool
//! tables): `actingctl routine-summary` prints the Runtime's `RoutineSummary` page as its line's
//! members, with the completed receipt's code, and refuses a bad command line by name; MCP
//! `ac_routine_summary` is an observer tool that only reads and answers the same page. The
//! Runtime runs in-process on a fixed clock, with no instance; the counting itself is runtime-host's
//! (`gate_429_r12`, `routine_summary::tests`).

use super::super::jobs::Jobs;
use super::super::protocol::Era;
use super::super::runtime::RuntimeAccess;
use super::super::tools::{Tier, TierSet, ToolContext, definition, find};
use crate::held_runtime::{HELD_SALT, NoInstances};
use crate::routine_summary::{self, Ended};
use actingcommand_contract::codes::ReceiptCode;
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{EventActor, EventSource, RoutineSummaryGranularity};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use actingcommand_runtime_host::{
    RuntimeClock, RuntimeClockSample, RuntimeHost, RuntimeHostConfig, RuntimeHostResult,
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;

/// The fixed clock's wall-clock reading before any wait.
const START_UNIX_MS: u64 = 1_800_000_000_000;

/// The Runtime's clock: fixed, moved only by the waits the Runtime itself takes.
struct FixedClock {
    elapsed_ms: AtomicU64,
}

impl RuntimeClock for FixedClock {
    fn sample(&self) -> RuntimeHostResult<RuntimeClockSample> {
        let elapsed_ms = self.elapsed_ms.load(Ordering::SeqCst);
        Ok(RuntimeClockSample {
            unix_ms: START_UNIX_MS + elapsed_ms,
            monotonic_ms: elapsed_ms,
        })
    }

    fn pause(&self, duration: Duration) {
        let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.elapsed_ms.fetch_add(ms, Ordering::SeqCst);
    }
}

/// The Runtime on `root`, with no instance.
fn start(root: &Path) -> RuntimeHost {
    RuntimeHost::start(
        RuntimeHostConfig::new(root, HELD_SALT).with_runtime_clock(Arc::new(FixedClock {
            elapsed_ms: AtomicU64::new(0),
        })),
        Arc::new(NoInstances),
    )
    .expect("the Runtime starts")
}

/// A reader of the Runtime on `root` with the CLI's origin.
fn reader(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a reader")
}

/// `actingctl <words> --state-root <root>`, what the command reads.
fn arguments(root: &Path, words: &[&str]) -> Vec<OsString> {
    let mut arguments = words.iter().map(OsString::from).collect::<Vec<_>>();
    arguments.push(OsString::from("--state-root"));
    arguments.push(root.as_os_str().to_owned());
    arguments
}

/// The members `actingctl <words>` prints beside its outcome; a refusal fails the test.
fn cli(root: &Path, words: &[&str]) -> Map<String, Value> {
    match routine_summary::answer(&arguments(root, words), Detail::Full) {
        Ok(members) => members,
        Err(Ended::Usage(outcome)) => panic!("actingctl {words:?}: usage {outcome}"),
        Err(Ended::Failure(failure)) => {
            panic!("actingctl {words:?}: refused {}", failure.outcome())
        }
    }
}

/// The outcome `actingctl <words>` ends with when it answers nothing.
fn refusal(root: &Path, words: &[&str]) -> Outcome {
    match routine_summary::answer(&arguments(root, words), Detail::Full) {
        Ok(members) => panic!("actingctl {words:?} answered {members:?}"),
        Err(Ended::Usage(outcome)) => outcome,
        Err(Ended::Failure(failure)) => failure.outcome(),
    }
}

// `routine-summary --hour` and `--day`, with and without `--since`, print the page the Runtime
// answers to the same read, member for member, beside the completed receipt's code. No width,
// both widths, a width twice, a cursor that is no number and an unknown flag are usage errors
// that name the flag; a selector no instance has is `instance_unknown`.
#[test]
fn actingctl_summary_prints_the_page() {
    let root = TempDir::new().expect("tempdir");
    let host = start(root.path());
    let client = reader(root.path());
    for (flag, granularity) in [
        ("--hour", RoutineSummaryGranularity::Hour),
        ("--day", RoutineSummaryGranularity::Day),
    ] {
        for since_revision in [None, Some(3_u64)] {
            let since = since_revision.map(|revision| revision.to_string());
            let mut words = vec!["routine-summary", flag];
            if let Some(since) = &since {
                words.extend(["--since", since.as_str()]);
            }
            let printed = cli(root.path(), &words);
            let page = client
                .routine_summary(None, granularity, since_revision)
                .expect("the Runtime's page");
            assert_eq!(Value::Object(printed), json!(page), "{words:?}");
        }
    }
    let line = crate::cli_line::line(
        Some(cli(root.path(), &["routine-summary", "--day"])),
        &Outcome::new(ReceiptCode::RoutineSummaryCompleted),
        Detail::Full,
    )
    .expect("the line");
    assert_eq!(
        line["outcome"]["code"],
        json!("request_routine_summary_completed")
    );
    assert_eq!(line["granularity"], json!("day"));
    assert_eq!(line["instances"], json!([]));

    for (words, argument, problem) in [
        (&["routine-summary"][..], "--hour|--day", "required"),
        (
            &["routine-summary", "--hour", "--day"][..],
            "--day",
            "conflicting",
        ),
        (
            &["routine-summary", "--day", "--day"][..],
            "--day",
            "repeated",
        ),
        (
            &["routine-summary", "--day", "--since", "later"][..],
            "--since",
            "wrong_type",
        ),
        (&["routine-summary", "--week"][..], "--week", "unknown"),
    ] {
        let outcome = refusal(root.path(), words);
        assert_eq!(
            outcome.code().as_str(),
            "actingctl_usage_invalid",
            "{words:?}"
        );
        assert_eq!(
            outcome.value("argument"),
            Some(&json!(argument)),
            "{words:?}"
        );
        assert_eq!(
            outcome.value("argument_problem"),
            Some(&json!(problem)),
            "{words:?}"
        );
    }
    let missing = refusal(
        root.path(),
        &[
            "routine-summary",
            "--day",
            "--instance",
            "r12.instance.none",
        ],
    );
    assert_eq!(missing.code().as_str(), "instance_unknown");
    drop(client);
    host.close().expect("close the Runtime");
}

// `ac_routine_summary` is in the tool table on the observer tier, which every session serves,
// marked read-only, not destructive and idempotent, with `granularity` required. It answers the
// page the Runtime answers to the same read (`since` as the cursor), the page `actingctl
// routine-summary` prints; a width it does not know and a missing one are `arguments_invalid`,
// and a selector no instance has is `instance_unknown`.
#[test]
fn mcp_routine_summary_is_an_observer_read() {
    let tool = find("ac_routine_summary").expect("ac_routine_summary is in the tool table");
    assert_eq!(tool.tier, Tier::Observer);
    assert!(TierSet::default().contains(tool.tier));
    let listed = definition(tool, Era::Modern);
    assert_eq!(listed["annotations"]["readOnlyHint"], json!(true));
    assert_eq!(listed["annotations"]["destructiveHint"], json!(false));
    assert_eq!(listed["annotations"]["idempotentHint"], json!(true));
    assert_eq!(listed["inputSchema"]["required"], json!(["granularity"]));

    let root = TempDir::new().expect("tempdir");
    let host = start(root.path());
    let runtime = RuntimeAccess::new(None, Some(root.path().to_path_buf()));
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: super::gate_mcpt1::unreached_deadline(),
        era: Era::Modern,
        session: 1,
        jobs: &jobs,
    };
    let call = |arguments: Value| {
        let Value::Object(arguments) = arguments else {
            panic!("the arguments are an object");
        };
        (tool.run)(&context, &arguments)
    };
    let client = reader(root.path());
    for (word, flag, granularity) in [
        ("hour", "--hour", RoutineSummaryGranularity::Hour),
        ("day", "--day", RoutineSummaryGranularity::Day),
    ] {
        for since_revision in [None, Some(5_u64)] {
            let mut arguments = json!({"granularity": word});
            if let Some(since) = since_revision {
                arguments["since"] = json!(since);
            }
            let answered = match call(arguments) {
                Ok(success) => success,
                Err(error) => panic!("ac_routine_summary was refused: {}", error.outcome_ref()),
            };
            assert!(answered.warnings.is_empty(), "{:?}", answered.warnings);
            let page = client
                .routine_summary(None, granularity, since_revision)
                .expect("the Runtime's page");
            assert_eq!(answered.result, json!(page), "{word} {since_revision:?}");
        }
        let Ok(answered) = call(json!({"granularity": word})) else {
            panic!("ac_routine_summary {word} was refused");
        };
        let printed = cli(root.path(), &["routine-summary", flag]);
        assert_eq!(answered.result, Value::Object(printed), "{word}");
    }

    for arguments in [json!({"granularity": "week"}), json!({})] {
        let Err(error) = call(arguments.clone()) else {
            panic!("{arguments} was answered");
        };
        assert_eq!(
            error.outcome_ref().code().as_str(),
            "arguments_invalid",
            "{arguments}"
        );
    }
    let Err(error) = call(json!({"granularity": "day", "instance": "r12.instance.none"})) else {
        panic!("an unknown instance was answered");
    };
    assert_eq!(error.outcome_ref().code().as_str(), "instance_unknown");
    drop(client);
    host.close().expect("close the Runtime");
}
