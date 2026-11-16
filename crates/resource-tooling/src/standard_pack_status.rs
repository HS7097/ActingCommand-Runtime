// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 S3-14 (register §1.446; `contracts/standard-pack-schedule.md`, "Where a pack
//! stands: `standard-pack status`"; the design `rd-1115/pack-stage3-design.md` §2.3, "One read for
//! one package and for a whole game root", and §4.2 "S3-14"): one read that says, for each pack of
//! a game root, where it stands, and the counts of the root. It reads only, offline: the game root
//! (by the readiness), the pack directories (their content), the Lab's state directory (the trial
//! records) and, with `--repo`, the checkout's pack directories. It writes nothing, starts nothing
//! and reads no installation and no Runtime. The train of S3-12 and S3-14 (register §1.446,
//! "Train"; the coordinator's rulings R1 to R6) made its trial and its content the trial module's
//! and said its words only of what the row knows.
//!
//! - **Formed**: `admitted`, whether the readiness item S1 holds; `content_hash_prefix`, the
//!   content identity the trial names a content by, by the trial module's one function
//!   (`content_hash_prefix_of`) over the pack's directory read as the trial reads one.
//! - **Conditions**: the pack's thirteen readiness items as `standard-pack ready` answers them, from
//!   the one function that judges them (`judge_readiness_and_placement`), never a copy; **missing**,
//!   each item that fails, the S2 of a held pack with the hold's reason and reference; **held**, the
//!   pack's hold whenever the root's `schedule.json` holds it, a pack in a supporting role too.
//! - **Trial**: the trial module's one summary of this content's trials (`trial_summary`, each
//!   record judged by `PackTrialRecord::passed`): `passed`, `failed` or `none`, the latest record and
//!   whether any record passed; a pack whose records the reader refuses answers `trial_not_read`
//!   instead, and the other packs are answered.
//! - **Enabled**: whether the root's `schedule.json` enables the pack, judged from the
//!   classification the readiness's S2 reads; and the authorization: without an installation's
//!   reading the placement journals are not read, so every pack answers `not_recorded` with
//!   `journals_not_read`. The tool never infers an authorization.
//! - **Stage**: `not_admitted`, `not_judged`, `can_be_tried`, `supporting` or `enabled`.
//!
//! Workflow #463 S3-15a (register §1.451; the design §2.3 and §4.2 "S3-15", its half of the
//! journals and the pairs): with what `--installation` read of the installation actinglab runs
//! from ([`StatusInstallation`], handed to [`standard_pack_status_with`]; this crate reads no
//! installation itself), a pack's `enabled.authorization` is the placement that last enabled it
//! (the rescue module's one read of its journals) or `not_recorded` with one cause word, each pack
//! answers its `pairs` (each pair of its catalog tasks and an instance that the installation's
//! `actingd suspended` names: its pause cause and its break-in row, mapped to the pack by the
//! composed bindings) or, in their place, the word saying why they were not read, and the root
//! answers `installation`, what was read of it. Without an installation the answer is S3-14's,
//! byte for byte. Fix round 1 (register §1.451, "Fix round 1"): the report's warning word goes in
//! the root's `installation.report_warnings`; the answer's `warnings` holds outcomes only, the
//! composition's warnings that concern the game; a pack the composed bindings bind no task to
//! answers its own word, `no_task_bound`.

use crate::codes::{
    AuthorizationUnrecorded, EnabledState, PackAuthorization, PairsUnread, ReadinessItem,
    ResourceToolingCode, StageState,
};
use crate::package_directory::{PACK_DEADLINE, pack_error};
use crate::standard_pack_ready::{
    ItemState, PackPlacement, PackReadiness, ReadinessAnswer, ReadinessHold,
    StandardPackReadyRequest, judge_readiness_and_placement,
};
use crate::standard_pack_trial::{
    NOT_RECOGNISED, TrialNotRead, TrialSummary, absolute, argument_refused, content_hash_prefix_of,
    trial_summary,
};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{LabError, LabErrorClass, LabResult};
use actingcommand_pack_containment::{Containment, ContainmentError};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The directory of a checkout's game working root that holds its packs, one directory per
/// package id (`contracts/standard-pack-authoring.md`).
const CHECKOUT_PACKS: &str = "packs";
/// The flag a refusal of an unknown package id names, as the command line writes it.
const PACKAGE_ID_FLAG: &str = "--package-id";

