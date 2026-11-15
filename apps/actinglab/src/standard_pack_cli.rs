// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #373 NAV-T (M-373 v1.1 §3.9 second row, §10.1 NAV-4; wave-18 ruling 1):
//! `actinglab standard-pack check --root <game root>` runs, on an installed game root
//! `<standard_packs.root>/<game>/`, the checks `package bundle` runs on the root it lays out,
//! and lists the packs whose entry pages the page list does not hold. It only reads. ROUT-L
//! RL-1a: it checks the root's `schedule.json` too, by the Runtime's composer. Workflow #392
//! DA-4: and its `display-layout.json`, by the Runtime's root reader.
//!
//! ROUT-L RL-1a (contracts/standard-pack-schedule.md): `actinglab standard-pack compose --root
//! <standard_packs.root> --instance <alias>=<game>/<server>... --out <new directory>` composes
//! the schedule rules of the bound games as the Runtime composes them and writes the four V2
//! documents and the manifest. It writes a new directory, so `--dry-run` refuses it.
//!
//! Workflow #464 P1 (contracts/standard-pack-authoring.md; register §1.377): `actinglab
//! standard-pack adopt --repo <checkout> (--record-id <id> | --package <path>) [--decide
//! <decision.json>] [--replace] [--state-dir <dir>]` makes a recorded package a member of its
//! game's standard pack in a resource repository's checkout. Under `--dry-run` it runs every check
//! and writes nothing in the repository. Workflow #463 S3-4: `--held-reason <text>
//! [--held-reference <text>]` in place of `--decide` is the decision of the class `held` without a
//! document, read by the same reader as one.
//!
//! Workflow #464 P2 (contracts/resource-verify.md, "`standard-pack build`"): `actinglab
//! standard-pack build --repo <checkout> --source-repository <owner>/<name> --out <directory>
//! [--commit <40-hex>]` builds a game's whole standard pack from one commit of its working root, on
//! the steps of `resource verify` (`crate::resource_verify`). It writes a directory, so `--dry-run`
//! refuses it.
//!
//! Workflow #464 P3 (contracts/standard-pack-schedule.md, "Readiness"): `actinglab standard-pack
//! ready --root <game root>` answers, for every pack of the root, whether the scheduler can
//! dispatch it or what it waits for, the thirteen items judged by the resource tooling's one
//! function. It reads the game root only: it writes nothing, starts nothing and reads no
//! installation, so the items of an installation are answered not judged (I3 an information
//! line), no pack is answered dispatchable, and the answer says `installation: not_read`.

use crate::{CliError, CliOutcome, FlagArgs, GlobalOptions};
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_resource_tooling::{
    AdoptDecision, AdoptSource, AdoptWorktree, StandardPackAdoptRequest, StandardPackCheckRequest,
    StandardPackComposeRequest, StandardPackReadyRequest, check_standard_pack_root,
    compose_standard_packs, judge_standard_pack_readiness, standard_pack_adopt,
};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};

/// Workflow #434 LAB-5B2: the flags `standard-pack check` takes.
pub(crate) const CHECK_FLAGS: [&str; 1] = ["--root"];
/// Workflow #434 LAB-5B2: the flags `standard-pack compose` takes (its own `--instance`, given
/// after the command's words, `cli_parse::COMMAND_OPTIONS`).
pub(crate) const COMPOSE_FLAGS: [&str; 3] = ["--root", "--instance", "--out"];
/// Workflow #464 P1: the flags `standard-pack adopt` takes. Workflow #463 S3-4: and the two of a
/// held decision without a document.
pub(crate) const ADOPT_FLAGS: [&str; 8] = [
    "--repo",
    "--record-id",
    "--package",
    DECIDE_FLAG,
    HELD_REASON_FLAG,
    HELD_REFERENCE_FLAG,
    "--replace",
    "--state-dir",
];
/// Workflow #463 S3-4: the decision document's flag, and the two of a held decision without one.
const DECIDE_FLAG: &str = "--decide";
const HELD_REASON_FLAG: &str = "--held-reason";
const HELD_REFERENCE_FLAG: &str = "--held-reference";
/// Workflow #464 P1: the flag of `standard-pack adopt` that takes no value, which takes none from
/// the next word but `true` or `false` (`FlagArgs::parse_switches`).
pub(crate) const ADOPT_SWITCHES: [&str; 1] = ["--replace"];
/// Bytes of git's stderr one `raw_text` keeps (the key table's bound).
const RAW_TEXT_LIMIT: usize = 4096;
/// The variables that would point git away from `--repo`.
const REDIRECTING_VARIABLES: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];
/// Workflow #464 P3: the flags `standard-pack ready` takes.
pub(crate) const READY_FLAGS: [&str; 1] = ["--root"];

