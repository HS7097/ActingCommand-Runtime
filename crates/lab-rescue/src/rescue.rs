// SPDX-License-Identifier: AGPL-3.0-only

//! The Lab's rescue and installation module (Workflow #434 LAB-3; `contracts/lab-two-entries.md`
//! §5 to §7; `contracts/lab-operations.json`, the rows of module `rescue`; register §1.287,
//! "Slice LAB-3").
//!
//! - **Words.** The table's rows; actinglab's CLI module and `serve --stdio` parse them
//!   (`apps/actinglab/src/cli_module/words.rs`) and hand the operation's name and parameters here.
//!   [`SERVED`] lists the rows this slice serves; a row whose source command is not built yet
//!   (`actingctl install` has no `config`, `instance`, `pack`, `new`, `prepare`, `convert` or
//!   `clean` before INST-3 to INST-6 and R-15c to R-15e) is not offered.
//! - **Where.** Every operation acts on the root of the program that runs it
//!   ([`actingcommand_lab_ops::locate`]); there is no `--root` and no `--state-root` (§7, §9.3), and
//!   a caller's `--root` or `--state-root` is refused as an unknown argument.
//! - **The initiator** (§4.3, §5). Every journal an operation writes records the initiator of the
//!   process ([`Face::Rescue`]); `unlock` passes it as actingd's `--actor`. The confirmation
//!   parameters are gone: no `--plan <id>` for an apply (`deploy`, `rollback` and `place` plan and
//!   apply in one call; `--plan` alone asks for the plan only), no `--accept-ledger-one-way` (a
//!   one-way update is journalled `one_way: true`), no `--known-pause` (every pause found on an
//!   instance is kept and listed), no `--ack-guard`; a confirmation parameter given anyway is
//!   `actingctl_usage_invalid` (`unknown`).
//! - **Only its own hold** (§5; LAB-3 fix 1). A hold of the module never takes a pause it did not
//!   set (`down --reason` refuses a reason an instance shows, `hold_not_owned`), keeps a global
//!   pause, and hands back only what its own pause step paused, each pause while it still shows
//!   the hold's reason, in any owner epoch (fix 2: the Runtime's restore numbers the pauses anew);
//!   `release-hold` of a reason no journal of the module recorded is `hold_not_owned`, and no
//!   answer's `next` names another party's hold. A refusal after the module paused something and
//!   before anything changed on disk hands its pauses back at once; an answer whose hold stays
//!   says `kept: true` and names the ways out that work today (`ways_out`). The running image of
//!   the module itself is renamed aside like `actingctl.exe`.
//! - **Guards report** (§6). A guard that fails or is not evaluated is noted
//!   (`install_caveat_noted`, `guard_failed`, `install_guards`) and the operation goes on; a start
//!   waits for memory up to `install.start.memory_wait_ms` and goes on (open point 4).
//! - **The composites** (§7): `deploy` (stage, plan, stop with hold, apply, start, alive,
//!   release), `config replace` and `config restore` (guards, stop with hold, the file swapped with
//!   the replaced one kept, check-config before anything changes, start, alive, release), `place`
//!   (`crate::install::place`) and `alive`. The download stays outside the Lab.
//! - **`instance add`** (Workflow #421 INST-5, first slice; register §1.300): an emulator
//!   instance composed into a candidate (`crate::install::instance_add`) and committed by `config
//!   replace`'s own path; after the start the new instance is paused with its own reason,
//!   `instance.added`, before the hold is handed back, and stays paused until a person resumes it.
//!   Workflow #421 INST-5b: the hold also sets one global pause of its own before the close
//!   (unless a global pause is in effect), which the Runtime restores at its start, so the new
//!   instance is never unpaused; the release hands it back after the new instance's pause.
//! - **The binding** (Workflow #434 LAB-6S2-B1; §2.4, §4.3). A request the module sends to a
//!   running Runtime keeps the CLI envelope `actingctl install`'s requests carry
//!   (`crate::install::runtime`), and its connection binds the Lab channel with the module's word,
//!   the one its journals record: `Lab-CLI`, `Lab-制包`, `Lab-救急`, or `Lab-MCP` for the Lab's MCP
//!   server, `serve --stdio --front mcp` and the detached run. None of the module's requests
//!   writes a record that carries a channel, and the receipt of each names the channel the map M of
//!   LAB-6S2-M gives, without the word: `cli`, as on 0.12.58, or `mcp` under `Lab-MCP`, where
//!   0.12.58 named `cli`. `actingctl install` kept the CLI channel without a word until
//!   LAB-6S2-B2, which binds it to the Lab channel with actingctl's word, `Lab-CLI`.
//! - **The MCP rows** (Workflow #434 LAB-5G5b; register §1.287, "Slice LAB-5G5b";
//!   `contracts/install-cli.md`, "The MCP entry"), read only, on the module's own root: `mcp plan`
//!   (the plan the entry's second generation asks, `entry_protocol` 2,
//!   `crate::install::mcp_entry::plan_g2`), `mcp entries` (the entries' status files of both
//!   generations, `crate::install::mcp_entry::entries`) and `mcp config` (the second
//!   generation's registration snippet, `crate::install::mcp_config`). The first generation's
//!   `actingctl mcp-entry plan|list` and `mcp-config` answer as before.
//! - **`emulators`** (Workflow #453 S10; `contracts/emulator-control.md`, "Offline instance
//!   discovery"), read only, offline: the live build's `actingd discover` of the module's own root,
//!   its report the answer.
//! - **`vision manifest`** and **`vision check`** (Workflow #453 S7;
//!   `contracts/vision-model-folders.md`, "The manifest"), read only, offline: the live build's
//!   `actingd vision manifest|check`, each ended at its bound; `vision check` also joins the models
//!   the packs of each game root need, from the Tools' `standard-pack check` of the root.

use crate::agent_channel::{self, Answer as Answered};
use crate::cli_line::{self, Exit};
use crate::codes::{
    ActingctlCode, ArgumentProblem, InstallCaveat, InstallLocation, InstallRefusal,
};
use crate::install::deploy::{self, HoldStart};
use crate::install::files;
use crate::install::hold::{self, HoldInputs, OwnedHold};
use crate::install::host::{Host, SystemHost};
use crate::install::journal::{self, Operation, StepKind};
use crate::install::lock::WriterLock;
use crate::install::manifest::Half;
use crate::install::plan::{self, Loaded};
use crate::install::programs::{self, Deadline};
use crate::install::runtime::Drained;
use crate::install::state::{KindLocation, Paths};
use crate::install::{self, Answer, Face, config_swap, instance_add, place};
use crate::rescue_reads;
use crate::runtime_start::{self, Mode};
use actingcommand_contract::SchedulingPauseReason;
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{
    Category, CauseRelation, IoOp, Outcome, OutcomeEnvelope, RawSource,
};
use actingcommand_lab_ops::core_reads::{
    Handed, Params, Part, ROOT_FLAG, STATE_ROOT_FLAG, Written,
};
use actingcommand_lab_ops::initiator::{self, Initiator};
use actingcommand_lab_ops::locate::Location;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The developer text of a refused command line of the rescue module (Workflow #434 CLI-HELP: it
/// names `actinglab help`, where it named the table's contract file).
const USAGE: &str = "usage: actinglab [--json] <rescue operation> [<positionals>] \
     [--<option> <value>] [--<switch>]; actinglab help lists the rows of module \
     rescue; no --root, no --state-root: the root is the one actinglab runs from";

/// The rows of module `rescue` this slice serves, by the table's name (Workflow #421 INST-5: and
/// `instance add`; Workflow #434 LAB-5G5b: and the three MCP rows; Workflow #453 S10: and
/// `emulators`; Workflow #453 S7: and `vision manifest` and `vision check`).
pub const SERVED: [&str; 36] = [
    "up",
    "restart",
    "down",
    "alive",
    "deploy",
    "rollback",
    "undo",
    "op status",
    "op resume",
    "leftovers",
    "guards",
    "release-hold",
    "release verify",
    "release stage",
    "adopt",
    "config replace",
    "config restore",
    "instance add",
    "place",
    "unlock",
    "check-config",
    "ledger verify",
    "emulators",
    "vision manifest",
    "vision check",
    "watchdog install",
    "watchdog uninstall",
    "agent add",
    "agent remove",
    "agent list",
    "agent claude-hook",
    "agent claude-register",
    "agent notice",
    "mcp config",
    "mcp entries",
    "mcp plan",
];

/// Whether this slice serves the rescue row `name`.
pub fn serves(name: &str) -> bool {
    SERVED.contains(&name)
}

/// The parameters a caller may never give the rescue module: the root and the state root (§7,
/// §9.3) and the confirmations (§5, §6). Each is refused as an unknown argument.
pub(crate) const REFUSED_PARAMETERS: [&str; 7] = [
    "root",
    "state_root",
    "accept_ledger_one_way",
    "ui_confirmed",
    "via",
    "known_pause",
    "ack_guard",
];

/// Where and as whom an operation runs, and what it reads the machine through.
pub(crate) struct Context<'a> {
    pub(crate) host: &'a dyn Host,
    pub(crate) location: &'a Location,
    pub(crate) initiator: Initiator,
}

impl Context<'_> {
    fn face(&self) -> Face {
        Face::Rescue(self.initiator)
    }

    /// The root as a command word of the installer.
    fn root_word(&self) -> String {
        self.location.root.to_string_lossy().into_owned()
    }

    fn paths(&self, operation: InstallLocation) -> Result<Paths, Outcome> {
        Paths::resolve(&self.location.root).map_err(|cause| {
            install::refusal(operation, InstallRefusal::StateInvalid)
                .path(&self.location.root)
                .caused_by(cause)
        })
    }

    /// An `actingctl install` command (the words after `install`) run with the module's face.
    fn install(&self, words: &[String]) -> Answer {
        // Workflow #434 LAB-5B1b: test builds keep each command line handed on ([`installed`]).
        #[cfg(test)]
        installed::record(words);
        let words = words.iter().map(String::as_str).collect::<Vec<_>>();
        install::execute_as(&words, self.host, self.face())
    }
}

/// Runs the rescue row `name` with `params` (the members the table's parser gives) for the root
/// `location`, as the initiator `initiator`, on the machine.
pub fn answer(
    name: &str,
    params: &Map<String, Value>,
    location: &Location,
    initiator: Initiator,
) -> Answered {
    answer_with(
        &Context {
            host: &SystemHost::of(Some(initiator)),
            location,
            initiator,
        },
        name,
        params,
    )
}

/// [`answer`] on `context`'s machine.
pub(crate) fn answer_with(
    context: &Context<'_>,
    name: &str,
    params: &Map<String, Value>,
) -> Answered {
    if let Some(refused) = refused_parameter(params) {
        return Answered::Line(cli_line::failure(&refused, Some(USAGE)));
    }
    // Workflow #434 LAB-5B1b: each row through its constant (`crate::rescue_reads`), which its code
    // reads the parameters through and builds a command line from.
    if let Some(handed) = rescue_reads::agent(name) {
        return agent(context, handed, params);
    }
    if let Some(handed) = rescue_reads::installer(name) {
        return Answered::Line(match name {
            "up" => up(context, handed, params),
            "down" => down(context, handed, params),
            _ => installer(context, handed, params),
        });
    }
    let Some(params) = rescue_reads::read_here(name, params) else {
        return Answered::Line(unknown_row(name));
    };
    Answered::Line(match name {
        "restart" => restart(context),
        "alive" => alive(context, params),
        "deploy" => deploy(context, params),
        "rollback" => rollback(context, params),
        "config replace" => config_swap(context, params, false),
        "config restore" => config_swap(context, params, true),
        "instance add" => instance_add(context, params),
        "place" => place(context, params),
        "unlock" => unlock(context),
        "check-config" => check_config(context, params),
        "ledger verify" => ledger_verify(context),
        "emulators" => emulators(context),
        "vision manifest" => vision_manifest(context, params),
        "vision check" => vision_check(context),
        "watchdog install" => watchdog(context, crate::watchdog::Asked::Install),
        "watchdog uninstall" => watchdog(context, crate::watchdog::Asked::Uninstall),
        "mcp plan" => mcp_plan(context, params),
        "mcp entries" => mcp_entries(context, params),
        "mcp config" => mcp_config(context, params),
        _ => unknown_row(name),
    })
}

