// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S11 (`contracts/state-root-installation.md`): the state root remembers which
//! installation claimed it.
//!
//! An installation is identified by a number minted once into its root,
//! `<installation root>\install\identity.json`; a state root records the installation that
//! claimed it in `<state root>\installation.json`. Both are files outside the ledger, a
//! registered exception to "program state lives in tagged ledger records": the check reads them
//! before the ledger is opened. A writer of the state root started from an installation root
//! checks the record before it writes anything; a start claims, under the owner lock, and so does
//! `actingd claim-state-root` (Workflow #453 S12), the explicit act of a person that hands a state
//! root over ([`hand_over`]). A writer not started from an installation root reads, mints and
//! writes nothing here.

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{IoOp, Outcome, RawSource};
use actingcommand_contract::{
    FactScalar, FactValue, OriginModule, OwnerEpoch, RuntimeErrorCode, RuntimeFactRecord,
    RuntimeFactScope,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

/// The schema of an installation's identity file.
pub const INSTALLATION_IDENTITY_SCHEMA: &str = "actingcommand.installation-identity.v1";
/// The schema of a state root's installation record.
pub const STATE_ROOT_INSTALLATION_SCHEMA: &str = "actingcommand.state-root-installation.v1";
/// The tagged runtime fact a claim is mirrored into, once: one row with the claim's time and
/// who claimed, never the installation's number.
pub const STATE_ROOT_INSTALLATION_FACT_KEY: &str = "host.state_root_installation";
/// The state root's installation record, `<state root>\installation.json`.
pub const STATE_ROOT_INSTALLATION_FILE: &str = "installation.json";
/// The installer's folder of an installation root.
const INSTALL_FOLDER: &str = "install";
/// The installer's own state file: a folder whose `install` folder holds it is an installation
/// root.
const INSTALLER_STATE_FILE: &str = "state.json";
/// The installation's identity file, in [`INSTALL_FOLDER`].
const IDENTITY_FILE: &str = "identity.json";
/// The suffix of the file a write lands in before it is linked into place.
const PART_SUFFIX: &str = ".part";
/// The largest identity file or record this reads.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// The bytes of an installation's number.
const INSTALLATION_ID_BYTES: usize = 16;
/// Workflow #453 S12: the marker of a file the hand-over sets aside, `<file>.old-<unix ms>`, a
/// kept name (`install leftovers` lists the identity file's, `contracts/install-cli.md`).
const KEPT_MARKER: &str = ".old-";
/// Workflow #453 S12: the members of the fact's row a start compares with the record's claim, and
/// the optional member that names the claim before a hand-over.
const CLAIMED_AT_MEMBER: &str = "claimed_at_unix_ms";
const OWNER_EPOCH_MEMBER: &str = "claimed_by_owner_epoch";
const PREVIOUS_CLAIMED_AT_MEMBER: &str = "previous_claimed_at_unix_ms";

/// What the read-only check found of a state root for the installation a writer runs from: the
/// value of check-config's `state_root_installation`. A record of another installation, and a
/// record or identity file that cannot be read, are refusals and never values
/// (`state_root_installation_mismatch`, `state_root_installation_record_invalid`; the contract
/// calls the two states `claimed_by_other` and `record_invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateRootInstallation {
    /// The writer runs from no installation root: nothing was read.
    NotInstalled,
    /// The state root holds no record.
    Unclaimed,
    /// The record names this installation.
    ClaimedByThis,
}

impl StateRootInstallation {
    /// The state's word.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Unclaimed => "unclaimed",
            Self::ClaimedByThis => "claimed_by_this",
        }
    }
}

/// `<installation root>\install\identity.json`, written once by the first start of the
/// installation that finds none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallationIdentity {
    schema_version: String,
    /// 16 bytes from the operating system's random source, lowercase hexadecimal.
    installation_id: String,
    minted_at_unix_ms: u64,
    minted_by: MintedBy,
}

/// The process that minted an identity and the commit it was built from (`null` when the
/// build does not know it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MintedBy {
    pid: u32,
    build_commit: Option<String>,
}

/// `<state root>\installation.json`, written once by the start that claimed the state root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateRootRecord {
    schema_version: String,
    installation_id: String,
    claimed_at_unix_ms: u64,
    claimed_by: ClaimedBy,
}

/// The start that claimed a state root: its process, its owner epoch and the commit it was
/// built from (`null` when the build does not know it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimedBy {
    pid: u32,
    owner_epoch: OwnerEpoch,
    build_commit: Option<String>,
}

/// What a start's claim found or placed under the owner lock: the state root's record of this
/// installation as read back from the disk. Its values are the runtime fact
/// [`STATE_ROOT_INSTALLATION_FACT_KEY`], recorded once the ledger is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateRootClaim {
    pub(crate) claimed_at_unix_ms: u64,
    pub(crate) pid: u32,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) build_commit: Option<String>,
    /// This start placed the record (its daemon line `state_root_claimed` is this start's
    /// alone); `false` when the record was this installation's already.
    pub(crate) placed: bool,
}