pub(crate) fn options() -> [&'static str; 1] {
    ["--root <installed game root: <standard_packs.root>/<game>>"]
}

/// Workflow #464 P3: the flags of `standard-pack ready`.
pub(crate) fn ready_options() -> [&'static str; 1] {
    ["--root <a game root, installed or a bundle laid out as one: <standard_packs.root>/<game>>"]
}

/// ROUT-L RL-1a: the flags of `standard-pack compose`.
pub(crate) fn compose_options() -> [&'static str; 3] {
    [
        "--root <standard_packs.root: the directory of the game roots>",
        "--instance <alias>=<game>/<server> (repeatable, once per bound instance)",
        "--out <new directory: tasks.json, pools.json, activity.json, timeline.json, manifest.json>",
    ]
}

/// Workflow #464 P1: the flags of `standard-pack adopt`.
pub(crate) fn adopt_options() -> [&'static str; 8] {
    [
        "--repo <the game's working root in a resource repository's checkout>",
        "--record-id <a stopped recording of the state directory>",
        "--package <a content directory, .zip or .json container>",
        "--decide <decision.json: actingcommand.pack-decision.v1; a first adoption needs it or --held-reason>",
        "--held-reason <text> (the decision of the class held without a document: its reason; not with --decide)",
        "--held-reference <text> (with --held-reason only: the held decision's reference)",
        "--replace (adopt a package adopted before again)",
        "--state-dir <the Lab's state directory; default actinglab's>",
    ]
}

pub(crate) fn run_standard_pack(
    sub: &str,
    global: &GlobalOptions,
    args: &[String],
) -> CliOutcome<Value> {
    match sub {
        "check" => {
            let flags = FlagArgs::parse_values(args)?;
            if let Some(flag) = flags
                .flags
                .keys()
                .find(|flag| !CHECK_FLAGS.contains(&flag.as_str()))
            {
                return Err(CliError::usage(format!(
                    "standard-pack check does not accept {flag}"
                )));
            }
            flags.expect_positionals("standard-pack check", 0)?;
            let response = check_standard_pack_root(StandardPackCheckRequest {
                root: flags.required_path("--root")?,
            })?;
            serde_json::to_value(response).map_err(|error| {
                CliError::device(format!("failed to serialize the check result: {error}"))
            })
        }
        "compose" => {
            let flags = FlagArgs::parse_values(args)?;
            if let Some(flag) = flags
                .flags
                .keys()
                .find(|flag| !COMPOSE_FLAGS.contains(&flag.as_str()))
            {
                return Err(CliError::usage(format!(
                    "standard-pack compose does not accept {flag}"
                )));
            }
            flags.expect_positionals("standard-pack compose", 0)?;
            for flag in ["--root", "--out"] {
                if flags.count(flag) > 1 {
                    return Err(CliError::usage(format!(
                        "standard-pack compose takes {flag} once"
                    )));
                }
            }
            let root = flags.required_path("--root")?;
            let out = flags.required_path("--out")?;
            let instances = flags
                .values("--instance")
                .iter()
                .map(|value| binding(value))
                .collect::<CliOutcome<Vec<_>>>()?;
            if instances.is_empty() {
                return Err(CliError::usage(
                    "missing --instance <alias>=<game>/<server>",
                ));
            }
            crate::dry_run_gate::refuse(
                global,
                "standard-pack compose",
                Some("standard-pack check"),
                Value::Null,
            )?;
            let response = compose_standard_packs(StandardPackComposeRequest {
                root,
                instances,
                out,
            })?;
            serde_json::to_value(response).map_err(|error| {
                CliError::device(format!("failed to serialize the composition: {error}"))
            })
        }
        // Workflow #464 P2: a game's whole standard pack from one commit of its working root.
        "build" => crate::resource_verify::run_standard_pack_build(global, args),
        "adopt" => run_adopt(global, args),
        // Workflow #464 P3: the readiness of every pack of a game root; it only reads.
        "ready" => {
            let flags = FlagArgs::parse_values(args)?;
            if let Some(flag) = flags
                .flags
                .keys()
                .find(|flag| !READY_FLAGS.contains(&flag.as_str()))
            {
                return Err(CliError::usage(format!(
                    "standard-pack ready does not accept {flag}"
                )));
            }
            flags.expect_positionals("standard-pack ready", 0)?;
            let response = judge_standard_pack_readiness(&StandardPackReadyRequest {
                root: flags.required_path("--root")?,
                declared_package: None,
                installation: None,
            })?;
            serde_json::to_value(response).map_err(|error| {
                CliError::device(format!("failed to serialize the readiness: {error}"))
            })
        }
        other => Err(CliError::usage(format!(
            "unknown standard-pack command: {other}"
        ))),
    }
}

