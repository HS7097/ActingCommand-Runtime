// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #453 S12 (`contracts/state-root-installation.md`, "Handing a state root over"):
//! `actingd claim-state-root` (`RuntimeHost::claim_state_root`) hands a state root over to the
//! installation it runs from, and the next start records the new claim. Every root is a temporary
//! folder: an installation root is a folder whose `install` folder holds the installer's own
//! `state.json`; the host is the start fixture (`config`, one fake instance); no test reads a clock
//! or sleeps. Every expected value comes from the coordinator's rulings 1 to 5 of the work order
//! (`rd-1114/brief-453-s12-state-root-claim.md`):
//! - another installation's record is kept under `installation.json.old-<ms>` byte for byte and
//!   this installation's written, the answer naming the old claim's time; the owner journal keeps
//!   its earlier records byte for byte and gains the command's epoch, opened and closed, which the
//!   new record names; the ledger, the integrity key and every other file are untouched; the next
//!   start records the new claim once, with the old claim's time, and a restart nothing more;
//! - no record: written; this installation's record: nothing written, nothing renamed, and the
//!   answer says nothing changed;
//! - a record that cannot be read is set aside and replaced;
//! - the identity: an absent one is minted and placed; one that cannot be read is refused without
//!   `--new-identity`, both files byte for byte as before, and set aside and renewed with it; a
//!   readable one with the flag is refused;
//! - a Runtime holding the owner refuses the command with `owner_conflict`, nothing written;
//! - a command stopped after each step of the hand-over is completed by running it again, and no
//!   file loses its bytes on any path.

use super::instance_workers::closed_ledger_events;
use super::*;
use crate::codes::test_spellings::TestCode;
use crate::state_root_installation::HandOverStep;
use crate::{HandedIdentity, HandedRecord, StateRootHandOver};
use actingcommand_contract::{FactScalar, RuntimeFactRecord, RuntimePayload};
use serde_json::{Value, json};

/// The state root's record.
const RECORD: &str = "installation.json";
/// The installation's identity file, in its `install` folder.
const IDENTITY: &str = "identity.json";
/// The owner journal.
const JOURNAL: &str = "owner.lock";
/// The tagged runtime fact a claim is mirrored into.
const FACT: &str = "host.state_root_installation";
const INVALID: &str = "state_root_installation_record_invalid";
/// Two installations' numbers.
const THIS_ID: &str = "ffeeddccbbaa99887766554433221100";
const OTHER_ID: &str = "00112233445566778899aabbccddeeff";
/// The claim time of a record a test writes.
const CLAIMED_AT_UNIX_MS: u64 = 1_800_000_000_000;
/// The bytes of an identity file that cannot be read as one.
const CORRUPT_IDENTITY: &[u8] = b"identity";
/// The bytes of a record that cannot be read as one.
const CORRUPT_RECORD: &[u8] = b"{\"schema_version\":";

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

/// An installation root whose identity file holds `bytes`.
fn installation_with(bytes: &[u8]) -> TempDir {
    let installation = installation_root();
    fs::write(install_folder(&installation).join(IDENTITY), bytes).expect("the identity file");
    installation
}

/// A state root whose record holds `bytes`.
fn state_with(bytes: &[u8]) -> TempDir {
    let state = TempDir::new().expect("tempdir");
    fs::write(state.path().join(RECORD), bytes).expect("the record");
    state
}

/// `actingd claim-state-root` of `state` from `installation`.
fn claim(
    state: &TempDir,
    installation: &Path,
    new_identity: bool,
) -> RuntimeHostResult<StateRootHandOver> {
    RuntimeHost::claim_state_root(
        config(state).with_installation_root(installation),
        new_identity,
    )
}