impl StateRootClaim {
    fn of_record(record: StateRootRecord, placed: bool) -> Self {
        Self {
            claimed_at_unix_ms: record.claimed_at_unix_ms,
            pid: record.claimed_by.pid,
            owner_epoch: record.claimed_by.owner_epoch,
            build_commit: record.claimed_by.build_commit,
            placed,
        }
    }
}

/// What a start's claim works on: the installation and state roots, the start's owner epoch, the
/// claim's time and the commit the build was made from.
pub(crate) struct ClaimRequest<'a> {
    pub(crate) installation_root: &'a Path,
    pub(crate) state_root: &'a Path,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) at_unix_ms: u64,
    pub(crate) build_commit: Option<&'a str>,
    /// Test only: bytes written as the installation's identity file between the mint and its
    /// placement, as another configuration of the same installation minting first would.
    #[cfg(test)]
    pub(crate) identity_before_placement: Option<&'a [u8]>,
    /// Test only (fix round 2): every hard link of the claim fails with "unsupported", as on a
    /// file system without hard links, so each file is placed by the fallback.
    #[cfg(test)]
    pub(crate) hard_link_fails: bool,
}

/// Ruling 2 of #453 S11: the installation root of a writer that is not told one, the directory
/// of its `--config` file when that directory holds the installer's own `install\state.json`;
/// `None` otherwise (a Runtime not started from an installation root: CI's example
/// configuration, a developer's run).
pub fn installation_root_of_config(config_path: &Path) -> RuntimeHostResult<Option<PathBuf>> {
    let directory = config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    installation_root_at(directory)
}

