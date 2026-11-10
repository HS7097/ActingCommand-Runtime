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
//   without the rescue module's members; with fix 1 (`rd-1108/arch-013/brief-lab3-fix1.md`, item
//   5) also the kit's `guards … --known-pause`, `update plan` without `--preview` and `status
//   --operation`, and the members the kit's scripts read.
// Fix 1 (`brief-lab3-fix1.md`):
// - `rescue_hands_back_only_its_own_hold` (item 1): another party's `alice-116-scheduler-stop`
//   is never released nor adopted; `down --reason r1`, `up`, `release-hold r1` resume only what
//   the module paused, and (adapted by fix 2, which matches the reason and the alias, not the
//   revision) a pause of the hold that another party replaced with its own reason is left;
// - `rescue_renames_its_own_running_image` (item 2): the running image of the process that plans
//   is renamed aside like `actingctl.exe`, another running `actinglab.exe` still blocks;
// - `place_holds_only_the_bundles_idle_instances` (item 4): on a neutral bundle, only the idle
//   instance of the bundle's game is paused and resumed, the other party's pause and the other
//   game's instance are untouched, `.old-<ms>` is kept, the journal carries the initiator, and
//   `undo` restores the game root; with #428 TB-R1 the bundle also replaces a table, and after
//   the placement and after `undo` the table reader lists that table alone.
// Fix 2 (`brief-lab3-fix2.md`):
// - `rescue_hands_back_its_own_hold_across_a_restart` (item 2): a deploy whose start times out
//   records no new owner epoch; while no Runtime answers the module's `release-hold` refuses with
//   the hold kept and the ways out named; after another hand starts the Runtime (the restore
//   numbers the pauses anew), `release-hold <its reason>` resumes exactly the instances its pause
//   step paused, another party's `alice-116-scheduler-stop` and a later pause of another alias
//   with another reason untouched;
// - `rescue_refused_after_its_pause_hands_its_own_back` and
//   `rescue_keeps_a_global_pause_and_goes_on` (item 3, fix 1's item 3): a refusal after the
//   operation paused A hands A back, leaves the other party's pause and says `kept: false` only
//   because nothing of its own stays paused; a global pause is kept, listed, and the stop goes on.
// Fix 3 (`brief-lab3-fix3.md`):
// - `rescue_notes_a_hold_whose_ownership_cannot_be_read` (item 2): a `release-hold` in `next`
//   whose ownership cannot be read is left out and noted (`hold_ownership_unconfirmed`), one not
//   owned is left out without a note.
// The machine is the fake host of `fixture.rs` with its fake Runtime, fake watchdog and fake
// clock: no device, no sleep; every root is a temporary folder with neutral files.

