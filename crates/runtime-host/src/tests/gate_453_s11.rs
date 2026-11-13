// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S11 (`contracts/state-root-installation.md`): the state root remembers its
//! installation. Every root is a temporary folder: an installation root is a folder whose
//! `install` folder holds the installer's own `state.json`; the host is the start fixture
//! (`config`, one fake instance). Every expected value comes from the coordinator's rulings 1 to 6
//! of the work order (`rd-1114/brief-453-s11-state-root-record.md`) and the contract:
//! - the first start of an installation without an identity on a state root an earlier release
//!   left unrecorded mints `install\identity.json` and writes `installation.json`, one file more
//!   in each folder, the tagged fact `host.state_root_installation` once (its time and who
//!   claimed, never the number) and the daemon line `state_root_claimed`; a restart passes and
//!   writes nothing more;
//! - another installation's record, and a record while this installation has no identity file,
//!   refuse the start with `state_root_installation_mismatch` before anything is written: the
//!   owner journal byte for byte as before, nothing minted;
//! - a corrupt record and a corrupt identity file each refuse with
//!   `state_root_installation_record_invalid`, the file as `path`, the read failure as the link;
//! - a record that appears between the check and the claim refuses after the owner lock was
//!   taken, and the owner epoch the start opened is closed;
//! - a moved installation root (the same identity file at another path) passes;
//! - a Runtime not started from an installation root writes the files and the start records it
//!   wrote before, and nothing of this slice;
//! - `unlock-owner` and a maintenance pass that takes the owner refuse a mismatch with nothing
//!   written, and neither mints nor claims.

use super::instance_workers::closed_ledger_events;
use super::*;
use crate::{LedgerMaintenanceOperation, LedgerMaintenanceRequest};
use actingcommand_contract::outcome::{Category, Outcome};
use actingcommand_contract::{
    FactScalar, OwnerUnlockActor, RuntimeFactRecord, RuntimeFactScope, RuntimePayload,
};
use actingcommand_runtime_database::MaintenanceLimits;
use serde_json::{Value, json};

/// The state root's record.
const RECORD: &str = "installation.json";
/// The installation's identity file, in its `install` folder.
const IDENTITY: &str = "identity.json";
/// The tagged runtime fact a claim is mirrored into.
const FACT: &str = "host.state_root_installation";
const MISMATCH: &str = "state_root_installation_mismatch";
const INVALID: &str = "state_root_installation_record_invalid";
/// Two installations' numbers.
const THIS_ID: &str = "ffeeddccbbaa99887766554433221100";
const OTHER_ID: &str = "00112233445566778899aabbccddeeff";
/// The claim time of a record a test writes.
const CLAIMED_AT_UNIX_MS: u64 = 1_800_000_000_000;

/// An installation root: its `install` folder holds the installer's own `state.json`.
fn installation_root() -> TempDir {
    let root = TempDir::new().expect("tempdir");
    fs::create_dir_all(root.path().join("install")).expect("the install folder");
    fs::write(root.path().join("install").join("state.json"), b"{}")
        .expect("the installer's state file");
    root
}

fn install_folder(installation: &TempDir) -> PathBuf {
    installation.path().join("install")
}

/// An identity file of the installation numbered `installation_id`.
fn identity(installation_id: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.installation-identity.v1",
        "installation_id": installation_id,
        "minted_at_unix_ms": CLAIMED_AT_UNIX_MS,
        "minted_by": {"pid": 4242, "build_commit": null}
    }))
    .expect("identity JSON")
}

/// A state root's record of the installation numbered `installation_id`.
fn record(installation_id: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": "actingcommand.state-root-installation.v1",
        "installation_id": installation_id,
        "claimed_at_unix_ms": CLAIMED_AT_UNIX_MS,
        "claimed_by": {
            "pid": 4242,
            "owner_epoch": "epoch_0123456789abcdef0123456789abcdef",
            "build_commit": null
        }
    }))
    .expect("record JSON")
}

fn host_config(state: &TempDir, installation: Option<&Path>) -> RuntimeHostConfig {
    match installation {
        Some(root) => config(state).with_installation_root(root),
        None => config(state),
    }
}