/// The refusal of a name that is no row the module serves.
fn unknown_row(name: &str) -> Exit {
    cli_line::failure(
        &cli_line::usage_invalid(Some(name), ArgumentProblem::Unknown),
        Some(USAGE),
    )
}

/// The first parameter the module never takes, as its refusal (`actingctl_usage_invalid`,
/// `unknown`, the flag as the caller wrote it).
fn refused_parameter(params: &Map<String, Value>) -> Option<Outcome> {
    params
        .keys()
        .find(|name| REFUSED_PARAMETERS.contains(&name.as_str()))
        .map(|name| {
            cli_line::usage_invalid(
                Some(format!("--{}", name.replace('_', "-")).as_str()),
                ArgumentProblem::Unknown,
            )
        })
}

// Workflow #434 LAB-5B1b: each reads by name through the row's constant ([`Params`]).

fn text<'a>(params: Params<'a>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn switch(params: Params<'_>, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// The row's parameters but those in `declared` (which the caller maps itself), as the source
/// command takes them: `--name value`, or `--name` for a switch that is set. Workflow #434
/// LAB-5A: the row declares them all (before it, they were the further flags of a row with
/// `params_more`); a switch given `false`, which only `serve --stdio` can give, is not written.
/// LAB-5A fix 1: and the one flag the command line still hands on undeclared, `op status
/// --operation <op>` (actinglab's `words::KEPT_FLAGS`), written as before. Workflow #434
/// LAB-5B1b: written where a handed row's constant has `Part::Further`, `declared` the parameters
/// its other parts placed.
fn further_flags(params: &Map<String, Value>, declared: &[&str]) -> Vec<String> {
    let mut words = Vec::new();
    for (name, value) in params {
        if declared.contains(&name.as_str()) {
            continue;
        }
        let flag = format!("--{}", name.replace('_', "-"));
        let values = match value {
            Value::Array(items) => items.clone(),
            other => vec![other.clone()],
        };
        for value in values {
            match value {
                Value::Bool(true) => words.push(flag.clone()),
                Value::Bool(false) => {}
                Value::String(text) => {
                    words.push(flag.clone());
                    words.push(text);
                }
                other => {
                    words.push(flag.clone());
                    words.push(other.to_string());
                }
            }
        }
    }
    words
}

/// An installer row: the command line its constant gives ([`installer_line`]), run with the
/// module's face.
fn installer(context: &Context<'_>, handed: &'static Handed, params: &Map<String, Value>) -> Exit {
    let words = installer_line(handed, params, &context.root_word());
    line(context, context.install(&words))
}

/// Workflow #434 LAB-5B1b: the command line the installer row `handed` gives for `params`: its
/// command's words, then each part in its place (`--root` and the root; a parameter under its flag
/// when it is given as text), then, with `Part::Further`, every member no part placed, in the
/// order given, under the flag of its name ([`further_flags`]: the row's `Written::Each`
/// parameters and the installer's own `--operation` that `op status`'s command line hands on).
/// Before the slice each arm gave its command's words, whether it took the root and its
/// parameters' flags itself, in the same order and forms.
fn installer_line(handed: &'static Handed, params: &Map<String, Value>, root: &str) -> Vec<String> {
    let read = rescue_reads::handing(handed, params);
    let mut words = handed
        .command
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    let mut placed = Vec::new();
    for part in handed.parts {
        match *part {
            Part::Root => {
                words.push(ROOT_FLAG.to_owned());
                words.push(root.to_owned());
            }
            Part::Param(name, Written::Flag(flag)) => {
                placed.push(name);
                if let Some(value) = text(read, name) {
                    words.push(flag.to_owned());
                    words.push(value.to_owned());
                }
            }
            Part::Further => words.extend(further_flags(params, &placed)),
            // Written with `Part::Further`, in the order given; no installer row has another part.
            _ => {}
        }
    }
    words
}

/// The line of an installer answer, in the module's words: `next` names the module's operations,
/// and a usage refusal the module's usage.
fn line(context: &Context<'_>, answer: Answer) -> Exit {
    install::print(module_answer(context, answer))
}

pub(crate) fn module_answer(context: &Context<'_>, mut answer: Answer) -> Answer {
    let notes = translate_next(context, &mut answer.members);
    answer.outcome = plan::with_notes(answer.outcome, &notes);
    if answer.text.is_some() {
        answer.text = Some(USAGE);
    }
    answer
}

/// The `next` forms of an answer in the module's words: `install start` is `up`, `install
/// release-hold --reason <r>` is `release-hold <r>`, `install undo --operation <id>` is `undo
/// <id>`, `install status` is `op status`. Workflow #434 LAB-3 fix 1: a `release-hold` of a hold
/// the module does not own (an `actingctl install` operation's, which `op status` or `op resume`
/// may answer) is never suggested: the module would refuse it. Fix 2: beside the module's own
/// `release-hold <r>` of a hold it owns, the ways out that work today ([`ways_out`]). Fix 3: a hold
/// whose ownership cannot be read (a reason that does not parse, a root or an operations folder
/// that cannot be read) is not suggested either, and is not dropped silently: the notes returned,
/// one per such hold, say so ([`ownership_unconfirmed`]); the caller links them to its answer.
fn translate_next(context: &Context<'_>, members: &mut Map<String, Value>) -> Vec<Outcome> {
    let mut notes = Vec::new();
    let Some(Value::Object(next)) = members.get_mut("next") else {
        return notes;
    };
    for form in next.values_mut() {
        if let Some(words) = form.as_array()
            && let Some(module) = module_form(words)
        {
            *form = module;
        }
    }
    let mut owned = Vec::new();
    next.retain(|_, form| match form.as_array().map(Vec::as_slice) {
        Some([Value::String(verb), Value::String(given)]) if verb == "release-hold" => {
            let reason = match SchedulingPauseReason::new(given.as_str()) {
                Ok(reason) => reason,
                Err(error) => {
                    notes.push(ownership_unconfirmed(None, Outcome::new(error.code())));
                    return false;
                }
            };
            match own_hold(context, &reason) {
                Ok(Some((paths, hold))) => {
                    owned.push((paths, reason, hold));
                    true
                }
                Ok(None) => false,
                Err(cause) => {
                    notes.push(ownership_unconfirmed(Some(&reason), cause));
                    false
                }
            }
        }
        _ => true,
    });
    for (paths, reason, hold) in &owned {
        ways_out(context, next, paths, reason, hold);
    }
    if next.is_empty() {
        members.remove("next");
    }
    notes
}

/// Workflow #434 LAB-3 fix 1: the hold `reason` if it is the module's own in the root: one of its
/// journals recorded it in its pause step (`install::deploy::owned_hold`), with the root's paths
/// and what the hold paused; `None` when no journal of the module recorded it. Fix 3: a root or an
/// operations folder that cannot be read is the failure, not "not owned".
fn own_hold(
    context: &Context<'_>,
    reason: &SchedulingPauseReason,
) -> Result<Option<(Paths, OwnedHold)>, Outcome> {
    let paths = Paths::resolve(&context.location.root)?;
    let owned = deploy::owned_hold(context.host, &paths, reason, None)?;
    Ok(owned.map(|owned| (paths, owned)))
}

/// Workflow #434 LAB-3 fix 3: the note of a `release-hold` left out of `next` because whether the
/// hold is the module's own could not be read: `install_caveat_noted` of `install_release_hold`
/// with `install_caveat` `hold_ownership_unconfirmed`, the hold's reason as `hold_reason` when it
/// parses, and the failure that kept it from being read as its cause.
fn ownership_unconfirmed(reason: Option<&SchedulingPauseReason>, cause: Outcome) -> Outcome {
    let note = install::caveat(
        InstallLocation::ReleaseHold,
        InstallCaveat::HoldOwnershipUnconfirmed,
    );
    let note = match reason {
        Some(reason) => note.hold_reason(reason.as_str().to_owned()),
        None => note,
    };
    note.caused_by(cause)
}

/// Workflow #434 LAB-3 fix 2 (install-cli.md, "The Lab's rescue module", "Ways out of a kept
/// hold"): the ways out of the module's own hold `reason` that work today without the module,
/// added to `next` beside its `release-hold <r>`. Until LAB-7 deletes actingctl:
/// `actingctl_release_hold`, `actingctl install release-hold --root <root> --reason <r>` (it
/// resumes the instances paused with exactly that reason, the module's own); and
/// `actingctl_resume`, for each instance the hold's pause step paused that the running Runtime
/// shows paused with the reason now, the core `resume` as actingctl runs it (`actingctl resume
/// --state-root <s> --instance <alias>`, on the CLI channel; since LAB-6S1b the Runtime also takes
/// the Lab's own `resume <alias>` on the Lab channel, which this list does not name). No form
/// names another reason or an
/// instance whose pause shows one; without a status read there is no `actingctl_resume`. Workflow
/// #421 INST-5b: `actingctl_resume_global`, the core global resume as actingctl runs it
/// (`actingctl resume --state-root <s>`), when the running Runtime shows the hold's own global
/// pause ([`hold::own_global`]); `actingctl install release-hold` leaves it.
fn ways_out(
    context: &Context<'_>,
    next: &mut Map<String, Value>,
    paths: &Paths,
    reason: &SchedulingPauseReason,
    owned: &OwnedHold,
) {
    next.insert(
        "actingctl_release_hold".to_owned(),
        json!([
            "actingctl",
            "install",
            "release-hold",
            "--root",
            paths.plain,
            "--reason",
            reason
        ]),
    );
    let Ok(Some(seen)) = context.host.runtime().status(&context.location.state_root) else {
        return;
    };
    let state_root = files::plain(&context.location.state_root);
    let resume = seen
        .instances
        .iter()
        .filter(|instance| {
            owned.paused.contains(&instance.alias)
                && instance
                    .pause
                    .as_ref()
                    .is_some_and(|pause| &pause.reason == reason)
        })
        .map(|instance| {
            json!([
                "actingctl",
                "resume",
                "--state-root",
                state_root,
                "--instance",
                instance.alias
            ])
        })
        .collect::<Vec<_>>();
    if !resume.is_empty() {
        next.insert("actingctl_resume".to_owned(), Value::Array(resume));
    }
    if hold::own_global(&seen, reason, owned).is_some() {
        next.insert(
            "actingctl_resume_global".to_owned(),
            json!(["actingctl", "resume", "--state-root", state_root]),
        );
    }
}

fn module_form(words: &[Value]) -> Option<Value> {
    let words = words
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    let after = |flag: &str| {
        words
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1])
    };
    match words.get(..2)? {
        ["install", "start"] => Some(json!(["up"])),
        ["install", "release-hold"] => Some(json!(["release-hold", after("--reason")?])),
        ["install", "undo"] => Some(json!(["undo", after("--operation")?])),
        ["install", "status"] => Some(json!(["op", "status"])),
        _ => None,
    }
}

/// A hold reason of the module's own: `lab-<word>-<unix ms>`, one per operation, so that the
/// release hands back only that operation's hold (`word` names the operation: `deploy`, `down`).
fn hold_reason(context: &Context<'_>, word: &str) -> Result<SchedulingPauseReason, Outcome> {
    SchedulingPauseReason::new(format!("lab-{word}-{}", context.host.now_unix_ms()))
        .map_err(|error| Outcome::new(error.code()))
}

/// `up`: on a single-location root the installer's start (`install start`, journalled, its
/// post-start checks; its constant `handed` hands no parameter on); on an A/B root the start of
/// its selection (`actingctl start`).
fn up(context: &Context<'_>, handed: &'static Handed, params: &Map<String, Value>) -> Exit {
    if context.location.installation.is_some() {
        return runtime_start::run_located(
            Mode::Start,
            &context.location.root,
            None,
            Some(context.initiator),
        );
    }
    installer(context, handed, params)
}

/// `restart`: `actingctl restart` for the root, with its configuration on a single-location root.
fn restart(context: &Context<'_>) -> Exit {
    let config = context
        .location
        .installation
        .is_none()
        .then_some(context.location.config.as_path());
    runtime_start::run_located(
        Mode::Restart,
        &context.location.root,
        config,
        Some(context.initiator),
    )
}

