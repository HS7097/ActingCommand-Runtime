// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-3 (register §1.287, the slice row LAB-3 and "Slice LAB-3"; contracts
// lab-two-entries.md §4.3, §5, §6, §7; coordinator work order `rd-1108/arch-013/brief-lab3.md`):
// - `rescue_without_actingd`: without `runtime-info.json` (no Runtime ran on the state root), the
//   rescue module's `op status`, `up` and `rollback` go through;
// - `irreversible_journal_initiator`: the journals of `deploy`, `rollback` and `config replace`
//   carry the initiator (`clean`, whose source R-15d has not built, is not served yet), and a
//   confirmation parameter (`--accept-ledger-one-way`, `--ui-confirmed`, `--ack-guard`,
//   `--known-pause`) is `actingctl_usage_invalid` (`unknown`);
// - `deploy_steps_in_order`: on the fake seams, stage → plan → stop → apply → start → alive →
//   release, in the answer's steps and in what the fake Runtime was asked;
// - `actingctl_install_answers_the_deploy_kit_as_before` (the work order's fourth): `actingctl
//   install` answers the coordinator's deploy kit's command lines as before LAB-3, its journals
//   without the rescue module's members.
// The machine is the fake host of `fixture.rs` with its fake Runtime, fake watchdog and fake
// clock: no device, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{BuildSpec, C1, C2, FakeHost, TAG, code, live_root, run, word, write_json};
use super::host::Host;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The initiator the tests run the module as: a Lab process the UI started (§4.1).
const WORD: Initiator = Initiator::LabRescue;

fn assert_done(answer: &super::Answer, operation: &str) {
    assert_eq!(code(answer), "install_command_done", "{answer:?}");
    assert_eq!(
        answer.outcome.value("operation").and_then(Value::as_str),
        Some(operation),
        "{answer:?}"
    );
}

/// A root whose live build is C1 (adopted), with the release folder of C2 beside it (its
/// release-facts file in the folder, as the download leaves it) and a start of C1 logged after
/// the adopt (the previous build's own open for a rollback's gate).
fn prepared(base: &Path, name: &str, host: &FakeHost) -> (PathBuf, PathBuf) {
    let root = live_root(base, name, &BuildSpec::new(C1));
    assert_done(
        &run(host, &["adopt", "--root", word(&root)]),
        "install_adopt",
    );
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

/// The rescue row `name` with `params` on `root`, as [`WORD`]; its one line and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    let location = location(root);
    let context = Context {
        host,
        location: &location,
        initiator: WORD,
    };
    let params = match params {
        Value::Object(params) => params,
        _ => Map::new(),
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

fn top_code(line: &Value) -> &str {
    line["outcome"]["code"].as_str().unwrap_or_default()
}

fn top_value<'a>(line: &'a Value, key: &str) -> &'a Value {
    &line["outcome"]["values"][key]
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

/// The journals written since `before` (their operation ids).
fn new_journals(root: &Path, before: &[(String, Value)]) -> Vec<(String, Value)> {
    journals(root)
        .into_iter()
        .filter(|(id, _)| !before.iter().any(|(known, _)| known == id))
        .collect()
}

fn calls(host: &FakeHost) -> Vec<String> {
    host.runtime.calls.borrow().clone()
}

/// The index of the first call that starts with `prefix`.
fn first(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .position(|call| call.starts_with(prefix))
        .unwrap_or_else(|| panic!("no call {prefix}: {calls:?}"))
}

/// The index of the last call that starts with `prefix`.
fn last(calls: &[String], prefix: &str) -> usize {
    calls
        .iter()
        .rposition(|call| call.starts_with(prefix))
        .unwrap_or_else(|| panic!("no call {prefix}: {calls:?}"))
}

// The register's `rescue_without_actingd`: with no Runtime (no `runtime-info.json` in the state
// root), `op status` reads the root, `up` starts the Runtime with the installer's start and its
// checks, and `rollback` (after an update) goes back to the build the live one replaced, files
// only, as the module's face journals it.
#[test]
fn rescue_without_actingd() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "offline", &host);
    let state_root = location(&root).state_root;
    assert!(!state_root.join("runtime-info.json").exists());

    let (status, exit) = rescue_line(&host, &root, "op status", json!({}));
    assert_eq!(exit, 0, "{status}");
    assert_eq!(top_code(&status), "install_command_done", "{status}");
    assert_eq!(top_value(&status, "operation"), "install_status");

    // A deploy without a Runtime: stage and plan, the update's files only.
    let (deployed, exit) = rescue_line(
        &host,
        &root,
        "deploy",
        json!({"target": TAG, "dir": word(&release)}),
    );
    assert_eq!(exit, 0, "{deployed}");
    assert_eq!(top_value(&deployed, "operation"), "install_deploy");
    assert_eq!(deployed["runtime_running"], json!(false));
    let names = deployed["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .filter_map(|step| step["step"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["stage", "plan", "apply"], "{deployed}");
    assert!(calls(&host).is_empty(), "nothing asked of a Runtime");

    // `rollback` goes back to C1, the build the live one replaced, without `--to`.
    let (rolled, exit) = rescue_line(&host, &root, "rollback", json!({}));
    assert_eq!(exit, 0, "{rolled}");
    assert_eq!(top_code(&rolled), "install_command_done", "{rolled}");
    assert_eq!(top_value(&rolled, "operation"), "install_rollback_apply");
    assert_eq!(top_value(&rolled, "commit_sha"), C1);

    // `up`: the installer's start, its post-start checks, journalled with the initiator.
    let before = journals(&root);
    let (up, exit) = rescue_line(&host, &root, "up", json!({}));
    assert_eq!(exit, 0, "{up}");
    assert_eq!(top_value(&up, "operation"), "install_start", "{up}");
    assert!(up["post_check"].is_object(), "{up}");
    assert!(host.runtime.running.get());
    let started = new_journals(&root, &before);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].1["kind"], "start");
    assert_eq!(started[0].1["initiator"], WORD.as_str());
}