use super::fixture::{
    BuildSpec, C1, C2, FakeHost, FakeStart, TAG, code, live_root, run, speaks, word, write_json,
};
use super::host::{Host, RunningImage};
use crate::agent_channel::Answer as Answered;
use crate::codes::InstallLocation;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
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

    // precheck.ps1 and guards_summary.ps1: `install guards --root <r> --for stop --hold <h>
    // --known-pause <alias>=<reason>`; the kit reads each row's `id` and `result`.
    let guarded = run(
        &host,
        &[
            "guards",
            "--root",
            word(&root),
            "--for",
            "stop",
            "--hold",
            "deploy-kit",
            "--known-pause",
            "neutral.b=alice-116",
        ],
    );
    assert_done(&guarded, "install_guards_report");
    let rows = guarded.members["guards"]
        .as_array()
        .expect("the guard rows")
        .clone();
    assert!(!rows.is_empty(), "{guarded:?}");
    for row in &rows {
        assert!(row["id"].is_string() && row["result"].is_string(), "{row}");
    }
    let foreign = rows
        .iter()
        .find(|row| row["id"] == "foreign_pauses")
        .expect("the foreign_pauses row");
    assert_eq!(foreign["result"], "pass", "{foreign}");
    assert_eq!(super::print(guarded).code, 0);

    // deploy.ps1's `plan`: `install update plan --root <r> --build <c>` without `--preview` is
    // refused while the Runtime runs.
    let unpreviewed = run(
        &host,
        &["update", "plan", "--root", word(&root), "--build", C2],
    );
    assert_eq!(
        code(&unpreviewed),
        "install_command_refused",
        "{unpreviewed:?}"
    );
    assert_eq!(
        unpreviewed.outcome.value("install_refusal"),
        Some(&json!("runtime_running"))
    );
    assert_eq!(super::print(unpreviewed).code, 1);

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
    // summarize_plan.py and plan_members.py: the members they read from the preview's line.
    let line = super::print(planned.clone())
        .line
        .expect("the preview's line");
    assert_eq!(line["outcome"]["code"], "install_command_done");
    assert!(line["outcome"]["category"].is_string(), "{line}");
    assert_eq!(line["plan_id"], json!(plan));
    assert_eq!(line["kind"], "update");
    let document = &line["plan"];
    assert!(document["inputs"].is_object(), "{document}");
    assert_eq!(document["live"]["commit"], C1);
    assert_eq!(document["target"]["commit"], C2);
    assert!(document["live"]["speaks"].is_object(), "{document}");
    assert!(document["target"]["speaks"].is_object(), "{document}");
    let edges = document["edges"].as_array().expect("the edges");
    assert!(!edges.is_empty(), "{document}");
    for edge in edges {
        assert!(
            edge["interface"].is_string()
                && edge["check"].is_string()
                && edge["result"].is_string(),
            "{edge}"
        );
    }
    assert!(document["ledger_verify"].is_boolean(), "{document}");
    assert!(document["check_config"].is_object(), "{document}");
    assert!(document["extra"].is_array(), "{document}");
    assert!(document["config_sha256"].is_string(), "{document}");
    assert!(document["files"].is_array(), "{document}");

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
    let applied_id = written[0].0.clone();
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

    // `install status --root <r>`: guards_summary.ps1 and wait_live.ps1 read `live.commit`,
    // `live.operation_id` and each kept build's `commit`, `tag` and `operation_id`.
    let status = run(&host, &["status", "--root", word(&root)]);
    assert_done(&status, "install_status");
    assert_eq!(status.members["live"]["commit"], C2);
    assert_eq!(status.members["live"]["operation_id"], json!(applied_id));
    let kept = status.members["builds"]
        .as_array()
        .expect("the kept builds")
        .iter()
        .find(|build| build["commit"] == C2)
        .expect("the staged build")
        .clone();
    assert_eq!(kept["tag"], TAG);
    assert!(kept["operation_id"].is_string(), "{kept}");
    assert_eq!(super::print(status).code, 0);

    // deploy.ps1's `status -Operation <id>`: `install status --root <r> --operation <id>`.
    let followed = run(
        &host,
        &[
            "status",
            "--root",
            word(&root),
            "--operation",
            applied_id.as_str(),
        ],
    );
    assert_done(&followed, "install_status");
    assert_eq!(
        followed.members["journal"]["operation_id"],
        json!(applied_id)
    );
    assert_eq!(followed.members["live"]["commit"], C2);
    assert_eq!(super::print(followed).code, 0);

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

/// The pause `alias` shows on the fake Runtime: its reason and its revision.
fn pause_seen(host: &FakeHost, alias: &str) -> Option<(String, u64)> {
    host.runtime
        .instances
        .borrow()
        .iter()
        .find(|instance| instance.alias == alias)
        .and_then(|instance| instance.pause.as_ref())
        .map(|pause| (pause.reason.as_str().to_owned(), pause.revision))
}

/// Whether a `next` form of the line names `reason`.
fn suggests(line: &Value, reason: &str) -> bool {
    line["next"]
        .as_object()
        .is_some_and(|next| next.values().any(|form| form.to_string().contains(reason)))
}

/// Every call of the fake Runtime that starts with `prefix`.
fn calls_of(host: &FakeHost, prefix: &str) -> Vec<String> {
    calls(host)
        .into_iter()
        .filter(|call| call.starts_with(prefix))
        .collect()
}

/// Another party's stop, as production carries it on its instances.
const OTHER_STOP: &str = "alice-116-scheduler-stop";