/// The start fixture on `state`, from `installation` when given, with the instance `instance`.
fn start(
    state: &TempDir,
    installation: Option<&Path>,
    instance: InstanceId,
) -> RuntimeHostResult<RuntimeHost> {
    RuntimeHost::start(
        host_config(state, installation),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
}

/// A start that passes, closed at once.
fn started_and_closed(state: &TempDir, installation: Option<&Path>, instance: InstanceId) {
    start(state, installation, instance)
        .expect("the start passes")
        .close()
        .expect("close the host");
}

/// A start the state root's record refuses.
fn refused(state: &TempDir, installation: &Path, instance: InstanceId) -> RuntimeHostError {
    match start(state, Some(installation), instance) {
        Ok(host) => {
            host.close().expect("close the host");
            panic!("the start must be refused");
        }
        Err(error) => error,
    }
}

/// The names in `folder`, but SQLite's own `-wal` and `-shm` sidecars.
fn names(folder: &Path) -> BTreeSet<String> {
    fs::read_dir(folder)
        .expect("list a folder")
        .map(|entry| {
            entry
                .expect("a folder entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
        .collect()
}

/// Every file below `folder` with its bytes, but SQLite's own `-wal` and `-shm` sidecars.
fn files(folder: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(current) = folders.pop() {
        for entry in fs::read_dir(&current).expect("list a folder") {
            let path = entry.expect("a folder entry").path();
            if path.is_dir() {
                folders.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            let bytes = fs::read(&path).expect("read a file");
            files.insert(
                path.strip_prefix(folder)
                    .expect("a path below the folder")
                    .to_path_buf(),
                bytes,
            );
        }
    }
    files
}

/// The files below `folder`, by path, but SQLite's own `-wal` and `-shm` sidecars.
fn paths(folder: &Path) -> BTreeSet<PathBuf> {
    files(folder).into_keys().collect()
}

fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read the document")).expect("a JSON document")
}

/// The records of `host.state_root_installation` in the closed ledger of `state`.
fn claim_facts(state: &Path) -> Vec<RuntimeFactRecord> {
    closed_ledger_events(state)
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded))
                if recorded.record().key == FACT =>
            {
                Some(recorded.record().clone())
            }
            _ => None,
        })
        .collect()
}

/// The records of a root's first start, through its start record, each as its type and, for a
/// runtime fact, its key.
fn first_start_records(state: &Path) -> Vec<String> {
    let mut records = Vec::new();
    for event in closed_ledger_events(state) {
        let key = match event.payload() {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded)) => {
                recorded.record().key.clone()
            }
            _ => String::new(),
        };
        records.push(
            format!("{:?} {key}", event.event_type())
                .trim_end()
                .to_owned(),
        );
        if event.event_type() == EventType::RuntimeStarted {
            return records;
        }
    }
    panic!("the root holds no start record: {records:?}");
}

/// The owner journal's records, oldest first.
fn journal(state: &Path) -> Vec<Value> {
    fs::read_to_string(state.join("owner.lock"))
        .expect("the owner journal")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("a journal record"))
        .collect()
}

/// A `caused_by` link of `outcome` with `code`.
fn caused_by(outcome: &Outcome, code: &str) -> bool {
    outcome.causes().iter().any(|link| {
        link.relation().as_str() == "caused_by" && link.outcome().code().as_str() == code
    })
}