// The register's `irreversible_journal_initiator`: every journal `deploy`, `rollback` and
// `config replace` write carries the initiator (an undo's record too); the confirmation
// parameters are refused as unknown arguments; `clean` is not served before its source is built.
#[test]
fn irreversible_journal_initiator() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "initiator", &host);
    host.runtime.boot(&["neutral.a"]);

    // deploy: the stage, the composite update and the release of its hold.
    let before = journals(&root);
    let (deployed, exit) = rescue_line(
        &host,
        &root,
        "deploy",
        json!({"target": TAG, "dir": word(&release)}),
    );
    assert_eq!(exit, 0, "{deployed}");
    let written = new_journals(&root, &before);
    let kinds = written
        .iter()
        .map(|(_, journal)| journal["kind"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        ["release_stage", "update", "release_hold"],
        "{deployed}"
    );
    for (id, journal) in &written {
        assert_eq!(journal["initiator"], WORD.as_str(), "{id}");
        assert_eq!(
            journal["acknowledged"],
            json!([]),
            "{id}: no acknowledgment"
        );
    }

    // rollback: to C1, the composite rollback and the release of its hold.
    let before = journals(&root);
    let (rolled, exit) = rescue_line(&host, &root, "rollback", json!({}));
    assert_eq!(exit, 0, "{rolled}");
    let written = new_journals(&root, &before);
    assert!(
        written
            .iter()
            .any(|(_, journal)| journal["kind"] == "rollback"),
        "{rolled}"
    );
    for (id, journal) in &written {
        assert_eq!(journal["initiator"], WORD.as_str(), "{id}");
    }

    // config replace: the file swapped, the replaced one kept, journalled with the initiator.
    let config = root.join("actingd.config.json");
    let candidate = temp.path().join("candidate.json");
    let document: Value =
        serde_json::from_slice(&fs::read(&config).expect("the configuration")).expect("JSON");
    fs::write(&candidate, serde_json::to_vec(&document).expect("JSON")).expect("the candidate");
    let before = journals(&root);
    let (replaced, exit) = rescue_line(
        &host,
        &root,
        "config replace",
        json!({"file": word(&candidate)}),
    );
    assert_eq!(exit, 0, "{replaced}");
    assert_eq!(top_value(&replaced, "operation"), "install_config_replace");
    let written = new_journals(&root, &before);
    let swap = written
        .iter()
        .find(|(_, journal)| journal["kind"] == "config_replace")
        .unwrap_or_else(|| panic!("a config_replace journal: {replaced}"));
    assert_eq!(swap.1["initiator"], WORD.as_str());
    for (id, journal) in &written {
        assert_eq!(journal["initiator"], WORD.as_str(), "{id}");
    }
    let backup = PathBuf::from(replaced["backup"].as_str().expect("the backup"));
    assert!(backup.is_file(), "the replaced configuration is kept");
    assert_eq!(
        fs::read(&config).expect("the live file"),
        fs::read(&candidate).expect("the candidate")
    );

    // `clean` comes with its source (R-15d); until then the module does not serve it.
    assert!(!rescue::serves("clean"));

    // A confirmation parameter given anyway is an unknown argument.
    for (name, params, argument) in [
        (
            "deploy",
            json!({"target": TAG, "accept_ledger_one_way": true}),
            "--accept-ledger-one-way",
        ),
        ("rollback", json!({"ui_confirmed": "all"}), "--ui-confirmed"),
        ("up", json!({"ack_guard": "memory"}), "--ack-guard"),
        (
            "down",
            json!({"known_pause": "neutral.a=other"}),
            "--known-pause",
        ),
        ("op status", json!({"root": word(&root)}), "--root"),
    ] {
        let (refused, exit) = rescue_line(&host, &root, name, params);
        assert_eq!(exit, 1, "{name}: {refused}");
        assert_eq!(top_code(&refused), "actingctl_usage_invalid", "{name}");
        assert_eq!(top_value(&refused, "argument"), argument, "{name}");
        assert_eq!(top_value(&refused, "argument_problem"), "unknown", "{name}");
    }
}

