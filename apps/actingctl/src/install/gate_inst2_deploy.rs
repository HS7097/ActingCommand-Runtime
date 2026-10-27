// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #421 INST-2, test rows D-1 to D-9 of the #421 deploy design (§8, the S2 row). Every
// expected value comes from the design (§4, §7, §8 S2), its contract (contracts/install-cli.md,
// "Formal stop, start and the hold", "Guards", "Steps"), Alice's decision 74 A (the automatic
// rollback only between the new start and the resume; after a rollback whose previous-build
// checks pass, scheduling resumes and the command still reports failure) and her decision 75 as
// changed on 10-27 14:2x (the hold records what it defers; the release hands it back):
// - D-1: a stop journals S0 (owner, pauses, instructions, ledger head) and what the hold defers,
//   and holds only the instances S0 shows unpaused;
// - D-2: a foreign pause unnamed refuses (a global pause even when acknowledged); a named one is
//   kept through the stop, the start and the release, and never resumed;
// - D-3: a start whose available memory is below the key refuses before any spawn; readiness is
//   reached on the fake clock; a start not ready at its deadline is `start_timeout`, left
//   running, and nothing is killed or started again;
// - D-4: each post-start check passes and fails (`one_way` is the later S6 watch: not
//   evaluated);
// - D-5: a fail-class check after the new start closes it, passes the gate, reverses the swaps,
//   starts the previous build, releases the hold and ends `deploy_rolled_back`;
// - D-6: a new start that exits does the same;
// - D-7: a gate that fails keeps the new build and the hold and ends `rollback_gate_failed`;
// - D-8: a composite interrupted after each of its steps is finished by `resume`;
// - D-9: `release-hold` resumes only the instances paused with exactly its reason.
// The machine is the fake host of `fixture.rs` with its fake Runtime and fake clock: no device,
// no sleep; every root is a temporary folder with neutral files.

use super::fixture::{
    BuildSpec, C1, C2, FakeHost, FakeStart, code, live_root, run, tree, word, write_json,
};
use super::hold::PauseIntent;
use super::host::Host;
use super::journal::S0;
use super::postcheck::{self, CheckInputs, CheckResult};
use super::runtime::{Drained, PauseSeen, PlanRow, PlanSeen};
use super::state::{InstallState, PackEntry};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

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

/// A root whose live build is `C1` (adopted), with `C2` kept, a start of `C1` logged after the
/// adopt (the previous build's own open for the rollback gate), and `config` merged into its
/// configuration.
fn prepared(base: &Path, name: &str, host: &FakeHost, config: Value) -> PathBuf {
    let root = live_root(base, name, &BuildSpec::new(C1));
    if let Value::Object(members) = config {
        let path = root.join("actingd.config.json");
        let mut document: Value =
            serde_json::from_slice(&fs::read(&path).expect("the configuration")).expect("JSON");
        for (member, value) in members {
            document[member] = value;
        }
        write_json(&path, &document);
    }
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
    root
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

fn state_of(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("install").join("state.json")).expect("state.json"))
        .expect("state JSON")
}

/// The listed files of `spec` as the root holds them now, by path.
fn holds_build(root: &Path, spec: &BuildSpec) -> bool {
    [("runtime", &spec.runtime), ("tools", &spec.tools)]
        .iter()
        .all(|(half, part)| {
            let live = tree(&root.join(half));
            part.entries()
                .iter()
                .all(|(path, bytes)| live.get(path) == Some(bytes))
        })
}

fn calls(host: &FakeHost) -> Vec<String> {
    host.runtime.calls.borrow().clone()
}