// Fix 1, item 1 (contract §5; install-cli.md, "The Lab's rescue module"): the rescue module hands
// back only its own hold. Instance A carries another party's pause `alice-116-scheduler-stop`,
// instance B is idle.
#[test]
fn rescue_hands_back_only_its_own_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "own", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    host.runtime.pause_as("neutral.a", OTHER_STOP);
    let other = pause_seen(&host, "neutral.a");
    assert!(other.is_some());

    // `release-hold alice-116-scheduler-stop`: refused; nothing is resumed or journalled.
    let before = journals(&root);
    let (refused, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": OTHER_STOP}));
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(top_code(&refused), "install_command_refused", "{refused}");
    assert_eq!(top_value(&refused, "install_refusal"), "hold_not_owned");
    assert_eq!(top_value(&refused, "hold_reason"), OTHER_STOP);
    assert!(!suggests(&refused, OTHER_STOP), "{refused}");
    assert!(new_journals(&root, &before).is_empty());

    // `down --reason alice-116-scheduler-stop`: refused before anything is paused.
    let (refused, exit) = rescue_line(&host, &root, "down", json!({"reason": OTHER_STOP}));
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "hold_not_owned",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "aliases"), &json!(["neutral.a"]));
    assert!(!suggests(&refused, OTHER_STOP), "{refused}");
    assert!(new_journals(&root, &before).is_empty());
    assert!(host.runtime.running.get());
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));
    assert_eq!(pause_seen(&host, "neutral.a"), other);
    assert_eq!(pause_seen(&host, "neutral.b"), None);

    // `down --reason r1` holds B only and closes the Runtime; `up` starts it again, whose restore
    // numbers the pauses anew; `release-hold r1` resumes B only, and A's pause is unchanged.
    let (down, exit) = rescue_line(&host, &root, "down", json!({"reason": "r1"}));
    assert_eq!(exit, 0, "{down}");
    assert_eq!(host.runtime.pause_of("neutral.b").as_deref(), Some("r1"));
    assert_eq!(pause_seen(&host, "neutral.a"), other);
    assert!(!suggests(&down, OTHER_STOP), "{down}");
    let (up, exit) = rescue_line(&host, &root, "up", json!({}));
    assert_eq!(exit, 0, "{up}");
    let (released, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": "r1"}));
    assert_eq!(exit, 0, "{released}");
    assert_eq!(top_code(&released), "install_command_done", "{released}");
    assert_eq!(top_value(&released, "count"), &json!(1));
    assert_eq!(calls_of(&host, "resume "), ["resume neutral.b"]);
    assert_eq!(pause_seen(&host, "neutral.b"), None);
    assert_eq!(pause_seen(&host, "neutral.a"), other);

    // A pause the module set that another party lifted and replaced with a reason of its own is no
    // longer the module's: left as it is, and nothing of the module's stays to hand back. (Fix 2
    // adapted this part: the module matches the reason and the alias, since the Runtime's restore
    // numbers the pauses anew; a pause showing the module's reason at another revision is its own,
    // `rescue_hands_back_its_own_hold_across_a_restart`.)
    let (down, exit) = rescue_line(&host, &root, "down", json!({"reason": "r2"}));
    assert_eq!(exit, 0, "{down}");
    let (up, exit) = rescue_line(&host, &root, "up", json!({}));
    assert_eq!(exit, 0, "{up}");
    host.runtime.pause_as("neutral.b", "other-later");
    let again = pause_seen(&host, "neutral.b");
    let (left, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": "r2"}));
    assert_eq!(exit, 0, "{left}");
    assert_eq!(top_code(&left), "install_command_done", "{left}");
    assert_eq!(left["release"]["resumed"], json!([]), "{left}");
    assert_eq!(pause_seen(&host, "neutral.b"), again);
    assert_eq!(pause_seen(&host, "neutral.a"), other);
    assert_eq!(calls_of(&host, "resume "), ["resume neutral.b"]);
}

/// Whether the line carries, as its top or as a link at any depth, an outcome `code` whose value
/// `key` is `value`.
fn carries(line: &Value, code: &str, key: &str, value: &Value) -> bool {
    match line {
        Value::Object(members) => {
            (members.get("code").and_then(Value::as_str) == Some(code)
                && members.get("values").and_then(|values| values.get(key)) == Some(value))
                || members
                    .values()
                    .any(|inner| carries(inner, code, key, value))
        }
        Value::Array(items) => items.iter().any(|inner| carries(inner, code, key, value)),
        _ => false,
    }
}

/// The root as the installer spells it in its output (`paths.plain`).
fn plain_root(root: &Path) -> String {
    super::files::plain(&fs::canonicalize(root).expect("the canonical root"))
}

// Fix 2, item 2 (`brief-lab3-fix2.md`; install-cli.md, "The Lab's rescue module"): a deploy whose
// start times out records no new owner epoch. The real Runtime's restore numbers every pause anew
// (`crates/runtime-host/src/host/scheduling_pause.rs`, revision 1 for a start that is not
// installer-held), so what is the module's own is its pause step's reason and aliases. A paused by
// another party with `alice-116-scheduler-stop`; B and C idle, held by the deploy; D registered
// later and paused with another reason.
#[test]
fn rescue_hands_back_its_own_hold_across_a_restart() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, release) = prepared(temp.path(), "restart", &host);
    host.runtime.boot(&["neutral.a", "neutral.b", "neutral.c"]);
    host.runtime.pause_as("neutral.a", OTHER_STOP);
    let epoch_held = host.runtime.owner.borrow().epoch.clone();
    host.runtime.starts.borrow_mut().push_back(FakeStart::Hangs);

    // The deploy: its new build's start is not ready at its deadline and is left running; the hold
    // stays, and the answer says so and names the ways out that work today, none another party's.
    let before = journals(&root);
    let (deployed, exit) = rescue_line(
        &host,
        &root,
        "deploy",
        json!({"target": TAG, "dir": word(&release)}),
    );
    assert_eq!(exit, 1, "{deployed}");
    assert_eq!(
        top_value(&deployed, "install_refusal"),
        "runtime_start_failed",
        "{deployed}"
    );
    let reason = deployed["hold"]["reason"]
        .as_str()
        .expect("the hold's reason")
        .to_owned();
    assert!(reason.starts_with("lab-deploy-"), "{deployed}");
    assert_eq!(deployed["hold"]["kept"], json!(true), "{deployed}");
    assert_eq!(
        deployed["next"]["release_hold"],
        json!(["release-hold", reason])
    );
    assert_eq!(
        deployed["next"]["actingctl_release_hold"],
        json!([
            "actingctl",
            "install",
            "release-hold",
            "--root",
            plain_root(&root),
            "--reason",
            reason
        ])
    );
    assert!(!suggests(&deployed, OTHER_STOP), "{deployed}");
    assert!(!host.runtime.running.get());
    for alias in ["neutral.b", "neutral.c"] {
        assert_eq!(
            host.runtime.pause_of(alias).as_deref(),
            Some(reason.as_str())
        );
    }
    // No new owner epoch is recorded: the start step has no `ready`, and the pause step's record
    // is of the epoch the hold was set in.
    let written = new_journals(&root, &before);
    let update = written
        .iter()
        .find(|(_, journal)| journal["kind"] == "update")
        .unwrap_or_else(|| panic!("an update journal: {deployed}"));
    let steps = update.1["steps"].as_array().expect("the steps");
    let step_of = |kind: &str| {
        steps
            .iter()
            .find(|step| step["kind"] == kind)
            .unwrap_or_else(|| panic!("a {kind} step"))
    };
    assert!(step_of("start")["evidence"]["ready"].is_null());
    assert_eq!(
        step_of("pause")["evidence"]["own_pauses"]["owner_epoch"],
        json!(epoch_held)
    );

    // While no Runtime answers, the module's `release-hold` refuses; the hold stays, and the
    // answer says so, with `up` and the ways out.
    let (early, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": reason}));
    assert_eq!(exit, 1, "{early}");
    assert_eq!(
        top_value(&early, "install_refusal"),
        "runtime_not_running",
        "{early}"
    );
    assert_eq!(early["hold"]["kept"], json!(true), "{early}");
    assert_eq!(early["next"]["start"], json!(["up"]), "{early}");
    assert_eq!(
        early["next"]["release_hold"],
        json!(["release-hold", reason])
    );
    assert!(!suggests(&early, OTHER_STOP), "{early}");

    // Another hand starts the Runtime: a new owner epoch, every pause restored at revision 1. Then
    // D, which the configuration gained, gets a pause with another reason.
    host.runtime.restart_restored();
    assert_ne!(host.runtime.owner.borrow().epoch, epoch_held);
    assert_eq!(pause_seen(&host, "neutral.b"), Some((reason.clone(), 1)));
    host.runtime.register("neutral.d");
    host.runtime.pause_as("neutral.d", "later-party");
    let other = pause_seen(&host, "neutral.a");
    let later = pause_seen(&host, "neutral.d");
    assert_eq!(
        other.as_ref().map(|(seen, _)| seen.as_str()),
        Some(OTHER_STOP)
    );
    assert!(calls_of(&host, "resume ").is_empty());

    // `release-hold <its reason>` resumes exactly B and C.
    let (released, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": reason}));
    assert_eq!(exit, 0, "{released}");
    assert_eq!(top_code(&released), "install_command_done", "{released}");
    assert_eq!(top_value(&released, "count"), &json!(2));
    assert_eq!(
        calls_of(&host, "resume "),
        ["resume neutral.b", "resume neutral.c"]
    );
    assert_eq!(released["hold"]["kept"], json!(false), "{released}");
    assert_eq!(pause_seen(&host, "neutral.b"), None);
    assert_eq!(pause_seen(&host, "neutral.c"), None);
    assert_eq!(pause_seen(&host, "neutral.a"), other);
    assert_eq!(pause_seen(&host, "neutral.d"), later);
}

// Fix 2, item 3 (fix 1's item 3, `brief-lab3-fix1.md`; contracts/lab-two-entries.md §6): the
// operation paused A, then the Runtime refuses B's pause (again once its queue is clear). A is
// handed back at once, C's pause (another party's) is untouched, and the answer says `kept:
// false` only because nothing of the operation's own stays paused.
#[test]
fn rescue_refused_after_its_pause_hands_its_own_back() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "refused", &host);
    host.runtime.boot(&["neutral.a", "neutral.b", "neutral.c"]);
    host.runtime.pause_as("neutral.c", OTHER_STOP);
    let other = pause_seen(&host, "neutral.c");
    *host.runtime.refuse_pause.borrow_mut() = vec!["neutral.b".to_owned(), "neutral.b".to_owned()];

    let (refused, exit) = rescue_line(&host, &root, "down", json!({}));
    assert_eq!(exit, 1, "{refused}");
    assert_eq!(
        top_value(&refused, "install_refusal"),
        "runtime_stop_failed",
        "{refused}"
    );
    assert_eq!(top_value(&refused, "alias"), "neutral.b", "{refused}");
    let reason = refused["hold"]["reason"]
        .as_str()
        .expect("the hold's reason")
        .to_owned();
    assert!(reason.starts_with("lab-down-"), "{refused}");
    // A was paused by the operation and handed back; C was never asked.
    assert!(
        calls_of(&host, "pause ").contains(&format!("pause neutral.a {reason}")),
        "{:?}",
        calls(&host)
    );
    assert!(calls_of(&host, "pause neutral.c").is_empty());
    assert_eq!(calls_of(&host, "resume "), ["resume neutral.a"]);
    assert_eq!(refused["hold"]["release"]["resumed"], json!(["neutral.a"]));
    assert_eq!(refused["hold"]["release"]["left"], json!([]));
    assert_eq!(pause_seen(&host, "neutral.c"), other);
    // `kept: false`, and nothing of the operation's own stays paused.
    assert_eq!(refused["hold"]["kept"], json!(false), "{refused}");
    for alias in ["neutral.a", "neutral.b", "neutral.c"] {
        assert_ne!(
            host.runtime.pause_of(alias).as_deref(),
            Some(reason.as_str()),
            "{alias}"
        );
    }
    assert!(!suggests(&refused, OTHER_STOP), "{refused}");
    assert!(host.runtime.running.get(), "nothing was closed");
    assert!(calls_of(&host, "shutdown").is_empty());
}

