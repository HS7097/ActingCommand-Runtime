// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S12 (contracts/state-root-installation.md, "Handing a state root over";
// contracts/install-cli.md, "The Lab's rescue module"; coordinator work order
// rd-1114/brief-453-s12-state-root-claim.md, rulings 6 and 7): the rescue row `state-root claim`
// and the member `state_root` of `op status`, on the fake host of `fixture.rs` (its actingd prints
// the line the test sets; no program is spawned; every root is a temporary folder):
// - `state_root_claim_answers_done_and_notes_a_replaced_record`: the live build's `actingd
//   claim-state-root --config <the root's configuration> --actor <initiator>`, once, and nothing
//   else; `install_command_done` of `install_state_root_claim` with actingd's report, the
//   replaced record's claim time as `previous` and the note `fingerprints_discontinuous`;
// - `state_root_claim_without_a_replaced_record_has_no_caveat`: a written record (with
//   `--new-identity` handed on) and an unchanged one: no note, no `previous`; nothing changed is
//   `installation_changed` false; actingd's refusal is the answer;
// - `state_root_claim_is_refused_while_an_actingd_image_runs_under_the_root`: the update path's
//   stopped rule, `runtime_running` with the image's pid, before any program runs;
// - `op_status_names_the_state_root_and_its_installation_state`: each of the five states as the
//   live build's check-config reports it, the root and its state root byte for byte as before.

use super::fixture::{BuildSpec, C1, FakeHost, live_root, tree};
use super::host::RunningImage;
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context};
use actingcommand_lab_ops::initiator::Initiator;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The claim time of the record a fake hand-over replaced.
const PREVIOUS_MS: u64 = 1_800_000_000_000;

/// A root and its state root.
struct Prepared {
    _temp: TempDir,
    host: FakeHost,
    root: PathBuf,
    state_root: PathBuf,
}

fn prepared() -> Prepared {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "claim", &BuildSpec::new(C1));
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    Prepared {
        _temp: temp,
        host,
        root,
        state_root,
    }
}