/// A composite update to `C2`: planned with `--preview` (the Runtime runs), applied with
/// `--stop --hold <reason> --start` and `extra`.
fn composite(host: &FakeHost, root: &Path, reason: &str, extra: &[&str]) -> super::Answer {
    let planned = run(
        host,
        &[
            "update",
            "plan",
            "--root",
            word(root),
            "--build",
            C2,
            "--preview",
        ],
    );
    assert_done(&planned, "install_update_plan");
    let plan_id = planned.members["plan_id"]
        .as_str()
        .expect("a plan id")
        .to_owned();
    let mut words = vec![
        "update",
        "apply",
        "--root",
        word(root),
        "--build",
        C2,
        "--plan",
        plan_id.as_str(),
        "--stop",
        "--hold",
        reason,
        "--start",
    ];
    words.extend_from_slice(extra);
    run(host, &words)
}

// D-1: the stop journals S0 and what the hold defers, and holds only the unpaused instances.
#[test]
fn inst2_d_1_stop_journals_s0_and_the_deferred_work_and_holds_only_unpaused_instances() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "stop", &host, json!({}));
    host.runtime.boot(&["node.a", "node.b"]);
    host.runtime.pause_as("node.b", "operator");
    *host.runtime.instructions.borrow_mut() =
        vec![("scheduling.example_key".to_owned(), "1".to_owned())];
    *host.runtime.plan.borrow_mut() = PlanSeen {
        evaluated_at_unix_ms: Some(1_800_000_000_000),
        instances: vec![PlanRow {
            alias: "node.a".to_owned(),
            next_wake_unix_ms: Some(1_800_000_600_000),
            selected: Some(json!({"task_id": "task.a", "decision_id": "decision-1",
                "dispatch_state": "admitted"})),
            due_tasks: vec!["task.b".to_owned()],
        }],
    };
    host.runtime.drains.borrow_mut().insert(
        "node.a".to_owned(),
        Drained {
            finished: 1,
            cancelled: 0,
        },
    );

    // The instance due now fails `schedule_near`; the person acknowledges it.
    let refused = run(
        &host,
        &[
            "stop",
            "--root",
            word(&root),
            "--hold",
            "deploy-1",
            "--known-pause",
            "node.b=operator",
        ],
    );
    assert_refused(&refused, "install_stop", "guard_failed");
    assert_eq!(
        refused.outcome.value("install_guards"),
        Some(&json!(["schedule_near"]))
    );
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));

    let stopped = run(
        &host,
        &[
            "stop",
            "--root",
            word(&root),
            "--hold",
            "deploy-1",
            "--known-pause",
            "node.b=operator",
            "--ack-guard",
            "schedule_near",
        ],
    );
    assert_done(&stopped, "install_stop");
    assert_eq!(
        calls(&host),
        ["pause node.a deploy-1", "shutdown"],
        "only the unpaused instance is held"
    );
    assert_eq!(host.runtime.pause_of("node.a").as_deref(), Some("deploy-1"));
    assert_eq!(host.runtime.pause_of("node.b").as_deref(), Some("operator"));
    assert!(!host.runtime.running.get());

    let journal = journal_of(&root, &operation_id(&stopped));
    assert_eq!(journal["kind"], "stop");
    assert_eq!(
        journal["s0"],
        json!({
            "head": 100,
            "epoch": "epoch-1",
            "pauses": {"global": null, "instances": [
                {"alias": "node.b", "reason": "operator", "revision": 1}
            ]},
            "instructions": [{"config_key": "scheduling.example_key", "value_json": "1"}],
        })
    );
    let pause = &journal["steps"][0];
    assert_eq!(pause["kind"], "pause");
    assert_eq!(pause["state"], "done");
    assert_eq!(pause["evidence"]["held"], json!(["node.a"]));
    assert_eq!(
        pause["evidence"]["drained"],
        json!([{"alias": "node.a", "finished": 1, "cancelled": 0}])
    );
    let deferred = &pause["evidence"]["deferred"]["instances"];
    assert_eq!(deferred.as_array().map(Vec::len), Some(1), "{deferred}");
    assert_eq!(deferred[0]["alias"], "node.a");
    assert_eq!(deferred[0]["due_tasks"], json!(["task.b"]));
    assert_eq!(deferred[0]["selected"]["task_id"], "task.a");
    assert_eq!(deferred[0]["next_wake_unix_ms"], 1_800_000_600_000_u64);
    assert_eq!(journal["steps"][1]["kind"], "close");
    assert_eq!(
        journal["steps"][1]["evidence"]["terminal"],
        "runtime_stopped"
    );
    assert!(
        journal["acknowledged"]
            .as_array()
            .is_some_and(|acks| acks.contains(&json!("guard:schedule_near"))),
        "{journal}"
    );
}

