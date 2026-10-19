// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #382 MCP-T1 (test plan A-5a), the in-binary rows: the tool result envelope and
//! its output budget (inventory key 30) and the background jobs (key 31). Every expected value
//! comes from the frozen #338 model and its rulings, and since Workflow #378 A5 from model v2.2
//! §7.3 (a tool error is `{class, outcome}`; a job's result is `job_result`). Tool-error codes
//! are read only through `tool_error_code`.

use super::super::jobs::{Job, Jobs};
use super::super::protocol::{Era, LegacyVersion};
use super::super::tools::{ToolSuccess, call_result, find, tier_not_enabled};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

/// Bounds a broken run only; every wait returns as soon as its event arrives.
const GATE_WAIT: Duration = Duration::from_secs(120);
const KIND: &str = "gate";
const PHASE: &str = "gate_running";
/// Background jobs that may run at the same time (#338 §四 线程).
const MAX_RUNNING: usize = 8;

/// A deadline no test reaches. It only anchors `Job::wait_until`'s bound and measures nothing
/// (the #672 ruling's form, allowed in `gate_map.rs` `DETERMINISM_ALLOWLIST`).
pub(in crate::mcp) fn unreached_deadline() -> Instant {
    Instant::now() + Duration::from_secs(60 * 60)
}

/// The code of a tool error envelope `{ok: false, error {class, outcome}}`: its outcome's code.
#[track_caller]
fn tool_error_code(envelope: &Value) -> &str {
    envelope["error"]["outcome"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("not a tool error envelope: {envelope}"))
}

/// Whether `value` holds a member named `name` at any depth.
fn has_member(value: &Value, name: &str) -> bool {
    match value {
        Value::Object(members) => members
            .iter()
            .any(|(key, member)| key == name || has_member(member, name)),
        Value::Array(items) => items.iter().any(|item| has_member(item, name)),
        _ => false,
    }
}

/// A result whose envelope is far over the 11 KiB payload budget; every row names itself.
fn oversized_result() -> Value {
    json!({
        "rows": (0..800).map(|index| format!("oversized-row-{index:04}")).collect::<Vec<_>>(),
    })
}

/// The text content of a tool result, parsed.
#[track_caller]
fn text_envelope(answer: &Value) -> Value {
    let text = answer["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("a tool result without text content: {answer}"));
    serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("text content is not JSON ({error}): {answer}"))
}

/// A job whose body blocks until the returned sender sends or is dropped.
#[track_caller]
fn blocked_job(jobs: &Jobs, handle: &str, outcome: Value) -> (Arc<Job>, Sender<()>) {
    let (release, held) = mpsc::channel::<()>();
    let job = jobs
        .start(
            handle.to_owned(),
            KIND,
            None,
            PHASE,
            Vec::new(),
            move || {
                let _ = held.recv();
                Ok(outcome)
            },
        )
        .unwrap_or_else(|error| panic!("job {handle} did not start: {}", error.to_value()));
    (job, release)
}

/// `Job::wait_until(unreached_deadline(), cancelled)` on a second thread; its answer, read
/// within `GATE_WAIT`.
fn wait_on_second_thread(
    job: &Arc<Job>,
    cancelled: &Arc<AtomicBool>,
) -> Result<bool, mpsc::RecvTimeoutError> {
    let (answer, answered) = mpsc::channel();
    let job = Arc::clone(job);
    let cancelled = Arc::clone(cancelled);
    thread::spawn(move || {
        let _ = answer.send(job.wait_until(unreached_deadline(), &cancelled));
    });
    answered.recv_timeout(GATE_WAIT)
}

#[test]
fn gate_a_result_over_the_budget_is_output_budget_exceeded_and_one_inside_is_unchanged() {
    // (era, whether the era carries structuredContent)
    let eras = [
        (Era::Legacy(LegacyVersion::V20250326), false),
        (Era::Legacy(LegacyVersion::V20251125), true),
        (Era::Modern, true),
    ];
    for (era, structured) in eras {
        let answer = call_result(Ok(ToolSuccess::new(oversized_result())), era);
        assert_eq!(answer["isError"], json!(true), "{era:?}: {answer}");
        let mut envelopes = vec![text_envelope(&answer)];
        if structured {
            envelopes.push(answer["structuredContent"].clone());
        }
        for envelope in &envelopes {
            assert_eq!(envelope["ok"], json!(false), "{era:?}: {answer}");
            assert_eq!(
                tool_error_code(envelope),
                "output_budget_exceeded",
                "{era:?}: {answer}"
            );
            assert!(!has_member(envelope, "export"), "{era:?}: {answer}");
        }
        assert!(
            !answer.to_string().contains("oversized-row"),
            "{era:?}: part of the oversized result is in the answer"
        );
        assert!(!has_member(&answer, "export"), "{era:?}: {answer}");

        let inside = json!({"rows": ["inside-row-1", "inside-row-2"], "count": 2});
        let answer = call_result(Ok(ToolSuccess::new(inside.clone())), era);
        let expected = json!({"ok": true, "result": inside});
        assert_eq!(answer["isError"], json!(false), "{era:?}: {answer}");
        assert_eq!(
            answer["content"][0]["type"],
            json!("text"),
            "{era:?}: {answer}"
        );
        assert_eq!(
            answer["content"][0]["text"],
            json!(expected.to_string()),
            "{era:?}: the text content is not the compact JSON of the envelope"
        );
        if structured {
            assert_eq!(answer["structuredContent"], expected, "{era:?}: {answer}");
        }
    }
}