/// `down [--reason <r>]`: the formal stop with its hold (`install stop`, its constant `handed`);
/// without `--reason` the hold's reason is the module's own.
fn down(context: &Context<'_>, handed: &'static Handed, params: &Map<String, Value>) -> Exit {
    let mut given = params.clone();
    if text(rescue_reads::handing(handed, params), "reason").is_none() {
        match hold_reason(context, "down") {
            Ok(reason) => {
                given.insert(
                    "reason".to_owned(),
                    Value::String(reason.as_str().to_owned()),
                );
            }
            Err(outcome) => return cli_line::failure(&outcome, None),
        }
    }
    installer(context, handed, &given)
}

/// The probe period of `alive --wait` (the readiness probe's, fixed by the contract).
const ALIVE_POLL_MS: u64 = 500;

/// What `alive` read: whether the watchdog judges the Runtime alive, its decision, the report's
/// members and how long it waited.
struct Alive {
    alive: bool,
    outcome: Outcome,
    members: Map<String, Value>,
}

/// The watchdog's `status` of the root, read again every 500 ms until it decides
/// `watchdog_alive` or `wait_ms` has passed (once without a wait).
fn read_alive(context: &Context<'_>, wait_ms: Option<u64>) -> Alive {
    let host = context.host;
    let since = host.now_unix_ms();
    loop {
        let answered = host.watchdog(crate::watchdog::Asked::Status, &context.location.root);
        let alive = answered.outcome.is(ActingctlCode::WatchdogAlive);
        let waited = host.now_unix_ms().saturating_sub(since);
        if alive || wait_ms.is_none_or(|wait| waited >= wait) {
            let mut members = match answered.line {
                Some(Value::Object(members)) => members,
                _ => Map::new(),
            };
            members.remove("outcome");
            members.insert("alive".to_owned(), Value::Bool(alive));
            members.insert("waited_ms".to_owned(), json!(waited));
            return Alive {
                alive,
                outcome: answered.outcome,
                members,
            };
        }
        host.pause_ms(ALIVE_POLL_MS);
    }
}

/// `alive [--wait <duration>]`: the watchdog's status and whether the Runtime is alive. Without
/// `--wait` the report's own decision is the top; with it, a Runtime not alive when the wait
/// ends refuses `runtime_not_running`, the decision as its link.
fn alive(context: &Context<'_>, params: Params<'_>) -> Exit {
    let wait_ms = match text(params, "wait").map(duration_ms) {
        Some(Some(ms)) => Some(ms),
        Some(None) => {
            return cli_line::failure(
                &cli_line::usage_invalid(Some("--wait"), ArgumentProblem::WrongType),
                Some(USAGE),
            );
        }
        None => None,
    };
    let read = read_alive(context, wait_ms);
    if wait_ms.is_some() && !read.alive {
        let refusal = install::refusal(InstallLocation::Alive, InstallRefusal::RuntimeNotRunning)
            .path(&context.location.root)
            .link(CauseRelation::Related, read.outcome);
        return cli_line::receipt(read.members, &refusal, true);
    }
    cli_line::finish(Some(read.members), &read.outcome, false, None)
}

/// A duration as the table writes it (a whole number with `ms`, `s`, `m` or `h`), in ms.
fn duration_ms(text: &str) -> Option<u64> {
    let split = text.find(|character: char| !character.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number = number.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(number),
        "s" => number.checked_mul(1_000),
        "m" => number.checked_mul(60_000),
        "h" => number.checked_mul(3_600_000),
        _ => None,
    }
}

/// One step of a composite as its answer lists it: its name, its outcome and its members.
fn step(name: &str, outcome: &Outcome, members: Value) -> Value {
    json!({
        "step": name,
        "outcome": cli_line::outcome_value(outcome, cli_line::detail().unwrap_or(actingcommand_contract::outcome::Detail::Full)),
        "result": members,
    })
}

/// A finished step of a composite from an installer answer, in the module's words (fix 3: its
/// outcome carries [`module_answer`]'s notes, its members the module's `next`).
fn answer_step(context: &Context<'_>, name: &str, answer: &Answer) -> Value {
    let translated = module_answer(context, answer.clone());
    step(name, &translated.outcome, Value::Object(translated.members))
}

/// A composite's answer: its steps so far and the top outcome; a step that refused or failed is
/// the top, its members (what stays, `next`) beside the steps.
fn composite_exit(
    context: &Context<'_>,
    steps: Vec<Value>,
    top: Outcome,
    mut members: Map<String, Value>,
) -> Exit {
    members.insert("steps".to_owned(), Value::Array(steps));
    let notes = translate_next(context, &mut members);
    let top = plan::with_notes(top, &notes);
    if top.category() > Category::Warning {
        return cli_line::receipt(members, &top, true);
    }
    cli_line::success(members, &top)
}

/// Whether an installer answer ended the composite: it refused or failed.
fn stops(answer: &Answer) -> bool {
    answer.outcome.category() > Category::Warning
}

/// A failed step's answer as the composite's end.
fn stopped(context: &Context<'_>, steps: Vec<Value>, answer: Answer) -> Exit {
    let answer = module_answer(context, answer);
    composite_exit(context, steps, answer.outcome, answer.members)
}

/// The kept build `target` names: a commit (8 hex digits or more, its kept build's prefix) or a
/// tag a kept build records; `None` unless exactly one kept build matches.
fn kept_commit(loaded: &Loaded, target: &str) -> Option<String> {
    let lower = target.to_ascii_lowercase();
    let hex = lower.len() >= 8 && lower.bytes().all(|byte| byte.is_ascii_hexdigit());
    let found = loaded
        .state
        .builds
        .iter()
        .filter(|build| {
            build.tag.as_deref() == Some(target) || (hex && build.commit.starts_with(&lower))
        })
        .map(|build| build.commit.clone())
        .collect::<BTreeSet<_>>();
    (found.len() == 1)
        .then(|| found.into_iter().next())
        .flatten()
}

/// The Runtime of the root: `Some` while one answers. A status that cannot be read is
/// `processes_unreadable` of `operation`.
fn running(
    context: &Context<'_>,
    loaded: &Loaded,
    operation: InstallLocation,
) -> Result<bool, Outcome> {
    context
        .host
        .runtime()
        .status(&loaded.state_root)
        .map(|seen| seen.is_some())
        .map_err(|cause| {
            install::refusal(operation, InstallRefusal::ProcessesUnreadable)
                .joined(CauseRelation::CausedBy, cause)
        })
}

