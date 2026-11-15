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
//!
//! Workflow #463 S3-10 (contracts/standard-pack-trial.md; register §1.417): `actinglab
//! standard-pack trial --instance <x> --package <dir> [--authorization <reference>] [--variable
//! <name>=<json>]... [--expect <outcome_key>] [--irreversible] [--root <game root>] [--state-dir
//! <dir>] --plan` answers the plan of one trial run on a real application and every refusal
//! before it (the resource tooling's `plan_standard_pack_trial`). A plan asks for no authorization
//! (fix round 2).
//!
//! Workflow #463 S3-11 (contracts/standard-pack-trial.md, "The run"; register §1.418): without
//! `--plan` the line sends the trial's one run, after everything the plan does and the refusals only
//! the run needs (the resource tooling's `check_trial_run`), and before anything is written: the
//! request `run start` sends, by its own function (`lab_ops::core_ops::run_start_prepared`), on the
//! Lab channel with this process's initiator, waited for as `run start` waits (to its receipt, or
//! up to `--wait` by `lab_ops::wait::wait_at_most`), then the run read as `run show` reads it
//! (`lab_ops::run_show::read`). The end it reads is answered and recorded once by the resource
//! tooling's writer; an end not read is answered with the run's request id and the line that reads
//! it later, `--collect <request_id>`, which sends nothing. A result that is not known is answered
//! as that; nothing is ever sent again. A passed trial is not an authorization to enable; a trial
//! counts nothing toward a break-in; a trial is one run, sent once. Fix round 2: `--collect` takes
//! only a run sent by hand on the Lab's channel; a run has one record, whatever content the
//! package directory holds when it is collected; a run is answered by its end state.