/// `root` when it is an installation root (its `install\state.json` is a file), else `None`.
/// The installer's state file that cannot be inspected for another reason than its absence is
/// refused (`state_root_installation_record_invalid` with the file as `path`): whether the
/// writer belongs to an installation is then unknown.
pub fn installation_root_at(root: &Path) -> RuntimeHostResult<Option<PathBuf>> {
    let marker = root.join(INSTALL_FOLDER).join(INSTALLER_STATE_FILE);
    match fs::metadata(&marker) {
        Ok(metadata) => Ok(metadata.is_file().then(|| root.to_path_buf())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(record_invalid(
            HostLocation::CheckStateRootInstallation,
            &marker,
            Outcome::from_io_error(&error, IoOp::Metadata),
        )),
    }
}

/// The read-only check of a writer of the state root and of check-config: what the state root
/// holds for the installation at `installation_root` (none:
/// [`StateRootInstallation::NotInstalled`], and nothing is read). Nothing is minted, claimed or
/// written.
pub fn check_state_root_installation(
    installation_root: Option<&Path>,
    state_root: &Path,
) -> RuntimeHostResult<StateRootInstallation> {
    match installation_root {
        Some(installation_root) => check(
            installation_root,
            state_root,
            HostLocation::CheckStateRootInstallation,
        ),
        None => Ok(StateRootInstallation::NotInstalled),
    }
}

/// The check before anything is written: the identity file and the record are read; no record
/// goes on, a record of this installation goes on; a record of another installation, or a
/// record while this installation has no identity file, refuses
/// (`state_root_installation_mismatch`); a file that cannot be read or has another shape refuses
/// (`state_root_installation_record_invalid`).
pub(crate) fn check(
    installation_root: &Path,
    state_root: &Path,
    location: HostLocation,
) -> RuntimeHostResult<StateRootInstallation> {
    let identity = read_identity(installation_root, location)?;
    let Some(record) = read_record(state_root, location)? else {
        return Ok(StateRootInstallation::Unclaimed);
    };
    judge(identity.as_ref(), &record, state_root, location)
}

/// The claim of a start, under the owner lock: the record is read again; one of this
/// installation passes and writes nothing; one of another installation (written since the
/// check: two starts raced) refuses as the check does. Without a record, the installation's
/// identity is minted and placed when it has none, then read back from the disk (a file another
/// configuration of the installation placed first is kept and used), and the record is placed
/// with that number and read back as well. Either way the answer is this installation's record
/// as the disk holds it, `placed` when this start placed it.
pub(crate) fn claim(request: &ClaimRequest<'_>) -> RuntimeHostResult<StateRootClaim> {
    const LOCATION: HostLocation = HostLocation::RecordStateRootInstallation;
    let identity = read_identity(request.installation_root, LOCATION)?;
    if let Some(record) = read_record(request.state_root, LOCATION)? {
        judge(identity.as_ref(), &record, request.state_root, LOCATION)?;
        return Ok(StateRootClaim::of_record(record, false));
    }
    let pid = std::process::id();
    let build_commit = request.build_commit.map(str::to_owned);
    // The hard link; a test may make it fail as a file system without hard links does.
    #[cfg(test)]
    let hard_link_fails = request.hard_link_fails;
    #[cfg(not(test))]
    let hard_link_fails = false;
    let link = |part: &Path, path: &Path| -> std::io::Result<()> {
        if hard_link_fails {
            Err(std::io::Error::from(ErrorKind::Unsupported))
        } else {
            fs::hard_link(part, path)
        }
    };
    let identity = match identity {
        Some(identity) => identity,
        None => {
            let path = identity_path(request.installation_root);
            let minted = InstallationIdentity {
                schema_version: INSTALLATION_IDENTITY_SCHEMA.to_owned(),
                installation_id: mint_installation_id(&path)?,
                minted_at_unix_ms: request.at_unix_ms,
                minted_by: MintedBy {
                    pid,
                    build_commit: build_commit.clone(),
                },
            };
            #[cfg(test)]
            if let Some(bytes) = request.identity_before_placement {
                fs::write(&path, bytes).expect("the test's identity before the placement");
            }
            place_once(&path, &minted, &link)?;
            // Placed or found: the identity the disk holds is the installation's.
            read_identity(request.installation_root, LOCATION)?.ok_or_else(|| vanished(&path))?
        }
    };
    let record = StateRootRecord {
        schema_version: STATE_ROOT_INSTALLATION_SCHEMA.to_owned(),
        installation_id: identity.installation_id.clone(),
        claimed_at_unix_ms: request.at_unix_ms,
        claimed_by: ClaimedBy {
            pid,
            owner_epoch: request.owner_epoch,
            build_commit,
        },
    };
    let record_path = request.state_root.join(STATE_ROOT_INSTALLATION_FILE);
    let placed = place_once(&record_path, &record, &link)?;
    // Placed or found: the record the disk holds is judged as the check judges it.
    let record =
        read_record(request.state_root, LOCATION)?.ok_or_else(|| vanished(&record_path))?;
    judge(Some(&identity), &record, request.state_root, LOCATION)?;
    Ok(StateRootClaim::of_record(record, placed))
}

/// The runtime fact [`STATE_ROOT_INSTALLATION_FACT_KEY`] of a claim: runtime scope, one row
/// `{claimed_at_unix_ms, claimed_by_pid, claimed_by_owner_epoch, claimed_by_build_commit?}`
/// (`claimed_by_build_commit` absent for a build that does not know its commit), observed at the
/// claim's time, source `runtime`, no lifetime. The installation's number is not in it.
pub(crate) fn claim_fact_record(claim: &StateRootClaim) -> RuntimeFactRecord {
    let mut row = BTreeMap::from([
        (
            "claimed_at_unix_ms".to_owned(),
            FactScalar::Integer(i64::try_from(claim.claimed_at_unix_ms).unwrap_or(i64::MAX)),
        ),
        (
            "claimed_by_pid".to_owned(),
            FactScalar::Integer(i64::from(claim.pid)),
        ),
        (
            "claimed_by_owner_epoch".to_owned(),
            FactScalar::String(crate::failure_identity::identifier_text(&claim.owner_epoch)),
        ),
    ]);
    if let Some(commit) = &claim.build_commit {
        row.insert(
            "claimed_by_build_commit".to_owned(),
            FactScalar::String(commit.clone()),
        );
    }
    RuntimeFactRecord {
        scope: RuntimeFactScope::Runtime,
        key: STATE_ROOT_INSTALLATION_FACT_KEY.to_owned(),
        value: FactValue::RecordList(vec![row]),
        observed_at_unix_ms: claim.claimed_at_unix_ms,
        source: OriginModule::Runtime,
        ttl_ms: None,
    }
}

/// Workflow #453 S12 (ruling 5; `contracts/state-root-installation.md`, "The state root's
/// record"): the fact a start records for the record `claim` it found or placed, given the latest
/// `host.state_root_installation` the fact store holds, `stored`. None when `stored` is of this
/// very claim (the same claim time and owner epoch); otherwise [`claim_fact_record`]. When `stored`
/// is of an earlier claim (the state root was handed over since, the ledger keeping the old
/// claim's fact), the row also carries that claim's time as `previous_claimed_at_unix_ms`, and the
/// record is observed after the stored one (at the claim's time, or one millisecond after the
/// stored observation when the claim's time is not later), so that the store takes it as the latest
/// and the next start finds its own claim and records nothing.
pub(crate) fn fact_for_start(
    claim: &StateRootClaim,
    stored: Option<&RuntimeFactRecord>,
) -> Option<RuntimeFactRecord> {
    let mut record = claim_fact_record(claim);
    let Some(stored) = stored else {
        return Some(record);
    };
    let stored_row = match &stored.value {
        FactValue::RecordList(rows) => rows.first(),
        _ => None,
    };
    let stored_claimed_at = stored_row
        .and_then(|row| row.get(CLAIMED_AT_MEMBER))
        .cloned();
    let stored_epoch = stored_row
        .and_then(|row| row.get(OWNER_EPOCH_MEMBER))
        .cloned();
    if let FactValue::RecordList(rows) = &mut record.value
        && let Some(row) = rows.first_mut()
    {
        if row.get(CLAIMED_AT_MEMBER) == stored_claimed_at.as_ref()
            && row.get(OWNER_EPOCH_MEMBER) == stored_epoch.as_ref()
        {
            return None;
        }
        if let Some(previous) = stored_claimed_at {
            row.insert(PREVIOUS_CLAIMED_AT_MEMBER.to_owned(), previous);
        }
    }
    record.observed_at_unix_ms = record
        .observed_at_unix_ms
        .max(stored.observed_at_unix_ms.saturating_add(1));
    Some(record)
}

/// Workflow #453 S12: what `actingd claim-state-root` did with the installation's identity file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandedIdentity {
    /// It was read and is used as it is.
    Read,
    /// There was none (an installation that never started): one was minted and placed only when
    /// absent, as a start places it.
    Minted,
    /// It could not be read and `--new-identity` was given: it was set aside under its kept name
    /// and a new one minted and placed.
    Renewed,
}