/// `--detach`: the module's command run again without it through the watchdog's manual-run
/// method, with the same initiator ([`detached_environment`]), its output appended to
/// `install\detached-<ms>.log`; the answer is at once (`op status` follows the operation).
fn detach(context: &Context<'_>, operation: InstallLocation, words: Vec<String>) -> Exit {
    let run = || -> Result<Exit, Outcome> {
        let paths = context.paths(operation)?;
        journal::refuse_unfinished(operation, &paths)?;
        plan::load(&paths, operation)?;
        files::create_dir_all(&paths.install())?;
        let log = paths
            .install()
            .join(format!("detached-{}.log", context.host.now_unix_ms()));
        let mut arguments = vec!["--json".to_owned()];
        arguments.extend(words);
        let environment = detached_environment(context.initiator);
        let pid = context
            .host
            .detach_rescue(&arguments, &environment, &paths.root, &log)?;
        let mut members = Map::new();
        members.insert("detached".to_owned(), Value::Bool(true));
        members.insert("log".to_owned(), Value::String(files::plain(&log)));
        members.insert("next".to_owned(), json!({"status": ["op", "status"]}));
        Ok(cli_line::success(
            members,
            &install::done(operation)
                .installation_changed(false)
                .pid(pid)
                .log(&log),
        ))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// The variables a detached run of the module is started with, so that it runs as `word`
/// (contracts/lab-two-entries.md §4.2): none for the default `Lab-CLI`; the word in
/// `ACTINGCOMMAND_INITIATOR` for any other; and, Workflow #434 LAB-6S0, for `Lab-MCP` beside it
/// the marker `ACTINGCOMMAND_DETACHED_RUN=rescue`, without which a Lab process refuses `Lab-MCP`
/// in the variable. This start is the one place that sets the marker.
fn detached_environment(word: Initiator) -> Vec<(&'static str, &'static str)> {
    let mut environment = Vec::new();
    if word != Initiator::LabCli {
        environment.push((initiator::ENVIRONMENT_VARIABLE, word.as_str()));
    }
    if word == Initiator::LabMcp {
        environment.push((
            initiator::DETACHED_RUN_VARIABLE,
            initiator::DETACHED_RUN_MARK,
        ));
    }
    environment
}

/// The module's words of `name` with `params`, without `--detach`: what a detached run runs.
fn words_without_detach(name: &str, params: Params<'_>, positionals: &[&str]) -> Vec<String> {
    let mut words = name.split(' ').map(str::to_owned).collect::<Vec<_>>();
    for positional in positionals {
        if let Some(value) = text(params, positional) {
            words.push(value.to_owned());
        }
    }
    for (param, value) in params.map() {
        if param == "detach" || positionals.contains(&param.as_str()) {
            continue;
        }
        let flag = format!("--{}", param.replace('_', "-"));
        match value {
            Value::Bool(true) => words.push(flag),
            Value::String(text) => {
                words.push(flag);
                words.push(text.clone());
            }
            _ => {}
        }
    }
    words
}

/// The steps of a finished composite update or rollback, from its journal: `stop` (the hold and
/// the close), `apply` (the file steps), `start` (the start and its checks).
fn composite_steps(
    context: &Context<'_>,
    paths: &Paths,
    operation_id: &str,
    applied: &Answer,
) -> Vec<Value> {
    let Ok(operation) = Operation::load(context.host, paths, operation_id, InstallLocation::Status)
    else {
        return Vec::new();
    };
    let journal = operation.journal();
    let evidence = |kind: StepKind| {
        journal
            .steps
            .iter()
            .filter(|step| step.kind == kind && !deploy::of_rollback(&step.intent))
            .map(|step| json!({"n": step.n, "state": step.state, "evidence": step.evidence}))
            .collect::<Vec<_>>()
    };
    let file_steps = journal
        .steps
        .iter()
        .filter(|step| {
            matches!(
                step.kind,
                StepKind::LedgerVerify
                    | StepKind::StageFile
                    | StepKind::SwapFile
                    | StepKind::VerifyTree
                    | StepKind::RecordState
            )
        })
        .count();
    let member = |name: &str| applied.members.get(name).cloned().unwrap_or(Value::Null);
    let done = install::done(journal.kind.location());
    vec![
        step(
            "stop",
            &done,
            json!({"pause": evidence(StepKind::Pause), "close": evidence(StepKind::Close)}),
        ),
        step(
            "apply",
            &done,
            json!({
                "steps": file_steps,
                "from_commit": member("from_commit"),
                "to_commit": member("to_commit"),
                "replaced": member("replaced"),
                "added": member("added"),
                "verified": member("verified"),
            }),
        ),
        step(
            "start",
            &done,
            json!({"start": member("start"), "post_check": member("post_check")}),
        ),
    ]
}

/// The hold a composite's answer says it kept: its reason.
fn kept_hold(answer: &Answer) -> Option<String> {
    let hold = answer.members.get("hold")?;
    (hold.get("kept").and_then(Value::as_bool) == Some(true))
        .then(|| {
            hold.get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .flatten()
}

/// What follows a composite's apply while a Runtime ran: `alive` (the watchdog decides the new
/// Runtime alive within `install.start.ready_timeout_ms`) and, when the operation kept its hold,
/// `release` (`install release-hold` of exactly that reason). A Runtime not alive keeps the hold
/// and ends the composite `runtime_not_running`.
fn alive_and_release(
    context: &Context<'_>,
    loaded: &Loaded,
    operation: InstallLocation,
    applied: &Answer,
    mut steps: Vec<Value>,
) -> Result<Vec<Value>, Exit> {
    let read = read_alive(context, Some(loaded.settings.start_ready_timeout_ms));
    steps.push(step(
        "alive",
        &read.outcome,
        Value::Object(read.members.clone()),
    ));
    let reason = kept_hold(applied);
    if !read.alive {
        let refusal = install::refusal(operation, InstallRefusal::RuntimeNotRunning)
            .path(&context.location.root)
            .link(CauseRelation::Related, read.outcome);
        let mut members = Map::new();
        if let Some(reason) = &reason {
            // LAB-3 fix 2: the hold stays, and the answer says so (`kept: true`).
            if let Some(hold) = applied.members.get("hold") {
                members.insert("hold".to_owned(), hold.clone());
            }
            members.insert(
                "next".to_owned(),
                json!({"release_hold": ["release-hold", reason], "alive": ["alive"]}),
            );
        }
        return Err(composite_exit(context, steps, refusal, members));
    }
    if let Some(reason) = reason {
        let released = context.install(&[
            "release-hold".to_owned(),
            "--root".to_owned(),
            context.root_word(),
            "--reason".to_owned(),
            reason,
        ]);
        if stops(&released) {
            return Err(stopped(context, steps, released));
        }
        steps.push(answer_step(context, "release", &released));
    }
    Ok(steps)
}

/// `deploy <tag|commit> [--dir <dir>] [--plan] [--detach]` (§7): stage (from `--dir`, its
/// `release-facts.json` when present; without `--dir` a kept build), plan (a preview while a
/// Runtime runs), then with a Runtime running the composite update (stop with hold, apply, start
/// and its checks) followed by `alive` and the release, without one the update alone. `--plan`
/// answers the plan only; `--detach` runs it detached. The hold's reason is always the module's
/// own `lab-deploy-<ms>` (LAB-3 fix 1: the table's `--reason` is gone, as §7's form has none).
fn deploy(context: &Context<'_>, params: Params<'_>) -> Exit {
    const DEPLOY: InstallLocation = InstallLocation::Deploy;
    let target = text(params, "target").unwrap_or_default().to_owned();
    let plan_only = switch(params, "plan");
    if switch(params, "detach") && !plan_only {
        return detach(
            context,
            DEPLOY,
            words_without_detach("deploy", params, &["target"]),
        );
    }
    let mut steps = Vec::new();
    let begin = || -> Result<(Paths, Loaded), Outcome> {
        let paths = context.paths(DEPLOY)?;
        let loaded = plan::load(&paths, DEPLOY)?;
        Ok((paths, loaded))
    };
    let (paths, loaded) = match begin() {
        Ok(found) => found,
        Err(outcome) => return cli_line::failure(&outcome, None),
    };
    // 1. stage.
    let commit = match text(params, "dir") {
        Some(dir) if !plan_only => {
            let mut words = vec![
                "release".to_owned(),
                "stage".to_owned(),
                "--root".to_owned(),
                context.root_word(),
                "--dir".to_owned(),
                dir.to_owned(),
            ];
            let facts = Path::new(dir).join(RELEASE_FACTS);
            if facts.is_file() {
                words.push("--facts".to_owned());
                words.push(facts.to_string_lossy().into_owned());
            }
            let lower = target.to_ascii_lowercase();
            if files::is_commit(&lower)
                || (lower.len() >= 8 && lower.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                words.push("--expect-commit".to_owned());
                words.push(lower);
            } else {
                words.push("--expect-tag".to_owned());
                words.push(target.clone());
            }
            let staged = context.install(&words);
            if stops(&staged) {
                return stopped(context, steps, staged);
            }
            let commit = staged
                .outcome
                .value("commit_sha")
                .and_then(Value::as_str)
                .map(str::to_owned);
            steps.push(step(
                "stage",
                &staged.outcome,
                Value::Object(staged.members.clone()),
            ));
            commit
        }
        _ => {
            let commit = kept_commit(&loaded, &target);
            steps.push(step(
                "stage",
                &install::done(InstallLocation::ReleaseStage).installation_changed(false),
                json!({"kept": commit}),
            ));
            commit
        }
    };
    let Some(commit) = commit else {
        let refusal = install::refusal(DEPLOY, InstallRefusal::BuildNotKept).path(paths.builds());
        return composite_exit(context, steps, refusal, Map::new());
    };
    // 2. plan.
    let live = match running(context, &loaded, DEPLOY) {
        Ok(live) => live,
        Err(outcome) => return composite_exit(context, steps, outcome, Map::new()),
    };
    let mut plan_words = vec![
        "update".to_owned(),
        "plan".to_owned(),
        "--root".to_owned(),
        context.root_word(),
        "--build".to_owned(),
        commit.clone(),
    ];
    if live {
        plan_words.push("--preview".to_owned());
    }
    let planned = context.install(&plan_words);
    if stops(&planned) {
        return stopped(context, steps, planned);
    }
    let plan_id = planned
        .members
        .get("plan_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    steps.push(step(
        "plan",
        &planned.outcome,
        Value::Object(planned.members.clone()),
    ));
    if plan_only {
        let done = install::done(DEPLOY)
            .commit_sha(commit)
            .plan_id(plan_id)
            .installation_changed(false);
        return composite_exit(context, steps, done, Map::new());
    }
    applied_composite(
        context, &paths, &loaded, DEPLOY, "--build", &commit, &plan_id, live, steps,
    )
}

/// The release-facts file `deploy` reads from its `--dir` when present (the download writes it
/// there, outside the Lab).
const RELEASE_FACTS: &str = "release-facts.json";

/// The apply of `deploy` or `rollback` (`kind_flag` `--build` or `--to`) with the plan `plan_id`:
/// with a Runtime running the composite (stop with hold, apply, start), then `alive` and the
/// release; without one the plain apply. The steps and the top outcome.
#[allow(clippy::too_many_arguments)]
fn applied_composite(
    context: &Context<'_>,
    paths: &Paths,
    loaded: &Loaded,
    operation: InstallLocation,
    kind_flag: &str,
    commit: &str,
    plan_id: &str,
    live: bool,
    mut steps: Vec<Value>,
) -> Exit {
    let verb = if kind_flag == "--to" {
        "rollback"
    } else {
        "update"
    };
    let mut words = vec![
        verb.to_owned(),
        "apply".to_owned(),
        "--root".to_owned(),
        context.root_word(),
        kind_flag.to_owned(),
        commit.to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
    ];
    if live {
        let reason = match hold_reason(
            context,
            if kind_flag == "--to" {
                "rollback"
            } else {
                "deploy"
            },
        ) {
            Ok(reason) => reason,
            Err(outcome) => return composite_exit(context, steps, outcome, Map::new()),
        };
        words.extend([
            "--stop".to_owned(),
            "--hold".to_owned(),
            reason.as_str().to_owned(),
            "--start".to_owned(),
        ]);
    }
    let applied = context.install(&words);
    if stops(&applied) {
        let operation_id = applied
            .outcome
            .value("install_operation_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(operation_id) = operation_id {
            steps.extend(composite_steps(context, paths, &operation_id, &applied));
        }
        return stopped(context, steps, applied);
    }
    let operation_id = applied
        .members
        .get("operation_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    if live {
        steps.extend(composite_steps(context, paths, &operation_id, &applied));
        steps = match alive_and_release(context, loaded, operation, &applied, steps) {
            Ok(steps) => steps,
            Err(exit) => return exit,
        };
    } else {
        steps.push(answer_step(context, "apply", &applied));
    }
    let done = install::done(operation)
        .commit_sha(commit.to_owned())
        .install_operation_id(operation_id)
        .installation_changed(true)
        .link(CauseRelation::Related, applied.outcome);
    let mut members = Map::new();
    members.insert("runtime_running".to_owned(), Value::Bool(live));
    composite_exit(context, steps, done, members)
}

/// `rollback [--to <tag|commit>] [--plan] [--detach]`: to the build the live one replaced unless
/// `--to` names a kept build; the gate in its plan; with a Runtime running the composite
/// rollback, `alive` and the release, without one the plain rollback.
fn rollback(context: &Context<'_>, params: Params<'_>) -> Exit {
    const ROLLBACK: InstallLocation = InstallLocation::RollbackApply;
    let plan_only = switch(params, "plan");
    if switch(params, "detach") && !plan_only {
        return detach(
            context,
            ROLLBACK,
            words_without_detach("rollback", params, &[]),
        );
    }
    let begin = || -> Result<(Paths, Loaded), Outcome> {
        let paths = context.paths(ROLLBACK)?;
        let loaded = plan::load(&paths, ROLLBACK)?;
        Ok((paths, loaded))
    };
    let (paths, loaded) = match begin() {
        Ok(found) => found,
        Err(outcome) => return cli_line::failure(&outcome, None),
    };
    let to = match text(params, "to") {
        Some(target) => kept_commit(&loaded, target),
        None => replaced_build(context, &paths, &loaded),
    };
    let Some(commit) = to else {
        let refusal = install::refusal(ROLLBACK, InstallRefusal::BuildNotKept).path(paths.builds());
        return composite_exit(context, Vec::new(), refusal, Map::new());
    };
    let live = match running(context, &loaded, ROLLBACK) {
        Ok(live) => live,
        Err(outcome) => return cli_line::failure(&outcome, None),
    };
    let mut plan_words = vec![
        "rollback".to_owned(),
        "plan".to_owned(),
        "--root".to_owned(),
        context.root_word(),
        "--to".to_owned(),
        commit.clone(),
    ];
    if live {
        plan_words.push("--preview".to_owned());
    }
    let planned = context.install(&plan_words);
    let mut steps = Vec::new();
    if stops(&planned) {
        return stopped(context, steps, planned);
    }
    let plan_id = planned
        .members
        .get("plan_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    steps.push(step(
        "plan",
        &planned.outcome,
        Value::Object(planned.members.clone()),
    ));
    if plan_only {
        let done = install::done(InstallLocation::RollbackPlan)
            .commit_sha(commit)
            .plan_id(plan_id)
            .installation_changed(false);
        return composite_exit(context, steps, done, Map::new());
    }
    applied_composite(
        context, &paths, &loaded, ROLLBACK, "--to", &commit, &plan_id, live, steps,
    )
}

/// The build the live one replaced: the previous live build of the operation that installed it.
fn replaced_build(context: &Context<'_>, paths: &Paths, loaded: &Loaded) -> Option<String> {
    let operation_id = loaded.state.live.as_ref()?.operation_id.clone()?;
    let operation = Operation::load(
        context.host,
        paths,
        &operation_id,
        InstallLocation::RollbackPlan,
    )
    .ok()?;
    operation.journal().previous.live.clone()
}

/// `config replace <file> [--sha <sha256>]` and `config restore <backup> [--sha <sha256>]`
/// (Workflow #434 LAB-5B1c: the second row declares `sha`, which this code read for it before).
fn config_swap(context: &Context<'_>, params: Params<'_>, restore: bool) -> Exit {
    let command = if restore {
        InstallLocation::ConfigRestore
    } else {
        InstallLocation::ConfigReplace
    };
    let file = text(params, if restore { "backup" } else { "file" })
        .map(PathBuf::from)
        .unwrap_or_default();
    let sha = text(params, "sha");
    let run = || -> Result<Exit, Outcome> {
        let paths = context.paths(command)?;
        journal::refuse_unfinished(command, &paths)?;
        let loaded = plan::load(&paths, command)?;
        commit(
            context,
            &paths,
            &loaded,
            &Commit {
                command,
                file: &file,
                sha,
                word: if restore { "restore" } else { "config" },
                adding: None,
            },
        )
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// What a commit puts in place: the file (and its expected SHA-256), as the command `command`
/// whose hold's reason is `lab-<word>-<ms>`; for `instance add` (Workflow #421 INST-5) also what
/// it adds.
struct Commit<'a> {
    command: InstallLocation,
    file: &'a Path,
    sha: Option<&'a str>,
    word: &'static str,
    adding: Option<&'a Adding>,
}

/// Workflow #421 INST-5: what `instance add` commits beyond a file: its candidate is already the
/// swap's part (`part`, a file name in the root), and what it added.
struct Adding {
    part: String,
    added: instance_add::Added,
}

/// The commit of `config replace`, `config restore` and `instance add`: the file checked with the
/// live build's check-config before anything changes; then, while a Runtime runs, the hold, the
/// close, the swap with the replaced file kept, the start and its checks (one `config_replace`
/// operation), `alive` and the release; without one the swap alone. `instance add` needs a
/// Runtime (it is refused `runtime_not_running` before anything changes) and pauses the added
/// instance after the start, before `alive` ([`pause_added`]). Workflow #421 INST-5b: its hold's
/// pause step also sets one global pause of its own (unless one is in effect), kept across the
/// restart and on every failure, which only the release, after the added instance's pause, hands
/// back with the hold's other pauses.
fn commit(
    context: &Context<'_>,
    paths: &Paths,
    loaded: &Loaded,
    commit: &Commit<'_>,
) -> Result<Exit, Outcome> {
    let command = commit.command;
    let candidate = config_swap::candidate(
        context.host,
        paths,
        loaded,
        command,
        commit.file,
        commit.sha,
    )?;
    let mut steps = vec![step(
        "check_config",
        &install::done(InstallLocation::CheckConfig),
        json!({"file": files::plain(&candidate.path), "sha256": candidate.sha256}),
    )];
    if candidate.sha256 == candidate.live_sha256 {
        let done = install::done(command).installation_changed(false);
        let mut members = Map::new();
        members.insert("unchanged".to_owned(), Value::Bool(true));
        return Ok(composite_exit(context, steps, done, members));
    }
    let _lock = WriterLock::take(paths, loaded.settings.lock_wait_ms, command)?;
    journal::refuse_unfinished(command, paths)?;
    let live = running(context, loaded, command)?;
    // INST-5: the added instance is paused on the Runtime that starts with it; with none running
    // nothing is committed (its next start would run the instance unpaused).
    if commit.adding.is_some() && !live {
        return Err(
            install::refusal(command, InstallRefusal::RuntimeNotRunning).path(&loaded.state_root)
        );
    }
    let begun = if live {
        // INST-5b: `instance add`'s hold also sets one global pause of its own before the close,
        // so that the new instance is held from the new Runtime's start on.
        let hold = HoldInputs {
            reason: hold_reason(context, commit.word)?,
            known: std::collections::BTreeMap::new(),
            only: None,
            global: commit.adding.is_some(),
        };
        match deploy::hold_start(
            context.host,
            paths,
            loaded,
            command,
            &hold,
            true,
            &[],
            context.face(),
        )? {
            Ok(begun) => Some(begun),
            Err(answer) => return Ok(stopped(context, steps, answer)),
        }
    } else {
        None
    };
    let staged = commit.adding.map(|adding| config_swap::Staged {
        part: adding.part.clone(),
        added: adding.added.value(),
    });
    let operation = config_swap::open(
        context.host,
        paths,
        loaded,
        &candidate,
        begun.as_ref(),
        Some(context.initiator),
        staged.as_ref(),
    )?;
    let operation_id = operation.id().to_owned();
    let mut swapped = crate::install::update::run(operation, loaded)?;
    if stops(&swapped) {
        // INST-5: a stop after the swap may leave a Runtime running the added instance.
        if let Some(adding) = commit.adding {
            let swapped_in = files::sha256_file(&paths.config())
                .is_ok_and(|(sha256, _)| sha256 == candidate.sha256);
            if swapped_in {
                let paused = pause_added(context, loaded, &adding.added.asked.alias);
                steps.push(step("pause_added", &paused.outcome, paused.result));
                if let Some(refusal) = paused.refusal {
                    add_next(
                        &mut swapped.members,
                        "pause_added",
                        pause_form(context, &adding.added.asked.alias),
                    );
                    swapped.outcome = plan::with_notes(swapped.outcome, &[refusal]);
                }
            }
            swapped
                .members
                .insert("added".to_owned(), adding.added.value());
        }
        return Ok(stopped(context, steps, swapped));
    }
    steps.push(answer_step(context, "swap", &swapped));
    if let Some(adding) = commit.adding {
        let alias = &adding.added.asked.alias;
        let paused = pause_added(context, loaded, alias);
        steps.push(step("pause_added", &paused.outcome, paused.result));
        if let Some(refusal) = paused.refusal {
            // The added instance is not paused: the hold stays, its global pause too (INST-5b), and
            // `next` says how to pause it and, by `ways_out`, how to lift the global pause.
            let mut members = Map::new();
            for member in ["hold", "next"] {
                if let Some(value) = swapped.members.get(member) {
                    members.insert(member.to_owned(), value.clone());
                }
            }
            add_next(&mut members, "pause_added", pause_form(context, alias));
            members.insert("added".to_owned(), adding.added.value());
            return Ok(composite_exit(context, steps, refusal, members));
        }
    }
    if live {
        steps = match alive_and_release(context, loaded, command, &swapped, steps) {
            Ok(steps) => steps,
            Err(exit) => return Ok(exit),
        };
    }
    let mut done = install::done(command)
        .install_operation_id(operation_id)
        .installation_changed(true);
    if let Some(adding) = commit.adding {
        done = done
            .alias(adding.added.asked.alias.clone())
            .game(adding.added.asked.game.clone());
    }
    let done = done.link(CauseRelation::Related, swapped.outcome.clone());
    let mut members = Map::new();
    members.insert(
        "backup".to_owned(),
        swapped
            .members
            .get("backup")
            .cloned()
            .unwrap_or(Value::Null),
    );
    members.insert("runtime_running".to_owned(), Value::Bool(live));
    if let Some(adding) = commit.adding {
        members.insert("added".to_owned(), adding.added.value());
    }
    Ok(composite_exit(context, steps, done, members))
}

/// `instance add <alias> --game <g> --server <s> --mumu-index <n> --touch-backend <b>
/// --capture-backend <b> [--application-id <id>]` (Workflow #421 INST-5, first slice; the
/// application id since Workflow #452, without the option the one the game's root declares for the
/// server): an emulator instance added to the root's configuration, both places of it
/// (`crate::install::instance_add`), committed by
/// [`commit`] with the Runtime restarted under a hold; the new instance is paused with its own
/// reason after the start. Every refusal before the candidate is written leaves the root as it
/// was; a Runtime must run.
fn instance_add(context: &Context<'_>, params: Params<'_>) -> Exit {
    const ADD: InstallLocation = instance_add::ADD;
    let run = || -> Result<Exit, Outcome> {
        let asked = instance_add::Asked::of(params)?;
        let paths = context.paths(ADD)?;
        journal::refuse_unfinished(ADD, &paths)?;
        let loaded = plan::load(&paths, ADD)?;
        if !running(context, &loaded, ADD)? {
            return Err(
                install::refusal(ADD, InstallRefusal::RuntimeNotRunning).path(&loaded.state_root)
            );
        }
        let composed = instance_add::compose(context.host, &paths, &loaded, &asked)?;
        let part = instance_add::stage(context.host, &paths, &composed.bytes)?;
        let file = paths.root.join(&part);
        let adding = Adding {
            part,
            added: composed.added,
        };
        commit(
            context,
            &paths,
            &loaded,
            &Commit {
                command: ADD,
                file: &file,
                sha: None,
                word: "instance",
                adding: Some(&adding),
            },
        )
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// Workflow #421 INST-5: what the pause of the added instance did: its step's outcome and result,
/// and the refusal that ends the command when the instance could not be paused.
struct PausedAdded {
    outcome: Outcome,
    result: Value,
    refusal: Option<Outcome>,
}

/// Workflow #421 INST-5 fix 1: the result of the step `pause_added`: the alias, the reason the
/// instance was paused with (or already shows), the name type as the hold's records keep a reason
/// (`install::hold`), whether this step set the pause, and what kept it from being set or what its
/// drain did.
#[derive(Serialize)]
struct PauseAddedResult<'a> {
    alias: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a SchedulingPauseReason>,
    set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registered: Option<bool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    already_paused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    drained: Option<Drained>,
}

impl<'a> PauseAddedResult<'a> {
    /// The pause of `alias` not set, with `reason` when it is known.
    fn unset(alias: &'a str, reason: Option<&'a SchedulingPauseReason>) -> Self {
        Self {
            alias,
            reason,
            set: false,
            runtime: None,
            registered: None,
            already_paused: false,
            drained: None,
        }
    }

    fn value(&self) -> Value {
        hold::record_value(self)
    }
}

/// Workflow #421 INST-5 (the coordinator's ruling of 11-11): the added instance paused with its
/// own reason (`instance.added`) on the Runtime that started with it, drained as every pause is,
/// before the hold is handed back; one already paused (by anyone) is left as it is. Not
/// registered by the running Runtime, no Runtime answering, or the pause refused: the refusal
/// `post_check_failed` (`alias`, `count` 1), the failure as its cause.
fn pause_added(context: &Context<'_>, loaded: &Loaded, alias: &str) -> PausedAdded {
    const ADD: InstallLocation = instance_add::ADD;
    let failed = |cause: Option<Outcome>, result: Value| {
        let refusal = install::refusal(ADD, InstallRefusal::PostCheckFailed)
            .alias(alias.to_owned())
            .count(1_u32);
        let refusal = match cause {
            Some(cause) => refusal.joined(CauseRelation::CausedBy, cause),
            None => refusal,
        };
        PausedAdded {
            outcome: refusal.clone(),
            result,
            refusal: Some(refusal),
        }
    };
    let reason = match instance_add::pause_reason() {
        Ok(reason) => reason,
        Err(cause) => {
            return failed(Some(cause), PauseAddedResult::unset(alias, None).value());
        }
    };
    let unset = PauseAddedResult::unset(alias, Some(&reason));
    let runtime = context.host.runtime();
    let seen = match runtime.status(&loaded.state_root) {
        Ok(Some(seen)) => seen,
        Ok(None) => {
            let result = PauseAddedResult {
                runtime: Some("not_running"),
                ..unset
            };
            return failed(None, result.value());
        }
        Err(cause) => {
            let result = PauseAddedResult {
                runtime: Some("status_unreadable"),
                ..unset
            };
            return failed(Some(cause), result.value());
        }
    };
    let Some(instance) = seen.instance(alias) else {
        let result = PauseAddedResult {
            registered: Some(false),
            ..unset
        };
        return failed(None, result.value());
    };
    if let Some(pause) = &instance.pause {
        let result = PauseAddedResult {
            already_paused: true,
            ..PauseAddedResult::unset(alias, Some(&pause.reason))
        };
        return PausedAdded {
            outcome: install::done(ADD).alias(alias.to_owned()),
            result: result.value(),
            refusal: None,
        };
    }
    match runtime.pause(
        &loaded.state_root,
        alias,
        &reason,
        loaded.settings.stop_drain_timeout_ms,
    ) {
        Ok(drained) => {
            let result = PauseAddedResult {
                set: true,
                drained: Some(drained),
                ..unset
            };
            PausedAdded {
                outcome: install::done(ADD).alias(alias.to_owned()),
                result: result.value(),
                refusal: None,
            }
        }
        Err(cause) => failed(Some(cause), unset.value()),
    }
}

/// Workflow #421 INST-5: a way to pause the added instance by hand: the core `pause` as actingctl
/// runs it, on the CLI channel, with the instance's own reason (since LAB-6S1b the Runtime also
/// takes the Lab's own `pause` on the Lab channel, which this form does not name).
fn pause_form(context: &Context<'_>, alias: &str) -> Value {
    json!([
        "actingctl",
        "pause",
        "--state-root",
        files::plain(&context.location.state_root),
        "--instance",
        alias,
        "--reason",
        instance_add::PAUSE_REASON
    ])
}

/// One more `next` form of an answer's members.
fn add_next(members: &mut Map<String, Value>, name: &str, form: Value) {
    let next = members
        .entry("next".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(next) = next {
        next.insert(name.to_owned(), form);
    }
}

/// `place <bundle.zip> [--sums <file>] [--facts <file>] [--plan]` (`crate::install::place`): the
/// plan, the hold of the instances bound to the bundle's game, the placement, the checks, the
/// release, and the schedule read after it; with no Runtime answering (Workflow #453 S6), the
/// plan and [`place_cold`].
fn place(context: &Context<'_>, params: Params<'_>) -> Exit {
    const PLACE: InstallLocation = InstallLocation::PackPlace;
    let bundle = text(params, "bundle")
        .map(PathBuf::from)
        .unwrap_or_default();
    let sums = text(params, "sums").map(PathBuf::from);
    let facts = text(params, "facts").map(PathBuf::from);
    let plan_only = switch(params, "plan");
    let run = || -> Result<Exit, Outcome> {
        let paths = context.paths(PLACE)?;
        let loaded = plan::load(&paths, PLACE)?;
        let placement = place::plan(context.host, &paths, &loaded, &bundle, sums.as_deref())?;
        let facts = match &facts {
            Some(path) => Some(
                files::read_json::<Value>(path)
                    .map_err(|cause| install::unusable("--facts").caused_by(cause))?,
            ),
            None => None,
        };
        let seen = context
            .host
            .runtime()
            .status(&loaded.state_root)
            .map_err(|cause| {
                install::refusal(PLACE, InstallRefusal::ProcessesUnreadable)
                    .joined(CauseRelation::CausedBy, cause)
            })?;
        let bound = seen
            .as_ref()
            .map(|seen| {
                seen.instances
                    .iter()
                    .filter(|instance| instance.game.as_deref() == Some(placement.game.as_str()))
                    .map(|instance| instance.alias.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut members = Map::new();
        members.insert("plan".to_owned(), placement.value());
        members.insert("instances".to_owned(), json!(bound));
        members.insert("facts".to_owned(), facts.unwrap_or(Value::Null));
        let mut steps = vec![step(
            "plan",
            &install::done(PLACE).plan_id(placement.id.clone()),
            placement.value(),
        )];
        if plan_only || placement.changes().next().is_none() {
            let done = install::done(PLACE)
                .plan_id(placement.id.clone())
                .game(placement.game.clone())
                .installation_changed(false);
            return Ok(composite_exit(context, steps, done, members));
        }
        // Workflow #453 S6: with no Runtime answering, the placement without a hold.
        if seen.is_none() {
            return place_cold(context, &paths, &loaded, &placement, steps, members);
        }
        let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, PLACE)?;
        journal::refuse_unfinished(PLACE, &paths)?;
        let hold = HoldInputs {
            reason: hold_reason(context, "place")?,
            known: std::collections::BTreeMap::new(),
            only: Some(bound.clone()),
            global: false,
        };
        let begun: HoldStart = match deploy::hold_start(
            context.host,
            &paths,
            &loaded,
            PLACE,
            &hold,
            false,
            &[],
            context.face(),
        )? {
            Ok(begun) => begun,
            Err(answer) => return Ok(stopped(context, steps, answer)),
        };
        // LAB-3 fix 1 (Fail Loud): the composition check after the placement compares with this
        // read; one that fails refuses before anything is paused.
        let catalog_before = context
            .host
            .runtime()
            .catalog_hash(&loaded.state_root)
            .map_err(|cause| {
                install::refusal(PLACE, InstallRefusal::RuntimeNotRunning)
                    .path(&loaded.state_root)
                    .joined(CauseRelation::CausedBy, cause)
            })?;
        let operation = place::open(
            context.host,
            &paths,
            &loaded,
            &place::Begun {
                placement: &placement,
                hold: &begun,
                catalog_before,
                initiator: Some(context.initiator),
                bound: &bound,
            },
        )?;
        let operation_id = operation.id().to_owned();
        let placed = crate::install::update::run(operation, &loaded)?;
        if stops(&placed) {
            return Ok(stopped(context, steps, placed));
        }
        steps.push(answer_step(context, "place", &placed));
        // The schedule after the resume: the planner's catalog, and its rows of the held
        // instances.
        let runtime = context.host.runtime();
        let schedule = match (
            runtime.catalog_hash(&loaded.state_root),
            runtime.plan(&loaded.state_root),
        ) {
            (Ok(catalog), Ok(plan)) => json!({
                "catalog_hash": catalog,
                "instances": plan
                    .instances
                    .iter()
                    .filter(|row| begun.held.contains(&row.alias))
                    .collect::<Vec<_>>(),
            }),
            (Err(failure), _) | (_, Err(failure)) => {
                json!({"failure": cli_line::warning(&failure)})
            }
        };
        steps.push(step("schedule", &install::done(PLACE), schedule));
        let done = install::done(PLACE)
            .install_operation_id(operation_id)
            .game(placement.game.clone())
            .installation_changed(true)
            .link(CauseRelation::Related, placed.outcome);
        Ok(composite_exit(context, steps, done, members))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// Workflow #453 S6: `place` while no Runtime answers. Under the writer lock, with no operation
/// unfinished, the stopped rule the update applies after its close and the installer's start
/// before it spawns (`launch::refuse_running`: the state root's owner lock held refuses
/// `owner_lock_held`, an actingd running under the root `runtime_running`, a probe that cannot
/// decide `processes_unreadable`), then one `pack_place` operation without a hold
/// (`crate::install::place::open_cold`): the items placed and the checks after the placement; no
/// pause, no release and no schedule read after it. While it is unfinished the watchdog stands
/// aside (`pack_place` is off row 7a″'s pass list), so no Runtime starts on a half-placed game
/// root. Fix 1: its `next` names the start (`composition_at_start`, the module's `up`): the
/// composition the placed game root gives is judged only by the Runtime's workbench, never by the
/// checks of a placement with no Runtime. Fix 2: the workbench composes the games of the
/// configured instances only, so with none the configuration binds to the game
/// (`place::configured_bound`, the journal's `bound`) the hint is
/// `composition_at_start_once_bound`: the composition is first judged at the first start after an
/// instance is bound to the game.
fn place_cold(
    context: &Context<'_>,
    paths: &Paths,
    loaded: &Loaded,
    placement: &place::Placement,
    mut steps: Vec<Value>,
    mut members: Map<String, Value>,
) -> Result<Exit, Outcome> {
    const PLACE: InstallLocation = InstallLocation::PackPlace;
    let _lock = WriterLock::take(paths, loaded.settings.lock_wait_ms, PLACE)?;
    journal::refuse_unfinished(PLACE, paths)?;
    crate::install::launch::refuse_running(context.host, paths, &loaded.state_root, PLACE)?;
    let operation = place::open_cold(
        context.host,
        paths,
        loaded,
        placement,
        Some(context.initiator),
    )?;
    let operation_id = operation.id().to_owned();
    let placed = crate::install::update::run(operation, loaded)?;
    if stops(&placed) {
        return Ok(stopped(context, steps, placed));
    }
    steps.push(answer_step(context, "place", &placed));
    // Fix 2: a start composes only the games of the configured instances, so with none bound to
    // the game the hint waits for one.
    let hint = if place::configured_bound(&loaded.config, &placement.game).is_empty() {
        "composition_at_start_once_bound"
    } else {
        "composition_at_start"
    };
    add_next(
        &mut members,
        hint,
        json!(["install", "start", "--root", paths.plain]),
    );
    let done = install::done(PLACE)
        .install_operation_id(operation_id)
        .game(placement.game.clone())
        .installation_changed(true)
        .link(CauseRelation::Related, placed.outcome);
    Ok(composite_exit(context, steps, done, members))
}

/// The configuration of the root, read, with its `install` section.
fn configuration(
    context: &Context<'_>,
    operation: InstallLocation,
) -> Result<
    (
        Value,
        actingcommand_runtime_config::install::InstallSettings,
    ),
    Outcome,
> {
    let config = &context.location.config;
    let unreadable = |cause: Outcome| {
        install::refusal(operation, InstallRefusal::ConfigUnreadable)
            .path(config)
            .caused_by(cause)
    };
    let value = files::read_json::<Value>(config).map_err(unreadable)?;
    let settings = actingcommand_runtime_config::install::settings(&value).map_err(|failure| {
        install::refusal(operation, InstallRefusal::InstallSectionInvalid)
            .path(config)
            .caused_by(failure.outcome())
    })?;
    Ok((value, settings))
}

/// `unlock`: actingd's `unlock-owner` for the root's state root, run from the live build, with
/// the initiator as `--actor` (§5: the caller passes no confirmation; until LAB-7 this module
/// passes actingd's `--confirm-resources-released` itself). actingd's report is the answer.
fn unlock(context: &Context<'_>) -> Exit {
    const UNLOCK: InstallLocation = InstallLocation::Unlock;
    let run = || -> Result<Exit, Outcome> {
        let (_, settings) = configuration(context, UNLOCK)?;
        let program = context
            .location
            .root
            .join(Half::Runtime.dir())
            .join(programs::DAEMON);
        let config = context.location.config.to_string_lossy().into_owned();
        let ran = programs::run(
            context.host,
            &program,
            &[
                "unlock-owner",
                "--config",
                config.as_str(),
                "--actor",
                context.initiator.as_str(),
                "--confirm-resources-released",
            ],
            Deadline::check_config(&settings),
        )
        // actingd's command did not answer: the owner lock is as it was.
        .map_err(|failure| {
            failure.onto(
                install::refusal(UNLOCK, InstallRefusal::OwnerLockHeld)
                    .path(&context.location.state_root),
            )
        })?;
        let report = serde_json::from_slice::<Value>(files::without_bom(&ran.stdout)).ok();
        let reported = report
            .as_ref()
            .and_then(|report| {
                serde_json::from_value::<OutcomeEnvelope>(report["outcome"].clone()).ok()
            })
            .map(OutcomeEnvelope::into_outcome);
        let mut members = Map::new();
        members.insert("report".to_owned(), report.unwrap_or(Value::Null));
        members.insert(
            "actor".to_owned(),
            Value::String(context.initiator.as_str().to_owned()),
        );
        Ok(match (ran.exit_code, reported) {
            (Some(0), Some(reported)) => cli_line::success(
                members,
                &install::done(UNLOCK)
                    .installation_changed(true)
                    .link(CauseRelation::Related, reported),
            ),
            (_, Some(reported)) => cli_line::receipt(members, &reported, true),
            (_, None) => cli_line::receipt(members, &programs::exited(&ran), true),
        })
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// `check-config [<file>]`: the live build's check-config of `file`, or of the root's
/// configuration, judged as the root's (`--install-root`).
fn check_config(context: &Context<'_>, params: Params<'_>) -> Exit {
    const CHECK: InstallLocation = InstallLocation::CheckConfig;
    let run = || -> Result<Exit, Outcome> {
        let (_, settings) = configuration(context, CHECK)?;
        let file = text(params, "file")
            .map(PathBuf::from)
            .unwrap_or_else(|| context.location.config.clone());
        let checked = programs::check_config(
            context.host,
            &context.location.root.join(Half::Runtime.dir()),
            &file,
            Some(&context.location.root),
            Deadline::check_config(&settings),
        )
        .map_err(|failure| {
            failure.onto(install::refusal(
                CHECK,
                InstallRefusal::CheckConfigUnanswered,
            ))
        })?;
        let mut members = Map::new();
        members.insert(
            "check_config".to_owned(),
            serde_json::to_value(&checked.summary).unwrap_or(Value::Null),
        );
        members.insert("install_root".to_owned(), Value::Bool(checked.install_root));
        Ok(if checked.passed() {
            cli_line::success(
                members,
                &install::done(CHECK)
                    .path(&file)
                    .link(CauseRelation::Related, checked.received),
            )
        } else {
            cli_line::receipt(
                members,
                &install::refusal(CHECK, InstallRefusal::ConfigRefusedByTarget)
                    .path(&file)
                    .joined(CauseRelation::CausedBy, checked.received),
                true,
            )
        })
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// `ledger verify`: the live build's `ledger-maintenance verify` of the root's configuration.
fn ledger_verify(context: &Context<'_>) -> Exit {
    const VERIFY: InstallLocation = InstallLocation::LedgerVerify;
    let run = || -> Result<Exit, Outcome> {
        let (_, settings) = configuration(context, VERIFY)?;
        programs::ledger_verify(
            context.host,
            &context.location.root.join(Half::Runtime.dir()),
            &context.location.config,
            Deadline::ledger_verify(&settings),
        )
        .map_err(|failure| {
            failure.onto(install::refusal(VERIFY, InstallRefusal::LedgerVerifyFailed))
        })?;
        Ok(cli_line::success(Map::new(), &install::done(VERIFY)))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// Workflow #453 S10 (contracts/emulator-control.md, "Offline instance discovery"): `emulators`,
/// the live build's `actingd discover --config <the root's configuration> --install-root <root>`,
/// which lists this machine's emulator instances without a Runtime and writes nothing. Fix 1
/// (review): within `programs::DISCOVER_TIMEOUT_MS` (60 s, the child's own three bounds and a
/// margin), at which the child is ended, never left running where an installer or the watchdog
/// would take it for the root's Runtime. A completed discovery is `install_command_done`
/// (operation `install_emulators`, actingd's outcome as its `related` link) with actingd's report
/// as `report`; anything else is `install_command_refused` with `install_refusal`
/// `emulator_discovery_failed`: actingd's outcome (or the child's failure) as its cause, or, for a
/// child ended at the bound, `timeout_ms` the bound and no cause.
fn emulators(context: &Context<'_>) -> Exit {
    const EMULATORS: InstallLocation = InstallLocation::Emulators;
    let refused = || install::refusal(EMULATORS, InstallRefusal::EmulatorDiscoveryFailed);
    let run = || -> Result<Exit, Outcome> {
        // The configuration's usual refusals (`config_unreadable`, `install_section_invalid`)
        // come first; the bound is no key of it.
        configuration(context, EMULATORS)?;
        let discovered = programs::discover(
            context.host,
            &context.location.root.join(Half::Runtime.dir()),
            &context.location.config,
            &context.location.root,
        )
        .map_err(|undiscovered| match undiscovered {
            programs::Undiscovered::Failed(cause) => {
                refused().joined(CauseRelation::CausedBy, cause)
            }
            programs::Undiscovered::Ended => refused().timeout_ms(programs::DISCOVER_TIMEOUT_MS),
        })?;
        if !discovered.completed() {
            return Err(refused().joined(CauseRelation::CausedBy, discovered.received));
        }
        let mut members = Map::new();
        members.insert("report".to_owned(), discovered.report);
        Ok(cli_line::success(
            members,
            &install::done(EMULATORS).link(CauseRelation::Related, discovered.received),
        ))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// Workflow #453 S7: the parameter of `vision manifest` that names the provider.
const PROVIDER: &str = "provider";
/// Workflow #453 S7: the configuration key that names the standard-pack root.
const STANDARD_PACKS_ROOT: &str = "standard_packs.root";

/// Workflow #453 S7: `refusal` with a vision row's child failure: its outcome as the `caused_by`
/// link, or, for a child ended at its bound, `timeout_ms` `bound_ms` and no cause.
fn unanswered_refusal(
    refusal: Outcome,
    unanswered: programs::Unanswered,
    bound_ms: u64,
) -> Outcome {
    match unanswered {
        programs::Unanswered::Failed(cause) => refusal.joined(CauseRelation::CausedBy, cause),
        programs::Unanswered::Ended => refusal.timeout_ms(bound_ms),
    }
}

/// Workflow #453 S7 fix 1 (review): the bound a child of the row `vision check` runs within.
#[derive(Clone, Copy)]
struct ChildBound {
    /// Its own bound, or what is left of the row's limit when that is less.
    run_ms: u64,
    /// The `timeout_ms` of its refusal when it is ended there: its own bound, or the row's limit
    /// when that ran out first.
    timeout_ms: u64,
}

/// Workflow #453 S7 fix 1 (review): the bound of a child of `vision check` whose own bound is
/// `own_ms`, the row having started at `started` on the host's clock: its own, or what is left of
/// `programs::VISION_CHECK_ROW_LIMIT_MS` when that is less; the row's limit, as the refusal's
/// `timeout_ms`, when none of it is left and the child is not started.
fn child_bound(host: &dyn Host, started: u64, own_ms: u64) -> Result<ChildBound, u64> {
    let spent = host.now_unix_ms().saturating_sub(started);
    match programs::VISION_CHECK_ROW_LIMIT_MS.saturating_sub(spent) {
        0 => Err(programs::VISION_CHECK_ROW_LIMIT_MS),
        left if left < own_ms => Ok(ChildBound {
            run_ms: left,
            timeout_ms: programs::VISION_CHECK_ROW_LIMIT_MS,
        }),
        _ => Ok(ChildBound {
            run_ms: own_ms,
            timeout_ms: own_ms,
        }),
    }
}

/// Workflow #453 S7 (`contracts/vision-model-folders.md`, "The manifest"): `vision manifest
/// --provider cpu|directml`, the live build's `actingd vision manifest --provider <provider>`:
/// what a vision root of that provider must hold. It reads nothing of the root and writes nothing.
/// Within `programs::VISION_MANIFEST_TIMEOUT_MS` (30 s), at which the child is ended. An answered
/// manifest is `install_command_done` (operation `install_vision_manifest`, actingd's outcome its
/// `related` link) with actingd's manifest as `report`; anything else is `install_command_refused`
/// with `install_refusal` `vision_report_unanswered` (fix 1: the row resolves no root, so its
/// failure names none): actingd's outcome (or the child's failure) as its cause, or, for a child
/// ended at the bound, `timeout_ms` the bound and no cause. No provider, or another one, is
/// `actingctl_usage_invalid` (`--provider`).
fn vision_manifest(context: &Context<'_>, params: Params<'_>) -> Exit {
    const MANIFEST: InstallLocation = InstallLocation::VisionManifest;
    let provider = match text(params, PROVIDER) {
        Some(provider @ ("cpu" | "directml")) => provider,
        given => {
            let problem = if given.is_none() {
                ArgumentProblem::Required
            } else {
                ArgumentProblem::NotRecognised
            };
            return module_refusal(&cli_line::usage_invalid(Some("--provider"), problem));
        }
    };
    let refused = || install::refusal(MANIFEST, InstallRefusal::VisionReportUnanswered);
    let run = || -> Result<Exit, Outcome> {
        let reported = programs::vision_manifest(
            context.host,
            &context.location.root.join(Half::Runtime.dir()),
            provider,
        )
        .map_err(|unanswered| {
            unanswered_refusal(refused(), unanswered, programs::VISION_MANIFEST_TIMEOUT_MS)
        })?;
        if !reported.completed() {
            return Err(refused().joined(CauseRelation::CausedBy, reported.received));
        }
        let mut members = Map::new();
        members.insert("report".to_owned(), reported.report);
        Ok(cli_line::success(
            members,
            &install::done(MANIFEST).link(CauseRelation::Related, reported.received),
        ))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// Workflow #453 S7 (`contracts/vision-model-folders.md`, "The manifest"): `vision check`. Fix 1
/// (review): first, for each game root below the configuration's `standard_packs.root`
/// ([`game_roots`]; every other entry there is named in the answer with why it is not checked),
/// the Tools' `standard-pack check` within `programs::STANDARD_PACK_CHECK_TIMEOUT_MS` (300 s);
/// then the live build's `actingd vision check --config <the root's configuration>
/// --install-root <root>`, with `--model` for each model the packs need, within
/// `programs::VISION_CHECK_TIMEOUT_MS` (160 s); each child ended at its bound, and the row within
/// `programs::VISION_CHECK_ROW_LIMIT_MS` (900 s) in all ([`child_bound`]); nothing is written.
/// The models the game roots' packs need (each check's `models`) are joined, each once with every
/// game and its packages that name it, and answered with whether a folder of its name is listed
/// (`present`) and whether its identity is the one they name (`identity_matches`). The answer
/// holds actingd's report as `report`, `standard_packs {root, games, skipped}` and
/// `required_models`. It is `install_command_done` (operation `install_vision_check`, actingd's
/// outcome its `related` link) when every manifest file is `ok` and every model of the manifest
/// and of the packs matches; else `install_caveat_noted` with `install_caveat`
/// `vision_incomplete` and `count` the files and the models, each pair of `model_ref` and
/// `model_sha256` once, that do not. Refusals: the configuration's (`config_unreadable`,
/// `install_section_invalid`); `config_unreadable` for a `standard_packs.root` that cannot be
/// listed (its path, the OS failure) or a game root's `bundle.json` whose kind cannot be read;
/// `pack_invalid` (`game`, `path`) for a game root whose check gives no models, which names the
/// root (its outcome or the child's failure as the cause, or `timeout_ms`); fix 1:
/// `vision_root_unresolved` only for actingd's refusal of the configuration or of the vision root
/// it names, `vision_report_unanswered` for every other failure of actingd (its outcome or the
/// child's failure as the cause, or `timeout_ms`). `timeout_ms` is the bound that ran out: the
/// child's own, or the row's limit.
fn vision_check(context: &Context<'_>) -> Exit {
    const CHECK: InstallLocation = InstallLocation::VisionCheck;
    let unanswered = || install::refusal(CHECK, InstallRefusal::VisionReportUnanswered);
    let run = || -> Result<Exit, Outcome> {
        let (config, _) = configuration(context, CHECK)?;
        let started = context.host.now_unix_ms();
        let packs = standard_packs_root(context, &config);
        let listed = match &packs {
            Some(packs) => game_roots(packs, CHECK)?,
            None => GameRoots::default(),
        };
        let tools = context.location.root.join(Half::Tools.dir());
        let mut required = BTreeMap::<(String, String), Vec<Value>>::new();
        for (game, game_root) in &listed.games {
            let refused = || {
                install::refusal(CHECK, InstallRefusal::PackInvalid)
                    .game(game.clone())
                    .path(game_root)
            };
            let bound = child_bound(
                context.host,
                started,
                programs::STANDARD_PACK_CHECK_TIMEOUT_MS,
            )
            .map_err(|limit_ms| refused().timeout_ms(limit_ms))?;
            let models =
                programs::standard_pack_models(context.host, &tools, game_root, bound.run_ms)
                    .map_err(|failure| unanswered_refusal(refused(), failure, bound.timeout_ms))?;
            for model in models {
                required
                    .entry((model.model_ref, model.model_sha256))
                    .or_default()
                    .push(json!({"game": game, "packages": model.packages}));
            }
        }
        // The folders actingd computes an identity of besides the manifest's, each once.
        let asked = required
            .keys()
            .map(|(model_ref, _)| model_ref.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let bound = child_bound(context.host, started, programs::VISION_CHECK_TIMEOUT_MS)
            .map_err(|limit_ms| unanswered().timeout_ms(limit_ms))?;
        let checked = programs::vision_check(
            context.host,
            &context.location.root.join(Half::Runtime.dir()),
            &context.location.config,
            &context.location.root,
            &asked,
            bound.run_ms,
        )
        .map_err(|failure| unanswered_refusal(unanswered(), failure, bound.timeout_ms))?;
        if !checked.completed() {
            // actingd's refusal of the configuration or of the vision root it names; its
            // command line and its own registry say nothing of a root.
            let resolving = checked.received.category() != Category::Success
                && !checked.received.is(SharedCode::InvalidArguments)
                && !checked.received.is(SharedCode::SerializationFailed);
            let refusal = if resolving {
                InstallRefusal::VisionRootUnresolved
            } else {
                InstallRefusal::VisionReportUnanswered
            };
            return Err(
                install::refusal(CHECK, refusal).joined(CauseRelation::CausedBy, checked.received)
            );
        }
        // The report's three lists: a report without one is no check report.
        let unread = |member: &str| {
            unanswered().caused_by(
                Outcome::new(ContractCode::ForeignDataError)
                    .raw_source(RawSource::Json)
                    .raw_text(format!(
                        "actingd's vision check report has no list {member}"
                    )),
            )
        };
        let checked_files = checked.report["files"]
            .as_array()
            .ok_or_else(|| unread("files"))?;
        let folders = checked.report["folders"]
            .as_array()
            .ok_or_else(|| unread("folders"))?;
        let manifest_models = checked.report["models"]
            .as_array()
            .ok_or_else(|| unread("models"))?;
        let required_models = required
            .into_iter()
            .map(|((model_ref, model_sha256), named_by)| {
                let folder = folders
                    .iter()
                    .find(|folder| folder["model_ref"] == model_ref.as_str());
                let identity = folder.and_then(|folder| folder["identity"].as_str());
                json!({
                    "model_ref": model_ref,
                    "model_sha256": model_sha256,
                    "games": named_by,
                    "present": folder.is_some(),
                    "identity_matches": identity == Some(model_sha256.as_str()),
                })
            })
            .collect::<Vec<_>>();
        let unmatched_models = manifest_models
            .iter()
            .chain(&required_models)
            .filter(|model| model["identity_matches"] != true)
            .map(|model| {
                (
                    model["model_ref"].to_string(),
                    model["model_sha256"].to_string(),
                )
            })
            .collect::<BTreeSet<_>>();
        let count = checked_files
            .iter()
            .filter(|file| file["state"] != "ok")
            .count()
            + unmatched_models.len();
        let mut members = Map::new();
        members.insert("report".to_owned(), checked.report.clone());
        members.insert(
            "standard_packs".to_owned(),
            json!({
                "root": packs.as_deref().map(files::plain),
                "games": listed.games.iter().map(|(game, _)| game).collect::<Vec<_>>(),
                "skipped": listed
                    .skipped
                    .iter()
                    .map(|(name, why)| json!({"name": name, "why": why}))
                    .collect::<Vec<_>>(),
            }),
        );
        members.insert("required_models".to_owned(), Value::Array(required_models));
        let top = if count == 0 {
            install::done(CHECK)
        } else {
            install::caveat(CHECK, InstallCaveat::VisionIncomplete).count(count)
        };
        Ok(cli_line::success(
            members,
            &top.link(CauseRelation::Related, checked.received),
        ))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// Workflow #453 S7: `standard_packs.root` of the root's configuration `config`, a relative one
/// against the configuration file's folder, as the Runtime reads it; `None` when it names none.
fn standard_packs_root(context: &Context<'_>, config: &Value) -> Option<PathBuf> {
    let root = actingcommand_runtime_config::member(config, STANDARD_PACKS_ROOT)
        .and_then(Value::as_str)
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)?;
    Some(if root.is_absolute() {
        root
    } else {
        context
            .location
            .config
            .parent()
            .unwrap_or(&context.location.root)
            .join(root)
    })
}

/// Workflow #453 S7: what is below the standard-pack root: the game roots `vision check` checks,
/// by name, and (fix 1, review) every other entry there, by name, with why it is not checked.
#[derive(Default)]
struct GameRoots {
    games: Vec<(String, PathBuf)>,
    skipped: Vec<(String, &'static str)>,
}

/// Workflow #453 S7 fix 1: why an entry below the standard-pack root is not checked. A junction
/// or a symbolic link: the Runtime lists no game root through one.
const SKIPPED_LINK: &str = "link";
/// A folder whose name is not UTF-8 (named in the answer lossily): the Runtime lists none.
const SKIPPED_NAME_NOT_UTF8: &str = "name_not_utf8";
/// A folder without `bundle.json`: it holds no pack a Runtime runs.
const SKIPPED_NO_BUNDLE: &str = "no_bundle";
/// A file, or anything else that is not a folder.
const SKIPPED_NOT_A_FOLDER: &str = "not_a_folder";

/// Workflow #453 S7: the game roots below `packs`, by name, as the Runtime lists them: each folder
/// there (no junction or link) whose name is UTF-8 and that holds `bundle.json`. Fix 1 (review):
/// every other entry is named with why it is not checked ([`GameRoots::skipped`], by name);
/// nothing is skipped silently. None when `packs` does not exist; a root that cannot be listed is
/// `config_unreadable` with its path and the OS failure, and so is a folder whose `bundle.json`
/// cannot be told a file or not (that file as the path).
fn game_roots(packs: &Path, operation: InstallLocation) -> Result<GameRoots, Outcome> {
    let unreadable = |path: &Path, error: std::io::Error, op: IoOp| {
        install::refusal(operation, InstallRefusal::ConfigUnreadable)
            .path(path)
            .caused_by(Outcome::from_io_error(&error, op))
    };
    let entries = match std::fs::read_dir(packs) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GameRoots::default());
        }
        Err(error) => return Err(unreadable(packs, error, IoOp::ReadDir)),
    };
    let mut listed = GameRoots::default();
    for entry in entries {
        let entry = entry.map_err(|error| unreadable(packs, error, IoOp::ReadDir))?;
        let kind = entry
            .file_type()
            .map_err(|error| unreadable(packs, error, IoOp::Metadata))?;
        let name = entry.file_name();
        let path = entry.path();
        let shown = name.to_string_lossy().into_owned();
        let why = if kind.is_symlink() {
            SKIPPED_LINK
        } else if !kind.is_dir() {
            SKIPPED_NOT_A_FOLDER
        } else if name.to_str().is_none() {
            SKIPPED_NAME_NOT_UTF8
        } else {
            let bundle = path.join("bundle.json");
            match std::fs::metadata(&bundle) {
                Ok(metadata) if metadata.is_file() => {
                    listed.games.push((shown, path));
                    continue;
                }
                Ok(_) => SKIPPED_NO_BUNDLE,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => SKIPPED_NO_BUNDLE,
                Err(error) => return Err(unreadable(bundle.as_path(), error, IoOp::Metadata)),
            }
        };
        listed.skipped.push((shown, why));
    }
    listed.games.sort();
    listed.skipped.sort();
    Ok(listed)
}

/// `watchdog install` and `watchdog uninstall`: the watchdog's own command for the root, its
/// report the answer (the developer lines it wrote before as `warnings`).
fn watchdog(context: &Context<'_>, asked: crate::watchdog::Asked) -> Exit {
    let answered = context.host.watchdog(asked, &context.location.root);
    let mut members = match answered.line {
        Some(Value::Object(members)) => members,
        _ => Map::new(),
    };
    members.remove("outcome");
    if !answered.developer_before.is_empty() {
        members.insert(
            "warnings".to_owned(),
            Value::Array(
                answered
                    .developer_before
                    .iter()
                    .map(cli_line::warning)
                    .collect(),
            ),
        );
    }
    cli_line::finish(Some(members), &answered.outcome, false, None)
}

/// Workflow #434 LAB-5G5b: `mcp plan`'s parameter that names the entry's protocol.
const ENTRY_PROTOCOL: &str = "entry_protocol";

/// Workflow #434 LAB-5G5b: `mcp plan --entry-protocol 2 [--current-build <commit>]`, the plan the
/// MCP entry's second generation asks of the module's own root
/// (`crate::install::mcp_entry::plan_g2`). Another protocol is `actingctl_usage_invalid`
/// (`--entry-protocol`, `not_recognised`), as `actingctl mcp-entry plan` refuses one other than
/// its own; the first generation's protocol is that command's.
fn mcp_plan(context: &Context<'_>, params: Params<'_>) -> Exit {
    const PLAN: InstallLocation = InstallLocation::McpEntryPlan;
    let protocol = params.get(ENTRY_PROTOCOL);
    if protocol.and_then(Value::as_u64) != Some(install::mcp_entry::ENTRY_PROTOCOL_G2) {
        let problem = match protocol {
            None => ArgumentProblem::Required,
            Some(_) => ArgumentProblem::NotRecognised,
        };
        return module_refusal(&cli_line::usage_invalid(Some("--entry-protocol"), problem));
    }
    let planned = context
        .paths(PLAN)
        .and_then(|paths| install::mcp_entry::plan_g2(&paths, text(params, "current_build")));
    line(context, planned.unwrap_or_else(Answer::new))
}

/// Workflow #434 LAB-5G5b: `mcp entries [--all]`, the entries' status files of the module's own
/// root, both generations side by side (`crate::install::mcp_entry::entries`).
fn mcp_entries(context: &Context<'_>, params: Params<'_>) -> Exit {
    let listed = context
        .paths(InstallLocation::McpEntryList)
        .and_then(|paths| install::mcp_entry::entries(&paths, switch(params, "all")));
    line(context, listed.unwrap_or_else(Answer::new))
}

/// Workflow #434 LAB-5G5b: `mcp config --client <client> [--list <modules>]`, the second
/// generation's registration snippet for the module's own root (`crate::install::mcp_config`);
/// it writes nothing.
fn mcp_config(context: &Context<'_>, params: Params<'_>) -> Exit {
    let Some(client) = text(params, "client") else {
        return module_refusal(&cli_line::usage_invalid(
            Some("--client"),
            ArgumentProblem::Required,
        ));
    };
    let list = match list_words(params) {
        Ok(list) => list,
        Err(refused) => return module_refusal(&refused),
    };
    let root = files::plain(&context.location.root);
    match install::mcp_config::snippet(&root, client, list.as_deref()) {
        Ok(answer) => line(context, answer),
        Err(refused) => module_refusal(&refused),
    }
}

/// `mcp config`'s `list`: the words of the server's `--list`, in their order; `None` when it is
/// not given. A value that is no list of strings (the table's parser gives none) is `wrong_type`.
fn list_words(params: Params<'_>) -> Result<Option<Vec<String>>, Outcome> {
    let Some(value) = params.get("list") else {
        return Ok(None);
    };
    let wrong = || cli_line::usage_invalid(Some("--list"), ArgumentProblem::WrongType);
    value
        .as_array()
        .ok_or_else(wrong)?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(wrong))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The line of a refusal of the module that has no members: `{outcome}`, with the module's usage
/// as the developer text of a usage refusal.
fn module_refusal(refused: &Outcome) -> Exit {
    let usage = refused
        .is(ActingctlCode::ActingctlUsageInvalid)
        .then_some(USAGE);
    cli_line::failure(refused, usage)
}

/// `agent <sub>`: `agent-channel <sub>` with the words the row's constant `handed` gives
/// ([`agent_line`]); its answer as the command gives it (a hook prints nothing, the notice its
/// text). Workflow #434 LAB-5A: the action is `claude-hook`'s positional `action` (a further word
/// before).
fn agent(context: &Context<'_>, handed: &'static Handed, params: &Map<String, Value>) -> Answered {
    agent_channel::run(&agent_line(handed, params, &context.location.state_root))
}

/// Workflow #434 LAB-5B1b: the words after `agent-channel` the agent row `handed` gives for
/// `params`, for the state root `state_root`: its command's word, then each part in its place
/// (`claude-hook`'s action as a word; `--state-root` and the state root, which `notice` does not
/// take), then every member no part placed, in the order given, under the flag of its name
/// ([`further_flags`]). Before the slice one arm served every `agent` row and read `action` on
/// each; the rows that do not declare it no longer read it (register §1.287, "Slice LAB-5B1b").
fn agent_line(
    handed: &'static Handed,
    params: &Map<String, Value>,
    state_root: &Path,
) -> Vec<OsString> {
    let read = rescue_reads::handing(handed, params);
    let mut words = handed
        .command
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let mut placed = Vec::new();
    for part in handed.parts {
        match *part {
            Part::Param(name, Written::Word) => {
                placed.push(name);
                if let Some(value) = text(read, name) {
                    words.push(OsString::from(value));
                }
            }
            Part::StateRoot => {
                words.push(OsString::from(STATE_ROOT_FLAG));
                words.push(state_root.as_os_str().to_owned());
            }
            Part::Further => {
                words.extend(
                    further_flags(params, &placed)
                        .into_iter()
                        .map(OsString::from),
                );
            }
            // Written with `Part::Further`, in the order given; no agent row has another part.
            _ => {}
        }
    }
    words
}

/// The words after `agent-channel` of the row `agent <sub>` with `params`, for the state root
/// `state_root` ([`agent_line`], for the tests of Workflow #434 LAB-5A and LAB-5B1b).
#[cfg(test)]
pub(crate) fn agent_words(
    sub: &str,
    params: &Map<String, Value>,
    state_root: &Path,
) -> Vec<OsString> {
    let handed = rescue_reads::agent(&format!("agent {sub}"))
        .unwrap_or_else(|| panic!("agent {sub} is an agent row"));
    agent_line(handed, params, state_root)
}

/// The rescue module's answer of a row actinglab's parser gave, located from this process: the
/// root of the running program ([`actingcommand_lab_ops::locate::locate`]).
pub fn answer_here(name: &str, params: &Map<String, Value>, initiator: Initiator) -> Answered {
    match actingcommand_lab_ops::locate::locate() {
        Ok(location) => answer(name, params, &location, initiator),
        Err(outcome) => Answered::Line(cli_line::failure(&outcome, None)),
    }
}

/// Whether a path is the root of `location` (for the tests' fake roots).
#[cfg(test)]
pub(crate) fn location_of(root: &Path, config: &Path, state_root: &Path) -> Location {
    Location {
        root: root.to_path_buf(),
        installation: None,
        config: config.to_path_buf(),
        state_root: state_root.to_path_buf(),
    }
}

/// Test builds only (Workflow #434 LAB-5B1b): the command lines this thread handed to `install`,
/// in order, which the test that pins them reads (`install::gate_434_lab5b1b_lines`).
#[cfg(test)]
pub(crate) mod installed {
    use std::cell::RefCell;

    thread_local! {
        static LINES: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
    }

    /// One more command line handed to `install`.
    pub(crate) fn record(words: &[String]) {
        LINES.with_borrow_mut(|lines| lines.push(words.to_vec()));
    }

    /// The command lines handed on since the last call.
    pub(crate) fn take() -> Vec<Vec<String>> {
        LINES.take()
    }
}

// Test-only: Workflow #434 LAB-5A, the rescue rows hand their declared parameters on as their
// further flags went before.
#[cfg(test)]
#[path = "gate_434_lab5a.rs"]
mod gate_434_lab5a;
