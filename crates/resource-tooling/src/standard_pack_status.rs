// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 S3-14 (register §1.446; `contracts/standard-pack-schedule.md`, "Where a pack
//! stands: `standard-pack status`"; the design `rd-1115/pack-stage3-design.md` §2.3, "One read for
//! one package and for a whole game root", and §4.2 "S3-14"): one read that says, for each pack of
//! a game root, where it stands, and the counts of the root. It reads only, offline: the game root
//! (by the readiness), the pack directories (their content), the Lab's state directory (the trial
//! records) and, with `--repo`, the checkout's pack directories. It writes nothing, starts nothing
//! and reads no installation and no Runtime.
//!
//! - **Formed**: `admitted`, whether the readiness item S1 holds; `content_hash_prefix`, the first
//!   [`CONTENT_PREFIX_DIGITS`] hex digits of the content hash the containment crate's
//!   `measure_content_hash` computes for the pack's directory, the hash and the prefix the trial
//!   names a content by.
//! - **Conditions**: the pack's thirteen readiness items as `standard-pack ready` answers them, from
//!   the one function that judges them (`judge_readiness_and_placement`), never a copy; **missing**,
//!   each item that fails, the S2 of a held pack with the hold's reason and reference.
//! - **Trial**: the latest record of this content among the trial records the trial itself keeps
//!   (the trial's own reader, `latest_trial_record`): `passed`, `failed` or `none`.
//! - **Enabled**: whether the root's `schedule.json` enables the pack, judged from the
//!   classification the readiness's S2 reads; and the authorization: the placement journals belong
//!   to the Lab's rescue module, whose reader this module cannot reach, so every pack answers
//!   `not_recorded` with `journals_not_read`. The tool never infers an authorization.
//! - **Stage**: `not_admitted`, `can_be_tried` or `enabled`.

use crate::codes::{
    AuthorizationUnrecorded, EnabledState, PackAuthorization, ReadinessItem, StageState, TrialState,
};
use crate::package_directory::{PACK_DEADLINE, pack_error};
use crate::standard_pack_ready::{
    ItemState, PackPlacement, PackReadiness, ReadinessAnswer, ReadinessHold,
    StandardPackReadyRequest, judge_readiness_and_placement,
};
use crate::standard_pack_trial::{
    CONTENT_PREFIX_DIGITS, PackTrialRecord, TrialRunEnd, latest_trial_record,
};
use actingcommand_contract::{LabError, LabResult, TaskOutcome};
use actingcommand_pack_containment::{Containment, ContainmentError};
use serde::{Serialize, Serializer};
use serde_json::json;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The directory of a checkout's game working root that holds its packs, one directory per
/// package id (`contracts/standard-pack-authoring.md`).
const CHECKOUT_PACKS: &str = "packs";
/// The flag a refusal of an unknown package id names, as the command line writes it.
const PACKAGE_ID_FLAG: &str = "--package-id";
/// The argument problem of a value that names nothing of its kind (the vocabulary
/// `argument_problem`).
const NOT_RECOGNISED: &str = "not_recognised";

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
    /// One row per pack, or the one `package_id` names: the part of the answer that grows with
    /// the root, where it may be cut.
    pub packs: Vec<PackStatus>,
}

/// The packs of the root at each `stage_state`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StageStateCounts {
    pub not_admitted: usize,
    pub can_be_tried: usize,
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
    /// The latest trial of this content; absent when the content could not be read (then the
    /// pack's S1 fails with the loader's refusal of the same directory).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trial: Option<PackTrial>,
    pub enabled: PackEnabled,
}