impl HandedIdentity {
    /// The word of the command's report.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Minted => "minted",
            Self::Renewed => "renewed",
        }
    }
}

/// Workflow #453 S12: what `actingd claim-state-root` did with the state root's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandedRecord {
    /// There was none: this installation's record was written, as its first start would have.
    Written,
    /// It was this installation's: nothing changed.
    Unchanged,
    /// It was another installation's, or it could not be read: it was set aside under its kept
    /// name and this installation's record written.
    Replaced,
}

impl HandedRecord {
    /// The word of the command's report.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Written => "written",
            Self::Unchanged => "unchanged",
            Self::Replaced => "replaced",
        }
    }
}

/// Workflow #453 S12: what `actingd claim-state-root` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRootHandOver {
    pub identity: HandedIdentity,
    pub record: HandedRecord,
    /// The claim time of the record the state root holds now.
    pub claimed_at_unix_ms: u64,
    /// The claim time of the record it replaced, when that record could be read.
    pub previous_claimed_at_unix_ms: Option<u64>,
    /// Each file set aside, under its kept name, in the order it was set aside.
    pub kept: Vec<PathBuf>,
    /// Fix round 1 (ruling 3): the kept old record the fingerprint caveat is about: the record
    /// this command set aside (`replaced`), or, when it wrote a record on a state root that held
    /// none (`written`), the newest `installation.json.old-<ms>` an earlier hand-over left there (a
    /// crash between setting the old record aside and placing the new one, or a put-back that
    /// failed); none for `unchanged`, or for `written` with no kept record.
    pub kept_record: Option<PathBuf>,
}

impl StateRootHandOver {
    /// Whether the command wrote either file: false only when the record was this installation's
    /// and its identity was read.
    pub fn changed(&self) -> bool {
        self.identity != HandedIdentity::Read || self.record != HandedRecord::Unchanged
    }
}

/// Workflow #453 S12, test only: the step of a hand-over after which a test stops it, as a
/// process that died there would.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandOverStep {
    IdentitySetAside,
    IdentityPlaced,
    RecordSetAside,
    RecordPlaced,
}

/// Workflow #453 S12: what a hand-over works on, under the owner lock the command took: the
/// installation and state roots, the command's owner epoch, the claim's time, the commit the build
/// was made from and whether `--new-identity` was given.
pub(crate) struct HandOverRequest<'a> {
    pub(crate) installation_root: &'a Path,
    pub(crate) state_root: &'a Path,
    pub(crate) owner_epoch: OwnerEpoch,
    pub(crate) at_unix_ms: u64,
    pub(crate) build_commit: Option<&'a str>,
    pub(crate) new_identity: bool,
    /// Test only: the step after which the hand-over stops.
    #[cfg(test)]
    pub(crate) crash_after: Option<HandOverStep>,
    /// Test only: every hard link fails with "unsupported", as on a file system without them.
    #[cfg(test)]
    pub(crate) hard_link_fails: bool,
}

/// Workflow #453 S12 (ruling 3): the hand-over's refusals that depend on the identity file alone,
/// read before the owner is taken so that a refusal writes nothing: an identity file that cannot be
/// read, without `--new-identity` (`state_root_installation_record_invalid`, as a start refuses
/// it), and `--new-identity` while the identity file can be read (`invalid_arguments`: the command
/// never replaces an identity that can be read).
pub(crate) fn check_hand_over(
    installation_root: &Path,
    new_identity: bool,
) -> RuntimeHostResult<()> {
    const LOCATION: HostLocation = HostLocation::CheckStateRootInstallation;
    match read_identity(installation_root, LOCATION) {
        Ok(Some(_)) if new_identity => Err(identity_readable(installation_root, LOCATION)),
        Ok(_) => Ok(()),
        Err(_) if new_identity => Ok(()),
        Err(error) => Err(error),
    }
}