/// The command stopped after the hand-over's step `step`.
fn stopped(state: &TempDir, installation: &Path, new_identity: bool, step: HandOverStep) {
    let error = RuntimeHost::claim_state_root_stopped_for_test(
        config(state).with_installation_root(installation),
        new_identity,
        step,
    )
    .expect_err("the command stops after the step");
    assert_eq!(
        error.code(),
        TestCode::HandOverCrashedForTest.as_str(),
        "{step:?}: {error}"
    );
}

/// The start fixture on `state` from `installation`, with the instance `instance`.
fn start(
    state: &TempDir,
    installation: &Path,
    instance: InstanceId,
) -> RuntimeHostResult<RuntimeHost> {
    RuntimeHost::start(
        config(state).with_installation_root(installation),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance,
            Arc::new(FakeState::default()),
        )),
    )
}

/// A start that passes, closed at once.
fn started_and_closed(state: &TempDir, installation: &Path, instance: InstanceId) {
    start(state, installation, instance)
        .expect("the start passes")
        .close()
        .expect("close the host");
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

/// [`files`] but the owner journal, the record and the kept files.
fn untouched_files(folder: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    files(folder)
        .into_iter()
        .filter(|(path, _)| {
            let path = path.to_string_lossy();
            path != JOURNAL && !path.starts_with(RECORD)
        })
        .collect()
}

/// The names in `folder` set aside from `name` (`<name>.old-<ms>`).
fn kept_names(folder: &Path, name: &str) -> Vec<String> {
    let prefix = format!("{name}.old-");
    names(folder)
        .into_iter()
        .filter(|kept| kept.starts_with(&prefix))
        .collect()
}

/// Whether every file of `before` keeps its bytes in `after`: under its name (the owner journal
/// may grow), or set aside under a kept name of its own (`<name>.old-<ms>`). Nothing is deleted.
fn nothing_deleted(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> bool {
    before.iter().all(|(path, bytes)| {
        let kept = format!("{}.old-", path.to_string_lossy());
        after.get(path).is_some_and(|now| now.starts_with(bytes))
            || after.iter().any(|(other, now)| {
                other.to_string_lossy().starts_with(&kept) && now.as_slice() == bytes.as_slice()
            })
    })
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

/// The one row of a claim's fact.
fn fact_row(fact: &RuntimeFactRecord) -> &BTreeMap<String, FactScalar> {
    let ContractFactValue::RecordList(rows) = &fact.value else {
        panic!("a record list: {fact:?}");
    };
    let [row] = rows.as_slice() else {
        panic!("one row: {rows:?}");
    };
    row
}

fn integer(value: u64) -> FactScalar {
    FactScalar::Integer(i64::try_from(value).expect("a time"))
}

/// Whether `text` is 32 lowercase hexadecimal digits.
fn is_number(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// Rulings 4 and 5: another installation's record is kept byte for byte under its kept name and
// this installation's written; the answer names the old claim's time. The owner journal keeps its
// earlier records byte for byte and gains the command's epoch only, opened and closed, which the
// new record names; the ledger, the integrity key and every other file of the state root are
// untouched. The next start of this installation passes, places nothing and records the new claim
// once, with the old claim's time; a restart records nothing more.
#[test]
fn s12_another_installations_record_is_kept_and_the_next_start_records_the_new_claim_once() {
    let installation = installation_with(&identity(THIS_ID));
    let state = TempDir::new().expect("tempdir");
    let instance = instance_id();
    // The state root another installation's start claimed and ran on: its record and its fact.
    let other = installation_root();
    started_and_closed(&state, other.path(), instance);
    let old_record = fs::read(state.path().join(RECORD)).expect("the other's record");
    let old_claimed_at = json_file(&state.path().join(RECORD))["claimed_at_unix_ms"]
        .as_u64()
        .expect("its claim time");
    let journal_before = fs::read(state.path().join(JOURNAL)).expect("the owner journal");
    let untouched_before = untouched_files(state.path());
    let names_before = names(state.path());
    assert_eq!(
        claim_facts(state.path()).len(),
        1,
        "the other's claim's fact"
    );

    let handed = claim(&state, installation.path(), false).expect("the claim");
    assert_eq!(handed.identity, HandedIdentity::Read);
    assert_eq!(handed.record, HandedRecord::Replaced);
    assert_eq!(handed.previous_claimed_at_unix_ms, Some(old_claimed_at));
    assert!(handed.changed());
    let [kept] = handed.kept.as_slice() else {
        panic!("one file set aside: {:?}", handed.kept);
    };
    let kept_name = kept
        .file_name()
        .expect("a kept name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(kept_names(state.path(), RECORD), [kept_name.clone()]);
    assert_eq!(
        fs::read(kept).ok(),
        Some(old_record),
        "the old record byte for byte"
    );
    let record = json_file(&state.path().join(RECORD));
    assert_eq!(record["installation_id"], json!(THIS_ID));
    assert_eq!(
        record["claimed_at_unix_ms"],
        json!(handed.claimed_at_unix_ms)
    );
    assert_eq!(record["claimed_by"]["pid"], json!(std::process::id()));
    // The owner journal: its earlier records byte for byte, then the command's epoch.
    let journal_after = fs::read(state.path().join(JOURNAL)).expect("the owner journal");
    assert!(
        journal_after.starts_with(&journal_before),
        "the earlier records byte for byte"
    );
    let added = String::from_utf8(journal_after[journal_before.len()..].to_vec())
        .expect("journal lines")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("a journal record"))
        .collect::<Vec<_>>();
    let [opened, closed] = added.as_slice() else {
        panic!("the command's open and close: {added:?}");
    };
    assert_eq!(opened["owner_epoch"], closed["owner_epoch"]);
    assert_eq!(opened["active"], json!(true));
    assert_eq!(closed["active"], json!(false));
    assert_eq!(record["claimed_by"]["owner_epoch"], opened["owner_epoch"]);
    // The ledger, the integrity key and every other file byte for byte; nothing deleted.
    assert_eq!(untouched_files(state.path()), untouched_before);
    let mut names_expected = names_before;
    names_expected.insert(kept_name);
    assert_eq!(names(state.path()), names_expected);

    // This installation's next start passes, places nothing and records the new claim once.
    let host = start(&state, installation.path(), instance).expect("this installation's start");
    assert!(
        host.state_root_claimed().is_none(),
        "the start placed no record"
    );
    host.close().expect("close the host");
    let facts = claim_facts(state.path());
    let [old_fact, new_fact] = facts.as_slice() else {
        panic!("the old claim's fact, then the new claim's once: {facts:?}");
    };
    let row = fact_row(new_fact);
    assert_eq!(
        row.get("claimed_at_unix_ms"),
        Some(&integer(handed.claimed_at_unix_ms))
    );
    assert_eq!(
        row.get("claimed_by_owner_epoch"),
        Some(&FactScalar::String(
            record["claimed_by"]["owner_epoch"]
                .as_str()
                .expect("the record's owner epoch")
                .to_owned()
        ))
    );
    assert_eq!(
        row.get("claimed_by_pid"),
        Some(&FactScalar::Integer(i64::from(std::process::id())))
    );
    assert_eq!(
        row.get("previous_claimed_at_unix_ms"),
        Some(&integer(old_claimed_at)),
        "the old claim's time"
    );
    assert_eq!(row.len(), 4, "no commit and never the number: {row:?}");
    assert!(new_fact.observed_at_unix_ms > old_fact.observed_at_unix_ms);
    assert!(new_fact.observed_at_unix_ms >= handed.claimed_at_unix_ms);

    // A restart records nothing more.
    started_and_closed(&state, installation.path(), instance);
    assert_eq!(
        claim_facts(state.path()).len(),
        2,
        "the new claim's fact once"
    );
}

// Ruling 4: no record: this installation's record is written, nothing set aside. Its own record:
// nothing written, nothing renamed, and the answer says nothing changed (the owner journal gains
// the command's epoch alone).
#[test]
fn s12_no_record_is_written_and_its_own_record_changes_nothing() {
    let installation = installation_with(&identity(THIS_ID));
    let install = install_folder(&installation);
    let state = TempDir::new().expect("tempdir");

    let handed = claim(&state, installation.path(), false).expect("the claim");
    assert_eq!(handed.identity, HandedIdentity::Read);
    assert_eq!(handed.record, HandedRecord::Written);
    assert_eq!(handed.previous_claimed_at_unix_ms, None);
    assert!(handed.kept.is_empty(), "{:?}", handed.kept);
    assert!(handed.changed());
    assert_eq!(
        names(state.path()),
        BTreeSet::from([JOURNAL.to_owned(), RECORD.to_owned()])
    );
    let record = json_file(&state.path().join(RECORD));
    assert_eq!(record["installation_id"], json!(THIS_ID));
    assert_eq!(
        record["claimed_at_unix_ms"],
        json!(handed.claimed_at_unix_ms)
    );

    let record_bytes = fs::read(state.path().join(RECORD)).expect("the record");
    let install_before = files(&install);
    let again = claim(&state, installation.path(), false).expect("the claim again");
    assert_eq!(again.identity, HandedIdentity::Read);
    assert_eq!(again.record, HandedRecord::Unchanged);
    assert!(!again.changed(), "the answer says nothing changed");
    assert!(again.kept.is_empty(), "{:?}", again.kept);
    assert_eq!(again.previous_claimed_at_unix_ms, None);
    assert_eq!(again.claimed_at_unix_ms, handed.claimed_at_unix_ms);
    assert_eq!(
        fs::read(state.path().join(RECORD)).ok(),
        Some(record_bytes),
        "nothing written"
    );
    assert_eq!(
        names(state.path()),
        BTreeSet::from([JOURNAL.to_owned(), RECORD.to_owned()]),
        "nothing renamed"
    );
    assert_eq!(files(&install), install_before);
}

// Ruling 4: a record that cannot be read is set aside under its kept name, byte for byte, and
// this installation's written (the rescue row notes the caveat for every replaced record).
#[test]
fn s12_an_unreadable_record_is_set_aside_and_replaced() {
    let installation = installation_with(&identity(THIS_ID));
    let state = state_with(CORRUPT_RECORD);

    let handed = claim(&state, installation.path(), false).expect("the claim");
    assert_eq!(handed.record, HandedRecord::Replaced);
    assert_eq!(handed.previous_claimed_at_unix_ms, None, "no time to read");
    let [kept] = handed.kept.as_slice() else {
        panic!("one file set aside: {:?}", handed.kept);
    };
    assert_eq!(
        fs::read(kept).ok(),
        Some(CORRUPT_RECORD.to_vec()),
        "byte for byte"
    );
    assert_eq!(kept_names(state.path(), RECORD).len(), 1);
    assert_eq!(
        json_file(&state.path().join(RECORD))["installation_id"],
        json!(THIS_ID)
    );
}

// Ruling 3: the identity. An absent one is minted and placed; one that cannot be read is refused
// without `--new-identity`, both files byte for byte as before and nothing else written, and with
// it is set aside under its kept name and a new one minted, the other's record replaced; a
// readable one with the flag is refused, nothing written.
#[test]
fn s12_the_identity_is_minted_refused_renewed_or_kept() {
    // Absent: minted and placed, the record naming it.
    let installation = installation_root();
    let install = install_folder(&installation);
    let state = TempDir::new().expect("tempdir");
    let handed = claim(&state, installation.path(), false).expect("the claim");
    assert_eq!(handed.identity, HandedIdentity::Minted);
    assert_eq!(handed.record, HandedRecord::Written);
    assert_eq!(
        names(&install),
        BTreeSet::from(["state.json".to_owned(), IDENTITY.to_owned()]),
        "no part file left"
    );
    let number = json_file(&install.join(IDENTITY))["installation_id"]
        .as_str()
        .expect("the number")
        .to_owned();
    assert!(is_number(&number), "{number}");
    assert_eq!(
        json_file(&state.path().join(RECORD))["installation_id"],
        json!(number)
    );

    // Unreadable, without the flag: refused, both files byte for byte, nothing else written.
    let installation = installation_with(CORRUPT_IDENTITY);
    let install = install_folder(&installation);
    let state = state_with(&record(OTHER_ID));
    let install_before = files(&install);
    let state_before = files(state.path());
    let error = claim(&state, installation.path(), false).expect_err("refused");
    assert_eq!(error.code(), INVALID, "{error}");
    assert_eq!(
        error.outcome().value("path"),
        Some(&json!(install.join(IDENTITY).to_string_lossy()))
    );
    assert_eq!(files(&install), install_before, "the identity as it was");
    assert_eq!(files(state.path()), state_before, "nothing written");

    // Unreadable, with the flag: set aside, a new one minted, the other's record replaced.
    let handed = claim(&state, installation.path(), true).expect("the claim with the flag");
    assert_eq!(handed.identity, HandedIdentity::Renewed);
    assert_eq!(handed.record, HandedRecord::Replaced);
    assert_eq!(handed.previous_claimed_at_unix_ms, Some(CLAIMED_AT_UNIX_MS));
    let [kept_identity, kept_record] = handed.kept.as_slice() else {
        panic!(
            "the identity, then the record, set aside: {:?}",
            handed.kept
        );
    };
    assert_eq!(
        fs::read(kept_identity).ok(),
        Some(CORRUPT_IDENTITY.to_vec())
    );
    assert_eq!(fs::read(kept_record).ok(), Some(record(OTHER_ID)));
    assert_eq!(kept_names(&install, IDENTITY).len(), 1);
    let number = json_file(&install.join(IDENTITY))["installation_id"]
        .as_str()
        .expect("the new number")
        .to_owned();
    assert!(is_number(&number), "{number}");
    assert_eq!(
        json_file(&state.path().join(RECORD))["installation_id"],
        json!(number)
    );

    // Readable, with the flag: refused, nothing written.
    let installation = installation_with(&identity(THIS_ID));
    let install = install_folder(&installation);
    let state = TempDir::new().expect("tempdir");
    let install_before = files(&install);
    let error = claim(&state, installation.path(), true).expect_err("refused");
    assert_eq!(error.code(), "invalid_arguments", "{error}");
    assert_eq!(files(&install), install_before, "the identity as it was");
    assert!(names(state.path()).is_empty(), "nothing written");
}

// Ruling 1: a Runtime holding the owner refuses the command with the existing `owner_conflict`;
// the record, the identity and the state root's names are as they were.
#[test]
fn s12_a_runtime_holding_the_owner_refuses_the_claim_and_nothing_is_written() {
    let installation = installation_with(&identity(THIS_ID));
    let install = install_folder(&installation);
    let state = TempDir::new().expect("tempdir");
    let other = installation_root();
    let running = start(&state, other.path(), instance_id()).expect("the other's Runtime");
    let record_before = fs::read(state.path().join(RECORD)).expect("the other's record");
    let install_before = files(&install);

    let refused = claim(&state, installation.path(), false);
    let names_after = names(state.path());
    running.close().expect("close the host");
    let error = refused.expect_err("refused while a Runtime holds the owner");
    assert_eq!(error.code(), "owner_conflict", "{error}");
    assert_eq!(
        fs::read(state.path().join(RECORD)).ok(),
        Some(record_before),
        "the record as it was"
    );
    assert!(
        !names_after
            .iter()
            .any(|name| name.starts_with(&format!("{RECORD}."))),
        "nothing set aside: {names_after:?}"
    );
    assert_eq!(files(&install), install_before, "nothing minted");
}

// Ruling 4: a command stopped after each step of the hand-over (a test-only stop, as a process
// that died there) is completed by running it again; no file loses its bytes on any path, and the
// end is the same as without the stop: this installation's record, the old record under one kept
// name. With `--new-identity`: after the new identity was placed the identity is readable, so the
// flag is refused, nothing written, and the command without it completes the hand-over.
#[test]
fn s12_a_command_stopped_after_each_step_is_completed_by_running_it_again() {
    for step in [HandOverStep::RecordSetAside, HandOverStep::RecordPlaced] {
        let installation = installation_with(&identity(THIS_ID));
        let state = state_with(&record(OTHER_ID));
        let before = files(state.path());
        stopped(&state, installation.path(), false, step);
        let stopped_at = files(state.path());
        assert!(
            nothing_deleted(&before, &stopped_at),
            "{step:?}: {stopped_at:?}"
        );

        let handed = claim(&state, installation.path(), false).expect("the command again");
        let expected = match step {
            HandOverStep::RecordSetAside => HandedRecord::Written,
            _ => HandedRecord::Unchanged,
        };
        assert_eq!(handed.record, expected, "{step:?}");
        let after = files(state.path());
        assert!(nothing_deleted(&stopped_at, &after), "{step:?}: {after:?}");
        assert!(nothing_deleted(&before, &after), "{step:?}: {after:?}");
        assert_eq!(
            json_file(&state.path().join(RECORD))["installation_id"],
            json!(THIS_ID),
            "{step:?}"
        );
        let kept = kept_names(state.path(), RECORD);
        assert_eq!(kept.len(), 1, "{step:?}: {kept:?}");
        assert_eq!(
            fs::read(state.path().join(&kept[0])).ok(),
            Some(record(OTHER_ID)),
            "{step:?}"
        );
    }

    for step in [HandOverStep::IdentitySetAside, HandOverStep::IdentityPlaced] {
        let installation = installation_with(CORRUPT_IDENTITY);
        let install = install_folder(&installation);
        let state = state_with(&record(OTHER_ID));
        let install_before = files(&install);
        let state_before = files(state.path());
        stopped(&state, installation.path(), true, step);
        let install_stopped = files(&install);
        let state_stopped = files(state.path());
        assert!(
            nothing_deleted(&install_before, &install_stopped),
            "{step:?}"
        );
        assert!(nothing_deleted(&state_before, &state_stopped), "{step:?}");

        let handed = if step == HandOverStep::IdentityPlaced {
            let error = claim(&state, installation.path(), true).expect_err("the flag refused");
            assert_eq!(error.code(), "invalid_arguments", "{error}");
            assert_eq!(files(&install), install_stopped, "nothing written");
            assert_eq!(files(state.path()), state_stopped, "nothing written");
            claim(&state, installation.path(), false).expect("the command without the flag")
        } else {
            claim(&state, installation.path(), true).expect("the command again")
        };
        assert_eq!(handed.record, HandedRecord::Replaced, "{step:?}");
        let install_after = files(&install);
        let state_after = files(state.path());
        assert!(nothing_deleted(&install_before, &install_after), "{step:?}");
        assert!(nothing_deleted(&state_before, &state_after), "{step:?}");
        let kept = kept_names(&install, IDENTITY);
        assert_eq!(kept.len(), 1, "{step:?}: {kept:?}");
        assert_eq!(
            fs::read(install.join(&kept[0])).ok(),
            Some(CORRUPT_IDENTITY.to_vec()),
            "{step:?}"
        );
        let number = json_file(&install.join(IDENTITY))["installation_id"]
            .as_str()
            .expect("the new number")
            .to_owned();
        assert!(is_number(&number), "{step:?}: {number}");
        assert_eq!(
            json_file(&state.path().join(RECORD))["installation_id"],
            json!(number),
            "{step:?}"
        );
        assert_eq!(kept_names(state.path(), RECORD).len(), 1, "{step:?}");
    }
}