/// What `actinglab standard-pack status --root <game root> [--repo <checkout game root>]
/// [--package-id <id>] [--state-dir <dir>]` asks.
#[derive(Debug, Clone)]
pub struct StandardPackStatusRequest {
    /// The game root: an installed one or a bundle laid out as one, its directory named as its
    /// game.
    pub root: PathBuf,
    /// The game's working root in a resource repository's checkout, whose `packs/<package_id>`
    /// is measured beside each pack (`formed.checkout`).
    pub repo: Option<PathBuf>,
    /// The one pack answered; every pack of the root when `None`. The counts are the root's.
    pub package_id: Option<String>,
    /// The Lab's state directory, where the trial records are read.
    pub state_dir: PathBuf,
}

/// The answer: the root's counts and the row of each pack asked for, in the readiness's order.
#[derive(Debug, Clone, Serialize)]
pub struct StandardPackStatus {
    pub root: String,
    pub game: String,
    /// The state directory the trial records were read in, absolute.
    pub state_dir: String,
    /// `--repo`, absolute; absent without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// How many packs of the root stand at each `stage_state`.
    pub stage_states: StageStateCounts,
    /// How many packs of the root answer each `enabled.state`.
    pub enabled_states: EnabledStateCounts,
    /// Workflow #463 S3-15a: what was read of the installation; absent without `--installation`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<InstallationReading>,
    /// Workflow #463 S3-15a fix round 1: this row's warnings, each an outcome in its own shape, as
    /// every row's `warnings` holds them: beside pairs that were read, the warnings the reading of
    /// the composed bindings gave that concern the root's game ([`ComposedBindings::warnings`]).
    /// Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Outcome>,
    /// One row per pack, or the one `package_id` names: the part of the answer that grows with
    /// the root, where it may be cut.
    pub packs: Vec<PackStatus>,
}

/// Workflow #463 S3-15a: the root's part of an answer that read the installation.
#[derive(Debug, Clone, Serialize)]
pub struct InstallationReading {
    /// How many operation journals of the installation were read, of any kind.
    pub operations_read: usize,
    /// The ids of the operations whose journal could not be read, oldest first.
    pub operations_unread: Vec<String>,
    /// Whether the installation's `actingd suspended` gave its report.
    pub suspension_report_read: bool,
    /// Fix round 1: the words of the report's warnings the row passes on, as the report spells
    /// them: `ledger_read_incomplete` when its ledger read ran out of its deadline, so that its
    /// rows are what it read. Absent when the report gave none of them, or was not read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub report_warnings: Vec<String>,
    /// Why no pack answers its pairs, the word each pack's `pairs` answers in their place; absent
    /// when they are answered (fix round 1: a pack may still answer its own word,
    /// `no_task_bound`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs_unread: Option<PairsUnread>,
    /// Beside `pairs_unread`: the outcome that says why, the child's own (as a diagnosis gives
    /// it) or the bindings' reader's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs_unread_outcome: Option<Outcome>,
}

/// Workflow #463 S3-15a: what `standard-pack status --installation` read of the installation
/// actinglab runs from, handed to [`standard_pack_status_with`].
#[derive(Debug, Clone)]
pub struct StatusInstallation {
    pub journals: InstallationJournals,
    pub pairs: InstallationPairs,
}

/// Workflow #463 S3-15a: the placement journals of the installation, read for the root's game by
/// the rescue module's one read of them.
#[derive(Debug, Clone, Default)]
pub struct InstallationJournals {
    /// How many operation journals were read, of any kind.
    pub operations_read: usize,
    /// The ids of the operations whose journal could not be read, oldest first.
    pub operations_unread: Vec<String>,
    /// Per package id, the placement that last enabled it.
    pub enabled_by: BTreeMap<String, PlacementAuthorization>,
}