/// Workflow #453 S12 (rulings 2 to 4; `contracts/state-root-installation.md`, "Handing a state
/// root over"): the hand-over of `actingd claim-state-root`, under the owner lock the command took.
/// The identity file is read and used; an absent one is minted and placed only when absent and read
/// back, as a start does; one that cannot be read is refused, unless `--new-identity` was given:
/// then it is set aside under its kept name and a new one minted (a readable one with the flag is
/// refused). Then the record: none is written; this installation's changes nothing; another
/// installation's, or one that cannot be read, is set aside as `installation.json.old-<ms>` and
/// this installation's written. Both files are placed with the start's primitive (a hard link of a
/// flushed part, an exclusive create without hard links) and read back; nothing is ever deleted,
/// and a placement that fails after its file was set aside gives the set-aside file its name back,
/// so the failure leaves the file where it was. A crash between the steps is completed by running
/// the command again.
pub(crate) fn hand_over(request: &HandOverRequest<'_>) -> RuntimeHostResult<StateRootHandOver> {
    const LOCATION: HostLocation = HostLocation::RecordStateRootInstallation;
    // The hard link; a test may make it fail as a file system without hard links does.
    #[cfg(test)]
    let hard_link_fails = request.hard_link_fails;
    #[cfg(not(test))]
    let hard_link_fails = false;
    let link = |from: &Path, to: &Path| -> std::io::Result<()> {
        if hard_link_fails {
            Err(std::io::Error::from(ErrorKind::Unsupported))
        } else {
            fs::hard_link(from, to)
        }
    };
    let identity_path = identity_path(request.installation_root);
    let mut kept = Vec::new();
    let (identity, handed_identity) = match read_identity(request.installation_root, LOCATION) {
        Ok(Some(_)) if request.new_identity => {
            return Err(identity_readable(request.installation_root, LOCATION));
        }
        Ok(Some(identity)) => (identity, HandedIdentity::Read),
        Ok(None) => (mint_identity(request, &link)?, HandedIdentity::Minted),
        Err(error) if !request.new_identity => return Err(error),
        Err(_) => {
            let aside = set_aside(&identity_path, request.at_unix_ms, &link)?;
            kept.push(aside.clone());
            #[cfg(test)]
            crash_point(request, HandOverStep::IdentitySetAside)?;
            let identity = mint_identity(request, &link)
                .map_err(|failed| put_back(&aside, &identity_path, &link, failed))?;
            (identity, HandedIdentity::Renewed)
        }
    };
    #[cfg(test)]
    if handed_identity != HandedIdentity::Read {
        crash_point(request, HandOverStep::IdentityPlaced)?;
    }
    let record_path = request.state_root.join(STATE_ROOT_INSTALLATION_FILE);
    let (replaced, previous_claimed_at_unix_ms) = match read_record(request.state_root, LOCATION) {
        Ok(Some(found)) if found.installation_id == identity.installation_id => {
            return Ok(StateRootHandOver {
                identity: handed_identity,
                record: HandedRecord::Unchanged,
                claimed_at_unix_ms: found.claimed_at_unix_ms,
                previous_claimed_at_unix_ms: None,
                kept,
                kept_record: None,
            });
        }
        Ok(Some(found)) => (true, Some(found.claimed_at_unix_ms)),
        Ok(None) => (false, None),
        // A record that cannot be read is replaced as another installation's is (ruling 4).
        Err(_) => (true, None),
    };
    let aside = if replaced {
        let aside = set_aside(&record_path, request.at_unix_ms, &link)?;
        kept.push(aside.clone());
        #[cfg(test)]
        crash_point(request, HandOverStep::RecordSetAside)?;
        Some(aside)
    } else {
        None
    };
    // Fix round 1 (ruling 3): a record written beside an old record an earlier hand-over kept
    // there is named, before anything is placed, so the caveat reaches this answer too.
    let kept_record = match &aside {
        Some(aside) => Some(aside.clone()),
        None => newest_kept_record(request.state_root)?,
    };
    let record = StateRootRecord {
        schema_version: STATE_ROOT_INSTALLATION_SCHEMA.to_owned(),
        installation_id: identity.installation_id.clone(),
        claimed_at_unix_ms: request.at_unix_ms,
        claimed_by: ClaimedBy {
            pid: std::process::id(),
            owner_epoch: request.owner_epoch,
            build_commit: request.build_commit.map(str::to_owned),
        },
    };
    if let Err(failed) = place_once(&record_path, &record, &link) {
        return Err(match &aside {
            Some(aside) => put_back(aside, &record_path, &link, failed),
            None => failed,
        });
    }
    // Placed or found: the record the disk holds is judged as the check judges it.
    let found = read_record(request.state_root, LOCATION)?.ok_or_else(|| vanished(&record_path))?;
    judge(Some(&identity), &found, request.state_root, LOCATION)?;
    #[cfg(test)]
    crash_point(request, HandOverStep::RecordPlaced)?;
    Ok(StateRootHandOver {
        identity: handed_identity,
        record: if replaced {
            HandedRecord::Replaced
        } else {
            HandedRecord::Written
        },
        claimed_at_unix_ms: found.claimed_at_unix_ms,
        previous_claimed_at_unix_ms,
        kept,
        kept_record,
    })
}