// Fix 2, item 3 (fix 1's item 3; contracts/lab-two-entries.md §6): with a global pause present,
// the rescue module's stop keeps it, lists it (the `foreign_pauses` row noted `guard_failed`, S0,
// the journal's row) and goes on: the instances held, the Runtime closed. After `up` and the
// module's `release-hold`, the global pause is still the same.
#[test]
fn rescue_keeps_a_global_pause_and_goes_on() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (root, _) = prepared(temp.path(), "global", &host);
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    host.runtime.pause_global_as("global-party");
    let global = host.runtime.global_pause.borrow().clone();
    assert!(global.is_some());

    let before = journals(&root);
    let (down, exit) = rescue_line(&host, &root, "down", json!({}));
    assert_eq!(exit, 0, "{down}");
    assert_eq!(top_code(&down), "install_command_done", "{down}");
    assert_eq!(top_value(&down, "operation"), "install_stop");
    let reason = down["hold"]["reason"]
        .as_str()
        .expect("the hold's reason")
        .to_owned();
    assert_eq!(down["hold"]["kept"], json!(true), "{down}");
    // Listed: noted, in S0 and in the journal's row.
    assert!(
        carries(
            &down,
            "install_caveat_noted",
            "install_caveat",
            &json!("guard_failed")
        ),
        "{down}"
    );
    assert_eq!(
        down["s0"]["pauses"]["global"]["reason"], "global-party",
        "{down}"
    );
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 1, "{written:?}");
    let row = written[0].1["guards"]
        .as_array()
        .expect("the guard rows")
        .iter()
        .find(|row| row["id"] == "foreign_pauses")
        .expect("the foreign_pauses row")
        .clone();
    assert_eq!(row["values"]["global"]["reason"], "global-party", "{row}");
    // Went on: both instances held, the Runtime closed, the global pause kept.
    for alias in ["neutral.a", "neutral.b"] {
        assert_eq!(
            host.runtime.pause_of(alias).as_deref(),
            Some(reason.as_str())
        );
    }
    assert!(!host.runtime.running.get());
    assert_eq!(calls_of(&host, "shutdown"), ["shutdown"]);
    assert_eq!(*host.runtime.global_pause.borrow(), global);

    // `up` and the module's `release-hold`: its own pauses handed back, the global pause kept.
    let (up, exit) = rescue_line(&host, &root, "up", json!({}));
    assert_eq!(exit, 0, "{up}");
    let (released, exit) = rescue_line(&host, &root, "release-hold", json!({"reason": reason}));
    assert_eq!(exit, 0, "{released}");
    assert_eq!(
        calls_of(&host, "resume "),
        ["resume neutral.a", "resume neutral.b"]
    );
    assert_eq!(*host.runtime.global_pause.borrow(), global);
}