/// The rescue row `name` with `params` on `root`, as a Lab process the UI started; its one line
/// and exit code.
fn rescue_line(host: &FakeHost, root: &Path, name: &str, params: Value) -> (Value, u8) {
    let config = root.join("actingd.config.json");
    let state_root = actingcommand_lab_ops::locate::configured_state_root(&config)
        .expect("the configured state root");
    let location = rescue::location_of(root, &config, &state_root);
    let context = Context {
        host,
        location: &location,
        initiator: Initiator::LabRescue,
    };
    let Value::Object(params) = params else {
        panic!("parameters are an object: {params}");
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

/// The line `actingd claim-state-root` prints for a hand-over whose record was `record`.
fn claimed(record: &str, changed: bool, previous: Option<u64>) -> Value {
    let mut line = json!({
        "schema_version": "actingcommand.actingd.claim-state-root.v1",
        "outcome": {"schema_version": "actingcommand.outcome.v1", "code": "state_root_claimed",
            "category": "info", "values": {"path": "C:\\ActingCommand\\state"}},
        "actor": Initiator::LabRescue.as_str(),
        "changed": changed,
        "identity": "read",
        "record": record,
        "claimed_at_unix_ms": PREVIOUS_MS + 1,
        "kept": [],
    });
    if let Some(previous) = previous {
        line["previous"] = json!({"claimed_at_unix_ms": previous});
    }
    line
}

/// The one program the row ran: the live build's actingd with the root's configuration and the
/// initiator, and `--new-identity` when given.
fn assert_ran_claim_once(host: &FakeHost, root: &Path, new_identity: bool) {
    let config = root.join("actingd.config.json");
    let mut expected = format!(
        "live actingcommand-actingd.exe claim-state-root --config {} --actor {}",
        config.to_string_lossy(),
        Initiator::LabRescue.as_str()
    );
    if new_identity {
        expected.push_str(" --new-identity");
    }
    assert_eq!(
        *host.log.borrow(),
        [expected],
        "the row runs actingd claim-state-root once and nothing else"
    );
}

/// The links of `line`'s outcome.
fn links(line: &Value) -> Vec<Value> {
    line["outcome"]["causes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn state_root_claim_answers_done_and_notes_a_replaced_record() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    let report = claimed("replaced", true, Some(PREVIOUS_MS));
    *host.programs.claim.borrow_mut() = (0, report.clone());
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = rescue_line(&host, &root, "state-root claim", json!({}));

    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["outcome"]["values"]["operation"], "install_state_root_claim",
        "{line}"
    );
    assert_eq!(
        line["outcome"]["values"]["installation_changed"], true,
        "{line}"
    );
    assert_eq!(line["report"], report, "actingd's report is in the answer");
    assert_eq!(line["actor"], Initiator::LabRescue.as_str(), "{line}");
    assert_eq!(
        line["previous"],
        json!({"claimed_at_unix_ms": PREVIOUS_MS}),
        "the replaced record's claim time: {line}"
    );
    let links = links(&line);
    assert!(
        links
            .iter()
            .any(|link| link["relation"] == "related" && link["code"] == "state_root_claimed"),
        "actingd's outcome: {line}"
    );
    let notes = links
        .iter()
        .filter(|link| link["relation"] == "note")
        .collect::<Vec<_>>();
    let [note] = notes.as_slice() else {
        panic!("one note: {line}");
    };
    assert_eq!(note["code"], "install_caveat_noted", "{line}");
    assert_eq!(
        note["values"]["install_caveat"], "fingerprints_discontinuous",
        "{line}"
    );
    assert_ran_claim_once(&host, &root, false);
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(
        tree(&state_root),
        state_before,
        "the row writes nothing itself"
    );
}

#[test]
fn state_root_claim_without_a_replaced_record_has_no_caveat() {
    // A written record, `--new-identity` handed on.
    let Prepared {
        _temp,
        host,
        root,
        state_root: _,
    } = prepared();
    *host.programs.claim.borrow_mut() = (0, claimed("written", true, None));
    let (line, exit) = rescue_line(
        &host,
        &root,
        "state-root claim",
        json!({"new_identity": true}),
    );
    assert_eq!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "install_command_done", "{line}");
    assert_eq!(
        line["outcome"]["values"]["installation_changed"], true,
        "{line}"
    );
    assert!(
        !links(&line).iter().any(|link| link["relation"] == "note"),
        "no caveat: {line}"
    );
    assert!(line.get("previous").is_none(), "{line}");
    assert_ran_claim_once(&host, &root, true);

    // This installation's own record: nothing changed.
    let Prepared {
        _temp,
        host,
        root,
        state_root: _,
    } = prepared();
    *host.programs.claim.borrow_mut() = (0, claimed("unchanged", false, None));
    let (line, exit) = rescue_line(&host, &root, "state-root claim", json!({}));
    assert_eq!(exit, 0, "{line}");
    assert_eq!(
        line["outcome"]["values"]["installation_changed"], false,
        "nothing changed: {line}"
    );
    assert!(
        !links(&line).iter().any(|link| link["relation"] == "note"),
        "no caveat: {line}"
    );
    assert_ran_claim_once(&host, &root, false);

    // actingd's refusal is the answer.
    let Prepared {
        _temp,
        host,
        root,
        state_root: _,
    } = prepared();
    *host.programs.claim.borrow_mut() = (
        2,
        json!({
            "schema_version": "actingcommand.actingd.claim-state-root.v1",
            "outcome": {"schema_version": "actingcommand.outcome.v1",
                "code": "owner_conflict", "category": "error", "values": {}},
        }),
    );
    let (line, exit) = rescue_line(&host, &root, "state-root claim", json!({}));
    assert_ne!(exit, 0, "{line}");
    assert_eq!(line["outcome"]["code"], "owner_conflict", "{line}");
    assert_ran_claim_once(&host, &root, false);
}

#[test]
fn state_root_claim_is_refused_while_an_actingd_image_runs_under_the_root() {
    let Prepared {
        _temp,
        host,
        root,
        state_root,
    } = prepared();
    *host.programs.claim.borrow_mut() = (0, claimed("replaced", true, Some(PREVIOUS_MS)));
    host.processes.borrow_mut().push(RunningImage {
        pid: 4711,
        path: format!(
            "{}\\runtime\\actingcommand-actingd.exe",
            super::files::plain(&root)
        ),
        mcp_serve: false,
        parent_pid: None,
    });
    let root_before = tree(&root);
    let state_before = tree(&state_root);

    let (line, exit) = rescue_line(&host, &root, "state-root claim", json!({}));

    assert_eq!(exit, 1, "{line}");
    let outcome = &line["outcome"];
    assert_eq!(outcome["code"], "install_command_refused", "{line}");
    assert_eq!(
        outcome["values"]["operation"], "install_state_root_claim",
        "{line}"
    );
    assert_eq!(
        outcome["values"]["install_refusal"], "runtime_running",
        "{line}"
    );
    assert_eq!(outcome["values"]["pid"], 4711, "{line}");
    assert!(
        host.log.borrow().is_empty(),
        "no program ran: {:?}",
        host.log.borrow()
    );
    assert_eq!(tree(&root), root_before, "nothing written in the root");
    assert_eq!(tree(&state_root), state_before, "the state root untouched");
}

#[test]
fn op_status_names_the_state_root_and_its_installation_state() {
    let check_config = |code: &str, category: &str, values: Value| {
        json!({
            "schema_version": "actingcommand.actingd.check-config.v2",
            "outcome": {"schema_version": "actingcommand.outcome.v1", "code": code,
                "category": category, "values": values},
        })
    };
    // The three states check-config reports as its report's member.
    for state in ["claimed_by_this", "unclaimed", "not_installed"] {
        let Prepared {
            _temp,
            host,
            root,
            state_root,
        } = prepared();
        host.programs
            .check_config_report
            .borrow_mut()
            .insert("state_root_installation", json!(state));
        let root_before = tree(&root);
        let state_before = tree(&state_root);
        let (line, exit) = rescue_line(&host, &root, "op status", json!({}));
        assert_eq!(exit, 0, "{state}: {line}");
        assert_eq!(
            line["outcome"]["values"]["operation"], "install_status",
            "{state}: {line}"
        );
        assert_eq!(
            line["state_root"],
            json!({"path": super::files::plain(&state_root), "installation": state}),
            "{state}: {line}"
        );
        assert!(
            host.log
                .borrow()
                .iter()
                .any(|ran| ran.contains("check-config") && ran.contains("--install-root")),
            "{state}: the live build's check-config of the root: {:?}",
            host.log.borrow()
        );
        assert_eq!(tree(&root), root_before, "{state}: read only");
        assert_eq!(tree(&state_root), state_before, "{state}: read only");
    }
    // The two states check-config refuses with the start's codes, at stage config_state_root.
    let refusals = [
        check_config(
            "state_root_installation_mismatch",
            "fatal",
            json!({"path": "C:\\ActingCommand\\state", "at_unix_ms": PREVIOUS_MS,
                "stage": "config_state_root"}),
        ),
        check_config(
            "state_root_installation_record_invalid",
            "fatal",
            json!({"path": "C:\\ActingCommand\\state\\installation.json",
                "stage": "config_state_root"}),
        ),
    ];
    for refused in refusals {
        let code = refused["outcome"]["code"].clone();
        let Prepared {
            _temp,
            host,
            root,
            state_root,
        } = prepared();
        *host.programs.check_config_line.borrow_mut() = Some((2, refused));
        let (line, exit) = rescue_line(&host, &root, "op status", json!({}));
        assert_eq!(exit, 0, "{code}: the status answers: {line}");
        let member = &line["state_root"];
        assert_eq!(
            member["path"],
            json!(super::files::plain(&state_root)),
            "{code}: {line}"
        );
        assert_eq!(member["installation"], Value::Null, "{code}: {line}");
        assert_eq!(
            member["check_config"]["outcome"]["code"], code,
            "{code}: as check-config reports it: {line}"
        );
    }
}