/// Workflow #453 S12 fix round 1 (ruling 3): the newest record an earlier hand-over set aside in
/// `state_root` (`installation.json.old-<ms>`, the largest `<ms>`), if any. A state root whose
/// entries cannot be listed is `state_root_claim_failed`; nothing has been placed then.
fn newest_kept_record(state_root: &Path) -> RuntimeHostResult<Option<PathBuf>> {
    let prefix = format!("{STATE_ROOT_INSTALLATION_FILE}{KEPT_MARKER}");
    let unlisted = |error: std::io::Error| claim_failed(state_root, &error, IoOp::ReadDir);
    let mut newest: Option<(u64, PathBuf)> = None;
    for entry in fs::read_dir(state_root).map_err(unlisted)? {
        let entry = entry.map_err(unlisted)?;
        let name = entry.file_name();
        let Some(at_unix_ms) = name
            .to_str()
            .and_then(|name| name.strip_prefix(prefix.as_str()))
            .and_then(|digits| digits.parse::<u64>().ok())
        else {
            continue;
        };
        if newest
            .as_ref()
            .is_none_or(|(newest_ms, _)| at_unix_ms > *newest_ms)
        {
            newest = Some((at_unix_ms, entry.path()));
        }
    }
    Ok(newest.map(|(_, path)| path))
}

/// Workflow #453 S12: a new identity for the hand-over, minted as a start mints it, placed only
/// when absent and read back from the disk: the identity the disk holds is the installation's.
fn mint_identity(
    request: &HandOverRequest<'_>,
    link: &HardLink<'_>,
) -> RuntimeHostResult<InstallationIdentity> {
    let path = identity_path(request.installation_root);
    let minted = InstallationIdentity {
        schema_version: INSTALLATION_IDENTITY_SCHEMA.to_owned(),
        installation_id: mint_installation_id(&path)?,
        minted_at_unix_ms: request.at_unix_ms,
        minted_by: MintedBy {
            pid: std::process::id(),
            build_commit: request.build_commit.map(str::to_owned),
        },
    };
    place_once(&path, &minted, link)?;
    read_identity(
        request.installation_root,
        HostLocation::RecordStateRootInstallation,
    )?
    .ok_or_else(|| vanished(&path))
}

/// Workflow #453 S12: sets the file at `path` aside under its kept name `<file>.old-<at_unix_ms>`
/// and answers that name. The file keeps its bytes and is never deleted: a hard link gives it the
/// kept name (it fails when the name is taken, so no kept file is ever replaced), then its old name
/// is removed. Without hard links (a link that fails for any other reason than "already exists") it
/// is renamed, only while the kept name is free. A step that fails is `state_root_claim_failed`;
/// the file is then under its name, or under both.
fn set_aside(path: &Path, at_unix_ms: u64, link: &HardLink<'_>) -> RuntimeHostResult<PathBuf> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!("{KEPT_MARKER}{at_unix_ms}"));
    let kept = path.with_file_name(name);
    match link(path, &kept) {
        Ok(()) => {
            fs::remove_file(path).map_err(|error| claim_failed(path, &error, IoOp::Remove))?;
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(claim_failed(&kept, &error, IoOp::Create));
        }
        Err(unlinked) => {
            match fs::symlink_metadata(&kept) {
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(claim_failed(
                        &kept,
                        &std::io::Error::from(ErrorKind::AlreadyExists),
                        IoOp::Rename,
                    ));
                }
                Err(error) => return Err(claim_failed(&kept, &error, IoOp::Metadata)),
            }
            fs::rename(path, &kept).map_err(|error| {
                claim_failed(path, &error, IoOp::Rename)
                    .with_note(Outcome::from_io_error(&unlinked, IoOp::Create))
            })?;
        }
    }
    Ok(kept)
}

/// Workflow #453 S12: after the placement of a new file at `path` failed (`failed`), the file set
/// aside as `kept` is given its name back (a hard link, or without hard links a copy by an exclusive
/// create; a file at `path` is left as it is), so the failed hand-over leaves the file where it
/// was, also under its kept name. A put-back that fails too is a note of `failed`.
fn put_back(
    kept: &Path,
    path: &Path,
    link: &HardLink<'_>,
    failed: RuntimeHostError,
) -> RuntimeHostError {
    let restored = match link(kept, path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(_) => fs::read(kept)
            .map_err(|error| Outcome::from_io_error(&error, IoOp::Read))
            .and_then(|bytes| {
                create_exclusive(path, &bytes)
                    .map(drop)
                    .map_err(|error| error.daemon_outcome())
            }),
    };
    match restored {
        Ok(()) => failed,
        Err(note) => failed.with_note(note),
    }
}

/// Workflow #453 S12: `--new-identity` while the identity file can be read is `invalid_arguments`:
/// the command never replaces an identity that can be read.
fn identity_readable(installation_root: &Path, location: HostLocation) -> RuntimeHostError {
    RuntimeHostError::fatal(
        SharedCode::InvalidArguments,
        location,
        RuntimeErrorCode::InvalidRequest,
    )
    .with_native_detail(format!(
        "--new-identity is refused: the identity file {} can be read, and an identity that can be read is never replaced",
        identity_path(installation_root).display()
    ))
}