/// Workflow #463 S3-15a: the placement that last enabled a pack, as its `pack_place` journal
/// records it: the reference of the authorization it named, as given, the initiator that began it
/// (absent where the journal names none), when it finished and its operation id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlacementAuthorization {
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiator: Option<String>,
    pub at_unix_ms: u64,
    /// The operation's id (`op-<ms>`), the member `operation`.
    #[serde(rename = "operation")]
    pub operation_id: String,
}

/// Workflow #463 S3-15a: what the pairs are read from.
#[derive(Debug, Clone)]
pub enum InstallationPairs {
    /// The installation's one `actingd suspended` printed its report (`report`, whole, as it
    /// printed it), and the composed bindings (`bindings`), or the outcome of their reader's
    /// refusal.
    Read {
        report: Value,
        bindings: Result<ComposedBindings, Outcome>,
    },
    /// It gave no report: the outcome a diagnosis gives the same child.
    ReportNotRead(Outcome),
}

/// Workflow #463 S3-15a: the composed bindings of the installation, as `actingctl packs` reads
/// them.
#[derive(Debug, Clone, Default)]
pub struct ComposedBindings {
    /// Each catalog task id with the package id it runs.
    pub tasks: BTreeMap<String, String>,
    /// Fix round 1: the warnings that reading gave which concern the root's game (a game root it
    /// could not read or that the composition refused, a package file it could not read), as it
    /// gave them; the answer's `warnings` beside pairs that were read.
    pub warnings: Vec<Outcome>,
}

/// Workflow #463 S3-15a: the authorization a pack was enabled under: the placement that last
/// enabled it, or `not_recorded` (with `enabled.authorization_unrecorded` saying why).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum EnabledAuthorization {
    Recorded(PlacementAuthorization),
    Unrecorded(PackAuthorization),
}

/// Workflow #463 S3-15a: a pack's pairs, or the word saying why none are answered.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PackPairs {
    Read(Vec<PairStatus>),
    NotRead(PairsUnread),
}

/// Workflow #463 S3-15a: one pair of a pack, a catalog task that runs it and an instance, that the
/// installation's suspension report names.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairStatus {
    pub task_id: String,
    pub instance_id: String,
    /// The pair's row of the report's `suspended`: its `pause_cause`, absent where the row names
    /// none; absent when the pair is not suspended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended: Option<PairSuspended>,
    /// The pair's row of the report's `break_ins`, its members as the report names them (but the
    /// pair's `task_id` and `instance_id`, above); absent when the report has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub break_in: Option<Map<String, Value>>,
}

/// Workflow #463 S3-15a: a suspended pair, as the report's `suspended` row names its pause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairSuspended {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pause_cause: Option<String>,
}

/// The packs of the root at each `stage_state`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StageStateCounts {
    pub not_admitted: usize,
    pub not_judged: usize,
    pub can_be_tried: usize,
    pub supporting: usize,
    pub enabled: usize,
}

/// The packs of the root at each `enabled.state`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EnabledStateCounts {
    pub enabled: usize,
    pub held: usize,
    pub not_placed: usize,
    pub not_scheduled: usize,
    pub not_judged: usize,
}

/// Where one pack stands.
#[derive(Debug, Clone, Serialize)]
pub struct PackStatus {
    pub package_id: String,
    pub server: String,
    pub stage_state: StageState,
    pub formed: PackFormed,
    /// The thirteen readiness items, S1 to S7 then I1 to I6, as `standard-pack ready` answers
    /// them for the same root.
    pub conditions: Vec<ReadinessAnswer>,
    /// Each item of `conditions` that fails, in their order.
    pub missing: Vec<MissingItem>,
    /// The train of S3-12 and S3-14 (ruling R5): the pack's hold as the `held` entry of the root's
    /// `schedule.json` writes it, whenever the file the composer accepted holds the pack, whatever
    /// S2 answers: a held pack in a supporting role, whose S2 holds by its role, shows it here.
    /// Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<ReadinessHold>,
    /// The trials of this content (the trial module's one summary, ruling R2); absent when the
    /// content could not be read (then the pack's S1 fails with the loader's refusal of the same
    /// directory) or its records were not read ([`Self::trial_not_read`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial: Option<TrialSummary>,
    /// The train (ruling R4): why the pack's trial records were not read, as adoption answers it;
    /// the other packs are answered. Absent otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial_not_read: Option<TrialNotRead>,
    pub enabled: PackEnabled,
    /// Workflow #463 S3-15a: with `--installation`, the pack's pairs, or the word saying why none
    /// are answered (the root's `installation` says more); absent without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs: Option<PackPairs>,
}