// The register's `deploy_steps_in_order`: with a Runtime running, `deploy <tag> --dir <release>`
// stages, plans (a preview), holds and stops the Runtime, applies the files, starts the new build
// and checks it, reads the watchdog's `alive`, and releases the hold, in this order.
#[test]
fn deploy_steps_in_order() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "steps", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    // Another party's pause: kept and listed, never refused (§5, §6).
    host.runtime.pause_as("neutral.b", "other-party");

    let (deployed, exit) = rescue_line(
        &host,
        &root,
        "deploy",
        json!({"target": TAG, "dir": word(&release)}),
    );
    assert_eq!(exit, 0, "{deployed}");
    assert_eq!(top_code(&deployed), "install_command_done", "{deployed}");
    assert_eq!(top_value(&deployed, "operation"), "install_deploy");
    assert_eq!(top_value(&deployed, "commit_sha"), C2);
    let names = deployed["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .filter_map(|step| step["step"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "stage", "plan", "stop", "apply", "start", "alive", "release"
        ],
        "{deployed}"
    );

    // What the Runtime was asked, in order: the hold, the close, the new build's start, the
    // release of the hold (only the operation's own: the other party's pause stays).
    let asked = calls(&host);
    let pause = last(&asked, "pause neutral.a lab-deploy-");
    let shutdown = first(&asked, "shutdown");
    let spawn = first(&asked, &format!("spawn {C2}"));
    let resume = first(&asked, "resume neutral.a");
    assert!(
        pause < shutdown && shutdown < spawn && spawn < resume,
        "{asked:?}"
    );
    assert!(
        !asked.iter().any(|call| call == "resume neutral.b"),
        "{asked:?}"
    );
    assert_eq!(
        host.runtime.pause_of("neutral.b").as_deref(),
        Some("other-party")
    );
    assert_eq!(host.runtime.pause_of("neutral.a"), None);
    // `alive` read the watchdog after the start.
    assert!(
        host.log
            .borrow()
            .iter()
            .any(|line| line == "watchdog status"),
        "{:?}",
        host.log.borrow()
    );
}

