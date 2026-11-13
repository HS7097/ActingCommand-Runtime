// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S11 (`contracts/state-root-installation.md`): the state root remembers which
//! installation claimed it.
//!
//! An installation is identified by a number minted once into its root,
//! `<installation root>\install\identity.json`; a state root records the installation that
//! claimed it in `<state root>\installation.json`. Both are files outside the ledger, a
//! registered exception to "program state lives in tagged ledger records": the check reads them
//! before the ledger is opened. A writer of the state root started from an installation root
//! checks the record before it writes anything; only a start claims, under the owner lock. A
//! writer not started from an installation root reads, mints and writes nothing here.

use crate::codes::{HostCode, HostLocation};
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::codes::ContractCode;
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
            place_once(&path, &minted)?;
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
    let placed = place_once(&record_path, &record)?;
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
fn place_once(path: &Path, document: &impl Serialize) -> RuntimeHostResult<bool> {
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
    let placed = match fs::hard_link(&part, path) {
        Ok(()) => true,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
        Err(error) => return Err(claim_failed(path, &error, IoOp::Create)),
    };
    fs::remove_file(&part).map_err(|error| claim_failed(&part, &error, IoOp::Remove))?;
    Ok(placed)
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