// Rulings 1, 3 and 5: the first start mints and claims, one file more in each folder, the fact
// and the daemon line once; a restart passes and writes nothing more.
#[test]
fn s11_the_first_start_mints_and_claims_once_and_a_restart_writes_nothing_more() {
    let installation = installation_root();
    let install = install_folder(&installation);
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    // A state root a release before this slice started on: no record.
    started_and_closed(&state, None, instance);
    let state_before = names(state.path());
    let install_before = names(&install);
    assert!(!state_before.contains(RECORD), "{state_before:?}");
    assert_eq!(install_before, BTreeSet::from(["state.json".to_owned()]));

    let host = start(&state, Some(installation.path()), instance).expect("the first start");
    let claimed = host.state_root_claimed().cloned();
    host.close().expect("close the host");
    let claimed = claimed.expect("the daemon line state_root_claimed");
    assert_eq!(claimed.code().as_str(), "state_root_claimed");
    assert_eq!(claimed.category(), Category::Info);
    assert_eq!(
        claimed.value("path"),
        Some(&json!(state.path().to_string_lossy()))
    );

    let mut state_expected = state_before;
    state_expected.insert(RECORD.to_owned());
    assert_eq!(names(state.path()), state_expected, "one file more");
    let mut install_expected = install_before;
    install_expected.insert(IDENTITY.to_owned());
    assert_eq!(names(&install), install_expected, "one file more");

    let identity = json_file(&install.join(IDENTITY));
    assert_eq!(
        identity["schema_version"],
        "actingcommand.installation-identity.v1"
    );
    let number = identity["installation_id"]
        .as_str()
        .expect("the installation's number")
        .to_owned();
    assert!(
        number.len() == 32
            && number
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{number}"
    );
    assert_eq!(identity["minted_by"]["pid"], json!(std::process::id()));
    let record = json_file(&state.path().join(RECORD));
    assert_eq!(
        record["schema_version"],
        "actingcommand.state-root-installation.v1"
    );
    assert_eq!(record["installation_id"], json!(number));
    assert_eq!(record["claimed_by"]["pid"], json!(std::process::id()));
    assert_eq!(record["claimed_by"]["build_commit"], Value::Null);
    let claimed_at = record["claimed_at_unix_ms"]
        .as_u64()
        .expect("the claim's time");
    // The claim's owner epoch is the claiming start's, whose close is the journal's last record.
    let last = journal(state.path()).pop().expect("a journal record");
    assert_eq!(record["claimed_by"]["owner_epoch"], last["owner_epoch"]);

    let facts = claim_facts(state.path());
    assert_eq!(facts.len(), 1, "{facts:?}");
    let fact = &facts[0];
    assert_eq!(fact.scope, RuntimeFactScope::Runtime);
    assert_eq!(fact.observed_at_unix_ms, claimed_at);
    let ContractFactValue::RecordList(rows) = &fact.value else {
        panic!("one row: {fact:?}");
    };
    let [row] = rows.as_slice() else {
        panic!("one row: {rows:?}");
    };
    assert_eq!(
        row.get("claimed_at_unix_ms"),
        Some(&FactScalar::Integer(
            i64::try_from(claimed_at).expect("a time")
        ))
    );
    assert_eq!(
        row.get("claimed_by_pid"),
        Some(&FactScalar::Integer(i64::from(std::process::id())))
    );
    let owner_epoch = record["claimed_by"]["owner_epoch"]
        .as_str()
        .expect("the claim's owner epoch")
        .to_owned();
    assert_eq!(
        row.get("claimed_by_owner_epoch"),
        Some(&FactScalar::String(owner_epoch))
    );
    assert_eq!(row.len(), 3, "no commit and never the number: {row:?}");
    assert!(
        !row.values()
            .any(|value| value == &FactScalar::String(number.clone())),
        "{row:?}"
    );

    // A restart passes and writes nothing more.
    let state_after = names(state.path());
    let identity_bytes = fs::read(install.join(IDENTITY)).expect("the identity file");
    let record_bytes = fs::read(state.path().join(RECORD)).expect("the record");
    let host = start(&state, Some(installation.path()), instance).expect("a restart");
    assert!(host.state_root_claimed().is_none());
    host.close().expect("close the host");
    assert_eq!(names(state.path()), state_after);
    assert_eq!(names(&install), install_expected);
    assert_eq!(
        fs::read(install.join(IDENTITY)).ok(),
        Some(identity_bytes),
        "the identity as it was"
    );
    assert_eq!(
        fs::read(state.path().join(RECORD)).ok(),
        Some(record_bytes),
        "the record as it was"
    );
    assert_eq!(claim_facts(state.path()).len(), 1, "the fact once");
}