// The work order's fourth: `actingctl install` answers the coordinator's deploy kit's command
// lines as before LAB-3 (contracts/lab-two-entries.md §5: the revision-1 face until LAB-7). The
// lines, the exit codes, the detached command line, and journals without `initiator` and
// `one_way`, the acknowledgments as the flags gave them.
#[test]
fn actingctl_install_answers_the_deploy_kit_as_before() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "kit", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    host.runtime.pause_as("neutral.b", "alice-116");
    let facts = release.join("release-facts.json");

    // `install release stage --root <r> --dir <d> --facts <f> --expect-tag <t> --expect-commit <c>`.
    let staged = run(
        &host,
        &[
            "release",
            "stage",
            "--root",
            word(&root),
            "--dir",
            word(&release),
            "--facts",
            word(&facts),
            "--expect-tag",
            TAG,
            "--expect-commit",
            C2,
        ],
    );
    assert_done(&staged, "install_release_stage");
    assert_eq!(super::print(staged).code, 0);

    // `install update plan --root <r> --build <c> --preview`.
    let planned = run(
        &host,
        &[
            "update",
            "plan",
            "--root",
            word(&root),
            "--build",
            C2,
            "--preview",
        ],
    );
    assert_done(&planned, "install_update_plan");
    let plan = planned.members["plan_id"]
        .as_str()
        .expect("a plan id")
        .to_owned();

    // The kit's apply with `--detach`: answered at once, the same words run again without it.
    let apply = [
        "update",
        "apply",
        "--root",
        word(&root),
        "--build",
        C2,
        "--plan",
        plan.as_str(),
        "--stop",
        "--hold",
        "deploy-kit",
        "--known-pause",
        "neutral.b=alice-116",
        "--start",
    ];
    let mut detached_words = apply.to_vec();
    detached_words.push("--detach");
    let detached = run(&host, &detached_words);
    assert_done(&detached, "install_update_apply");
    assert_eq!(detached.members["detached"], json!(true));
    let canonical = fs::canonicalize(&root).expect("the canonical root");
    let plain = canonical
        .display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .to_owned();
    assert_eq!(
        detached.members["next"],
        json!({"status": ["install", "status", "--root", plain]})
    );
    let mut expected = vec!["install".to_owned()];
    expected.extend(apply.iter().map(|word| (*word).to_owned()));
    expected[4] = plain.clone();
    assert_eq!(host.detached.borrow().last(), Some(&expected));

    // The same apply in the foreground: the composite, its hold kept, the known pause kept.
    let before = journals(&root);
    let applied = run(&host, &apply);
    assert_done(&applied, "install_update_apply");
    assert_eq!(applied.members["hold"]["kept"], json!(true), "{applied:?}");
    let next = &applied.members["next"]["release_hold"];
    assert_eq!(
        next,
        &json!([
            "install",
            "release-hold",
            "--root",
            plain,
            "--reason",
            "deploy-kit"
        ])
    );
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 1);
    let journal = &written[0].1;
    assert!(journal.get("initiator").is_none(), "{journal}");
    assert!(journal.get("one_way").is_none(), "{journal}");
    assert_eq!(journal["acknowledged"], json!([]));
    assert_eq!(
        journal["inputs"]["known_pauses"],
        json!({"neutral.b": "alice-116"})
    );
    assert_eq!(super::print(applied).code, 0);

    // `install release-hold --root <r> --reason <h>`: only the hold's own instances.
    let released = run(
        &host,
        &[
            "release-hold",
            "--root",
            word(&root),
            "--reason",
            "deploy-kit",
        ],
    );
    assert_done(&released, "install_release_hold");
    assert_eq!(released.outcome.value("count"), Some(&json!(1)));
    assert_eq!(
        host.runtime.pause_of("neutral.b").as_deref(),
        Some("alice-116")
    );

    // `install status --root <r>`.
    let status = run(&host, &["status", "--root", word(&root)]);
    assert_done(&status, "install_status");
    assert_eq!(super::print(status).code, 0);

    // The confirmations `actingctl install` keeps until LAB-7: an unnamed foreign pause still
    // refuses the stop, and a missing `--plan` is still its usage error with its own usage text.
    host.runtime.pause_as("neutral.a", "unnamed-party");
    let refused = run(
        &host,
        &["stop", "--root", word(&root), "--hold", "deploy-kit-2"],
    );
    assert_eq!(code(&refused), "install_command_refused", "{refused:?}");
    assert_eq!(
        refused.outcome.value("install_refusal"),
        Some(&json!("guard_failed"))
    );
    let usage = run(
        &host,
        &["update", "apply", "--root", word(&root), "--build", C2],
    );
    assert_eq!(code(&usage), "actingctl_usage_invalid");
    assert!(
        usage
            .text
            .is_some_and(|text| text.starts_with("usage: actingctl install")),
        "{usage:?}"
    );
    assert_eq!(super::print(usage).code, 1);
}