/// Whether a pack is formed, and its content.
#[derive(Debug, Clone, Serialize)]
pub struct PackFormed {
    /// Whether its readiness item S1 holds.
    pub admitted: bool,
    /// Its directory's content identity as the trial names a content; absent when the directory
    /// could not be read and S1 fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash_prefix: Option<String>,
    /// With `--repo`: the checkout's directory of the pack and its content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout: Option<CheckoutContent>,
}

/// The checkout's directory of a pack, `<repo>/packs/<package_id>`, and its content.
#[derive(Debug, Clone, Serialize)]
pub struct CheckoutContent {
    pub pack: String,
    /// Its content hash prefix, measured as the root's pack is; `null` when there is none.
    pub content_hash_prefix: Option<String>,
}

/// One item that fails: its answer as the readiness gives it and, on the S2 of a held pack, the
/// hold's reason and reference as its `held` entry writes them.
#[derive(Debug, Clone, Serialize)]
pub struct MissingItem {
    #[serde(flatten)]
    pub answer: ReadinessAnswer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<ReadinessHold>,
}

/// Whether the root's schedule enables a pack, and under which authorization. Workflow #463
/// S3-15a: `authorization` the placement that last enabled the pack, whatever `state` says now,
/// or `not_recorded` beside `authorization_unrecorded`, which is absent when one is recorded.
#[derive(Debug, Clone, Serialize)]
pub struct PackEnabled {
    pub state: EnabledState,
    pub authorization: EnabledAuthorization,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_unrecorded: Option<AuthorizationUnrecorded>,
}

