// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5B1c (register §1.287, "Slice LAB-5B1c"; coordinator work order
// rd-1113/brief-lab5b1c.md): what LAB-5B1b left, the receivers' flag lists, and `config restore`'s
// `sha`.
// - `rescue_handed_flags_are_what_the_receivers_take`: each row whose answer is an installer
//   command (`rescue_reads::INSTALLER`) or the agent channel's words (`AGENT`) has exactly one
//   receiver's list, the constant the parse of its receiver reads (`install::FlagLists` beside the
//   installer command its line names; `agent_forward::cli::*_FLAGS` of the agent channel's
//   command), and every flag its constant writes (the root's, the state root's, a parameter's) is
//   one that list takes. A row added later whose command has no list fails, naming it.
// - `rescue_receivers_refuse_no_declared_parameter`: each receiver, fed the command line its row
//   builds for each of the row's declared parameters (a made-up value of the declared type, beside
//   the row's required ones), refuses none of them as `unknown`: the installer as the module runs
//   it (the one line the module handed on is the receiver's command), on a root that does not
//   exist, which each installer command refuses only after its flags are read; the agent
//   channel's flag reader with the command's list, on the words after the command's own.
// - `config_restore_refuses_a_wrong_sha_as_config_replace_does`: on an adopted fake root, `config
//   restore <file> --sha <wrong>` is refused as `config replace <file> --sha <wrong>` is
//   (`actingctl_usage_invalid`, `--sha`, `unusable`), the same line and exit, before anything
//   changes: the root byte for byte as it was, nothing more asked of a Runtime.
// A failure names the row, the parameter or flag and the direction. The embedded table, the
// constants and the fake host of `fixture.rs` with its fake clock: no Runtime, no device, nothing
// written but the test's temporary folder.

use super::fixture::{BuildSpec, C1, FakeHost, code, live_root, run, tree, word};
use super::{FlagLists, adopt, deploy, release, status, undo};
use crate::agent_channel::Answer as Answered;
use crate::rescue::{self, Context, installed};
use crate::rescue_reads::{AGENT, INSTALLER};
use actingcommand_agent_forward::cli::{self as agent_cli, CliFailure, UsageProblem};
use actingcommand_lab_ops::core_reads::{Handed, Part, ROOT_FLAG, STATE_ROOT_FLAG, Written};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_lab_ops::table::{OperationTable, Param, ParamType};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// The root the module acts on in the receivers' test: no folder, so that every installer
/// command refuses at it once its flags are read.
const ROOT: &str = "neutral-root-of-no-folder";
/// The state root the agent channel's words name.
const STATE: &str = "state";
/// A SHA-256 no file of the test has.
const WRONG_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn table() -> OperationTable {
    actingcommand_lab_ops::table::operation_table().expect("the operation table")
}

/// The lists of the installer command the line of `handed` names, which that command's parse
/// reads; `None` for a command of no known list.
fn installer_lists(handed: &Handed) -> Option<&'static FlagLists> {
    match handed.command {
        ["start"] => Some(&deploy::START_FLAGS),
        ["stop"] => Some(&deploy::STOP_FLAGS),
        ["guards"] => Some(&deploy::GUARDS_FLAGS),
        ["release-hold"] => Some(&deploy::RELEASE_HOLD_FLAGS),
        ["undo"] => Some(&undo::UNDO_FLAGS),
        ["status"] => Some(&status::STATUS_FLAGS),
        ["leftovers"] => Some(&status::LEFTOVERS_FLAGS),
        ["resume"] => Some(&super::RESUME_FLAGS),
        ["release", "verify"] => Some(&release::VERIFY_FLAGS),
        ["release", "stage"] => Some(&release::STAGE_FLAGS),
        ["adopt"] => Some(&adopt::ADOPT_FLAGS),
        _ => None,
    }
}

/// The list of the agent channel's command the words of `handed` name, which that command's arm
/// parses by; `None` for a command of no known list.
fn agent_list(handed: &Handed) -> Option<&'static [&'static str]> {
    let list: &'static [&'static str] = match handed.command {
        ["add"] => &agent_cli::ADD_FLAGS,
        ["remove"] => &agent_cli::REMOVE_FLAGS,
        ["list"] => &agent_cli::LIST_FLAGS,
        ["claude-hook"] => &agent_cli::CLAUDE_HOOK_FLAGS,
        ["claude-register"] => &agent_cli::CLAUDE_REGISTER_FLAGS,
        ["notice"] => &agent_cli::NOTICE_FLAGS,
        _ => return None,
    };
    Some(list)
}