/// Workflow #464 P1: `standard-pack adopt` (module doc). The recording `--record-id` names is the
/// state directory's `record-artifacts/<record>/lab`, as `record stop` left it.
fn run_adopt(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_switches(args, &ADOPT_SWITCHES);
    if let Some(flag) = flags
        .flags
        .keys()
        .find(|flag| !ADOPT_FLAGS.contains(&flag.as_str()))
    {
        return Err(CliError::usage(format!(
            "standard-pack adopt does not accept {flag}"
        )));
    }
    flags.expect_positionals("standard-pack adopt", 0)?;
    for flag in ADOPT_FLAGS {
        if flags.count(flag) > 1 {
            return Err(CliError::usage(format!(
                "standard-pack adopt takes {flag} once"
            )));
        }
        if flag != "--replace" && flags.given(flag) {
            flags.required(flag)?;
        }
    }
    let decision = adopt_decision(&flags)?;
    let repo = flags.required_path("--repo")?;
    let state_dir = crate::session_state_dir_from_flags(&flags)?;
    let source = match (
        flags.optional("--record-id"),
        flags.optional_path("--package"),
    ) {
        (Some(record_id), None) => AdoptSource::Recording {
            lab_dir: state_dir
                .join("record-artifacts")
                .join(crate::safe_file_stem(&record_id))
                .join("lab"),
            record_id,
        },
        (None, Some(package)) => AdoptSource::Package(package),
        _ => {
            return Err(CliError::usage(
                "standard-pack adopt takes one of --record-id <id> and --package <path>",
            ));
        }
    };
    let response = standard_pack_adopt(
        StandardPackAdoptRequest {
            repo,
            source,
            decision,
            replace: flags.bool("--replace"),
            dry_run: global.dry_run,
            state_dir,
            at_unix_ms: crate::current_unix_ms(),
        },
        &GitWorktree,
    )?;
    serde_json::to_value(response)
        .map_err(|error| CliError::device(format!("failed to serialize the adoption: {error}")))
}

/// Workflow #463 S3-4: the decision of `standard-pack adopt`, taken from its flags before anything
/// is read or written: `--decide <decision.json>`, the document at that path; or `--held-reason
/// <text> [--held-reference <text>]`, the decision of the class `held` from the two texts as given,
/// which the resource tooling builds as that document and reads by the document's reader, so that
/// the texts are checked as a document's are. `--decide` beside either is refused naming the held
/// flag, and `--held-reference` without `--held-reason`, which it needs, naming itself.
fn adopt_decision(flags: &FlagArgs) -> CliOutcome<Option<AdoptDecision>> {
    match (
        flags.optional_path(DECIDE_FLAG),
        flags.optional(HELD_REASON_FLAG),
        flags.optional(HELD_REFERENCE_FLAG),
    ) {
        (Some(_), Some(_), _) => Err(conflicting(
            HELD_REASON_FLAG,
            "standard-pack adopt takes --decide <decision.json> or --held-reason <text>, not both",
        )),
        (Some(_), None, Some(_)) => Err(conflicting(
            HELD_REFERENCE_FLAG,
            "standard-pack adopt takes --held-reference only with --held-reason <text>, and \
             neither with --decide <decision.json>",
        )),
        (None, None, Some(_)) => Err(conflicting(
            HELD_REFERENCE_FLAG,
            "standard-pack adopt takes --held-reference only with --held-reason <text>",
        )),
        (Some(document), None, None) => Ok(Some(AdoptDecision::Document(document))),
        (None, Some(rationale), reference) => Ok(Some(AdoptDecision::Held {
            rationale,
            reference,
        })),
        (None, None, None) => Ok(None),
    }
}