// D-2: a foreign pause unnamed refuses; a named one is kept and never resumed.
#[test]
fn inst2_d_2_foreign_pauses_refuse_unnamed_and_are_kept_when_named() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "foreign", &host, json!({}));
    host.runtime.boot(&["node.a", "node.b"]);
    host.runtime.pause_as("node.b", "operator");

    let refused = run(
        &host,
        &["stop", "--root", word(&root), "--hold", "deploy-2"],
    );
    assert_refused(&refused, "install_stop", "guard_failed");
    assert_eq!(
        refused.outcome.value("install_guards"),
        Some(&json!(["foreign_pauses"]))
    );
    assert!(calls(&host).is_empty());
    assert!(host.runtime.running.get());

    // A global pause refuses even when the guard is acknowledged.
    *host.runtime.global_pause.borrow_mut() = Some(PauseSeen {
        reason: "operator".to_owned(),
        revision: 9,
    });
    let refused = run(
        &host,
        &[
            "stop",
            "--root",
            word(&root),
            "--hold",
            "deploy-2",
            "--known-pause",
            "node.b=operator",
            "--ack-guard",
            "foreign_pauses",
        ],
    );
    assert_refused(&refused, "install_stop", "guard_failed");
    assert!(calls(&host).is_empty());
    *host.runtime.global_pause.borrow_mut() = None;

    // Named: kept through the stop, the start and the release, never resumed.
    let stopped = run(
        &host,
        &[
            "stop",
            "--root",
            word(&root),
            "--hold",
            "deploy-2",
            "--known-pause",
            "node.b=operator",
        ],
    );
    assert_done(&stopped, "install_stop");
    let started = run(&host, &["start", "--root", word(&root)]);
    assert_done(&started, "install_start");
    let released = run(
        &host,
        &[
            "release-hold",
            "--root",
            word(&root),
            "--reason",
            "deploy-2",
        ],
    );
    assert_done(&released, "install_release_hold");
    assert_eq!(released.outcome.value("count"), Some(&json!(1)));
    assert_eq!(host.runtime.pause_of("node.a"), None);
    assert_eq!(host.runtime.pause_of("node.b").as_deref(), Some("operator"));
    assert!(
        !calls(&host).iter().any(|call| call == "resume node.b"),
        "{:?}",
        calls(&host)
    );
}

// D-3: the start's memory guard, readiness on the fake clock, and a start left running at its
// deadline.
#[test]
fn inst2_d_3_start_refuses_low_memory_reaches_readiness_and_leaves_a_timed_out_start_running() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(
        temp.path(),
        "start",
        &host,
        json!({"install": {"start": {"ready_timeout_ms": 60_000}}}),
    );

    // Memory below `install.guard.min_available_memory_mb` (10 000 MB): nothing is spawned.
    host.memory_mb.set(5_000);
    let refused = run(&host, &["start", "--root", word(&root)]);
    assert_refused(&refused, "install_start", "guard_failed");
    assert_eq!(
        refused.outcome.value("install_guards"),
        Some(&json!(["memory"]))
    );
    assert!(calls(&host).is_empty(), "{:?}", calls(&host));

    // Ready 3 s after the spawn, on the fake clock.
    host.memory_mb.set(64_000);
    host.runtime
        .starts
        .borrow_mut()
        .push_back(FakeStart::Ready { after_ms: 3_000 });
    let started = run(&host, &["start", "--root", word(&root)]);
    assert_done(&started, "install_start");
    let ready_ms = started.members["start"]["ready_ms"]
        .as_u64()
        .expect("ready_ms");
    assert!(ready_ms >= 3_000, "{ready_ms}");
    assert!(started.outcome.value("pid").is_some(), "{started:?}");
    assert!(host.runtime.running.get());

    // Not ready at `install.start.ready_timeout_ms`: `start_timeout`, left running, no second
    // attempt, nothing closed or killed.
    host.runtime.running.set(false);
    host.runtime.calls.borrow_mut().clear();
    host.runtime.starts.borrow_mut().push_back(FakeStart::Hangs);
    let timed_out = run(&host, &["start", "--root", word(&root)]);
    assert_refused(&timed_out, "install_start", "runtime_start_failed");
    assert!(
        timed_out
            .outcome
            .causes()
            .iter()
            .any(|link| link.outcome().code().as_str() == "start_timeout"),
        "{timed_out:?}"
    );
    assert_eq!(timed_out.members["start"]["left_running"], true);
    assert_eq!(calls(&host), [format!("spawn {C1}")]);
}

