// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 SD-1 (register §1.525; contracts/install-cli.md, "The close"; the coordinator's
// ruling 3 on board #503, test 5): the close step asks `request-shutdown` again after a refusal
// that says when to ask again (`retry_after_ms`, which the Runtime's `runtime_shutdown_settling`
// carries), as long as the asks and the wait stay within `install.stop.shutdown_wait_ms` (60 s by
// default), and lists every ask and its answer in the close step's evidence; a shutdown accepted
// on a later ask is an ordinary close. Any other refusal fails the step at once, as before, and a
// failed step's result in the journal carries the last answer as its cause and every earlier
// answer as a `related` link. The machine is the fake host of `fixture.rs` with its fake Runtime
// and fake clock: no device, no sleep; every root is a temporary folder with neutral files, and
// every refusal is one a Runtime's receipt carries, as received.

use super::fixture::{BuildSpec, C1, C2, FakeHost, code, live_root, run, word};
use super::host::Host;
use actingcommand_contract::outcome::Outcome;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The retry a settling answer carries in these tests.
const RETRY_MS: u64 = 1_000;

fn text<'a>(answer: &'a super::Answer, key: &str) -> Option<&'a str> {
    answer.outcome.value(key).and_then(Value::as_str)
}

fn assert_done(answer: &super::Answer, operation: &str) {
    assert_eq!(code(answer), "install_command_done", "{answer:?}");
    assert_eq!(text(answer, "operation"), Some(operation), "{answer:?}");
}

fn assert_refused(answer: &super::Answer, operation: &str, why: &str) {
    assert_eq!(code(answer), "install_command_refused", "{answer:?}");
    assert_eq!(text(answer, "operation"), Some(operation), "{answer:?}");
    assert_eq!(text(answer, "install_refusal"), Some(why), "{answer:?}");
}

/// A root whose live build is `C1` (adopted) with `C2` staged, as the deploy tests prepare it.
fn prepared(base: &Path, name: &str, host: &FakeHost) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    assert_done(
        &run(host, &["adopt", "--root", word(&root)]),
        "install_adopt",
    );
    let release = base.join(format!("{name}-release"));
    BuildSpec::new(C2).write_release(&release);
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
                "--local-build",
            ],
        ),
        "install_release_stage",
    );
    root
}

/// A refusal of `request-shutdown` as the module's client receives it from a Runtime's receipt.
fn received(document: Value) -> Outcome {
    serde_json::from_value(document).expect("a refusal as a receipt carries it")
}

/// The Runtime's settling answer: lifecycle work still in flight, ask again after `retry_after_ms`.
fn settling(retry_after_ms: u64) -> Outcome {
    received(json!({
        "code": "runtime_shutdown_settling",
        "category": "error",
        "values": {
            "runtime_code": "runtime_busy",
            "host_halted": false,
            "retry_after_ms": retry_after_ms,
        },
    }))
}

/// The Runtime's refusal of a shutdown that a held lease keeps busy: final.
fn denied() -> Outcome {
    received(json!({
        "code": "runtime_shutdown_denied",
        "category": "error",
        "values": {
            "runtime_code": "runtime_busy",
            "host_halted": false,
            "shutdown_obstacle": "lease",
        },
    }))
}

fn journal_of(root: &Path, operation_id: &str) -> Value {
    serde_json::from_slice(
        &fs::read(
            root.join("install")
                .join("operations")
                .join(format!("{operation_id}.json")),
        )
        .expect("the journal"),
    )
    .expect("journal JSON")
}

fn operation_id(answer: &super::Answer) -> String {
    text(answer, "install_operation_id")
        .unwrap_or_else(|| panic!("an operation id: {answer:?}"))
        .to_owned()
}

fn calls(host: &FakeHost) -> Vec<String> {
    host.runtime.calls.borrow().clone()
}