/// Whether a pack is formed, and its content.
#[derive(Debug, Clone, Serialize)]
pub struct PackFormed {
    /// Whether its readiness item S1 holds.
    pub admitted: bool,
    /// The first [`CONTENT_PREFIX_DIGITS`] hex digits of its directory's content hash; absent
    /// when the directory could not be read and S1 fails.
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

/// The latest trial of a pack's content, as its record keeps it.
#[derive(Debug, Clone, Serialize)]
pub struct PackTrial {
    pub state: TrialState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// How its run ended: the business end is its `outcome_key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<TrialRunEnd>,
    /// Whether the end was the one the trial expected; absent when it expected none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
    /// When its run ended, as the record says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_unix_ms: Option<u64>,
}

/// Whether the root's schedule enables a pack, and under which authorization.
#[derive(Debug, Clone, Serialize)]
pub struct PackEnabled {
    pub state: EnabledState,
    pub authorization: PackAuthorization,
    pub authorization_unrecorded: AuthorizationUnrecorded,
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

impl Serialize for TrialState {
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

impl StageStateCounts {
    fn count(&mut self, state: StageState) {
        let counted = match state {
            StageState::NotAdmitted => &mut self.not_admitted,
            StageState::CanBeTried => &mut self.can_be_tried,
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

impl PackTrial {
    /// The trial of a content whose latest record is `record`: `passed` when its run ended
    /// normally, on whichever end, `failed` when it failed or was cancelled; `none` without one.
    fn of(record: Option<PackTrialRecord>) -> Self {
        let Some(record) = record else {
            return Self {
                state: TrialState::NotTried,
                request_id: None,
                end: None,
                matched: None,
                at_unix_ms: None,
            };
        };
        let state = if record.end.outcome == TaskOutcome::Success {
            TrialState::Passed
        } else {
            TrialState::Failed
        };
        Self {
            state,
            request_id: Some(record.request_id),
            end: Some(record.end),
            matched: record.matched,
            at_unix_ms: Some(record.at_unix_ms),
        }
    }
}

/// Workflow #463 S3-14 (module doc): where each pack of the game root `request.root` stands, or
/// the one `request.package_id` names, and the counts of the root. The readiness's refusals of the
/// root are this read's; then a `package_id` the root does not register is `validation_failed`
/// naming `--package-id` (`not_recognised`), a `--repo` that is no directory `validation_failed`,
/// a pack directory or a checkout's pack directory the containment crate cannot read
/// `package_invalid` (but a pack whose S1 fails: its content is then not answered), and a trial
/// record that does not read as one `trial_record_unreadable`, as the trial's reader refuses it.
pub fn standard_pack_status(request: &StandardPackStatusRequest) -> LabResult<StandardPackStatus> {
    let (readiness, placements) = judge_readiness_and_placement(&StandardPackReadyRequest {
        root: request.root.clone(),
        declared_package: None,
        installation: None,
    })?;
    if let Some(named) = &request.package_id
        && !readiness.packs.iter().any(|pack| pack.package_id == *named)
    {
        return Err(unknown_package_id(named, &readiness.game));
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
        packs.push(row.status(&state_dir, repo.as_deref())?);
    }
    Ok(StandardPackStatus {
        root: readiness.root,
        game: readiness.game,
        state_dir: state_dir.display().to_string(),
        repo: repo.map(|checkout| checkout.display().to_string()),
        stage_states,
        enabled_states,
        packs,
    })
}

/// A pack's stage: `not_admitted` while S1 does not hold, else `enabled` when the root's schedule
/// enables it, else `can_be_tried`.
fn stage_of(admitted: bool, enabled: EnabledState) -> StageState {
    if !admitted {
        StageState::NotAdmitted
    } else if enabled == EnabledState::Enabled {
        StageState::Enabled
    } else {
        StageState::CanBeTried
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
    /// items, the failing ones, the latest trial of its content under `state_dir`, and whether the
    /// root's schedule enables it.
    fn status(&self, state_dir: &Path, repo: Option<&Path>) -> LabResult<PackStatus> {
        let pack = self.pack;
        let directory = &self.placement.directory;
        let content = match content_prefix(directory) {
            Ok(prefix) => Some(prefix),
            // The loader reads the same directory by the same reader and refused it: S1 says why.
            Err(_) if !self.admitted => None,
            Err(error) => return Err(pack_error(directory, error)),
        };
        let trial = content
            .as_deref()
            .map(|prefix| latest_trial_record(state_dir, &pack.package_id, prefix))
            .transpose()?
            .map(PackTrial::of);
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
            trial,
            enabled: PackEnabled {
                state: self.placement.enabled,
                authorization: PackAuthorization::NotRecorded,
                authorization_unrecorded: AuthorizationUnrecorded::JournalsNotRead,
            },
        })
    }
}

/// The content hash prefix of the directory `directory`, as the trial names a content: the first
/// [`CONTENT_PREFIX_DIGITS`] hex digits of the hash the containment crate's `measure_content_hash`
/// computes, the hash the trial's plan computes from its one reading.
fn content_prefix(directory: &Path) -> Result<String, ContainmentError> {
    let hash =
        Containment::new().measure_content_hash(directory, Instant::now() + PACK_DEADLINE)?;
    Ok(hash.chars().take(CONTENT_PREFIX_DIGITS).collect())
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

/// `validation_failed` naming `--package-id` with `not_recognised`: the root of `game` registers
/// no pack `package_id`.
fn unknown_package_id(package_id: &str, game: &str) -> LabError {
    LabError::usage(format!(
        "{PACKAGE_ID_FLAG} {package_id:?} is no pack of the game root of {game}"
    ))
    .with_details(json!({
        "argument": PACKAGE_ID_FLAG,
        "argument_problem": NOT_RECOGNISED,
        "arguments": [{"argument": PACKAGE_ID_FLAG, "argument_problem": NOT_RECOGNISED}],
    }))
}

fn absolute(path: &Path) -> LabResult<PathBuf> {
    std::path::absolute(path)
        .map_err(|error| LabError::usage(format!("{}: {error}", path.display())))
}