// Fix 1, item 2: the rescue module runs from `<root>\tools\actinglab.exe`. The running image of
// the process that plans (matched by its pid) is renamed aside as `actingctl.exe` is; another
// running `actinglab.exe` still blocks.
#[test]
fn rescue_renames_its_own_running_image() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let with_lab = |commit: &str| {
        let mut build = BuildSpec::new(commit);
        build.tools.files.push((
            "actinglab.exe".to_owned(),
            format!("neutral lab {commit}").into_bytes(),
        ));
        build
    };
    let root = live_root(temp.path(), "image", &with_lab(C1));
    assert_done(
        &run(&host, &["adopt", "--root", word(&root)]),
        "install_adopt",
    );
    let release = temp.path().join("image-release");
    let target = with_lab(C2);
    target.write_release(&release);
    let facts = release.join("release-facts.json");
    write_json(&facts, &target.facts(&release));
    assert_done(
        &run(
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
            ],
        ),
        "install_release_stage",
    );
    let lab = super::files::plain(
        &fs::canonicalize(&root)
            .expect("the canonical root")
            .join("tools")
            .join("actinglab.exe"),
    );
    let image = |pid: u32| RunningImage {
        pid,
        path: lab.clone(),
        mcp_serve: false,
        parent_pid: None,
    };
    // This process (5101) and another actinglab (5102) run the live `tools\actinglab.exe`.
    host.own_pid.set(Some(5101));
    *host.processes.borrow_mut() = vec![image(5101), image(5102)];

    // The plan: the other actinglab blocks, the program that plans is renamed aside.
    let (planned, exit) = rescue_line(&host, &root, "deploy", json!({"target": C2, "plan": true}));
    assert_eq!(exit, 1, "{planned}");
    assert_eq!(
        top_value(&planned, "install_refusal"),
        "running_image_blocks",
        "{planned}"
    );
    assert_eq!(top_value(&planned, "pid"), &json!(5102));
    let pids = |member: &str| {
        planned["running"][member]
            .as_array()
            .map(|images| {
                images
                    .iter()
                    .filter_map(|image| image["pid"].as_u64())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    assert_eq!(pids("renamed_own"), [5101], "{planned}");
    assert_eq!(pids("blocked"), [5102], "{planned}");

    // Without the other one the deploy goes through: the running image is renamed aside and
    // kept, and the new build's takes its place.
    host.processes
        .borrow_mut()
        .retain(|image| image.pid == 5101);
    let (deployed, exit) = rescue_line(&host, &root, "deploy", json!({"target": C2}));
    assert_eq!(exit, 0, "{deployed}");
    assert_eq!(top_code(&deployed), "install_command_done", "{deployed}");
    let tools = root.join("tools");
    assert_eq!(
        fs::read(tools.join("actinglab.exe")).expect("the new image"),
        format!("neutral lab {C2}").into_bytes()
    );
    let kept = fs::read_dir(tools.as_path())
        .expect("the tools folder")
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("actinglab.exe.old-")
        });
    assert!(kept, "the running image is kept beside its place");
}