// Ruling 4: a state root another installation recorded refuses the start before anything is
// written: the owner journal byte for byte as before, nothing minted, the record's time and the
// state root named.
#[test]
fn s11_a_state_root_another_installation_recorded_is_refused_before_anything_is_written() {
    let installation = installation_root();
    let install = install_folder(&installation);
    fs::write(install.join(IDENTITY), identity(THIS_ID)).expect("this installation's identity");
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    // The state root another installation's start claimed and ran on.
    let other = installation_root();
    started_and_closed(&state, Some(other.path()), instance);
    let journal_before = fs::read(state.path().join("owner.lock")).expect("the owner journal");
    let record_before = fs::read(state.path().join(RECORD)).expect("the record");
    let state_before = paths(state.path());
    let install_before = files(&install);
    let claimed_at = json_file(&state.path().join(RECORD))["claimed_at_unix_ms"].clone();

    let error = refused(&state, installation.path(), instance);
    assert_eq!(error.code(), MISMATCH, "{error}");
    assert!(error.is_fatal());
    let outcome = error.outcome();
    assert_eq!(
        outcome.value("path"),
        Some(&json!(state.path().to_string_lossy()))
    );
    assert_eq!(outcome.value("at_unix_ms"), Some(&claimed_at));
    assert!(
        error
            .native_detail()
            .is_some_and(|text| text.contains("names another installation")),
        "{:?}",
        error.native_detail()
    );
    assert_eq!(
        fs::read(state.path().join("owner.lock")).ok(),
        Some(journal_before),
        "the owner journal byte for byte"
    );
    assert_eq!(
        fs::read(state.path().join(RECORD)).ok(),
        Some(record_before),
        "the record as it was"
    );
    assert_eq!(paths(state.path()), state_before, "no file written");
    assert_eq!(files(&install), install_before, "nothing minted");
}

// Ruling 4: a record while this installation has no identity file refuses the start; nothing is
// minted and nothing written.
#[test]
fn s11_a_record_while_this_installation_has_no_identity_is_refused_and_nothing_is_minted() {
    let installation = installation_root();
    let install = install_folder(&installation);
    let state = TempDir::new().expect("tempdir");
    fs::write(state.path().join(RECORD), record(OTHER_ID)).expect("a record");

    let error = refused(&state, installation.path(), instance_id());
    assert_eq!(error.code(), MISMATCH, "{error}");
    assert_eq!(
        error.outcome().value("at_unix_ms"),
        Some(&json!(CLAIMED_AT_UNIX_MS))
    );
    assert!(
        error
            .native_detail()
            .is_some_and(|text| text.contains("no identity file")),
        "{:?}",
        error.native_detail()
    );
    assert_eq!(
        names(&install),
        BTreeSet::from(["state.json".to_owned()]),
        "nothing minted"
    );
    assert_eq!(
        names(state.path()),
        BTreeSet::from([RECORD.to_owned()]),
        "nothing written"
    );
}

// Ruling 4: a corrupt record, a record of another shape and a corrupt identity file each refuse
// the start with `state_root_installation_record_invalid`, the file as `path` and the read
// failure as the `caused_by` link; nothing is written or minted.
#[test]
fn s11_a_corrupt_record_and_a_corrupt_identity_are_each_refused_as_invalid() {
    let documents: [(&str, Vec<u8>); 2] = [
        ("not JSON", b"{\"schema_version\":".to_vec()),
        (
            "another schema",
            serde_json::to_vec(&json!({
                "schema_version": "actingcommand.state-root-installation.v0",
                "installation_id": THIS_ID,
                "claimed_at_unix_ms": CLAIMED_AT_UNIX_MS,
                "claimed_by": {
                    "pid": 4242,
                    "owner_epoch": "epoch_0123456789abcdef0123456789abcdef",
                    "build_commit": null
                }
            }))
            .expect("record JSON"),
        ),
    ];
    for (label, bytes) in documents {
        let installation = installation_root();
        fs::write(
            install_folder(&installation).join(IDENTITY),
            identity(THIS_ID),
        )
        .expect("this installation's identity");
        let state = TempDir::new().expect("tempdir");
        fs::write(state.path().join(RECORD), &bytes).expect("a record");
        let error = refused(&state, installation.path(), instance_id());
        assert_eq!(error.code(), INVALID, "{label}: {error}");
        let outcome = error.outcome();
        assert_eq!(
            outcome.value("path"),
            Some(&json!(state.path().join(RECORD).to_string_lossy())),
            "{label}"
        );
        assert!(
            caused_by(&outcome, "foreign_data_error"),
            "{label}: {outcome}"
        );
        assert_eq!(
            names(state.path()),
            BTreeSet::from([RECORD.to_owned()]),
            "{label}: nothing written"
        );
        assert_eq!(
            fs::read(state.path().join(RECORD)).ok(),
            Some(bytes),
            "{label}"
        );
    }

    // A corrupt identity file, on an unrecorded state root.
    let installation = installation_root();
    let identity_path = install_folder(&installation).join(IDENTITY);
    fs::write(&identity_path, b"identity").expect("a corrupt identity file");
    let state = TempDir::new().expect("tempdir");
    let error = refused(&state, installation.path(), instance_id());
    assert_eq!(error.code(), INVALID, "{error}");
    let outcome = error.outcome();
    assert_eq!(
        outcome.value("path"),
        Some(&json!(identity_path.to_string_lossy()))
    );
    assert!(caused_by(&outcome, "foreign_data_error"), "{outcome}");
    assert!(names(state.path()).is_empty(), "nothing written");
    assert_eq!(
        fs::read(&identity_path).ok(),
        Some(b"identity".to_vec()),
        "nothing minted"
    );
}