/// Workflow #463 S3-4: `flag` refused beside a flag it excludes, or without one it needs, as the
/// command line's other conflicts are: `validation_failed` naming it, `argument_problem`
/// `conflicting`.
fn conflicting(flag: &str, message: &str) -> CliError {
    let problem = ArgumentProblem::Conflicting.as_str();
    CliError::usage(message).with_details(json!({
        "argument": flag,
        "argument_problem": problem,
        "arguments": [{"argument": flag, "argument_problem": problem}],
    }))
}

/// The checkout's version control as adopt asks it: `git status --porcelain=v1 -z
/// --untracked-files=all -- <relative>` in `--repo`, which writes nothing (`GIT_OPTIONAL_LOCKS=0`,
/// no index refresh); the variables that would point git at another repository, work tree or
/// index are not passed on.
struct GitWorktree;

impl AdoptWorktree for GitWorktree {
    fn changes_under(&self, repo: &Path, relative: &str) -> Result<Vec<String>, Outcome> {
        let mut command = Command::new("git");
        command
            .current_dir(repo)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args([
                "-c",
                "core.quotepath=off",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--",
            ])
            .arg(relative)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in REDIRECTING_VARIABLES {
            command.env_remove(variable);
        }
        let output = command
            .output()
            .map_err(|error| Outcome::from_io_error(&error, IoOp::Spawn))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut end = stderr.len().min(RAW_TEXT_LIMIT);
            while !stderr.is_char_boundary(end) {
                end -= 1;
            }
            let mut cause = Outcome::new(ContractCode::ForeignOtherError)
                .raw_source(RawSource::Process)
                .raw_text(&stderr[..end]);
            if end < stderr.len() {
                cause = cause.raw_text_truncated(true);
            }
            if let Some(exit) = output.status.code() {
                cause = cause.exit_code(exit);
            }
            return Err(cause);
        }
        Ok(changed_paths(&output.stdout))
    }
}

/// The paths of a `git status --porcelain=v1 -z` answer: each entry `XY <path>`, a rename's or a
/// copy's source following it as a field of its own.
fn changed_paths(stdout: &[u8]) -> Vec<String> {
    let fields = stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(field) = fields.get(index) {
        index += if matches!(field.first(), Some(b'R' | b'C')) {
            2
        } else {
            1
        };
        paths.push(String::from_utf8_lossy(field.get(3..).unwrap_or_default()).into_owned());
    }
    paths
}

/// One `--instance <alias>=<game>/<server>`: the alias before the first `=`, the game and the
/// server around the `/` after it.
fn binding(value: &str) -> CliOutcome<(String, String, String)> {
    let parsed = value.split_once('=').and_then(|(alias, place)| {
        let (game, server) = place.split_once('/')?;
        (!alias.is_empty() && !game.is_empty() && !server.is_empty() && !server.contains('/'))
            .then(|| (alias.to_owned(), game.to_owned(), server.to_owned()))
    });
    parsed.ok_or_else(|| {
        CliError::usage(format!(
            "--instance {value}: expected <alias>=<game>/<server>"
        ))
    })
}

// Workflow #373 NAV-T (exec key 80), test row NT-3. Its game root is also the fixture of the
// consumer command lines of Workflow #434 LAB-2 fix 1 (`gate_434_lab2`).
#[cfg(test)]
#[path = "tests/gate_373_navt.rs"]
pub(crate) mod gate_373_navt;