/// The neutral game, pack, table and server of the placement test (made-up ids, no real game).
const GAME: &str = "neutral-game";
const PACK: &str = "neutral.pack";
const TABLE: &str = "neutral_scores";

/// The bytes of a valid table [`TABLE`] of [`GAME`] (`actingcommand.table.v1`) whose one row
/// holds `score`.
fn table_bytes(score: u32) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.table.v1",
        "game": GAME,
        "table_id": TABLE,
        "row_key": ["unit"],
        "columns": [
            {"name": "unit", "type": "key"},
            {"name": "score_milli", "type": "integer", "min": 0, "max": 100000}
        ],
        "rows": [["unit.a", score]]
    }))
    .expect("JSON")
}

/// Writes a neutral bundle zip of [`GAME`] to `path`: `bundle.json` with one pack and the table
/// [`TABLE`], the pack's `control.json` (`control`), `tables/<TABLE>.json` (`table`), and the
/// package interface the fixture's installer speaks. The bytes of its `bundle.json`.
fn write_bundle(path: &Path, control: &[u8], table: &[u8]) -> Vec<u8> {
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACK,
            "server": "neutral-server",
            "entry_task_id": "neutral.entry",
            "path": format!("packs/{PACK}"),
            "file_count": 1,
            "byte_count": control.len(),
        }],
        "tables": [TABLE],
    }))
    .expect("JSON");
    let interfaces = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.component-interfaces.v1",
        "speaks": speaks(),
    }))
    .expect("JSON");
    let mut writer = zip::ZipWriter::new(fs::File::create(path).expect("create the bundle"));
    for (name, bytes) in [
        ("bundle.json".to_owned(), bundle.clone()),
        ("interfaces.json".to_owned(), interfaces),
        (format!("packs/{PACK}/control.json"), control.to_vec()),
        (format!("tables/{TABLE}.json"), table.to_vec()),
    ] {
        writer
            .start_file(name, zip::write::FileOptions::default())
            .expect("start a zip entry");
        writer.write_all(&bytes).expect("write a zip entry");
    }
    writer.finish().expect("finish the bundle");
    bundle
}