// Ruling 5: a record that appears between the check and the claim (two starts raced) refuses
// after the owner lock was taken; the owner epoch the start opened is closed, as a failed start's
// is, and the record stays as the other start wrote it.
#[test]
fn s11_a_record_that_appears_between_the_two_reads_is_refused_after_the_acquire() {
    let installation = installation_root();
    let install = install_folder(&installation);
    fs::write(install.join(IDENTITY), identity(THIS_ID)).expect("this installation's identity");
    let state = TempDir::new().expect("tempdir");
    let raced = record(OTHER_ID);
    let result = RuntimeHost::start(
        config(&state)
            .with_installation_root(installation.path())
            .with_state_root_record_after_check_for_test(raced.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    );
    let error = match result {
        Ok(host) => {
            host.close().expect("close the host");
            panic!("the claim must refuse the raced record");
        }
        Err(error) => error,
    };
    assert_eq!(error.code(), MISMATCH, "{error}");
    assert_eq!(error.operation(), "record_state_root_installation");
    // The start's owner epoch: opened, then closed.
    let records = journal(state.path());
    let [opened, closed] = records.as_slice() else {
        panic!("the start's two records: {records:?}");
    };
    assert_eq!(opened["owner_epoch"], closed["owner_epoch"]);
    assert_eq!(opened["active"], json!(true));
    assert_eq!(closed["active"], json!(false));
    assert!(closed["closed_at_unix_ms"].is_u64(), "{closed}");
    assert_eq!(
        names(state.path()),
        BTreeSet::from(["owner.lock".to_owned(), RECORD.to_owned()]),
        "nothing more written"
    );
    assert_eq!(fs::read(state.path().join(RECORD)).ok(), Some(raced));
    assert_eq!(
        fs::read(install.join(IDENTITY)).ok(),
        Some(identity(THIS_ID))
    );
}

// Ruling 1: a moved installation root (the same identity file at another path) is the same
// installation: its start passes, claims nothing and writes nothing more.
#[test]
fn s11_a_moved_installation_root_passes() {
    let first = installation_root();
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    started_and_closed(&state, Some(first.path()), instance);
    let record_bytes = fs::read(state.path().join(RECORD)).expect("the record");
    // The installation's files at another path.
    let moved = TempDir::new().expect("tempdir");
    let moved_install = moved.path().join("install");
    fs::create_dir_all(&moved_install).expect("the install folder");
    for name in ["state.json", IDENTITY] {
        fs::copy(install_folder(&first).join(name), moved_install.join(name))
            .expect("move a file of the installation");
    }

    let host = start(&state, Some(moved.path()), instance).expect("the moved installation starts");
    assert!(host.state_root_claimed().is_none());
    host.close().expect("close the host");
    assert_eq!(
        names(&moved_install),
        BTreeSet::from(["state.json".to_owned(), IDENTITY.to_owned()])
    );
    assert_eq!(fs::read(state.path().join(RECORD)).ok(), Some(record_bytes));
    assert_eq!(claim_facts(state.path()).len(), 1);
}

// Ruling 2: a Runtime not started from an installation root (the start fixture as it is today)
// writes the files and the start records it wrote before this slice: the same fixture started
// from an installation root on a state root of its own writes exactly the record and the fact
// more, and nothing else. (Two roots: the comparison is of names and record kinds, never of
// identities or times.)
#[test]
fn s11_a_runtime_not_started_from_an_installation_root_writes_what_it_wrote_before() {
    let plain = TempDir::new().expect("tempdir");
    started_and_closed(&plain, None, instance_id());
    let installation = installation_root();
    let installed = TempDir::new().expect("tempdir");
    started_and_closed(&installed, Some(installation.path()), instance_id());

    assert!(!plain.path().join(RECORD).exists());
    assert!(claim_facts(plain.path()).is_empty());
    let mut expected = names(plain.path());
    expected.insert(RECORD.to_owned());
    assert_eq!(names(installed.path()), expected);

    let plain_records = first_start_records(plain.path());
    let claim = format!("{:?} {FACT}", EventType::RuntimeFactRecorded);
    let installed_records = first_start_records(installed.path());
    assert_eq!(
        installed_records
            .iter()
            .filter(|record| **record == claim)
            .count(),
        1,
        "{installed_records:?}"
    );
    let without_claim = installed_records
        .into_iter()
        .filter(|record| *record != claim)
        .collect::<Vec<_>>();
    let differing = plain_records
        .iter()
        .zip(&without_claim)
        .enumerate()
        .filter(|(_, (plain, installed))| plain != installed)
        .map(|(index, (plain, installed))| format!("#{index}: {plain} / {installed}"))
        .collect::<Vec<_>>();
    assert!(
        differing.is_empty() && plain_records.len() == without_claim.len(),
        "records that differ: {differing:?}; {} and {} records",
        plain_records.len(),
        without_claim.len()
    );
}

// Ruling 6: `unlock-owner` and a ledger maintenance pass that takes the owner make the start's
// read-only check: another installation's record refuses them with nothing written; on an
// unrecorded state root of an installation without an identity they mint and claim nothing.
#[test]
fn s11_unlock_owner_and_a_maintenance_pass_refuse_a_mismatch_and_never_mint_or_claim() {
    let actor = OwnerUnlockActor::new("fixture-operator").expect("an actor");
    let installation = installation_root();
    let install = install_folder(&installation);
    fs::write(install.join(IDENTITY), identity(THIS_ID)).expect("this installation's identity");
    let state = TempDir::new().expect("tempdir");
    let other = installation_root();
    started_and_closed(&state, Some(other.path()), instance_id());
    let state_before = files(state.path());
    let install_before = files(&install);

    let failure = RuntimeHost::unlock_owner(
        config(&state).with_installation_root(installation.path()),
        actor.clone(),
        true,
    )
    .expect_err("unlock-owner refuses another installation's state root");
    assert_eq!(failure.error.code(), MISMATCH, "{}", failure.error);
    assert!(!failure.journal_appended);
    assert_eq!(files(state.path()), state_before, "nothing written");

    let destinations = TempDir::new().expect("tempdir");
    let backup = destinations.path().join("backup");
    let refused = RuntimeHost::maintain_ledger(
        config(&state).with_installation_root(installation.path()),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Backup,
            backup: Some(backup.clone()),
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query: false,
            full: false,
        },
    )
    .expect_err("a backup refuses another installation's state root");
    assert_eq!(refused.outcome().code().as_str(), MISMATCH, "{refused}");
    assert_eq!(files(state.path()), state_before, "nothing written");
    assert!(!backup.exists());
    assert_eq!(files(&install), install_before, "nothing minted");

    // An installation without an identity, a state root without a record: neither command mints
    // or claims, whatever it answers.
    let bare = installation_root();
    let unrecorded = TempDir::new().expect("tempdir");
    started_and_closed(&unrecorded, None, instance_id());
    if let Err(failure) = RuntimeHost::unlock_owner(
        config(&unrecorded).with_installation_root(bare.path()),
        actor,
        true,
    ) {
        assert_ne!(failure.error.code(), MISMATCH, "{}", failure.error);
        assert_ne!(failure.error.code(), INVALID, "{}", failure.error);
    }
    if let Err(failure) = RuntimeHost::maintain_ledger(
        config(&unrecorded).with_installation_root(bare.path()),
        LedgerMaintenanceRequest {
            operation: LedgerMaintenanceOperation::Backup,
            backup: Some(destinations.path().join("unrecorded")),
            target: None,
            artifact_root: None,
            limits: MaintenanceLimits::default(),
            frame_query: false,
            full: false,
        },
    ) {
        let code = failure.outcome().code().as_str().to_owned();
        assert!(code != MISMATCH && code != INVALID, "{failure}");
    }
    assert!(!unrecorded.path().join(RECORD).exists(), "nothing claimed");
    assert_eq!(
        names(&install_folder(&bare)),
        BTreeSet::from(["state.json".to_owned()]),
        "nothing minted"
    );
}