/// One change of D-4's inputs that makes one check fail.
type Change<'a> = Box<dyn Fn(&mut Case) + 'a>;

/// D-4's inputs, as a closure changes them.
struct Case {
    ready: Result<u64, String>,
    config: Value,
    config_sha256: String,
    added: Vec<String>,
}

// D-4: each post-start check passes, and fails.
#[test]
fn inst2_d_4_each_post_start_check_passes_and_fails() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let log = temp.path().join("actingd-1.log");
    fs::write(&log, b"").expect("a start log");
    let s0 = S0 {
        head: Some(100),
        epoch: Some("epoch-1".to_owned()),
        pauses: Some(json!({"global": null, "instances": [
            {"alias": "node.b", "reason": "operator", "revision": 1}
        ]})),
        instructions: Some(json!([{"config_key": "scheduling.example_key", "value_json": "1"}])),
    };
    let hold = PauseIntent {
        reason: "deploy-4".to_owned(),
        instances: vec!["node.a".to_owned()],
        known: BTreeMap::from([("node.b".to_owned(), "operator".to_owned())]),
        drain_timeout_ms: 1_000,
    };
    let state = InstallState {
        config_sha256: Some("sha256:committed".to_owned()),
        packs: vec![PackEntry {
            game: "neutral".to_owned(),
            repository: None,
            commit: None,
            tag: None,
            package_range: None,
        }],
        ..InstallState::default()
    };
    let key = "install.start.sample_interval_ms";
    let passing = || Case {
        ready: Ok(3_000),
        config: json!({"instances": [{"alias": "node.a"}, {"alias": "node.b"}]}),
        config_sha256: "sha256:committed".to_owned(),
        added: vec![key.to_owned()],
    };
    let reset = || {
        host.runtime.boot(&["node.a", "node.b"]);
        host.runtime.pause_as("node.a", "deploy-4");
        host.runtime.pause_as("node.b", "operator");
        *host.runtime.instructions.borrow_mut() =
            vec![("scheduling.example_key".to_owned(), "1".to_owned())];
        *host.runtime.sources.borrow_mut() =
            BTreeMap::from([(key.to_owned(), "default".to_owned())]);
        host.runtime.report_patch.borrow_mut().insert(
            "live".to_owned(),
            json!({"standard_packs": [{"game": "neutral", "pack_count": 3}]}),
        );
    };
    let evaluate = |case: &Case| {
        postcheck::evaluate(
            &host,
            temp.path(),
            &CheckInputs {
                ready: case.ready.clone(),
                log: &log,
                s0: Some(&s0),
                hold: Some(&hold),
                config: &case.config,
                config_sha256: &case.config_sha256,
                state: &state,
                added_keys: &case.added,
            },
        )
    };
    let result = |rows: &[postcheck::CheckRow], id: &str| {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("the check {id}"))
            .result
    };
    let patch = |members: Value| {
        let mut report = json!({"standard_packs": [{"game": "neutral", "pack_count": 3}]});
        if let Value::Object(members) = members {
            for (member, value) in members {
                report[member] = value;
            }
        }
        host.runtime
            .report_patch
            .borrow_mut()
            .insert("live".to_owned(), report);
    };

    // Every check passes; `one_way` is the later S6 watch.
    reset();
    let rows = evaluate(&passing());
    for row in &rows {
        let expected = if row.id == "one_way" {
            CheckResult::NotEvaluated
        } else {
            CheckResult::Pass
        };
        assert_eq!(row.result, expected, "{row:?}");
    }
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "ready",
            "open",
            "no_seeding",
            "bindings",
            "packs",
            "pauses",
            "instructions",
            "no_dispatch",
            "new_keys",
            "config",
            "indexes",
            "one_way"
        ]
    );
    assert!(postcheck::failed(&rows).is_empty());

    // Each one fails.
    let open = json!({"phases": {"sql_read_ms": 0, "verify_ms": 0, "material_ms": 0,
        "restore_ms": 0, "meta_ms": 0, "tail_ms": 0, "index_ms": 1_000, "import_ms": 0},
        "sum_ms": 1_000, "deadline_ms": 600_000, "slow": true});
    let cases: Vec<(&str, Change<'_>, bool)> = vec![
        (
            "ready",
            Box::new(|case: &mut Case| case.ready = Err("exited_during_startup".to_owned())),
            true,
        ),
        (
            "open",
            Box::new(|_: &mut Case| patch(json!({"open": open.clone()}))),
            false,
        ),
        (
            "no_seeding",
            Box::new(|_: &mut Case| patch(json!({"tagged_state": "seed"}))),
            true,
        ),
        (
            "bindings",
            Box::new(|case: &mut Case| {
                case.config = json!({"instances": [{"alias": "node.a"}, {"alias": "node.b"},
                    {"alias": "node.c"}]});
            }),
            true,
        ),
        (
            "packs",
            Box::new(|_: &mut Case| patch(json!({"standard_packs": []}))),
            true,
        ),
        (
            "pauses",
            Box::new(|_: &mut Case| {
                if let Some(instance) = host
                    .runtime
                    .instances
                    .borrow_mut()
                    .iter_mut()
                    .find(|instance| instance.alias == "node.a")
                {
                    instance.pause = None;
                }
            }),
            true,
        ),
        (
            "instructions",
            Box::new(|_: &mut Case| {
                *host.runtime.instructions.borrow_mut() = Vec::new();
            }),
            true,
        ),
        (
            "no_dispatch",
            Box::new(|_: &mut Case| {
                patch(json!({"dispatches_since_start": [
                    {"instance_alias": "node.a", "count": 1}
                ]}));
            }),
            true,
        ),
        (
            "new_keys",
            Box::new(|_: &mut Case| {
                *host.runtime.sources.borrow_mut() =
                    BTreeMap::from([(key.to_owned(), "explicit".to_owned())]);
            }),
            true,
        ),
        (
            "config",
            Box::new(|case: &mut Case| case.config_sha256 = "sha256:changed".to_owned()),
            true,
        ),
        (
            "indexes",
            Box::new(|_: &mut Case| {
                let mut noted = open.clone();
                noted["slow"] = json!(false);
                noted["index_note"] = json!("ledger_index_mismatch");
                patch(json!({"open": noted}));
            }),
            false,
        ),
    ];
    for (id, change, fail_class) in cases {
        reset();
        let mut case = passing();
        change(&mut case);
        let rows = evaluate(&case);
        assert_eq!(result(&rows, id), CheckResult::Fail, "{id}: {rows:?}");
        assert_eq!(
            postcheck::failed(&rows).iter().any(|failed| failed == id),
            fail_class,
            "{id}: {rows:?}"
        );
    }
}