impl Serialize for StageState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for EnabledState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for PackAuthorization {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for AuthorizationUnrecorded {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl Serialize for PairsUnread {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl StageStateCounts {
    fn count(&mut self, state: StageState) {
        let counted = match state {
            StageState::NotAdmitted => &mut self.not_admitted,
            StageState::NotJudged => &mut self.not_judged,
            StageState::CanBeTried => &mut self.can_be_tried,
            StageState::Supporting => &mut self.supporting,
            StageState::Enabled => &mut self.enabled,
        };
        *counted += 1;
    }
}

impl EnabledStateCounts {
    fn count(&mut self, state: EnabledState) {
        let counted = match state {
            EnabledState::Enabled => &mut self.enabled,
            EnabledState::Held => &mut self.held,
            EnabledState::NotPlaced => &mut self.not_placed,
            EnabledState::NotScheduled => &mut self.not_scheduled,
            EnabledState::NotJudged => &mut self.not_judged,
        };
        *counted += 1;
    }
}

/// Workflow #463 S3-14 (module doc): where each pack of the game root `request.root` stands, or
/// the one `request.package_id` names, and the counts of the root. The readiness's refusals of the
/// root are this read's; then a `package_id` the root does not register is `validation_failed`
/// naming `--package-id` (`not_recognised`), a `--repo` that is no directory `validation_failed`,
/// and a pack directory or a checkout's pack directory the containment crate cannot read
/// `package_invalid` (but a pack whose S1 fails: its content is then not answered). The train of
/// S3-12 and S3-14 (ruling R4): a pack whose trial records the reader refuses answers
/// `trial_not_read`, and refuses nothing.
pub fn standard_pack_status(request: &StandardPackStatusRequest) -> LabResult<StandardPackStatus> {
    standard_pack_status_with(request, None)
}

/// Workflow #463 S3-15a (module doc): [`standard_pack_status`] with what `--installation` read of
/// the installation, `installation`: each pack's `enabled.authorization` from its journals, its
/// `pairs` from its suspension report and its composed bindings, and the root's `installation`
/// and (fix round 1) `warnings`, the composition's warnings that concern the game. `None` answers
/// S3-14's answer, byte for byte. Nothing more is refused.
pub fn standard_pack_status_with(
    request: &StandardPackStatusRequest,
    installation: Option<&StatusInstallation>,
) -> LabResult<StandardPackStatus> {
    let (readiness, placements) = judge_readiness_and_placement(&StandardPackReadyRequest {
        root: request.root.clone(),
        declared_package: None,
        installation: None,
    })?;
    if let Some(named) = &request.package_id
        && !readiness.packs.iter().any(|pack| pack.package_id == *named)
    {
        return Err(argument_refused(
            PACKAGE_ID_FLAG,
            NOT_RECOGNISED,
            format!(
                "{PACKAGE_ID_FLAG} {named:?} is no pack of the game root of {}",
                readiness.game
            ),
        ));
    }
    let state_dir = absolute(&request.state_dir)?;
    let repo = match &request.repo {
        None => None,
        Some(given) => {
            let checkout = absolute(given)?;
            if !fs::metadata(&checkout).is_ok_and(|metadata| metadata.is_dir()) {
                return Err(LabError::usage(format!(
                    "--repo {} is no directory",
                    checkout.display()
                )));
            }
            Some(checkout)
        }
    };
    let installed = installation.map(Installed::of);
    let mut stage_states = StageStateCounts::default();
    let mut enabled_states = EnabledStateCounts::default();
    let mut packs = Vec::new();
    for pack in &readiness.packs {
        let placement = placements.get(&pack.package_id).ok_or_else(|| {
            LabError::package_invalid(format!(
                "{} is no package of the readiness of {}",
                pack.package_id, readiness.root
            ))
        })?;
        let admitted = pack
            .item(ReadinessItem::Admitted)
            .is_some_and(|answer| answer.state == ItemState::Holds);
        let stage_state = stage_of(admitted, placement.enabled);
        stage_states.count(stage_state);
        enabled_states.count(placement.enabled);
        if request
            .package_id
            .as_ref()
            .is_some_and(|named| *named != pack.package_id)
        {
            continue;
        }
        let row = Row {
            pack,
            placement,
            admitted,
            stage_state,
        };
        packs.push(row.status(&state_dir, repo.as_deref(), installed.as_ref())?);
    }
    let (reading, warnings) = match installed {
        Some(installed) => (Some(installed.reading), installed.warnings),
        None => (None, Vec::new()),
    };
    Ok(StandardPackStatus {
        root: readiness.root,
        game: readiness.game,
        state_dir: state_dir.display().to_string(),
        repo: repo.map(|checkout| checkout.display().to_string()),
        stage_states,
        enabled_states,
        installation: reading,
        warnings,
        packs,
    })
}

/// Workflow #463 S3-15a: the suspension report's warning that its ledger read ran out of its
/// deadline (`contracts/policy-suspension.md`, "`actingd suspended`"), the one warning of the
/// report the answer passes on, as the report spells it (fix round 1: in the root's
/// `installation.report_warnings`).
const LEDGER_READ_INCOMPLETE: &str = "ledger_read_incomplete";

/// Workflow #463 S3-15a: each package's pairs, by package id; within a package by task, then
/// instance.
type FoundPairs = BTreeMap<String, BTreeMap<(String, String), PairStatus>>;

/// Workflow #463 S3-15a: the pairs of the installation's report by package, and (fix round 1) the
/// packages a composed task runs.
struct ReadPairs {
    by_package: BTreeMap<String, Vec<PairStatus>>,
    bound: BTreeSet<String>,
}

/// Workflow #463 S3-15a: what the answer says of an installation, prepared once for every row.
struct Installed<'a> {
    journals: &'a InstallationJournals,
    /// The pairs that were read, or the word saying why none are answered.
    pairs: Result<ReadPairs, PairsUnread>,
    reading: InstallationReading,
    /// Fix round 1: the row's warnings, the composition's that concern the game.
    warnings: Vec<Outcome>,
}

impl<'a> Installed<'a> {
    fn of(installation: &'a StatusInstallation) -> Self {
        let journals = &installation.journals;
        let (pairs, unread_outcome, report_read) = match &installation.pairs {
            InstallationPairs::ReportNotRead(outcome) => (
                Err(PairsUnread::SuspensionReportNotRead),
                Some(outcome.clone()),
                false,
            ),
            InstallationPairs::Read {
                report,
                bindings: Ok(bindings),
            } => (
                Ok(ReadPairs {
                    by_package: pairs_by_package(report, &bindings.tasks),
                    bound: bindings.tasks.values().cloned().collect(),
                }),
                None,
                true,
            ),
            InstallationPairs::Read {
                bindings: Err(outcome),
                ..
            } => (
                Err(PairsUnread::BindingsNotRead),
                Some(outcome.clone()),
                true,
            ),
        };
        let (passed_on, warnings) = match &installation.pairs {
            InstallationPairs::Read { report, bindings } => (
                report_warnings(report),
                bindings
                    .as_ref()
                    .map(|read| read.warnings.clone())
                    .unwrap_or_default(),
            ),
            InstallationPairs::ReportNotRead(_) => (Vec::new(), Vec::new()),
        };
        let reading = InstallationReading {
            operations_read: journals.operations_read,
            operations_unread: journals.operations_unread.clone(),
            suspension_report_read: report_read,
            report_warnings: passed_on,
            pairs_unread: pairs.as_ref().err().copied(),
            pairs_unread_outcome: unread_outcome,
        };
        Self {
            journals,
            pairs,
            reading,
            warnings,
        }
    }