use crate::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::codes::{ContractCode, ContractLocation};
use actingcommand_contract::outcome::{Detail, IoOp, Outcome, RawSource};
use actingcommand_contract::{EventQuery, ProjectionProfile, RequestId};
use actingcommand_lab_ops::cli_line::{self, Exit};
use actingcommand_lab_ops::codes::{ArgumentProblem, RunState};
use actingcommand_lab_ops::core_ops::{parse_duration, run_start_prepared};
use actingcommand_lab_ops::wait::{Waited, wait_at_most};
use actingcommand_lab_ops::{instance_selector, run_evidence, run_show, wait};
use actingcommand_resource_tooling::{
    AdoptDecision, AdoptSource, AdoptWorktree, PackTrialRecord, ResourceToolingCode,
    StandardPackAdoptRequest, StandardPackCheckRequest, StandardPackComposeRequest,
    StandardPackReadyRequest, StandardPackTrialAnswer, StandardPackTrialRequest,
    TrialCollectProblem, TrialRecordPlan, TrialRunEnd, check_standard_pack_root, check_trial_run,
    compose_standard_packs, judge_standard_pack_readiness, plan_standard_pack_trial,
    standard_pack_adopt, trial_record_of_run, write_trial_record,
};
use actingcommand_runtime_client::{
    ContainedRunState, ContainedRunStatus, RunDispatch, RunKey, RunOrigin, RuntimeClient,
    RuntimeClientError,
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

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
/// Workflow #463 S3-10: the flags `standard-pack trial` takes; its instance is actinglab's global
/// `--instance`, as on every own command that acts on one. Workflow #463 S3-11: and the run's
/// two, `--collect` and `--wait`.
pub(crate) const TRIAL_FLAGS: [&str; 10] = [
    "--package",
    "--authorization",
    "--variable",
    "--expect",
    "--irreversible",
    "--root",
    "--state-dir",
    "--plan",
    COLLECT_FLAG,
    WAIT_FLAG,
];
/// Workflow #463 S3-11: the run's two flags: the run of an earlier trial whose end is read and
/// recorded, and how long the run is waited for. A plan takes neither.
const COLLECT_FLAG: &str = "--collect";
const WAIT_FLAG: &str = "--wait";
/// Workflow #463 S3-10: the flags of `standard-pack trial` that take no value, which take none
/// from the next word but `true` or `false` (`FlagArgs::parse_switches`).
pub(crate) const TRIAL_SWITCHES: [&str; 2] = ["--irreversible", "--plan"];
/// Workflow #463 S3-10: the row's name, which its refusals name.
const TRIAL: &str = "standard-pack trial";

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

/// Workflow #463 S3-10: the flags of `standard-pack trial`. Workflow #463 S3-11: the run's.
pub(crate) fn trial_options() -> [&'static str; 11] {
    [
        "--instance <the instance the trial is for; the run is sent to it>",
        "--package <a pack directory: packs/<id> of a checkout or a placed pack; no container>",
        "--authorization <the owner's authorization reference, 1 to 1024 bytes; the run needs it>",
        "--variable <name>=<json> (repeatable; judged by the plan; a run carries none yet)",
        "--expect <an outcome key the package declares; the run answers whether it matched>",
        "--irreversible (acknowledges a package the irreversible mark flags; its run needs it)",
        "--root <judges the mark; a pack it does not register is marked; without it not judged>",
        "--state-dir <the Lab's state directory, where trial records go; default actinglab's>",
        "--plan (the plan only; nothing is sent or written)",
        "--collect <request_id> (reads that run's end and records it; sends nothing)",
        "--wait <duration> (the longest the run is waited for; absent: until its receipt)",
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
        // Workflow #463 S3-10: the plan of a trial on a real application; Workflow #463 S3-11:
        // without `--plan`, its one run.
        "trial" => run_trial(global, args),
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

/// Workflow #463 S3-10: `standard-pack trial` (module doc). The flags' own refusals come first,
/// each naming its flag: a flag the row does not take or a positional word (`unknown`), a flag
/// given twice but `--variable` (`repeated`), an option without its value (`value_missing`,
/// `--authorization` among them), `--instance` or `--package` absent (`required`), a `--variable`
/// as the scan replay refuses it (its own parser, `replay_cli::given_variables`). Fix round 2: a
/// plan asks for no authorization; without `--authorization` it says that the run will require
/// one.
///
/// Workflow #463 S3-11: `--plan` is no longer required. Without it the line is the run (or, with
/// `--collect`, the reading of an earlier run's end): `--collect` or `--wait` beside `--plan`, and
/// `--wait` beside `--collect`, are `conflicting`; a `--collect` that is no request id
/// `not_recognised`, a `--wait` that is no duration `wrong_type`; under the global `--dry-run` the
/// run and `--collect` are `dry_run_unsupported` after the flags, before anything is read. Then
/// everything the plan does, by the same function, then the refusals only the run needs
/// (`check_trial_run`), and only then the Runtime is reached ([`TrialRun`]).
fn run_trial(global: &GlobalOptions, args: &[String]) -> CliOutcome<Value> {
    let flags = FlagArgs::parse_switches(args, &TRIAL_SWITCHES);
    if let Some(flag) = flags
        .flags
        .keys()
        .find(|flag| !TRIAL_FLAGS.contains(&flag.as_str()))
    {
        return Err(trial_refused(flag, ArgumentProblem::Unknown));
    }
    if let Some(word) = flags.positionals.first() {
        return Err(trial_refused(word, ArgumentProblem::Unknown));
    }
    if let Some(flag) = TRIAL_FLAGS
        .into_iter()
        .find(|flag| *flag != "--variable" && flags.count(flag) > 1)
    {
        return Err(trial_refused(flag, ArgumentProblem::Repeated));
    }
    let plan_only = flags.bool("--plan");
    if let Some(flag) = [COLLECT_FLAG, WAIT_FLAG].into_iter().find(|flag| {
        flags.given(flag) && (plan_only || (*flag == WAIT_FLAG && flags.given(COLLECT_FLAG)))
    }) {
        return Err(trial_refused(flag, ArgumentProblem::Conflicting));
    }
    let instance = global
        .instance
        .clone()
        .filter(|instance| !instance.trim().is_empty())
        .ok_or_else(|| trial_refused("--instance", ArgumentProblem::Required))?;
    let package = trial_value(&flags, "--package")?
        .ok_or_else(|| trial_refused("--package", ArgumentProblem::Required))?;
    let authorization = trial_value(&flags, "--authorization")?;
    let expect = trial_value(&flags, "--expect")?;
    let root = trial_value(&flags, "--root")?.map(PathBuf::from);
    trial_value(&flags, "--state-dir")?;
    let collect = match trial_value(&flags, COLLECT_FLAG)? {
        Some(id) => Some(
            run_evidence::parse_request_id(&id)
                .map(|request_id| (id, request_id))
                .ok_or_else(|| trial_refused(COLLECT_FLAG, ArgumentProblem::NotRecognised))?,
        ),
        None => None,
    };
    let wait_for = match trial_value(&flags, WAIT_FLAG)? {
        Some(text) => Some(
            parse_duration(&text)
                .ok_or_else(|| trial_refused(WAIT_FLAG, ArgumentProblem::WrongType))?,
        ),
        None => None,
    };
    let variables = crate::replay_cli::given_variables(&flags, TRIAL)?;
    if !plan_only {
        crate::dry_run_gate::refuse(
            global,
            TRIAL,
            Some("standard-pack trial --plan"),
            Value::Null,
        )?;
    }
    let request = StandardPackTrialRequest {
        package: PathBuf::from(package),
        instance,
        authorization,
        variables,
        expect,
        irreversible: flags.bool("--irreversible"),
        root,
        state_dir: crate::session_state_dir_from_flags(&flags)?,
    };
    let answer = plan_standard_pack_trial(&request)?;
    let encode_failed = |error: serde_json::Error| {
        CliError::device(format!("failed to serialize the trial plan: {error}"))
    };
    if plan_only {
        return serde_json::to_value(&answer).map_err(encode_failed);
    }
    let plan = serde_json::to_value(&answer.plan).map_err(encode_failed)?;
    let run = TrialRun {
        kept: check_trial_run(&answer, &request)?,
        request: &request,
        answer: &answer,
        plan,
    };
    let client = RuntimeClient::connect(crate::lab_client_config(crate::runtime_state_root()?))
        .map_err(crate::runtime_slice_cli::map_runtime_error)?;
    match collect {
        Some((id, request_id)) => run.collect(&client, &id, request_id),
        None => run.send(&client, wait_for),
    }
}

/// Workflow #463 S3-11: one trial's run, after its plan and the refusals only the run needs
/// (contracts/standard-pack-trial.md, "The run").
struct TrialRun<'a> {
    /// The trial as the line asked for it.
    request: &'a StandardPackTrialRequest,
    /// Its plan, as `--plan` answers it.
    answer: &'a StandardPackTrialAnswer,
    /// The plan as the record keeps it, with the authorization the run is sent under.
    kept: TrialRecordPlan,
    /// The plan as the answer shows it.
    plan: Value,
}

impl TrialRun<'_> {
    /// Sends the trial's one run on `client`: the request `run start` sends, by its own function,
    /// for the package directory tried, waited for as `run start` waits (to its receipt, the run's
    /// deadline bounding it, or up to `wait_for`); then its end, read as `run show` reads it, is
    /// answered and recorded ([`Self::ended`]). Nothing is ever sent again: a wait that ends first
    /// is `command_wait_expired`, a Runtime that refused the run answers its refusal, and after a
    /// result that is not known (no receipt came back) the end is read on the same connection,
    /// which, lost, answers at once with the client's failure; each names the request id, and
    /// those whose run may still end name the line that reads it later.
    fn send(&self, client: &RuntimeClient, wait_for: Option<Duration>) -> CliOutcome<Value> {
        let mut params = Map::new();
        params.insert(
            "instance".to_owned(),
            Value::String(self.answer.plan.instance.clone()),
        );
        params.insert(
            "package".to_owned(),
            Value::String(self.answer.plan.package.clone()),
        );
        let prepared = run_start_prepared(&params, client).map_err(|exit| self.not_sent(&exit))?;
        let request_id = prepared.request_id();
        let correlation_id = prepared.correlation_id();
        let id = run_evidence::id_text(&request_id);
        let submitted = match wait_for {
            None => client.submit_prepared(prepared),
            Some(wait_for) => {
                let submitter = client.clone();
                match wait_at_most(wait_for, move || submitter.submit_prepared(prepared)) {
                    Waited::Done(submitted) => submitted,
                    Waited::Expired => {
                        let (_, outcome) = wait::expired(
                            ContractLocation::RunContainedTask,
                            wait_for,
                            Some(request_id),
                            Some(correlation_id),
                        );
                        return Err(self.unread(
                            &outcome,
                            format!(
                                "the trial's run {id} was sent and has not answered within \
                                 {wait_for:?}; it goes on in the Runtime and no record was \
                                 written: read its end later with the line in next"
                            ),
                            &id,
                            true,
                        ));
                    }
                }
            }
        };
        // A receipt without a terminal: the Runtime refused the run, which never ran.
        if let Err(error) = &submitted
            && error.committed_receipt().is_none()
            && error.received_receipt().is_some()
        {
            return Err(refusal(
                &error.outcome(),
                error.to_string(),
                self.members(true, Some(&id)),
                ErrorKind::DeviceInstance,
            ));
        }
        // Otherwise the end is read: after a receipt, and after a result that is not known (no
        // receipt came back), on the same connection, which, lost, answers the read with the
        // client's failure at once and sends nothing.
        let run = run_show::read(client, RunKey::RequestId(request_id))
            .map_err(|error| self.unread(&error.outcome(), error.to_string(), &id, true))?;
        self.ended(client, &id, request_id, run, true)
    }

    /// Workflow #463 S3-11: `--collect <request_id>`: sends nothing; reads the run `request_id` as
    /// `run show` reads it, after one status read that resolves the trial's instance; a run whose
    /// facts do not name the trial's package or its instance is `trial_collect_mismatch`; then its
    /// end is answered and recorded as a sent run's is ([`Self::ended`]). Fix round 2: so is a run
    /// that this command cannot have sent ([`sent_otherwise`]): the scheduler's, one whose request's
    /// origin is not recorded, or one sent by hand on another channel than the Lab's. An ordinary
    /// `run start` of the same package on the Lab's channel is not told apart: nothing the run's
    /// facts hold names the command that sent it.
    fn collect(
        &self,
        client: &RuntimeClient,
        id: &str,
        request_id: RequestId,
    ) -> CliOutcome<Value> {
        let plan = &self.answer.plan;
        let relayed = |error: RuntimeClientError| {
            refusal(
                &error.outcome(),
                error.to_string(),
                self.members(false, Some(id)),
                ErrorKind::DeviceInstance,
            )
        };
        let status = client.status().map_err(relayed)?;
        let instance =
            instance_selector::select(status.instances(), &plan.instance).map_err(|miss| {
                refusal(
                    &miss.outcome(&plan.instance),
                    format!(
                        "the Runtime's status names no single instance {}",
                        plan.instance
                    ),
                    self.members(false, Some(id)),
                    ErrorKind::DeviceInstance,
                )
            })?;
        let run = run_show::read(client, RunKey::RequestId(request_id))
            .map_err(|error| self.unread(&error.outcome(), error.to_string(), id, false))?;
        if run.state != ContainedRunState::NotFound {
            let problem = if run.package_label.as_deref() != Some(plan.package_id.as_str()) {
                Some(TrialCollectProblem::OtherPackage)
            } else if run.instance_id != Some(instance.instance_id()) {
                Some(TrialCollectProblem::OtherInstance)
            } else {
                sent_otherwise(run.dispatch, run.origin)
            };
            if let Some(problem) = problem {
                return Err(refusal(
                    &Outcome::new(ResourceToolingCode::TrialCollectMismatch)
                        .request_id(id)
                        .trial_collect_problem(problem),
                    format!(
                        "the run {id} is not a trial's run of {} on {} ({}): give --collect the \
                         request id this trial's own answer named",
                        plan.package_id,
                        plan.instance,
                        problem.as_str()
                    ),
                    self.members(false, Some(id)),
                    ErrorKind::UsageValidation,
                ));
            }
        }
        self.ended(client, id, request_id, run, false)
    }

    /// Workflow #463 S3-11: the end of the run `id` as `run show` read it (`run`): with its terminal
    /// and the time the ledger recorded that terminal (one read of that record by its request and
    /// its sequence: the time names the record, so a run is recorded under one name), the record
    /// is written once by the resource tooling's writer and the answer is `{sent, plan,
    /// request_id, run_id?, end: {state, outcome_key?, final_page?, failure_code?,
    /// failure_values?}, expected?, matched?, record, part_left?, warnings?}`. A run that ended
    /// normally succeeds (another end than `--expect` named is `matched` false with the warning
    /// `trial_end_unexpected`); a failed or cancelled run is refused with its own code and values
    /// beside the same members, as `run start` answers a failed run. Without a terminal, or
    /// without its record, nothing is written ([`Self::no_end`], [`Self::terminal_unrecorded`]).
    /// A refused write answers the writer's code beside the same members.
    ///
    /// Fix round 2: one run, one record: before the writer, a record of the run `id` among the
    /// package's records, whatever content it names, is `trial_record_exists` naming it, and
    /// nothing is written. A refusal of that reading or of the writer carries `next`, since the run
    /// happened and only its record is missing. Whether the run failed is read from its end state as
    /// `run show` names it, never from whether its terminal names a code: a failed or cancelled run
    /// whose terminal names none is `trial_end_uncoded`, its record written.
    fn ended(
        &self,
        client: &RuntimeClient,
        id: &str,
        request_id: RequestId,
        run: ContainedRunStatus,
        sent: bool,
    ) -> CliOutcome<Value> {
        let state = run.state;
        let Some(terminal) = run.terminal else {
            return Err(self.no_end(id, state, sent));
        };
        let recorded = client
            .query_events(
                EventQuery {
                    request_id: Some(request_id),
                    from_sequence: Some(terminal.sequence),
                    to_sequence: Some(terminal.sequence),
                    ..EventQuery::default()
                },
                ProjectionProfile::Forensic,
            )
            .map_err(|error| self.unread(&error.outcome(), error.to_string(), id, sent))?;
        let Some(at_unix_ms) = recorded
            .iter()
            .find(|event| event.sequence == terminal.sequence)
            .map(|event| event.timestamp_unix_ms)
        else {
            return Err(self.terminal_unrecorded(id, state, sent));
        };
        let record = PackTrialRecord::of_run(
            self.kept.clone(),
            at_unix_ms,
            id.to_owned(),
            run.run_id.map(|run_id| run_evidence::id_text(&run_id)),
            TrialRunEnd {
                outcome: terminal.outcome,
                outcome_key: terminal.outcome_key,
                final_page: terminal.final_page,
                failure_code: terminal.failure_code,
                failure_values: terminal.failure_values,
            },
        );
        let mut members = self.members(sent, Some(id));
        if let Some(run_id) = &record.run_id {
            members.insert("run_id".to_owned(), json!(run_id));
        }
        members.insert("end".to_owned(), end_answer(state, &record.end));
        if let Some(expected) = &record.plan.expect {
            members.insert("expected".to_owned(), json!(expected));
        }
        if let Some(matched) = record.matched {
            members.insert("matched".to_owned(), json!(matched));
        }
        let mut warnings = record.unexpected_end().into_iter().collect::<Vec<_>>();
        let state_dir = &self.request.state_dir;
        match trial_record_of_run(state_dir, &record.plan.package_id, id) {
            Ok(None) => {}
            Ok(Some(kept)) => {
                return Err(refusal(
                    &Outcome::new(ResourceToolingCode::TrialRecordExists).path(&kept),
                    format!(
                        "the run {id} is recorded already, in {}: a run has one record, whatever \
                         the package directory holds now; nothing was written",
                        kept.display()
                    ),
                    members,
                    ErrorKind::UsageValidation,
                ));
            }
            Err(error) => return Err(self.not_recorded(error, members, id)),
        }
        let written = match write_trial_record(state_dir, &record) {
            Ok(written) => written,
            Err(error) => return Err(self.not_recorded(error, members, id)),
        };
        members.insert(
            "record".to_owned(),
            Value::String(written.path.display().to_string()),
        );
        if let Some(part) = &written.part_left {
            members.insert(
                "part_left".to_owned(),
                Value::String(part.display().to_string()),
            );
        }
        warnings.extend(written.warnings);
        if !warnings.is_empty() {
            members.insert("warnings".to_owned(), json!(warnings));
        }
        if state == ContainedRunState::Succeeded {
            return Ok(Value::Object(members));
        }
        let Some(failed) = &record.end.failure_code else {
            let word = run_state_word(state);
            return Err(refusal(
                &Outcome::new(ResourceToolingCode::TrialEndUncoded)
                    .request_id(id)
                    .run_state(word),
                format!(
                    "the trial's run {id} ended {} and its terminal names no failure code; its \
                     record is {}",
                    word.as_str(),
                    written.path.display()
                ),
                members,
                ErrorKind::DeviceInstance,
            ));
        };
        members.insert(
            "outcome".to_owned(),
            json!({"code": failed, "values": record.end.failure_values}),
        );
        Err(CliError::new(
            ErrorKind::DeviceInstance,
            failed.as_str(),
            format!(
                "the trial's run {id} ended {} with {}; its record is {}",
                run_state_word(state).as_str(),
                failed.as_str(),
                written.path.display()
            ),
            &[],
        )
        .with_details(Value::Object(members)))
    }

    /// The members every answer of the run carries: `sent`, `plan`, and the run's `request_id`
    /// once it has one.
    fn members(&self, sent: bool, request_id: Option<&str>) -> Map<String, Value> {
        let mut members = Map::new();
        members.insert("sent".to_owned(), Value::Bool(sent));
        members.insert("plan".to_owned(), self.plan.clone());
        if let Some(request_id) = request_id {
            members.insert(
                "request_id".to_owned(),
                Value::String(request_id.to_owned()),
            );
        }
        members
    }

    /// The refusal `outcome` (`message`) of a run `id` whose end was not read, so that no record
    /// was written: the members and `next`, the line that reads the end later.
    fn unread(&self, outcome: &Outcome, message: String, id: &str, sent: bool) -> CliError {
        let mut members = self.members(sent, Some(id));
        members.insert("next".to_owned(), self.collect_line(id));
        refusal(outcome, message, members, ErrorKind::DeviceInstance)
    }

    /// The run `id` read in `state` without a terminal, so that no record was written (fix round 2:
    /// each cause its own code, as the next step differs). A run interrupted by a later Runtime
    /// start or takeover (`interrupted_unterminated`) is `trial_run_interrupted`, without `next`:
    /// it has no end to record, and nothing this command does gives it one. Any other state
    /// (`not_found`, `admitted`, `running`) is a run that has not ended yet, or has no fact yet:
    /// `trial_end_unread` with `run_state` and `next`, the line that reads it again later.
    fn no_end(&self, id: &str, state: ContainedRunState, sent: bool) -> CliError {
        let word = run_state_word(state);
        let mut error = match state {
            ContainedRunState::InterruptedUnterminated => refusal(
                &Outcome::new(ResourceToolingCode::TrialRunInterrupted).request_id(id),
                format!(
                    "the run {id} was interrupted: the Runtime started or took over after its first \
                     fact and it has no terminal, so it has no end to record; no record was written"
                ),
                self.members(sent, Some(id)),
                ErrorKind::DeviceInstance,
            ),
            _ => self.unread(
                &Outcome::new(ResourceToolingCode::TrialEndUnread)
                    .request_id(id)
                    .run_state(word),
                format!(
                    "the run {id} reads {} and has not ended yet: no record was written; read it \
                     again later with the line in next",
                    word.as_str()
                ),
                id,
                sent,
            ),
        };
        if let Some(Value::Object(details)) = error.details.as_mut() {
            details.insert("end".to_owned(), json!({"state": state}));
        }
        error
    }

    /// Fix round 2: `trial_terminal_unrecorded`: the run `id` read in `state` names its terminal,
    /// but the read of the ledger's record of that terminal, by its request and its sequence, did
    /// not find it, so no record was written; the next step is to look at the run's facts in the
    /// ledger, so the answer carries no `next`.
    fn terminal_unrecorded(&self, id: &str, state: ContainedRunState, sent: bool) -> CliError {
        let word = run_state_word(state);
        let mut members = self.members(sent, Some(id));
        members.insert("end".to_owned(), json!({"state": state}));
        refusal(
            &Outcome::new(ResourceToolingCode::TrialTerminalUnrecorded)
                .request_id(id)
                .run_state(word),
            format!(
                "the run {id} reads {} with a terminal, but the ledger's record of that terminal was \
                 not found at its sequence: no record was written; look at the run's facts in the \
                 ledger",
                word.as_str()
            ),
            members,
            ErrorKind::DeviceInstance,
        )
    }

    /// Fix round 2: the refusal `error` of the reading of the package's records or of the writer,
    /// beside `members` and `next`, the line that records the run `id` later: the run happened and
    /// only its record is missing.
    fn not_recorded(&self, error: CliError, mut members: Map<String, Value>, id: &str) -> CliError {
        members.insert("next".to_owned(), self.collect_line(id));
        beside(error, members)
    }

    /// The refusal of a run that was not sent: the line `run start`'s own function answered
    /// (`exit`), its outcome on top, beside the plan.
    fn not_sent(&self, exit: &Exit) -> CliError {
        let outcome = match &exit.line {
            Ok(line) => line.get("outcome").cloned(),
            Err(_) => None,
        }
        .unwrap_or_else(|| cli_line::outcome_value(&Exit::output_failed(), Detail::Full));
        let message = exit
            .developer
            .as_ref()
            .and_then(|developer| developer.text.clone())
            .or_else(|| exit.line.as_ref().err().cloned())
            .unwrap_or_else(|| "the trial's run was not sent".to_owned());
        let code = outcome
            .get("code")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| Exit::output_failed().code().as_str().to_owned());
        let mut members = self.members(false, None);
        members.insert("outcome".to_owned(), outcome);
        CliError::new(ErrorKind::DeviceInstance, code, message, &[])
            .with_details(Value::Object(members))
    }

    /// The line that reads the end of the run `request_id` later and records it, as the words of
    /// actinglab's command line after the program's name: the trial's own arguments with
    /// `--collect <request_id>` (`{collect: [...]}`).
    fn collect_line(&self, request_id: &str) -> Value {
        let plan = &self.answer.plan;
        let mut words = vec![
            "standard-pack".to_owned(),
            "trial".to_owned(),
            "--instance".to_owned(),
            plan.instance.clone(),
            "--package".to_owned(),
            plan.package.clone(),
            "--authorization".to_owned(),
            self.kept.authorization.clone(),
        ];
        if let Some(expected) = &plan.expect {
            words.extend(["--expect".to_owned(), expected.clone()]);
        }
        if plan.irreversible_acknowledged {
            words.push("--irreversible".to_owned());
        }
        if let Some(root) = &self.request.root {
            words.extend(["--root".to_owned(), root.display().to_string()]);
        }
        words.extend([
            "--state-dir".to_owned(),
            self.request.state_dir.display().to_string(),
            COLLECT_FLAG.to_owned(),
            request_id.to_owned(),
        ]);
        json!({"collect": words})
    }
}