/// The flags the command line of `handed` can carry: the root's, the state root's and each
/// parameter's.
fn written_flags(handed: &Handed) -> BTreeSet<&'static str> {
    let mut flags = BTreeSet::new();
    for part in handed.parts {
        match *part {
            Part::Root => {
                flags.insert(ROOT_FLAG);
            }
            Part::StateRoot => {
                flags.insert(STATE_ROOT_FLAG);
            }
            Part::Param(
                _,
                Written::Flag(written)
                | Written::Switch(written)
                | Written::Joined(written)
                | Written::Each(written)
                | Written::SinceNow(written),
            ) => {
                flags.insert(written);
            }
            Part::Param(_, Written::RunOrRequest(run, request)) => {
                flags.insert(run);
                flags.insert(request);
            }
            Part::Param(_, Written::Word | Written::Beside) | Part::Further => {}
        }
    }
    flags
}

/// Each flag the constant `handed` writes against the flags its receiver takes.
fn held_to(handed: &Handed, taken: &BTreeSet<&'static str>, failures: &mut Vec<String>) {
    for written in written_flags(handed) {
        if !taken.contains(written) {
            failures.push(format!(
                "{}: {written}: row to receiver: its command line carries it, the receiver of \
                 {:?} does not take it",
                handed.row, handed.command
            ));
        }
    }
}

#[test]
fn rescue_handed_flags_are_what_the_receivers_take() {
    let mut failures = Vec::new();
    for handed in &INSTALLER {
        let Some(lists) = installer_lists(handed) else {
            failures.push(format!(
                "{}: no receiver's list: the installer command {:?} has no known constant",
                handed.row, handed.command
            ));
            continue;
        };
        let taken = lists
            .with_value
            .iter()
            .chain(lists.lists)
            .chain(lists.switches)
            .copied()
            .collect::<BTreeSet<_>>();
        held_to(handed, &taken, &mut failures);
    }
    for handed in &AGENT {
        let Some(list) = agent_list(handed) else {
            failures.push(format!(
                "{}: no receiver's list: the agent channel's command {:?} has no known constant",
                handed.row, handed.command
            ));
            continue;
        };
        let taken = list.iter().copied().collect::<BTreeSet<_>>();
        held_to(handed, &taken, &mut failures);
    }
    assert!(
        failures.is_empty(),
        "command lines that differ from their receiver:\n{}",
        failures.join("\n")
    );
}

/// A made-up value of `param`'s declared type.
fn made_up(param: &Param) -> Value {
    match &param.value_type {
        ParamType::Boolean => json!(true),
        ParamType::Enum(words) => json!(words[0]),
        ParamType::OperationId => json!("op-1"),
        ParamType::Dir | ParamType::File | ParamType::Path => json!("neutral-of-no-file"),
        _ => json!("neutral"),
    }
}

/// The parameters of `handed`'s row to feed its receiver: its required ones, then with each of
/// its declared parameters in turn; each named.
fn cases(table: &OperationTable, handed: &Handed) -> Vec<(String, Map<String, Value>)> {
    let operation = table
        .operation(handed.row)
        .unwrap_or_else(|| panic!("{} is a row of the table", handed.row));
    let base = operation
        .params
        .iter()
        .filter(|param| param.required)
        .map(|param| (param.name.clone(), made_up(param)))
        .collect::<Map<String, Value>>();
    let mut cases = vec![("(none but the required)".to_owned(), base.clone())];
    for param in &operation.params {
        let mut params = base.clone();
        params.insert(param.name.clone(), made_up(param));
        cases.push((param.name.clone(), params));
    }
    cases
}