/// Workflow #453 S12, test only: the hand-over stops after `step` when the test asked so.
#[cfg(test)]
fn crash_point(request: &HandOverRequest<'_>, step: HandOverStep) -> RuntimeHostResult<()> {
    use crate::codes::test_spellings::{TestCode, TestLocation};
    if request.crash_after == Some(step) {
        return Err(RuntimeHostError::fatal(
            TestCode::HandOverCrashedForTest,
            TestLocation::CrashHandOverForTest,
            RuntimeErrorCode::RuntimeHalted,
        ));
    }
    Ok(())
}

/// The record against this installation's identity: the same number passes; another number, or
/// no identity at all, is `state_root_installation_mismatch` (`path` the state root,
/// `at_unix_ms` the record's claim time), its text saying which of the two it was.
fn judge(
    identity: Option<&InstallationIdentity>,
    record: &StateRootRecord,
    state_root: &Path,
    location: HostLocation,
) -> RuntimeHostResult<StateRootInstallation> {
    let text = match identity {
        Some(identity) if identity.installation_id == record.installation_id => {
            return Ok(StateRootInstallation::ClaimedByThis);
        }
        Some(_) => "the state root's record installation.json names another installation",
        None => {
            "the state root has a record installation.json, and this installation has no identity file install\\identity.json"
        }
    };
    Err(RuntimeHostError::fatal(
        HostCode::StateRootInstallationMismatch,
        location,
        RuntimeErrorCode::RuntimeHalted,
    )
    .with_outcome_values(|outcome| {
        outcome
            .path(state_root)
            .at_unix_ms(record.claimed_at_unix_ms)
    })
    .with_native_detail(text.to_owned()))
}

fn identity_path(installation_root: &Path) -> PathBuf {
    installation_root.join(INSTALL_FOLDER).join(IDENTITY_FILE)
}

fn read_identity(
    installation_root: &Path,
    location: HostLocation,
) -> RuntimeHostResult<Option<InstallationIdentity>> {
    read_file(&identity_path(installation_root), location, |identity| {
        if identity.schema_version != INSTALLATION_IDENTITY_SCHEMA {
            Some("schema_version is not actingcommand.installation-identity.v1")
        } else if !is_installation_id(&identity.installation_id) {
            Some("installation_id is not 32 lowercase hexadecimal digits")
        } else {
            None
        }
    })
}

fn read_record(
    state_root: &Path,
    location: HostLocation,
) -> RuntimeHostResult<Option<StateRootRecord>> {
    read_file(
        &state_root.join(STATE_ROOT_INSTALLATION_FILE),
        location,
        |record| {
            if record.schema_version != STATE_ROOT_INSTALLATION_SCHEMA {
                Some("schema_version is not actingcommand.state-root-installation.v1")
            } else if !is_installation_id(&record.installation_id) {
                Some("installation_id is not 32 lowercase hexadecimal digits")
            } else {
                None
            }
        },
    )
}

/// The document at `path`: `None` when there is no file; a file that cannot be read, is larger
/// than 64 KiB, is not the document or breaks `shape` is `state_root_installation_record_invalid`
/// (`path` the file, the read failure as its `caused_by` link).
fn read_file<T: DeserializeOwned>(
    path: &Path,
    location: HostLocation,
    shape: fn(&T) -> Option<&'static str>,
) -> RuntimeHostResult<Option<T>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(record_invalid(
                location,
                path,
                Outcome::from_io_error(&error, IoOp::Open),
            ));
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            record_invalid(location, path, Outcome::from_io_error(&error, IoOp::Read))
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        return Err(record_invalid(
            location,
            path,
            data_failure("the file is larger than 64 KiB"),
        ));
    }
    let document: T = serde_json::from_slice(&bytes)
        .map_err(|error| record_invalid(location, path, data_failure(&error.to_string())))?;
    match shape(&document) {
        Some(problem) => Err(record_invalid(location, path, data_failure(problem))),
        None => Ok(Some(document)),
    }
}