/// Workflow #463 S3-11: the refusal of a trial's run with `outcome` on top (its code the error's
/// code, its envelope the details' `outcome`, as actinglab's line reads it) and `members` beside
/// it, of the class `class`.
fn refusal(
    outcome: &Outcome,
    message: String,
    mut members: Map<String, Value>,
    class: ErrorKind,
) -> CliError {
    members.insert(
        "outcome".to_owned(),
        cli_line::outcome_value(outcome, Detail::Full),
    );
    CliError::new(class, outcome.code().as_str(), message, &[]).with_details(Value::Object(members))
}

/// Workflow #463 S3-11: the refusal `error` of the resource tooling (the writer's) with `members`
/// beside its own details, which keep its outcome.
fn beside(mut error: CliError, members: Map<String, Value>) -> CliError {
    let mut details = match error.details.take() {
        Some(Value::Object(details)) => details,
        _ => Map::new(),
    };
    for (name, value) in members {
        details.entry(name).or_insert(value);
    }
    error.with_details(Value::Object(details))
}

/// Workflow #463 S3-11: the end a trial's answer shows: the run's state as `run show` names it and
/// its terminal's `outcome_key`, `final_page`, `failure_code` and `failure_values`, each when it
/// has one.
fn end_answer(state: ContainedRunState, end: &TrialRunEnd) -> Value {
    let mut answer = Map::new();
    answer.insert("state".to_owned(), json!(state));
    if let Some(outcome_key) = &end.outcome_key {
        answer.insert("outcome_key".to_owned(), json!(outcome_key));
    }
    if let Some(final_page) = &end.final_page {
        answer.insert("final_page".to_owned(), json!(final_page));
    }
    if let Some(failure_code) = &end.failure_code {
        answer.insert("failure_code".to_owned(), json!(failure_code));
    }
    if !end.failure_values.is_empty() {
        answer.insert(
            "failure_values".to_owned(),
            Value::Object(end.failure_values.clone()),
        );
    }
    Value::Object(answer)
}

