// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S10 (register §1.346; contracts/emulator-control.md, "Offline instance discovery";
// coordinator work orders rd-1113/brief-453-s10-offline-discovery.md and, fix 1,
// rd-1113/brief-1681-fix1.md): the rescue row `emulators` runs the live build's `actingd discover
// --config <the root's configuration> --install-root <root>`, once, and nothing else, and writes
// nothing in the root or its state root:
// - `emulators_answers_the_live_builds_report`: a completed discovery is `install_command_done`
//   of `install_emulators` with actingd's report as `report` and actingd's outcome as its link;
// - `emulators_refuses_with_actingds_outcome_as_its_cause`: a refused discovery is
//   `install_command_refused`, `install_refusal` `emulator_discovery_failed`, with actingd's
//   outcome as its `caused_by` link and no report;
// - `emulators_fix1_a_discovery_that_never_ends_is_ended_at_its_bound` (fix 1): an actingd that
//   never exits is ended at the row's bound of 60 s, once, and the row refuses
//   `emulator_discovery_failed` with `timeout_ms` the bound, no cause and no child left running.
// The machine is the fake host of `fixture.rs`, its actingd a fake that prints the line the test
// sets: no program is spawned, no emulator is read; every root is a temporary folder.

use super::fixture::{BuildSpec, C1, FakeHost, live_root, tree};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Map, Value, json};
use std::path::Path;
use tempfile::TempDir;

/// The rescue row `emulators` on `root`, as a Lab process the UI started; its one line and exit
/// code.
fn emulators_line(host: &FakeHost, root: &Path) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
    let context = Context {
        host,
        location: &location,
        initiator: Initiator::LabRescue,
    };
    match rescue::answer_with(&context, "emulators", &Map::new()) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("emulators: a line, not {other:?}"),
    }
}

/// The one program the row ran: the live build's actingd with the root's configuration and the
/// root.
fn assert_ran_discover_once(host: &FakeHost, root: &Path) {
    let config = root.join("actingd.config.json");
    assert_eq!(
        *host.log.borrow(),
        [format!(
            "live actingcommand-actingd.exe discover --config {} --install-root {}",
            config.to_string_lossy(),
            root.to_string_lossy()
        )],
        "the row runs actingd discover once and nothing else"
    );
}

/// A root and its state root, with what each holds before the row runs.
struct Prepared {
    _temp: TempDir,
    host: FakeHost,
    root: std::path::PathBuf,
    state_root: std::path::PathBuf,
}

fn prepared() -> Prepared {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "emulators", &BuildSpec::new(C1));
    let state_root = temp.path().join("emulators-state");
    Prepared {
        _temp: temp,
        host,
        root,
        state_root,
    }
}

#[test]
fn emulators_answers_the_live_builds_report() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    let report = json!({
        "mumu_root": {"path": "C:\\Emulator", "source": "config"},
        "provider_version": "6.5.7.0",
        "instances": [
            {"instance_index": 0, "instance_name": "neutral-a", "adb_host": "127.0.0.1",
                "adb_port": 16384, "running": true, "bound_alias": "neutral.a",
                "android_version": "12"},
            {"instance_index": 1, "instance_name": "neutral-b", "running": false}
        ]
    });
    *host.programs.discover.borrow_mut() = (
        0,
        json!({
            "schema_version": "actingcommand.actingd.discover.v1",
            "outcome": {"schema_version": "actingcommand.outcome.v1",
                "code": "actingd_discover_completed", "category": "success", "values": {}},
            "report": report,
        }),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = emulators_line(&host, &root);

    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_emulators",
        "{line}"
    );
    assert_eq!(line["report"], report, "actingd's report is the answer");
    let causes = line["outcome"]["causes"]
        .as_array()
        .expect("the outcome's links");
    assert_eq!(causes.len(), 1, "{line}");
    assert_eq!(causes[0]["relation"], "related", "{line}");
    assert_eq!(causes[0]["code"], "actingd_discover_completed", "{line}");
    assert_ran_discover_once(&host, &root);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn emulators_refuses_with_actingds_outcome_as_its_cause() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    *host.programs.discover.borrow_mut() = (
        1,
        json!({
            "schema_version": "actingcommand.actingd.discover.v1",
            "outcome": {"schema_version": "actingcommand.outcome.v1",
                "code": "instance_discovery_unavailable", "category": "error", "values": {}},
        }),
    );
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = emulators_line(&host, &root);

    assert_eq!(exit, 1, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_refused", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_emulators",
        "{line}"
    );
    assert_eq!(
        line["outcome"]["values"]["install_refusal"], "emulator_discovery_failed",
        "{line}"
    );
    let causes = line["outcome"]["causes"]
        .as_array()
        .expect("the outcome's links");
    assert_eq!(causes.len(), 1, "{line}");
    assert_eq!(causes[0]["relation"], "caused_by", "{line}");
    assert_eq!(
        causes[0]["code"], "instance_discovery_unavailable",
        "{line}"
    );
    assert!(
        line.get("report").is_none(),
        "a refusal carries no report: {line}"
    );
    assert_ran_discover_once(&host, &root);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

/// Fix 1 (review, the row never leaves its child behind): an actingd discover that never exits is
/// ended at the row's bound, 60 s (the child's own three bounds of 10 s and a margin), not at
/// `install.child.check_config_timeout_ms`, and the row refuses with a reason that says the
/// discovery timed out: `emulator_discovery_failed` with `timeout_ms` the bound. Nothing of the
/// child remains: it was ended there, and the refusal names no process left running (the stall
/// rule's `pid` and `config_key`).
#[test]
fn emulators_fix1_a_discovery_that_never_ends_is_ended_at_its_bound() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    host.programs.stall.borrow_mut().push("discover");
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = emulators_line(&host, &root);

    assert_eq!(exit, 1, "{line}");
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_command_refused", "{line}");
    assert_eq!(
        outcome["values"]["operation"], "install_emulators",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["install_refusal"], "emulator_discovery_failed",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["timeout_ms"], 60_000,
        "the discovery timed out at the row's bound: {line}"
    );
    assert!(
        outcome["values"].get("pid").is_none() && outcome["values"].get("config_key").is_none(),
        "no child is left running to name: {line}"
    );
    assert!(
        outcome["causes"].as_array().is_none_or(Vec::is_empty),
        "an ended discovery has no cause: {line}"
    );
    assert!(
        line.get("report").is_none(),
        "a refusal carries no report: {line}"
    );
    assert_eq!(
        *host.programs.ended.borrow(),
        [("discover".to_owned(), 60_000)],
        "the child is ended at the bound, once"
    );
    assert_ran_discover_once(&host, &root);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}