// Fix 1, item 4: `place` on a fake Runtime with three instances: (a) bound to the bundle's game
// and idle, (b) the same game and already paused by another party, (c) another game. Only (a) is
// paused and resumed; (b)'s pause is kept and (c) is untouched; each replaced item is kept as
// `.old-<ms>`; the journal `pack_place` carries the initiator; `undo` restores the game root.
// Workflow #428 TB-R1 (register §1.293): the bundle also replaces a table, whose old copy is kept
// in `tables/`; after the placement and after `undo` the table reader lists that one table, read,
// and nothing else.
#[test]
fn place_holds_only_the_bundles_idle_instances() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "place", &BuildSpec::new(C1));
    // The standard-pack root the configuration names, with what the bundle replaces.
    let packs = temp.path().join("place-packs");
    let game_root = packs.join(GAME);
    fs::create_dir_all(game_root.join(PACK)).expect("the pack folder");
    fs::create_dir_all(game_root.join("tables")).expect("the tables folder");
    let old_control = br#"{"neutral": 1}"#.to_vec();
    let old_bundle = br#"{"neutral": "the bundle before"}"#.to_vec();
    let old_table = table_bytes(1000);
    fs::write(game_root.join(PACK).join("control.json"), &old_control).expect("the old pack");
    fs::write(game_root.join("bundle.json"), &old_bundle).expect("the old bundle.json");
    let table = game_root.join("tables").join(format!("{TABLE}.json"));
    fs::write(&table, &old_table).expect("the old table");
    // The tables of the game root as every reader lists them: exactly the one table, read, with
    // the bytes `expected`; a kept copy beside it is no table.
    let listed_alone = |expected: &[u8]| {
        let files = actingcommand_contract::read_standard_pack_tables(game_root.as_path(), GAME)
            .expect("tables/ is listed");
        assert_eq!(files.len(), 1, "{files:?}");
        assert_eq!(files[0].table_id, TABLE, "{files:?}");
        assert!(files[0].read.is_ok(), "{files:?}");
        assert_eq!(fs::read(&files[0].path).expect("the table"), expected);
    };
    let config_path = root.join("actingd.config.json");
    let mut config: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("the configuration")).expect("JSON");
    config["standard_packs"] = json!({"root": packs});
    write_json(&config_path, &config);
    assert_done(
        &run(&host, &["adopt", "--root", word(&root)]),
        "install_adopt",
    );
    let zip = temp.path().join("neutral-bundle.zip");
    let new_control = br#"{"neutral": 2}"#.to_vec();
    let new_table = table_bytes(2000);
    let new_bundle = write_bundle(&zip, &new_control, &new_table);

    host.runtime.boot(&["neutral.a", "neutral.b", "neutral.c"]);
    for instance in host.runtime.instances.borrow_mut().iter_mut() {
        let game = if instance.alias == "neutral.c" {
            "neutral-other"
        } else {
            GAME
        };
        instance.game = Some(game.to_owned());
    }
    host.runtime.pause_as("neutral.b", "other-party");
    let other = pause_seen(&host, "neutral.b");

    let before = journals(&root);
    let (placed, exit) = rescue_line(&host, &root, "place", json!({"bundle": word(&zip)}));
    assert_eq!(exit, 0, "{placed}");
    assert_eq!(top_code(&placed), "install_command_done", "{placed}");
    assert_eq!(top_value(&placed, "operation"), "install_pack_place");

    // Only (a) was paused and resumed; (b)'s pause stays; (c) was not touched.
    let paused = calls_of(&host, "pause ");
    assert_eq!(paused.len(), 1, "{:?}", calls(&host));
    assert!(
        paused[0].starts_with("pause neutral.a lab-place-"),
        "{paused:?}"
    );
    assert_eq!(calls_of(&host, "resume "), ["resume neutral.a"]);
    assert_eq!(pause_seen(&host, "neutral.a"), None);
    assert_eq!(pause_seen(&host, "neutral.b"), other);
    assert_eq!(pause_seen(&host, "neutral.c"), None);

    // The items placed, each replaced one kept beside its place as `.old-<ms>`.
    let control = game_root.join(PACK).join("control.json");
    let bundle = game_root.join("bundle.json");
    assert_eq!(fs::read(&control).expect("the placed pack"), new_control);
    assert_eq!(
        fs::read(&bundle).expect("the placed bundle.json"),
        new_bundle
    );
    let kept = |prefix: &str| {
        fs::read_dir(game_root.as_path())
            .expect("the game root")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
    };
    assert!(kept(&format!("{PACK}.old-")), "the replaced pack is kept");
    assert!(kept("bundle.json.old-"), "the replaced bundle.json is kept");
    // The replaced table is kept in `tables/`, and the reader lists the placed table alone.
    let kept_table = format!("{TABLE}.json.old-");
    let table_kept = fs::read_dir(game_root.join("tables"))
        .expect("the tables folder")
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(kept_table.as_str())
        });
    assert!(table_kept, "the replaced table is kept beside it");
    listed_alone(new_table.as_slice());

    // The journal: kind `pack_place`, with the initiator.
    let written = new_journals(&root, &before);
    assert_eq!(written.len(), 1, "{written:?}");
    let (operation_id, journal) = &written[0];
    assert_eq!(journal["kind"], "pack_place");
    assert_eq!(journal["initiator"], WORD.as_str());

    // `undo` restores the game root: the placed items set aside, the kept ones back in place.
    let (undone, exit) = rescue_line(&host, &root, "undo", json!({"op": operation_id}));
    assert_eq!(exit, 0, "{undone}");
    assert_eq!(
        fs::read(control.as_path()).expect("the pack again"),
        old_control
    );
    assert_eq!(
        fs::read(bundle.as_path()).expect("bundle.json again"),
        old_bundle
    );
    // The old table is back in place; the placed one, set aside beside it, is no table.
    assert_eq!(
        fs::read(table.as_path()).expect("the table again"),
        old_table
    );
    listed_alone(old_table.as_slice());
}