// D-5: a fail-class check after the new start rolls the composite back by itself.
#[test]
fn inst2_d_5_a_failed_check_rolls_back_to_the_previous_build_and_releases_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "rollback", &host, json!({}));
    host.runtime.boot(&["node.a"]);
    host.runtime
        .report_patch
        .borrow_mut()
        .insert(C2.to_owned(), json!({"tagged_state": "seed"}));

    let answer = composite(&host, &root, "deploy-5", &[]);
    assert_refused(&answer, "install_update_apply", "deploy_rolled_back");
    assert_eq!(text(&answer, "commit_sha"), Some(C1));
    assert_eq!(answer.members["failed_checks"], json!(["no_seeding"]));
    assert_eq!(
        calls(&host),
        [
            "pause node.a deploy-5".to_owned(),
            "shutdown".to_owned(),
            format!("spawn {C2}"),
            "shutdown".to_owned(),
            format!("spawn {C1}"),
            "resume node.a".to_owned(),
        ]
    );
    assert!(
        holds_build(&root, &BuildSpec::new(C1)),
        "the swaps reversed"
    );
    assert_eq!(state_of(&root)["live"]["commit"], C1);
    assert!(host.runtime.running.get());
    assert_eq!(*host.runtime.build.borrow(), C1);
    assert_eq!(host.runtime.pause_of("node.a"), None, "the hold released");
    let journal = journal_of(&root, &operation_id(&answer));
    assert_eq!(journal["rollback"]["to"], C1);
    assert!(
        journal["rollback"]["reversed_at_unix_ms"].is_u64(),
        "{journal}"
    );
    assert_eq!(
        journal["result"]["values"]["install_refusal"],
        "deploy_rolled_back"
    );
}