/// 32 lowercase hexadecimal digits.
fn is_installation_id(value: &str) -> bool {
    value.len() == INSTALLATION_ID_BYTES * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A new installation number: 16 bytes from the operating system's random source
/// (`fill_system_random`, `BCryptGenRandom` with the system-preferred generator), lowercase
/// hexadecimal. A source that fails is `state_root_claim_failed` for the identity file.
fn mint_installation_id(identity_path: &Path) -> RuntimeHostResult<String> {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; INSTALLATION_ID_BYTES];
    actingcommand_host_metrics::fill_system_random(&mut bytes)
        .map_err(|error| claim_failed(identity_path, &error, IoOp::Read))?;
    let mut text = String::with_capacity(INSTALLATION_ID_BYTES * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(text)
}

/// Places `document` at `path` only when no file is there. The bytes go to `<path>.part`,
/// created anew (a part an earlier crash left is removed first: its name is reused, and a file it
/// may still name as a second link is never written through), and are flushed to disk; the part
/// is then hard-linked to `path`. The hard link is the placement: it adds the one directory entry
/// `path` for the complete, flushed file in a single file-system operation (`CreateHardLinkW` on
/// Windows, `link` elsewhere) and fails when `path` exists, never replacing it, so a file another
/// writer placed first is kept as it is. The part's name is removed afterwards. `true` when this
/// call placed the file, `false` when one was there. A failed step is `state_root_claim_failed`.
///
/// Fix round 2: a hard link that fails for any other reason than "already exists" (a file system
/// without hard links: FAT32, exFAT) falls back to [`create_exclusive`] of `path` itself; only
/// when that fails too is the claim failed, with the link's failure as a `note` link. `link` is
/// the hard link ([`fs::hard_link`]; a test gives one that fails).
fn place_once(
    path: &Path,
    document: &impl Serialize,
    link: &HardLink<'_>,
) -> RuntimeHostResult<bool> {
    let mut bytes = serde_json::to_vec(document).map_err(|_| {
        RuntimeHostError::fatal(
            HostCode::StateRootClaimFailed,
            HostLocation::RecordStateRootInstallation,
            RuntimeErrorCode::RuntimeHalted,
        )
        .with_outcome_values(|outcome| outcome.path(path))
    })?;
    bytes.push(b'\n');
    let mut part_name = path.file_name().unwrap_or_default().to_os_string();
    part_name.push(PART_SUFFIX);
    let part = path.with_file_name(part_name);
    match fs::remove_file(&part) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(claim_failed(&part, &error, IoOp::Remove)),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&part)
        .map_err(|error| claim_failed(&part, &error, IoOp::Create))?;
    file.write_all(&bytes)
        .map_err(|error| claim_failed(&part, &error, IoOp::Write))?;
    file.sync_all()
        .map_err(|error| claim_failed(&part, &error, IoOp::Sync))?;
    drop(file);
    let placed = match link(&part, path) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
        Err(unlinked) => create_exclusive(path, &bytes)
            .map_err(|error| error.with_note(Outcome::from_io_error(&unlinked, IoOp::Create)))?,
    };
    fs::remove_file(&part).map_err(|error| claim_failed(&part, &error, IoOp::Remove))?;
    Ok(placed)
}

/// The hard link of a part file to its target, as [`place_once`] takes it.
type HardLink<'a> = dyn Fn(&Path, &Path) -> std::io::Result<()> + 'a;

/// Fix round 2: the placement where no hard link can be made: an exclusive create of `path`
/// itself (it fails when `path` exists), the bytes written and flushed. `false` when the file is
/// there ("already exists": another start placed it, and it is kept as it is). What it gives up
/// against the link: the file appears before its bytes are written, so a crash while they are
/// written can leave an incomplete file, which the next start refuses as
/// `state_root_installation_record_invalid`. A failed step is `state_root_claim_failed`.
fn create_exclusive(path: &Path, bytes: &[u8]) -> RuntimeHostResult<bool> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(claim_failed(path, &error, IoOp::Create)),
    };
    file.write_all(bytes)
        .map_err(|error| claim_failed(path, &error, IoOp::Write))?;
    file.sync_all()
        .map_err(|error| claim_failed(path, &error, IoOp::Sync))?;
    Ok(true)
}

/// Test only (fix round 2): [`place_once`] of `document` at `path` with a hard link that always
/// fails with "unsupported", as on a file system without hard links.
#[cfg(test)]
pub(crate) fn place_once_without_hard_link_for_test(
    path: &Path,
    document: &serde_json::Value,
) -> RuntimeHostResult<bool> {
    place_once(path, document, &|_, _| {
        Err(std::io::Error::from(ErrorKind::Unsupported))
    })
}

/// `state_root_claim_failed` for a file the claim placed or found that is gone when it is read
/// back (`path` the file, `not_found` as its cause).
fn vanished(path: &Path) -> RuntimeHostError {
    claim_failed(path, &std::io::Error::from(ErrorKind::NotFound), IoOp::Read)
}

/// `state_root_installation_record_invalid` (`path` the file, `cause` the read failure).
fn record_invalid(location: HostLocation, path: &Path, cause: Outcome) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::StateRootInstallationRecordInvalid,
        location,
        RuntimeErrorCode::RuntimeHalted,
    )
    .with_outcome_values(|outcome| outcome.path(path))
    .with_caused_by(cause)
}

/// `state_root_claim_failed` (`path` the file, the operating system's failure as its cause).
fn claim_failed(path: &Path, error: &std::io::Error, op: IoOp) -> RuntimeHostError {
    RuntimeHostError::fatal(
        HostCode::StateRootClaimFailed,
        HostLocation::RecordStateRootInstallation,
        RuntimeErrorCode::RuntimeHalted,
    )
    .with_outcome_values(|outcome| outcome.path(path))
    .with_caused_by(Outcome::from_io_error(error, op))
}

/// A document that is not what it must be: `foreign_data_error` with `raw_source` json.
fn data_failure(text: &str) -> Outcome {
    Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(text)
}