/// Workflow #463 S3-11 fix round 2: why a run of the trial's package on its instance was not sent by
/// this command, as far as its facts tell (`dispatch` and `origin`, which its request's origin
/// record gives); `None` for a run sent by hand on the Lab's channel, as a trial's run is sent. An
/// ordinary `run start` on the Lab's channel answers `None` too: its facts name no command.
fn sent_otherwise(dispatch: RunDispatch, origin: RunOrigin) -> Option<TrialCollectProblem> {
    match (dispatch, origin) {
        (RunDispatch::Manual, RunOrigin::Lab) => None,
        (RunDispatch::Scheduled, _) => Some(TrialCollectProblem::Scheduled),
        (RunDispatch::Unknown, _) => Some(TrialCollectProblem::OriginUnknown),
        (RunDispatch::Manual, _) => Some(TrialCollectProblem::OtherChannel),
    }
}

/// Workflow #463 S3-11: a run's state as the word of the vocabulary `run_state`, which names the
/// same states.
fn run_state_word(state: ContainedRunState) -> RunState {
    match state {
        ContainedRunState::NotFound => RunState::NotFound,
        ContainedRunState::Admitted => RunState::Admitted,
        ContainedRunState::Running => RunState::Running,
        ContainedRunState::Succeeded => RunState::Succeeded,
        ContainedRunState::Failed => RunState::Failed,
        ContainedRunState::Cancelled => RunState::Cancelled,
        ContainedRunState::InterruptedUnterminated => RunState::InterruptedUnterminated,
    }
}

/// Workflow #463 S3-10: the refusal of the word `word` of a `standard-pack trial` line, as the CLI
/// module refuses a row's short words (`validation_failed` with `argument` and
/// `argument_problem`).
fn trial_refused(word: &str, problem: ArgumentProblem) -> CliError {
    crate::cli_module::refused_word(TRIAL, word, problem)
}

/// Workflow #463 S3-10: the value of the option `flag`, `None` when it is not given; given without
/// a value, or with a blank one, it is refused `value_missing`.
fn trial_value(flags: &FlagArgs, flag: &str) -> CliOutcome<Option<String>> {
    match flags.optional(flag) {
        None => Ok(None),
        Some(value) if value == "true" || value.trim().is_empty() => {
            Err(trial_refused(flag, ArgumentProblem::ValueMissing))
        }
        Some(value) => Ok(Some(value)),
    }
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