#[test]
fn rescue_receivers_refuse_no_declared_parameter() {
    let table = table();
    let work = TempDir::new().expect("a temporary folder");
    let root = Path::new(ROOT);
    let location = rescue::location_of(root, &root.join("actingd.config.json"), Path::new(STATE));
    let mut failures = Vec::new();
    for handed in &INSTALLER {
        let row = handed.row;
        for (named, params) in cases(&table, handed) {
            let host = FakeHost::new(work.path());
            let context = Context {
                host: &host,
                location: &location,
                initiator: Initiator::LabRescue,
            };
            installed::take();
            let line = match rescue::answer_with(&context, row, &params) {
                Answered::Line(exit) => exit.line.expect("an encoded line"),
                other => panic!("{row}: {named}: a line, not {other:?}"),
            };
            // The one line handed on is the receiver's command: the answer is its.
            let lines = installed::take();
            let reached = lines.len() == 1
                && lines[0]
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .starts_with(handed.command);
            if !reached {
                failures.push(format!(
                    "{row}: {named}: handed {lines:?}, not one line of the installer command {:?}",
                    handed.command
                ));
                continue;
            }
            let outcome = &line["outcome"];
            if outcome["code"] == "actingctl_usage_invalid"
                && outcome["values"]["argument_problem"] == "unknown"
            {
                failures.push(format!(
                    "{row}: {named}: the installer refuses {} as unknown",
                    outcome["values"]["argument"]
                ));
            }
        }
    }
    for handed in &AGENT {
        let row = handed.row;
        let Some(list) = agent_list(handed) else {
            failures.push(format!("{row}: no receiver's list"));
            continue;
        };
        for (named, params) in cases(&table, handed) {
            let words = rescue::agent_words(handed.command[0], &params, Path::new(STATE));
            // The command's words, and `claude-hook`'s action: the channel reads them before its
            // flags.
            let own = handed.command.len()
                + handed
                    .params()
                    .filter(|&(name, written)| {
                        written == Written::Word && params.contains_key(name)
                    })
                    .count();
            let Some(flags) = words.get(own..) else {
                failures.push(format!(
                    "{row}: {named}: its words {words:?} are fewer than its own {own}"
                ));
                continue;
            };
            if let Err(CliFailure::Usage {
                argument,
                problem: UsageProblem::Unknown,
                ..
            }) = agent_cli::flags(flags, list)
            {
                failures.push(format!(
                    "{row}: {named}: the agent channel refuses {argument:?} as unknown"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "declared parameters their receiver refuses:\n{}",
        failures.join("\n")
    );
}

/// The rescue row `name` with `params` on `root`, as the module's initiator; its one line and
/// exit code.
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
    let params = match params {
        Value::Object(params) => params,
        other => panic!("the parameters are an object: {other}"),
    };
    match rescue::answer_with(&context, name, &params) {
        Answered::Line(exit) => (exit.line.expect("an encoded line"), exit.code),
        other => panic!("{name}: a line, not {other:?}"),
    }
}

#[test]
fn config_restore_refuses_a_wrong_sha_as_config_replace_does() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    let root = live_root(temp.path(), "sha", &BuildSpec::new(C1));
    let adopted = run(&host, &["adopt", "--root", word(&root)]);
    assert_eq!(code(&adopted), "install_command_done", "{adopted:?}");
    // A kept configuration beside the root: a copy of the live one.
    let kept = temp.path().join("kept.json");
    fs::copy(root.join("actingd.config.json"), &kept).expect("the kept configuration");
    let before = tree(&root);
    let asked = host.runtime.calls.borrow().len();

    let (replaced, replaced_exit) = rescue_line(
        &host,
        &root,
        "config replace",
        json!({"file": word(&kept), "sha": WRONG_SHA}),
    );
    let (restored, restored_exit) = rescue_line(
        &host,
        &root,
        "config restore",
        json!({"backup": word(&kept), "sha": WRONG_SHA}),
    );

    // `config replace`'s refusal: the file's SHA-256 is not `--sha`.
    let outcome = &replaced["outcome"];
    assert_eq!(outcome["code"], "actingctl_usage_invalid", "{replaced}");
    assert_eq!(outcome["values"]["argument"], "--sha", "{replaced}");
    assert_eq!(
        outcome["values"]["argument_problem"], "unusable",
        "{replaced}"
    );
    assert_ne!(replaced_exit, 0, "{replaced}");
    // `config restore`'s is the same, line and exit.
    assert_eq!(restored, replaced);
    assert_eq!(restored_exit, replaced_exit);
    // Refused before anything changed.
    assert!(tree(&root) == before, "the root changed");
    assert_eq!(
        host.runtime.calls.borrow().len(),
        asked,
        "nothing more asked of a Runtime"
    );
}