    /// The authorization of the pack `package_id`: the placement that last enabled it; else
    /// `not_recorded`, because some journals could not be read (`journals_unreadable`) or because
    /// every one was read and none names it (`no_journal_names_it`).
    fn enabled(&self, package_id: &str) -> (EnabledAuthorization, Option<AuthorizationUnrecorded>) {
        let unrecorded = EnabledAuthorization::Unrecorded(PackAuthorization::NotRecorded);
        match self.journals.enabled_by.get(package_id) {
            Some(by) => (EnabledAuthorization::Recorded(by.clone()), None),
            None if self.journals.operations_unread.is_empty() => {
                (unrecorded, Some(AuthorizationUnrecorded::NoJournalNamesIt))
            }
            None => (
                unrecorded,
                Some(AuthorizationUnrecorded::JournalsUnreadable),
            ),
        }
    }

    /// The pairs of the pack `package_id` (none when the report names none of its tasks), or the
    /// word saying why none are answered: the root's, or (fix round 1) `no_task_bound` when the
    /// composed bindings hold no task that runs the pack.
    fn pairs(&self, package_id: &str) -> PackPairs {
        match &self.pairs {
            Ok(read) if !read.bound.contains(package_id) => {
                PackPairs::NotRead(PairsUnread::NoTaskBound)
            }
            Ok(read) => {
                PackPairs::Read(read.by_package.get(package_id).cloned().unwrap_or_default())
            }
            Err(unread) => PackPairs::NotRead(*unread),
        }
    }
}

/// Workflow #463 S3-15a: the report's warning [`LEDGER_READ_INCOMPLETE`], when its `warnings`
/// list it (fix round 1: the root's `installation.report_warnings`).
fn report_warnings(report: &Value) -> Vec<String> {
    report_rows(report, "warnings")
        .filter_map(Value::as_str)
        .filter(|warning| *warning == LEDGER_READ_INCOMPLETE)
        .map(str::to_owned)
        .collect()
}

/// The rows of the report's list `list`; none where it has no such list.
fn report_rows<'r>(report: &'r Value, list: &'static str) -> impl Iterator<Item = &'r Value> {
    report
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Workflow #463 S3-15a: the pairs of the report `report` by package: each row of its `suspended`
/// and of its `break_ins` whose catalog task (`task_id`) the composed bindings `bindings` bind to
/// a package, one pair per task and instance (`instance_id`), with the row's pause cause and its
/// break-in row. A row whose task no binding names belongs to no package of the installation's
/// composition and to none of this root's packs.
fn pairs_by_package(
    report: &Value,
    bindings: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<PairStatus>> {
    let mut found = FoundPairs::new();
    for row in report_rows(report, "suspended") {
        if let Some(pair) = pair_of(&mut found, row, bindings) {
            pair.suspended = Some(PairSuspended {
                pause_cause: row
                    .get("pause_cause")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    for row in report_rows(report, "break_ins") {
        if let Some(pair) = pair_of(&mut found, row, bindings) {
            let mut members = row.as_object().cloned().unwrap_or_default();
            members.remove("task_id");
            members.remove("instance_id");
            pair.break_in = Some(members);
        }
    }
    found
        .into_iter()
        .map(|(package_id, pairs)| (package_id, pairs.into_values().collect()))
        .collect()
}

/// The pair of the report row `row` in `found`, made when it is new; `None` for a row that names
/// no task or instance, or whose task no binding names.
fn pair_of<'f>(
    found: &'f mut FoundPairs,
    row: &Value,
    bindings: &BTreeMap<String, String>,
) -> Option<&'f mut PairStatus> {
    let task_id = row.get("task_id")?.as_str()?;
    let instance_id = row.get("instance_id")?.as_str()?;
    let package_id = bindings.get(task_id)?;
    Some(
        found
            .entry(package_id.clone())
            .or_default()
            .entry((task_id.to_owned(), instance_id.to_owned()))
            .or_insert_with(|| PairStatus {
                task_id: task_id.to_owned(),
                instance_id: instance_id.to_owned(),
                suspended: None,
                break_in: None,
            }),
    )
}

/// Workflow #463 S3-15a: the refusal of `--installation` beside a `--root` that is not the
/// installed game root of its game in the installation actinglab runs from (`installed`, where
/// the installation places one): `package_status_root_not_installed` naming the root given, before
/// the game root, a journal or the suspension report is read (fix round 1: and before the child is
/// started). Exit 1 on actinglab's command line, which prints the CLI module's line; in the
/// envelope a `--via` line prints, exit 2 (the class `usage_validation`).
pub fn status_root_not_installed(root: &Path, installed: Option<&Path>) -> LabError {
    let outcome = Outcome::new(ResourceToolingCode::PackageStatusRootNotInstalled).path(root);
    let message = match installed {
        Some(installed) => format!(
            "--root {} is not the installed game root of its game: the installation places it at {}",
            root.display(),
            installed.display()
        ),
        None => format!(
            "--root {} is no installed game root: the installation's configuration names no standard_packs.root, or the root names no game",
            root.display()
        ),
    };
    refusal(&outcome, message)
}

/// Workflow #463 S3-15a: the refusal of `--installation` by a build of actinglab without the
/// rescue module (it is built for Windows only): `package_status_installation_unbuilt`, before
/// anything is read. Exit 1 on actinglab's command line, which prints the CLI module's line; in the
/// envelope a `--via` line prints, exit 2 (the class `usage_validation`).
pub fn status_installation_unbuilt() -> LabError {
    refusal(
        &Outcome::new(ResourceToolingCode::PackageStatusInstallationUnbuilt),
        "--installation: this build of actinglab holds no rescue module (built for Windows only), so it reads no installation".to_owned(),
    )
}

/// The Lab error of the refusal `outcome`, its envelope in the details.
fn refusal(outcome: &Outcome, message: String) -> LabError {
    LabError::new(
        LabErrorClass::UsageValidation,
        outcome.code().as_str(),
        message,
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}

/// A pack's stage, saying only what the row knows (the train, ruling R5): `not_admitted` while S1
/// does not hold; else, by `enabled.state`, `not_judged` when the composer refuses the root's
/// `schedule.json`, `enabled` when a rule enables it, `supporting` when it plays a supporting part
/// other packs' runs use and no rule names it (`not_scheduled`), `can_be_tried` when it is held or
/// nothing places it.
fn stage_of(admitted: bool, enabled: EnabledState) -> StageState {
    if !admitted {
        return StageState::NotAdmitted;
    }
    match enabled {
        EnabledState::NotJudged => StageState::NotJudged,
        EnabledState::Enabled => StageState::Enabled,
        EnabledState::NotScheduled => StageState::Supporting,
        EnabledState::Held | EnabledState::NotPlaced => StageState::CanBeTried,
    }
}

/// One pack of the readiness and what the status read of it so far.
struct Row<'a> {
    pack: &'a PackReadiness,
    placement: &'a PackPlacement,
    admitted: bool,
    stage_state: StageState,
}

impl Row<'_> {
    /// The pack's row: its content (the root's directory, and the checkout's under `repo`), its
    /// items, the failing ones, its hold, the trials of its content under `state_dir`, and
    /// whether the root's schedule enables it. Workflow #463 S3-15a: with an installation's
    /// reading (`installed`), its authorization from the journals and its pairs.
    fn status(
        &self,
        state_dir: &Path,
        repo: Option<&Path>,
        installed: Option<&Installed<'_>>,
    ) -> LabResult<PackStatus> {
        let pack = self.pack;
        let directory = &self.placement.directory;
        let content = match content_prefix(directory) {
            Ok(prefix) => Some(prefix),
            // The loader reads the same directory by the same reader and refused it: S1 says why.
            Err(_) if !self.admitted => None,
            Err(error) => return Err(pack_error(directory, error)),
        };
        let (trial, trial_not_read) = match content.as_deref() {
            None => (None, None),
            Some(prefix) => match trial_summary(state_dir, &pack.package_id, prefix) {
                Ok(trial) => (Some(trial), None),
                Err(not_read) => (None, Some(not_read)),
            },
        };
        let checkout = repo
            .map(|checkout| checkout_content(checkout, &pack.package_id))
            .transpose()?;
        let missing = pack
            .items
            .iter()
            .filter(|answer| answer.state == ItemState::Fails)
            .map(|answer| MissingItem {
                answer: answer.clone(),
                held: if answer.item == ReadinessItem::Classified {
                    pack.held.clone()
                } else {
                    None
                },
            })
            .collect();
        let (authorization, authorization_unrecorded) = match installed {
            Some(installed) => installed.enabled(&pack.package_id),
            None => (
                EnabledAuthorization::Unrecorded(PackAuthorization::NotRecorded),
                Some(AuthorizationUnrecorded::JournalsNotRead),
            ),
        };
        Ok(PackStatus {
            package_id: pack.package_id.clone(),
            server: pack.server.clone(),
            stage_state: self.stage_state,
            formed: PackFormed {
                admitted: self.admitted,
                content_hash_prefix: content,
                checkout,
            },
            conditions: pack.items.clone(),
            missing,
            held: self.placement.held.clone(),
            trial,
            trial_not_read,
            enabled: PackEnabled {
                state: self.placement.enabled,
                authorization,
                authorization_unrecorded,
            },
            pairs: installed.map(|installed| installed.pairs(&pack.package_id)),
        })
    }
}

/// The content identity of the directory `directory`, as the trial names a content (the train,
/// ruling R3): the directory read as the trial reads the one it tries (containment's
/// `snapshot_content_directory`) and named by the trial module's `content_hash_prefix_of`.
fn content_prefix(directory: &Path) -> Result<String, ContainmentError> {
    let entries =
        Containment::new().snapshot_content_directory(directory, Instant::now() + PACK_DEADLINE)?;
    Ok(content_hash_prefix_of(&entries))
}

/// The checkout's directory of the pack `package_id` under the working root `repo` and its
/// content prefix, `None` when nothing is at that path; anything there the containment crate
/// cannot read as a directory is `package_invalid`.
fn checkout_content(repo: &Path, package_id: &str) -> LabResult<CheckoutContent> {
    let pack = repo.join(CHECKOUT_PACKS).join(package_id);
    let content_hash_prefix = match fs::symlink_metadata(&pack) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        _ => Some(content_prefix(&pack).map_err(|error| pack_error(&pack, error))?),
    };
    Ok(CheckoutContent {
        pack: pack.display().to_string(),
        content_hash_prefix,
    })
}
