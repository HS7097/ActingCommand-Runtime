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
//! - **Until LAB-6** a request the module sends to a running Runtime goes as `actingctl install`'s
//!   do, on the CLI channel with the CLI envelope (`crate::install::runtime`): the Runtime refuses
//!   a pause, a resume and a shutdown from a Lab envelope until LAB-6 deletes those origin gates
//!   (§2.4), and `BindChannel` takes no initiator before `runtime-client` 7. The journal records
//!   the initiator now; the binding carries it from LAB-6.

use crate::agent_channel::{self, Answer as Answered};
use crate::cli_line::{self, Exit};
use crate::codes::{
    ActingctlCode, ArgumentProblem, InstallCaveat, InstallLocation, InstallRefusal,
};
use crate::install::deploy::{self, HoldStart};
use crate::install::files;
use crate::install::hold::{HoldInputs, OwnedHold};
use crate::install::host::{Host, SystemHost};
use crate::install::journal::{self, Operation, StepKind};
use crate::install::lock::WriterLock;
use crate::install::manifest::Half;
use crate::install::plan::{self, Loaded};
use crate::install::programs::{self, Deadline};
use crate::install::state::{KindLocation, Paths};
use crate::install::{self, Answer, Face, config_swap, place};
use crate::runtime_start::{self, Mode};
use actingcommand_contract::SchedulingPauseReason;
use actingcommand_contract::outcome::{Category, CauseRelation, Outcome, OutcomeEnvelope};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_lab_ops::locate::Location;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The developer text of a refused command line of the rescue module.
const USAGE: &str = "usage: actinglab [--json] <rescue operation> [<positionals>] \
     [--<option> <value>] [--<switch>]; contracts/lab-operations.json lists the rows of module \
     rescue; no --root, no --state-root: the root is the one actinglab runs from";

/// The rows of module `rescue` this slice serves, by the table's name.
pub const SERVED: [&str; 29] = [
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
    "place",
    "unlock",
    "check-config",
    "ledger verify",
    "watchdog install",
    "watchdog uninstall",
    "agent add",
    "agent remove",
    "agent list",
    "agent claude-hook",
    "agent claude-register",
    "agent notice",
];

/// Whether this slice serves the rescue row `name`.
pub fn serves(name: &str) -> bool {
    SERVED.contains(&name)
}