// D-6: a new start that exits rolls back the same way.
#[test]
fn inst2_d_6_a_new_start_that_exits_rolls_back_the_same_way() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "exits", &host, json!({}));
    host.runtime.boot(&["node.a"]);
    {
        let mut starts = host.runtime.starts.borrow_mut();
        starts.push_back(FakeStart::Exits { code: 3 });
        starts.push_back(FakeStart::Exits { code: 3 });
    }

    let answer = composite(&host, &root, "deploy-6", &[]);
    assert_refused(&answer, "install_update_apply", "deploy_rolled_back");
    assert_eq!(answer.members["failed_checks"], json!(["ready"]));
    assert_eq!(
        calls(&host),
        [
            "pause node.a deploy-6".to_owned(),
            "shutdown".to_owned(),
            format!("spawn {C2}"),
            format!("spawn {C2}"),
            format!("spawn {C1}"),
            "resume node.a".to_owned(),
        ]
    );
    assert!(
        holds_build(&root, &BuildSpec::new(C1)),
        "the swaps reversed"
    );
    assert_eq!(state_of(&root)["live"]["commit"], C1);
    assert_eq!(*host.runtime.build.borrow(), C1);
    assert_eq!(host.runtime.pause_of("node.a"), None, "the hold released");
}

// D-7: a gate that fails keeps the new build and the hold: a forward fix only.
#[test]
fn inst2_d_7_a_failed_gate_keeps_the_new_build_and_the_hold() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "gate", &host, json!({}));
    host.runtime.boot(&["node.a"]);
    host.runtime
        .report_patch
        .borrow_mut()
        .insert(C2.to_owned(), json!({"tagged_state": "seed"}));
    // The previous build's reader refuses the records the new build wrote (from the head on).
    *host.programs.unreadable.borrow_mut() = Some((C1.to_owned(), 100));

    let answer = composite(&host, &root, "deploy-7", &[]);
    assert_refused(&answer, "install_update_apply", "rollback_gate_failed");
    assert_eq!(
        answer.outcome.value("rollback_gate_parts"),
        Some(&json!(["ledger_readability"]))
    );
    assert!(
        holds_build(&root, &BuildSpec::new(C2)),
        "the new build stays"
    );
    assert_eq!(state_of(&root)["live"]["commit"], C2);
    assert_eq!(
        host.runtime.pause_of("node.a").as_deref(),
        Some("deploy-7"),
        "the hold stays"
    );
    assert_eq!(
        calls(&host),
        [
            "pause node.a deploy-7".to_owned(),
            "shutdown".to_owned(),
            format!("spawn {C2}"),
            "shutdown".to_owned(),
        ]
    );
    assert_eq!(answer.members["next"]["release_hold"][5], json!("deploy-7"));
}