// Fix 3, item 2 (`brief-lab3-fix3.md`; install-cli.md, "The Lab's rescue module", "Only its own
// hold"): a `release-hold` in `next` whose hold the module cannot prove its own is left out, and
// when the cause is a read failure rather than "not owned" the answer says so. On an adopted root
// a reason no journal of the module recorded is left out without a note; a reason that does not
// parse, and on a root whose `install\operations` is a file an operations folder that cannot be
// listed, are left out with the note `install_caveat_noted` (`hold_ownership_unconfirmed`,
// `install_release_hold`), the read failure as its cause; the answer itself stays a success.
#[test]
fn rescue_notes_a_hold_whose_ownership_cannot_be_read() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let (readable, _) = prepared(temp.path(), "readable", &host);
    let unreadable = live_root(temp.path(), "unreadable", &BuildSpec::new(C1));
    fs::create_dir_all(unreadable.join("install")).expect("the install folder");
    fs::write(
        unreadable.join("install").join("operations"),
        b"a file, not a folder",
    )
    .expect("a file where the operations folder goes");

    // The module's line of an installer answer whose `next` names `release-hold <reason>`.
    let answered = |root: &Path, reason: &str| -> Value {
        let location = location(root);
        let context = Context {
            host: &host,
            location: &location,
            initiator: WORD,
        };
        let mut answer = super::Answer::new(super::done(InstallLocation::Stop));
        answer.members.insert(
            "next".to_owned(),
            json!({"release_hold": ["install", "release-hold", "--root", word(root), "--reason", reason]}),
        );
        super::print(rescue::module_answer(&context, answer))
            .line
            .expect("an encoded line")
    };
    let noted = |line: &Value| {
        carries(
            line,
            "install_caveat_noted",
            "install_caveat",
            &json!("hold_ownership_unconfirmed"),
        ) && carries(
            line,
            "install_caveat_noted",
            "operation",
            &json!("install_release_hold"),
        )
    };

    // Not owned: left out, nothing noted.
    let line = answered(&readable, OTHER_STOP);
    assert_eq!(top_code(&line), "install_command_done", "{line}");
    assert!(line.get("next").is_none(), "{line}");
    assert!(!noted(&line), "{line}");

    // A reason that does not parse: left out and noted, the parse failure as the cause.
    let line = answered(&readable, "Not A Reason");
    assert_eq!(top_code(&line), "install_command_done", "{line}");
    assert!(line.get("next").is_none(), "{line}");
    assert!(noted(&line), "{line}");
    assert!(
        line.to_string()
            .contains("\"invalid_scheduling_pause_reason\""),
        "{line}"
    );

    // An operations folder that cannot be listed: left out and noted with the reason, the OS's
    // failure as the cause.
    let line = answered(&unreadable, "r1");
    assert_eq!(top_code(&line), "install_command_done", "{line}");
    assert!(line.get("next").is_none(), "{line}");
    assert!(noted(&line), "{line}");
    assert!(
        carries(&line, "install_caveat_noted", "hold_reason", &json!("r1")),
        "{line}"
    );
    assert!(
        carries(&line, "foreign_os_error", "io_op", &json!("read_dir")),
        "{line}"
    );
}