/// The parameters a caller may never give the rescue module: the root and the state root (§7,
/// §9.3) and the confirmations (§5, §6). Each is refused as an unknown argument.
const REFUSED_PARAMETERS: [&str; 7] = [
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
            host: &SystemHost,
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
    if let Some(sub) = name.strip_prefix("agent ") {
        return agent(context, sub, params);
    }
    Answered::Line(match name {
        "up" => up(context),
        "restart" => restart(context),
        "down" => down(context, params),
        "alive" => alive(context, params),
        "deploy" => deploy(context, params),
        "rollback" => rollback(context, params),
        "undo" => installer(context, &["undo"], params, &[("op", "--operation")], true),
        "op status" => installer(context, &["status"], params, &[("op", "--operation")], true),
        "op resume" => installer(context, &["resume"], params, &[("op", "--operation")], true),
        "leftovers" => installer(context, &["leftovers"], params, &[], true),
        "guards" => installer(context, &["guards"], params, &[("for", "--for")], true),
        "release-hold" => installer(
            context,
            &["release-hold"],
            params,
            &[("reason", "--reason")],
            true,
        ),
        "release verify" => installer(
            context,
            &["release", "verify"],
            params,
            &[("dir", "--dir")],
            false,
        ),
        "release stage" => installer(
            context,
            &["release", "stage"],
            params,
            &[("dir", "--dir")],
            true,
        ),
        "adopt" => installer(context, &["adopt"], params, &[], true),
        "config replace" => config_swap(context, params, false),
        "config restore" => config_swap(context, params, true),
        "place" => place(context, params),
        "unlock" => unlock(context),
        "check-config" => check_config(context, params),
        "ledger verify" => ledger_verify(context),
        "watchdog install" => watchdog(context, crate::watchdog::Asked::Install),
        "watchdog uninstall" => watchdog(context, crate::watchdog::Asked::Uninstall),
        _ => cli_line::failure(
            &cli_line::usage_invalid(Some(name), ArgumentProblem::Unknown),
            Some(USAGE),
        ),
    })
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

fn text<'a>(params: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

fn switch(params: &Map<String, Value>, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// The row's parameters but those in `declared` (which the caller maps itself), as the source
/// command takes them: `--name value`, or `--name` for a switch that is set. Workflow #434
/// LAB-5A: the row declares them all (before it, they were the further flags of a row with
/// `params_more`); a switch given `false`, which only `serve --stdio` can give, is not written.
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

/// An installer command: `verb`, `--root <root>` (unless the command takes none), the row's
/// parameters as the flags `named` maps them to, and its other parameters under their own flags
/// ([`further_flags`]).
fn installer(
    context: &Context<'_>,
    verb: &[&str],
    params: &Map<String, Value>,
    named: &[(&str, &str)],
    rooted: bool,
) -> Exit {
    let mut words = verb
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    if rooted {
        words.push("--root".to_owned());
        words.push(context.root_word());
    }
    for (param, flag) in named {
        if let Some(value) = text(params, param) {
            words.push((*flag).to_owned());
            words.push(value.to_owned());
        }
    }
    let declared = named.iter().map(|(param, _)| *param).collect::<Vec<_>>();
    words.extend(further_flags(params, &declared));
    line(context, context.install(&words))
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
/// --state-root <s> --instance <alias>`, on the CLI channel: the Lab's own `resume` goes on the Lab
/// channel, whose resume the Runtime refuses until LAB-6). No form names another reason or an
/// instance whose pause shows one; without a status read there is no `actingctl_resume`.
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
/// post-start checks); on an A/B root the start of its selection (`actingctl start`).
fn up(context: &Context<'_>) -> Exit {
    if context.location.installation.is_some() {
        return runtime_start::run_located(Mode::Start, &context.location.root, None);
    }
    installer(context, &["start"], &Map::new(), &[], true)
}

/// `restart`: `actingctl restart` for the root, with its configuration on a single-location root.
fn restart(context: &Context<'_>) -> Exit {
    let config = context
        .location
        .installation
        .is_none()
        .then_some(context.location.config.as_path());
    runtime_start::run_located(Mode::Restart, &context.location.root, config)
}

/// `down [--reason <r>]`: the formal stop with its hold (`install stop`); without `--reason` the
/// hold's reason is the module's own.
fn down(context: &Context<'_>, params: &Map<String, Value>) -> Exit {
    let reason = match text(params, "reason") {
        Some(reason) => reason.to_owned(),
        None => match hold_reason(context, "down") {
            Ok(reason) => reason.as_str().to_owned(),
            Err(outcome) => return cli_line::failure(&outcome, None),
        },
    };
    let words = vec![
        "stop".to_owned(),
        "--root".to_owned(),
        context.root_word(),
        "--hold".to_owned(),
        reason,
    ];
    line(context, context.install(&words))
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
fn alive(context: &Context<'_>, params: &Map<String, Value>) -> Exit {
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
/// method, with the same initiator, its output appended to `install\detached-<ms>.log`; the
/// answer is at once (`op status` follows the operation).
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
        let initiator =
            (context.initiator != Initiator::LabCli).then_some(context.initiator.as_str());
        let pid = context
            .host
            .detach_rescue(&arguments, initiator, &paths.root, &log)?;
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

/// The module's words of `name` with `params`, without `--detach`: what a detached run runs.
fn words_without_detach(
    name: &str,
    params: &Map<String, Value>,
    positionals: &[&str],
) -> Vec<String> {
    let mut words = name.split(' ').map(str::to_owned).collect::<Vec<_>>();
    for positional in positionals {
        if let Some(value) = text(params, positional) {
            words.push(value.to_owned());
        }
    }
    for (param, value) in params {
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
fn deploy(context: &Context<'_>, params: &Map<String, Value>) -> Exit {
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
fn rollback(context: &Context<'_>, params: &Map<String, Value>) -> Exit {
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

/// `config replace <file> [--sha <sha256>]` and `config restore <backup>`.
fn config_swap(context: &Context<'_>, params: &Map<String, Value>, restore: bool) -> Exit {
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
        let candidate = config_swap::candidate(context.host, &paths, &loaded, command, &file, sha)?;
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
        let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, command)?;
        journal::refuse_unfinished(command, &paths)?;
        let live = running(context, &loaded, command)?;
        let begun = if live {
            let hold = HoldInputs {
                reason: hold_reason(context, if restore { "restore" } else { "config" })?,
                known: std::collections::BTreeMap::new(),
                only: None,
            };
            match deploy::hold_start(
                context.host,
                &paths,
                &loaded,
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
        let operation = config_swap::open(
            context.host,
            &paths,
            &loaded,
            &candidate,
            begun.as_ref(),
            Some(context.initiator),
        )?;
        let operation_id = operation.id().to_owned();
        let swapped = crate::install::update::run(operation, &loaded)?;
        if stops(&swapped) {
            return Ok(stopped(context, steps, swapped));
        }
        steps.push(answer_step(context, "swap", &swapped));
        if live {
            steps = match alive_and_release(context, &loaded, command, &swapped, steps) {
                Ok(steps) => steps,
                Err(exit) => return Ok(exit),
            };
        }
        let done = install::done(command)
            .install_operation_id(operation_id)
            .installation_changed(true)
            .link(CauseRelation::Related, swapped.outcome.clone());
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
        Ok(composite_exit(context, steps, done, members))
    };
    run().unwrap_or_else(|outcome| cli_line::failure(&outcome, None))
}

/// `place <bundle.zip> [--sums <file>] [--facts <file>] [--plan]` (`crate::install::place`): the
/// plan, the hold of the instances bound to the bundle's game, the placement, the checks, the
/// release, and the schedule read after it.
fn place(context: &Context<'_>, params: &Map<String, Value>) -> Exit {
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
        if seen.is_none() {
            return Err(
                install::refusal(PLACE, InstallRefusal::RuntimeNotRunning).path(&loaded.state_root)
            );
        }
        let _lock = WriterLock::take(&paths, loaded.settings.lock_wait_ms, PLACE)?;
        journal::refuse_unfinished(PLACE, &paths)?;
        let hold = HoldInputs {
            reason: hold_reason(context, "place")?,
            known: std::collections::BTreeMap::new(),
            only: Some(bound),
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
fn check_config(context: &Context<'_>, params: &Map<String, Value>) -> Exit {
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

/// `agent <sub>`: `agent-channel <sub>` with `claude-hook`'s action word, the root's state root
/// (`--state-root`) and the row's other parameters as flags; its answer as the command gives it
/// (a hook prints nothing, the notice its text). Workflow #434 LAB-5A: the action is the row's
/// positional `action` (a further word before).
fn agent(context: &Context<'_>, sub: &str, params: &Map<String, Value>) -> Answered {
    agent_channel::run(&agent_words(sub, params, &context.location.state_root))
}

/// The words after `agent-channel` of the row `agent <sub>` with `params`, for the state root
/// `state_root`.
fn agent_words(sub: &str, params: &Map<String, Value>, state_root: &Path) -> Vec<OsString> {
    let mut words = vec![OsString::from(sub)];
    if let Some(action) = text(params, AGENT_ACTION) {
        words.push(OsString::from(action));
    }
    if sub != "notice" {
        words.push(OsString::from("--state-root"));
        words.push(state_root.as_os_str().to_owned());
    }
    words.extend(
        further_flags(params, &[AGENT_ACTION])
            .into_iter()
            .map(OsString::from),
    );
    words
}

/// `agent claude-hook`'s positional: `start` or `end`.
const AGENT_ACTION: &str = "action";

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

// Test-only: Workflow #434 LAB-5A, the rescue rows hand their declared parameters on as their
// further flags went before.
#[cfg(test)]
#[path = "gate_434_lab5a.rs"]
mod gate_434_lab5a;