#[test]
fn gate_eight_jobs_run_a_ninth_is_refused_unrun_and_a_start_succeeds_once_one_ended() {
    let jobs = Jobs::new();
    let mut running = (0..MAX_RUNNING)
        .map(|index| blocked_job(&jobs, &format!("gate_job_{index}"), json!(index)))
        .collect::<Vec<_>>();

    let (ran, ran_signal) = mpsc::channel::<()>();
    let ninth = jobs.start(
        "gate_job_ninth".to_owned(),
        KIND,
        None,
        PHASE,
        Vec::new(),
        move || {
            let _ = ran.send(());
            Ok(Value::Null)
        },
    );
    assert!(ninth.is_err(), "a ninth job started while eight ran");
    assert_eq!(
        ran_signal.try_recv(),
        Err(TryRecvError::Disconnected),
        "the refused ninth body ran"
    );

    let (first, release_first) = running.remove(0);
    release_first.send(()).expect("release the first body");
    assert_eq!(
        wait_on_second_thread(&first, &Arc::new(AtomicBool::new(false))),
        Ok(true),
        "the released first job did not end"
    );
    running.push(blocked_job(&jobs, "gate_job_after", json!("after")));

    for (_, release) in running {
        release.send(()).expect("release a body");
    }
}

#[test]
fn gate_find_returns_a_started_job_by_its_handle_and_none_for_one_never_started() {
    let jobs = Jobs::new();
    let (_job, release) = blocked_job(&jobs, "gate_job_found", json!("found"));
    let found = jobs
        .find("gate_job_found")
        .unwrap_or_else(|| panic!("a started job is not found by its handle"));
    assert_eq!(found.handle, "gate_job_found");
    assert_eq!(found.kind, KIND);
    assert!(
        jobs.find("gate_job_never_started").is_none(),
        "a handle never started is found"
    );
    release.send(()).expect("release the body");
}

#[test]
fn gate_a_cancelled_wait_returns_false_and_leaves_the_job_running() {
    let jobs = Jobs::new();
    let outcome = json!({"released": true});
    let (job, release) = blocked_job(&jobs, "gate_job_waited", outcome.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let (answer, answered) = mpsc::channel();
    {
        let job = Arc::clone(&job);
        let cancelled = Arc::clone(&cancelled);
        thread::spawn(move || {
            let _ = answer.send(job.wait_until(unreached_deadline(), &cancelled));
        });
    }
    cancelled.store(true, Ordering::SeqCst);
    assert_eq!(
        answered.recv_timeout(GATE_WAIT),
        Ok(false),
        "a cancelled wait did not return false"
    );
    assert!(!job.finished(), "the job ended with the cancelled wait");
    assert_eq!(
        job.phase(),
        PHASE,
        "the cancelled wait changed the job's phase"
    );

    release.send(()).expect("release the body");
    cancelled.store(false, Ordering::SeqCst);
    assert_eq!(
        wait_on_second_thread(&job, &cancelled),
        Ok(true),
        "a wait with a cleared flag did not see the job end"
    );
    let snapshot = job.snapshot();
    assert_eq!(snapshot["phase"], json!("done"), "{snapshot}");
    // Workflow #378 A5 (L6): the job's result is `job_result`; `outcome` names only the
    // envelope.
    assert_eq!(snapshot["job_result"], outcome, "{snapshot}");
    assert!(snapshot.get("outcome").is_none(), "{snapshot}");
}

#[test]
fn gate_a_tool_error_is_exactly_class_and_outcome_and_tier_not_enabled_names_its_blocker() {
    // Workflow #378 A5 (model v2.2 §7.3): a tool error is `{class, outcome}` and nothing else;
    // what must change is the value `mcp_blocker`, and `tier_not_enabled` names the tool's
    // tier (`required_tier`).
    let tool = find("ac_run_pack").expect("ac_run_pack is in the tool table");
    for era in [
        Era::Legacy(LegacyVersion::V20250326),
        Era::Legacy(LegacyVersion::V20251125),
        Era::Modern,
    ] {
        let answer = call_result(Err(tier_not_enabled(tool)), era);
        assert_eq!(answer["isError"], json!(true), "{era:?}: {answer}");
        let mut envelopes = vec![text_envelope(&answer)];
        if era != Era::Legacy(LegacyVersion::V20250326) {
            envelopes.push(answer["structuredContent"].clone());
        }
        for envelope in &envelopes {
            let error = &envelope["error"];
            let members = error
                .as_object()
                .map(|members| members.keys().map(String::as_str).collect::<BTreeSet<_>>())
                .unwrap_or_default();
            assert_eq!(
                members,
                BTreeSet::from(["class", "outcome"]),
                "{era:?}: {answer}"
            );
            assert_eq!(error["class"], json!("safety"), "{era:?}: {answer}");
            let outcome = &error["outcome"];
            assert_eq!(
                outcome["code"],
                json!("tier_not_enabled"),
                "{era:?}: {answer}"
            );
            assert_eq!(outcome["category"], json!("error"), "{era:?}: {answer}");
            assert_eq!(
                outcome["values"]["mcp_blocker"],
                json!("server_tier"),
                "{era:?}: {answer}"
            );
            assert_eq!(
                outcome["values"]["required_tier"],
                json!("operator"),
                "{era:?}: {answer}"
            );
        }
    }
}