/// The close step of a journal.
fn close_step(journal: &Value) -> Value {
    journal["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["kind"] == "close"))
        .cloned()
        .unwrap_or_else(|| panic!("a close step: {journal}"))
}

/// The codes of the links of `relation` of the journal's result, the refusal the operation ended
/// with (`install_command_refused` with `install_refusal` `runtime_stop_failed`), in their order.
fn result_links(journal: &Value, relation: &str) -> Vec<String> {
    journal["result"]["causes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|link| link["relation"] == relation)
        .filter_map(|link| link["code"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn sd1_5_the_close_asks_again_after_each_settling_answer_and_its_evidence_lists_the_asks() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "settling", &host);
    host.runtime.boot(&["node.a"]);
    host.runtime
        .shutdown_refusals
        .borrow_mut()
        .extend([settling(RETRY_MS), settling(RETRY_MS)]);
    let before = host.now_unix_ms();

    let stopped = run(
        &host,
        &["stop", "--root", word(&root), "--hold", "deploy-sd1"],
    );
    assert_done(&stopped, "install_stop");
    assert_eq!(
        calls(&host),
        [
            "pause node.a deploy-sd1",
            "shutdown",
            "shutdown",
            "shutdown"
        ],
        "asked three times, the third accepted"
    );
    assert!(!host.runtime.running.get(), "the Runtime closed");
    assert!(
        host.now_unix_ms() - before >= 2 * RETRY_MS,
        "each ask again waited its retry_after_ms"
    );
    let close = close_step(&journal_of(&root, &operation_id(&stopped)));
    assert_eq!(close["state"], "done", "{close}");
    assert_eq!(
        close["evidence"]["asks"],
        json!([
            {"accepted": false, "refusal": "runtime_shutdown_settling", "retry_after_ms": RETRY_MS},
            {"accepted": false, "refusal": "runtime_shutdown_settling", "retry_after_ms": RETRY_MS},
            {"accepted": true},
        ]),
        "{close}"
    );
    assert_eq!(close["evidence"]["wait"]["state"], "completed", "{close}");
}

#[test]
fn sd1_5_any_other_refusal_fails_the_close_at_once_and_the_journal_shows_every_answer() {
    // A refusal that names no retry: one ask, the step fails `runtime_stop_failed` with it.
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "denied", &host);
    host.runtime.boot(&["node.a"]);
    host.runtime
        .shutdown_refusals
        .borrow_mut()
        .push_back(denied());
    let before = host.now_unix_ms();

    let refused = run(
        &host,
        &["stop", "--root", word(&root), "--hold", "deploy-sd1"],
    );
    assert_refused(&refused, "install_stop", "runtime_stop_failed");
    assert_eq!(calls(&host), ["pause node.a deploy-sd1", "shutdown"]);
    assert!(host.runtime.running.get(), "nothing closed");
    assert!(
        host.now_unix_ms() - before < RETRY_MS,
        "no wait before the step failed"
    );
    let journal = journal_of(&root, &operation_id(&refused));
    assert_eq!(close_step(&journal)["state"], "started", "{journal}");
    assert_eq!(
        result_links(&journal, "caused_by"),
        ["runtime_shutdown_denied"],
        "the answer is the failure's cause: {journal}"
    );
    assert!(
        result_links(&journal, "related").is_empty(),
        "no earlier answer: {journal}"
    );

    // A settling answer, then a final refusal: asked twice, the step fails with the final one as
    // its cause and the settling answer before it as a `related` link.
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "settling-then-denied", &host);
    host.runtime.boot(&["node.a"]);
    host.runtime
        .shutdown_refusals
        .borrow_mut()
        .extend([settling(RETRY_MS), denied()]);

    let refused = run(
        &host,
        &["stop", "--root", word(&root), "--hold", "deploy-sd1"],
    );
    assert_refused(&refused, "install_stop", "runtime_stop_failed");
    assert_eq!(
        calls(&host),
        ["pause node.a deploy-sd1", "shutdown", "shutdown"]
    );
    let journal = journal_of(&root, &operation_id(&refused));
    assert_eq!(
        result_links(&journal, "caused_by"),
        ["runtime_shutdown_denied"],
        "the last answer is the failure's cause: {journal}"
    );
    assert_eq!(
        result_links(&journal, "related"),
        ["runtime_shutdown_settling"],
        "the earlier answer: {journal}"
    );
}

#[test]
fn sd1_5_a_settling_answer_whose_retry_passes_the_wait_fails_the_close_at_once() {
    // `install.stop.shutdown_wait_ms` is 60 s: a retry of 120 s would ask again after it.
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "too-late", &host);
    host.runtime.boot(&["node.a"]);
    host.runtime
        .shutdown_refusals
        .borrow_mut()
        .push_back(settling(120_000));
    let before = host.now_unix_ms();

    let refused = run(
        &host,
        &["stop", "--root", word(&root), "--hold", "deploy-sd1"],
    );
    assert_refused(&refused, "install_stop", "runtime_stop_failed");
    assert_eq!(calls(&host), ["pause node.a deploy-sd1", "shutdown"]);
    assert!(host.runtime.running.get(), "nothing closed");
    assert!(
        host.now_unix_ms() - before < RETRY_MS,
        "no wait past the step's own wait"
    );
    let journal = journal_of(&root, &operation_id(&refused));
    assert_eq!(
        result_links(&journal, "caused_by"),
        ["runtime_shutdown_settling"],
        "the settling answer is the failure's cause: {journal}"
    );
}