// D-8: a composite interrupted after each of its steps is finished by `resume`.
#[test]
fn inst2_d_8_resume_finishes_a_composite_interrupted_after_each_step() {
    let temp = TempDir::new().expect("tempdir");
    let config = json!({"install": {"hold_after_start": false}});
    // The uninterrupted composite: its steps.
    let host = FakeHost::new(&temp.path().join("work-0"));
    let root = prepared(temp.path(), "whole", &host, config.clone());
    host.runtime.boot(&["node.a", "node.b"]);
    host.runtime.pause_as("node.b", "operator");
    let whole = composite(
        &host,
        &root,
        "deploy-8",
        &["--known-pause", "node.b=operator"],
    );
    assert_done(&whole, "install_update_apply");
    let steps = journal_of(&root, &operation_id(&whole))["steps"]
        .as_array()
        .map(Vec::len)
        .expect("steps");
    assert!(steps >= 8, "{steps}");

    for step in 1..=steps {
        let host = FakeHost::new(&temp.path().join(format!("work-{step}")));
        let root = prepared(temp.path(), &format!("step-{step}"), &host, config.clone());
        host.runtime.boot(&["node.a", "node.b"]);
        host.runtime.pause_as("node.b", "operator");
        host.crash_after.set(Some(step));
        let stopped = composite(
            &host,
            &root,
            "deploy-8",
            &["--known-pause", "node.b=operator"],
        );
        assert_eq!(
            code(&stopped),
            "install_operation_interrupted",
            "step {step}: {stopped:?}"
        );
        let id = text(&stopped, "install_operation_id")
            .expect("the operation")
            .to_owned();
        let resumed = run(
            &host,
            &["resume", "--root", word(&root), "--operation", &id],
        );
        assert_done(&resumed, "install_resume");
        assert!(holds_build(&root, &BuildSpec::new(C2)), "step {step}");
        assert_eq!(state_of(&root)["live"]["commit"], C2, "step {step}");
        assert!(host.runtime.running.get(), "step {step}");
        assert_eq!(*host.runtime.build.borrow(), C2, "step {step}");
        assert_eq!(host.runtime.pause_of("node.a"), None, "step {step}");
        assert_eq!(
            host.runtime.pause_of("node.b").as_deref(),
            Some("operator"),
            "step {step}"
        );
        let journal = journal_of(&root, &id);
        assert!(journal["finished_at_unix_ms"].is_u64(), "step {step}");
        assert!(
            !root.join("install").join("current.json").exists(),
            "step {step}"
        );
    }
}

// D-9: `release-hold` resumes only the instances paused with exactly its reason.
#[test]
fn inst2_d_9_release_hold_resumes_only_its_own_reason() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = prepared(temp.path(), "release", &host, json!({}));
    host.runtime.boot(&["node.a", "node.b", "node.c"]);
    host.runtime.pause_as("node.a", "deploy-9");
    host.runtime.pause_as("node.b", "operator");
    host.runtime.pause_as("node.c", "deploy-9x");

    let released = run(
        &host,
        &[
            "release-hold",
            "--root",
            word(&root),
            "--reason",
            "deploy-9",
        ],
    );
    assert_done(&released, "install_release_hold");
    assert_eq!(released.outcome.value("count"), Some(&json!(1)));
    assert_eq!(calls(&host), ["resume node.a"]);
    assert_eq!(host.runtime.pause_of("node.a"), None);
    assert_eq!(host.runtime.pause_of("node.b").as_deref(), Some("operator"));
    assert_eq!(
        host.runtime.pause_of("node.c").as_deref(),
        Some("deploy-9x")
    );
    assert_eq!(released.members["release"]["resumed"], json!(["node.a"]));
}
