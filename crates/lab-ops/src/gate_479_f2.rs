// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #479 F-2 (register §1.482; contracts/device-binding.md, "An iOS handset"; the
// coordinator's rulings in work order rd-1118/brief-479-f2-ios-handset-config-binding.md): the
// Lab's `devices` lists the phones `acib devices` sees beside adb's rows, and `devices bind` binds
// an iOS handset's phone by its key. `acib` is scripted the way the DB-2 tests script adb: the
// install root's `tools\acib.exe` is an empty file the scripted run stands for, and the run answers
// one phone's line as `acib` makes it, its keys under the salts its call's environment holds by
// the variables its arguments name. No device, no adb, no `acib`, no process and no clock read
// here; the serial and the phone's identifier are neutral fakes and never leave the scripts.
// - f2_t1: each phone's row says `system` `ios`, its `device_key` under the caller's salt, else
//   under the installation's own secret; the child's arguments hold no salt and its environment
//   holds the salts under the Lab's names, the caller's variable not inherited; adb's rows are
//   what they were.
// - f2_t2: `acib` not installed, Apple's service not answering, `acib` failing, not started or
//   answering a line the Lab does not read: adb's rows answered, the one member
//   `ios_devices_unlisted` with its word, the listing a success; with no iOS handset configured
//   and no `acib`, the answer of before.
// - f2_t3: a bind writes the iOS row with the key under the installation's secret and no
//   identifier and no salt anywhere in the file, which is version 2; a listing finds the alias on
//   the phone's row; unbind removes the row and the file is version 1 again.
// - f2_t4: each refusal of a bind: a key no phone has, an alias that is no handset, a phone bound
//   to another alias, `acib` not installed, an Android device's key or handle for an iOS handset
//   and an iOS phone's key for an Android one; none writes anything.
// - f2_t5: by construction, no row, answer, member or file row of this slice has a member that
//   could hold an identifier of a phone, and a call's `Debug` shows no salt.
// - f2_t6: the private file reads a version-1 file as before and writes it back byte for byte; a
//   version-2 file holds iOS rows, each key of its form and none twice; a version-1 file with an
//   iOS row and a row of another shape are malformed; fix round 1: a version-1 Android row with a
//   `system` member, `null` included, is malformed, as before this slice.
// - f2_t7 (fix round 1, item 4): `acib` is called with its own deadline and followed within the
//   Lab's: on a scripted child and a clock that moves only when it is asked to wait, a child that
//   never ends is ended past the wait, `no_answer_in_time`; one that prints without end is ended
//   past the cap, `answer_too_long` (its output or its errors); one that ends gives its line;
//   one that cannot be ended is `not_ended`; a listing answers adb's rows and the word.

use crate::cli_line::Exit;
use crate::devices::{
    self, ACIB_BINDING_SALT_VARIABLE, ACIB_KEY_SALT_VARIABLE, AcibBounds, AcibCall, AcibChild,
    AcibClock, AcibRun, AdbRun,
};
use actingcommand_contract::{
    DeviceBinding, DeviceBindingProblem, DeviceBindingsFile, DeviceBoundVia, DeviceSystem,
    InstanceId, OriginModule, RuntimeFactRecord, RuntimeFactScope, RuntimeFactSnapshot,
    SecretField, SecretFingerprinter, device_bindings_schema, parse_device_bindings,
};
use actingcommand_ledger::Sha256SecretFingerprinter;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The candidate configuration's iOS handsets, its Android handset and its emulator.
const IPHONE: &str = "neutral.iphone";
const IPHONE_B: &str = "neutral.iphone-b";
const PHONE: &str = "neutral.phone";
const EMULATOR: &str = "neutral.emulator";
/// The one Android device adb lists, and the one phone the scripted `acib` lists: the identifier
/// `acib` keeps to itself, which nothing of the Lab may ever show.
const SERIAL: &str = "TESTSERIAL0479";
const IDENTIFIER: &str = "TESTPHONEID0479";
/// The candidate's own secret and a caller's salt, read from the variable [`SALT_VARIABLE`].
const SECRET: &str = "neutral-secret-0479-f2";
const SALT_A: &str = "neutral-salt-a-0479-0001";
const SALT_VARIABLE: &str = "NEUTRAL_DEVICE_KEY_SALT";
/// adb's listing: the Android device on USB, transport id 7.
const LISTING: &str = "List of devices attached\n\
    TESTSERIAL0479         device usb:1-1 product:neutral model:Neutral_Phone device:neutral transport_id:7\n\n";

/// A root that holds its tools, `acib` laid unless `with_acib` is false, and a candidate
/// configuration naming a state root that does not exist yet, all in `temp`.
struct Roots {
    /// Kept so that the folders live as long as the test.
    _temp: TempDir,
    root: PathBuf,
    candidate: PathBuf,
    state_root: PathBuf,
}

fn roots(instances: &Value, with_acib: bool) -> Roots {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("tools").join("platform-tools")).expect("the root's tools");
    if with_acib {
        fs::write(acib_of(&root), b"").expect("the laid acib");
    }
    let state_root = temp.path().join("state");
    let candidate = temp.path().join("candidate.json");
    fs::write(
        &candidate,
        json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": state_root,
            "secret_fingerprint_salt": SECRET,
            "instances": instances,
        })
        .to_string(),
    )
    .expect("the candidate");
    Roots {
        _temp: temp,
        root,
        candidate,
        state_root,
    }
}

/// Two iOS handsets, an Android handset and an emulator.
fn mixed() -> Value {
    json!([
        {"alias": IPHONE, "device": {"kind": "handset", "system": "ios"}},
        {"alias": IPHONE_B, "device": {"kind": "handset", "system": "ios"}},
        {"alias": PHONE, "device": {"kind": "handset"}},
        {"alias": EMULATOR, "port": 5555},
    ])
}

fn acib_of(root: &Path) -> PathBuf {
    root.join("tools").join("acib.exe")
}

fn adb_of(root: &Path) -> PathBuf {
    root.join("tools").join("platform-tools").join("adb.exe")
}

/// adb's listing, answered only for the adb at `adb`.
fn listing_of(adb: PathBuf) -> impl Fn(&Path) -> AdbRun {
    move |asked: &Path| {
        if asked == adb.as_path() {
            AdbRun::Printed(LISTING.to_owned())
        } else {
            AdbRun::NotStarted(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }
}

/// The contract's fingerprint of `identifier` under `salt`, the function `acib` and the ledger
/// share.
fn fingerprint(salt: &str, identifier: &str) -> String {
    Sha256SecretFingerprinter::new(salt)
        .expect("a fingerprinter")
        .fingerprint(SecretField::DeviceSerial, identifier)
        .expect("a fingerprint")
        .as_str()
        .to_owned()
}

/// A scripted run of `acib`.
type ScriptedAcib<'a> = &'a dyn Fn(&Path, &AcibCall) -> AcibRun;

/// One call the scripted `acib` received.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Received {
    arguments: Vec<String>,
    environment: Vec<(String, String)>,
    removed: Vec<String>,
}

/// The scripted `acib devices`, as `acib` answers: the one phone on USB, its `device_key` under the
/// salt in the variable `--key-salt-env` names and its `binding_key` under the one
/// `--binding-salt-env` names, each from the call's environment; every call recorded in `calls`.
fn scripted(calls: &RefCell<Vec<Received>>) -> impl Fn(&Path, &AcibCall) -> AcibRun + '_ {
    move |_: &Path, call: &AcibCall| {
        calls.borrow_mut().push(Received {
            arguments: call.arguments.clone(),
            environment: call
                .environment
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
            removed: call.removed.clone(),
        });
        let salt_of = |flag: &str| {
            let at = call
                .arguments
                .iter()
                .position(|argument| argument == flag)?;
            let variable = call.arguments.get(at + 1)?;
            call.environment
                .iter()
                .find(|(name, _)| name == variable)
                .map(|(_, value)| value.clone())
        };
        let mut row = json!({"device": 3, "connection": "usb"});
        if let Some(salt) = salt_of("--key-salt-env") {
            row["device_key"] = json!(fingerprint(&salt, IDENTIFIER));
        }
        if let Some(salt) = salt_of("--binding-salt-env") {
            row["binding_key"] = json!(fingerprint(&salt, IDENTIFIER));
        }
        AcibRun::Answered(
            json!({"command": "devices", "devices": [row]})
                .to_string()
                .into_bytes(),
        )
    }
}

/// An environment whose one variable [`SALT_VARIABLE`] holds [`SALT_A`].
fn salted(name: &str) -> Option<OsString> {
    (name == SALT_VARIABLE).then(|| OsString::from(SALT_A))
}

fn nothing(_: &str) -> Option<OsString> {
    None
}

fn words(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

fn text(path: &Path) -> String {
    path.to_str()
        .expect("the temporary folder is UTF-8")
        .to_owned()
}

/// `devices` with `words` and `extra`, against the candidate of `at` and its root.
fn run(
    at: &Roots,
    head: &[&str],
    extra: &[&str],
    acib: &dyn Fn(&Path, &AcibCall) -> AcibRun,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Exit {
    let (candidate, root) = (text(&at.candidate), text(&at.root));
    let mut argv = words(head);
    argv.extend(words(extra));
    argv.extend(words(&[
        "--config",
        candidate.as_str(),
        "--root",
        root.as_str(),
    ]));
    devices::dispatch_in_with_acib(&argv, &listing_of(adb_of(&at.root)), acib, environment)
        .expect("devices answers")
}

/// The success line of `exit`: its members but `outcome`, and its outcome's code.
fn answered(exit: &Exit) -> (Value, Value) {
    assert_eq!(exit.code, 0, "{exit:?}");
    let mut line = exit.line.clone().expect("the line encodes");
    let code = line["outcome"]["code"].clone();
    if let Some(members) = line.as_object_mut() {
        members.remove("outcome");
    }
    (line, code)
}

/// The outcome of the refused line `exit`.
fn refused(exit: &Exit) -> Value {
    assert_eq!(exit.code, 1, "{exit:?}");
    let line = exit.line.clone().expect("the line encodes");
    line["outcome"].clone()
}

/// What `exit` prints.
fn printed(exit: &Exit) -> String {
    let line = exit.line.as_ref().map(Value::to_string).unwrap_or_default();
    let developer = exit
        .developer
        .as_ref()
        .map(|developer| developer.line("actingctl"))
        .unwrap_or_default();
    format!("{line}\n{developer}")
}

/// Neither the serial, the phone's identifier, a salt nor the secret in `printed`.
fn nothing_private(printed: &str) {
    for value in [SERIAL, IDENTIFIER, SALT_A, SECRET] {
        assert!(!printed.contains(value), "{value} in {printed}");
    }
}

fn rows(members: &Value) -> Vec<Value> {
    members["devices"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a device list: {members}"))
}

/// The private binding file's bytes: the one file of the state root's `private` folder.
fn private_file(state_root: &Path) -> Option<Vec<u8>> {
    let folder = state_root.join("private");
    if !folder.exists() {
        return None;
    }
    let mut entries = fs::read_dir(&folder)
        .expect("list the private folder")
        .map(|entry| entry.expect("a folder entry").path())
        .collect::<Vec<_>>();
    assert!(entries.len() <= 1, "{entries:?}");
    entries
        .pop()
        .map(|path| fs::read(path).expect("read the file"))
}

/// The one item of `items`.
fn only<T: std::fmt::Debug>(mut items: Vec<T>) -> T {
    assert_eq!(items.len(), 1, "exactly one: {items:?}");
    items.remove(0)
}

/// The members' names of the JSON object `value`.
fn members(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .cloned()
        .collect()
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// adb's one row, with its key under `salt`, as DB-2 answers it.
fn android_row(salt: &str) -> Value {
    json!({
        "handle": "usb:7",
        "device_key": fingerprint(salt, SERIAL),
        "transport": "usb",
        "state": "device",
        "model": "Neutral_Phone",
    })
}

#[test]
fn f2_t1_the_listing_adds_each_phone_with_its_system_and_its_key_under_the_callers_salt() {
    let at = roots(&mixed(), true);
    let calls = RefCell::new(Vec::new());
    let acib = scripted(&calls);

    // Under the caller's salt, read from the Lab's own variable.
    let exit = run(
        &at,
        &["devices", "list"],
        &["--key-salt-env", SALT_VARIABLE],
        &acib,
        &salted,
    );
    let (listed, code) = answered(&exit);
    assert_eq!(code, "device_list_reported", "{exit:?}");
    assert_eq!(
        rows(&listed),
        [
            android_row(SALT_A),
            json!({"device_key": fingerprint(SALT_A, IDENTIFIER), "system": "ios", "transport": "usb"}),
        ]
    );
    assert!(listed.get("ios_devices_unlisted").is_none(), "{listed}");
    nothing_private(&printed(&exit));
    let received = only(calls.borrow().clone());
    // Fix round 1: and `acib`'s own deadline, 10 s.
    assert_eq!(
        received.arguments,
        [
            "devices",
            "--key-salt-env",
            ACIB_KEY_SALT_VARIABLE,
            "--binding-salt-env",
            ACIB_BINDING_SALT_VARIABLE,
            "--timeout-ms",
            "10000",
        ]
    );
    for argument in &received.arguments {
        assert!(
            !argument.contains(SALT_A) && !argument.contains(SECRET),
            "a salt in the arguments: {argument}"
        );
    }
    assert_eq!(
        received.environment,
        [
            (ACIB_KEY_SALT_VARIABLE.to_owned(), SALT_A.to_owned()),
            (ACIB_BINDING_SALT_VARIABLE.to_owned(), SECRET.to_owned()),
        ]
    );
    assert_eq!(received.removed, [SALT_VARIABLE]);

    // Without a salt: the installation's own secret, as for adb's rows.
    calls.borrow_mut().clear();
    let exit = run(&at, &["devices", "list"], &[], &acib, &nothing);
    let (listed, _) = answered(&exit);
    assert_eq!(
        rows(&listed),
        [
            android_row(SECRET),
            json!({"device_key": fingerprint(SECRET, IDENTIFIER), "system": "ios", "transport": "usb"}),
        ]
    );
    let received = only(calls.borrow().clone());
    assert_eq!(
        received.environment,
        [
            (ACIB_KEY_SALT_VARIABLE.to_owned(), SECRET.to_owned()),
            (ACIB_BINDING_SALT_VARIABLE.to_owned(), SECRET.to_owned()),
        ]
    );
    assert!(received.removed.is_empty());
    for argument in &received.arguments {
        assert!(!argument.contains(SECRET), "{argument}");
    }
    nothing_private(&printed(&exit));
}

#[test]
fn f2_t2_without_acib_or_with_its_service_down_or_failing_the_android_rows_answer_with_one_member()
{
    let android = [android_row(SECRET)];
    // acib not installed: nothing is started; adb's rows and the member's word.
    let at = roots(&mixed(), false);
    let never = |_: &Path, _: &AcibCall| -> AcibRun { panic!("acib is not installed") };
    let exit = run(&at, &["devices", "list"], &[], &never, &nothing);
    let (listed, code) = answered(&exit);
    assert_eq!(code, "device_list_reported");
    assert_eq!(rows(&listed), android);
    assert_eq!(
        listed["ios_devices_unlisted"],
        json!({"why": "not_installed"})
    );

    // acib laid: Apple's service not answering, acib failing, not started, answering a line the
    // Lab does not read, refusing with a word that is none of its own.
    let at = roots(&mixed(), true);
    let refusing = |line: &'static str| {
        move |_: &Path, _: &AcibCall| AcibRun::Refused(line.as_bytes().to_vec())
    };
    let unreachable = refusing(r#"{"command":"devices","problem":"usbmuxd_unreachable"}"#);
    let list_failed = refusing(r#"{"command":"devices","problem":"usbmuxd_list_failed"}"#);
    let deadline = refusing(r#"{"command":"devices","problem":"deadline_passed"}"#);
    let foreign = refusing(r#"{"command":"devices","problem":"neutral_unknown_word"}"#);
    let not_started = |_: &Path, _: &AcibCall| {
        AcibRun::NotStarted(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    };
    let garbage = |_: &Path, _: &AcibCall| AcibRun::Answered(b"not a line".to_vec());
    let cases: [(ScriptedAcib<'_>, &str); 6] = [
        (&unreachable, "usbmuxd_unreachable"),
        (&list_failed, "usbmuxd_list_failed"),
        (&deadline, "deadline_passed"),
        (&not_started, "not_started"),
        (&garbage, "answer_unreadable"),
        (&foreign, "answer_unreadable"),
    ];
    for (acib, word) in cases {
        let exit = run(&at, &["devices", "list"], &[], acib, &nothing);
        let (listed, code) = answered(&exit);
        assert_eq!(code, "device_list_reported", "{word}");
        assert_eq!(rows(&listed), android, "{word}");
        assert_eq!(listed["ios_devices_unlisted"], json!({"why": word}));
        nothing_private(&printed(&exit));
    }

    // No iOS handset configured and no acib: the answer of before, no member, no row's system.
    let at = roots(
        &json!([{"alias": PHONE, "device": {"kind": "handset"}}]),
        false,
    );
    let exit = run(&at, &["devices", "list"], &[], &never, &nothing);
    let (listed, _) = answered(&exit);
    assert_eq!(listed, json!({"devices": android}));
}

#[test]
fn f2_t3_a_bind_writes_the_key_under_the_secret_and_no_identifier_and_unbind_removes_it() {
    let at = roots(&mixed(), true);
    let calls = RefCell::new(Vec::new());
    let acib = scripted(&calls);
    let listed_key = fingerprint(SALT_A, IDENTIFIER);
    let exit = run(
        &at,
        &["devices", "bind", IPHONE],
        &[
            "--device-key",
            listed_key.as_str(),
            "--key-salt-env",
            SALT_VARIABLE,
        ],
        &acib,
        &salted,
    );
    let (answer, code) = answered(&exit);
    assert_eq!(code, "device_binding_changed");
    assert_eq!(
        answer,
        json!({
            "alias": IPHONE,
            "device_key": listed_key,
            "system": "ios",
            "restart_required": true,
            "config": text(&at.candidate),
        })
    );
    nothing_private(&printed(&exit));
    let received = calls.borrow().clone();
    assert_eq!(received.len(), 1, "{received:?}");
    assert!(
        received[0]
            .arguments
            .iter()
            .all(|argument| !argument.contains(SALT_A) && !argument.contains(SECRET))
    );

    // The file: version 2, the iOS row with the phone's key under the secret, nothing else of it.
    let bytes = private_file(&at.state_root).expect("the private file");
    let written = String::from_utf8(bytes.clone()).expect("the file is UTF-8");
    nothing_private(&written);
    let file: Value = serde_json::from_slice(&bytes).expect("the file is JSON");
    assert_eq!(file["schema"], "actingcommand.device-bindings.v2");
    let row = only(file["bindings"].as_array().cloned().unwrap_or_default());
    assert_eq!(
        members(&row),
        names(&[
            "alias",
            "system",
            "device_key",
            "bound_at_unix_ms",
            "bound_via"
        ])
    );
    assert_eq!(
        (
            &row["alias"],
            &row["system"],
            &row["device_key"],
            &row["bound_via"]
        ),
        (
            &json!(IPHONE),
            &json!("ios"),
            &json!(fingerprint(SECRET, IDENTIFIER)),
            &json!("cli")
        )
    );
    let read = parse_device_bindings(&bytes).expect("the file's own rules");
    let binding = read.get(IPHONE).expect("the phone's binding");
    assert_eq!(binding.system(), DeviceSystem::Ios);
    assert_eq!(binding.serial(), None);

    // A listing finds the alias on the phone's row.
    let exit = run(&at, &["devices", "list"], &[], &acib, &nothing);
    let (listed, _) = answered(&exit);
    assert_eq!(rows(&listed)[1]["bound_alias"], IPHONE, "{listed}");
    assert!(rows(&listed)[0].get("bound_alias").is_none(), "{listed}");

    // unbind removes it; the file is version 1 again, with no binding.
    let state = text(&at.state_root);
    let exit = devices::dispatch_in_with_acib(
        &words(&["devices", "unbind", IPHONE, "--state-root", state.as_str()]),
        &listing_of(adb_of(&at.root)),
        &acib,
        &nothing,
    )
    .expect("devices answers");
    let (answer, _) = answered(&exit);
    assert_eq!(
        answer,
        json!({"alias": IPHONE, "unbound": true, "restart_required": true})
    );
    let file: Value = serde_json::from_slice(&private_file(&at.state_root).expect("the file"))
        .expect("the file is JSON");
    assert_eq!(
        file,
        json!({"schema": "actingcommand.device-bindings.v1", "bindings": []})
    );
}

#[test]
fn f2_t4_each_refusal_of_a_bind_names_its_word_and_writes_nothing() {
    let at = roots(&mixed(), true);
    let calls = RefCell::new(Vec::new());
    let acib = scripted(&calls);
    let salt = ["--key-salt-env", SALT_VARIABLE];
    let bind = |alias: &str, device: &[&str], acib: &dyn Fn(&Path, &AcibCall) -> AcibRun| {
        let mut extra = device.to_vec();
        if device.first() == Some(&"--device-key") {
            extra.extend(salt);
        }
        run(&at, &["devices", "bind", alias], &extra, acib, &salted)
    };
    let code = |exit: &Exit| refused(exit)["code"].clone();
    let values = |exit: &Exit| refused(exit)["values"].clone();
    let phone_key = fingerprint(SALT_A, IDENTIFIER);
    let android_key = fingerprint(SALT_A, SERIAL);

    // A key no phone has.
    let unknown = format!("sha256:{}", "0".repeat(64));
    let exit = bind(IPHONE, &["--device-key", unknown.as_str()], &acib);
    assert_eq!(code(&exit), "device_key_unknown", "{exit:?}");
    // An alias that is no handset.
    let exit = bind(EMULATOR, &["--device-key", phone_key.as_str()], &acib);
    assert_eq!(code(&exit), "device_kind_unsupported", "{exit:?}");
    // An Android device's key, and its handle, for an iOS handset.
    let exit = bind(IPHONE, &["--device-key", android_key.as_str()], &acib);
    assert_eq!(code(&exit), "device_row_unbindable", "{exit:?}");
    assert_eq!(
        values(&exit),
        json!({"alias": IPHONE, "device_row_problem": "system_mismatch", "handle": "usb:7"})
    );
    let exit = bind(IPHONE, &["usb:7"], &acib);
    assert_eq!(
        values(&exit),
        json!({"alias": IPHONE, "device_row_problem": "system_mismatch", "handle": "usb:7"})
    );
    // An iOS phone's key for an Android handset.
    let exit = bind(PHONE, &["--device-key", phone_key.as_str()], &acib);
    assert_eq!(code(&exit), "device_row_unbindable", "{exit:?}");
    assert_eq!(
        values(&exit),
        json!({"alias": PHONE, "device_row_problem": "system_mismatch"})
    );
    // acib not installed.
    let never = |_: &Path, _: &AcibCall| -> AcibRun { panic!("acib is not installed") };
    fs::remove_file(acib_of(&at.root)).expect("take acib away");
    let exit = bind(IPHONE, &["--device-key", phone_key.as_str()], &never);
    assert_eq!(code(&exit), "device_ios_list_failed", "{exit:?}");
    assert_eq!(
        values(&exit),
        json!({"alias": IPHONE, "ios_list_problem": "not_installed"})
    );
    nothing_private(&printed(&exit));
    assert!(private_file(&at.state_root).is_none(), "nothing is written");

    // A phone bound to another alias: the same word as an Android device's.
    fs::write(acib_of(&at.root), b"").expect("lay acib again");
    let exit = bind(IPHONE, &["--device-key", phone_key.as_str()], &acib);
    answered(&exit);
    let before = private_file(&at.state_root).expect("the first binding");
    let exit = bind(IPHONE_B, &["--device-key", phone_key.as_str()], &acib);
    assert_eq!(code(&exit), "device_bindings_invalid", "{exit:?}");
    assert_eq!(
        refused(&exit)["values"]["device_binding_problem"],
        "duplicate_serial"
    );
    assert_eq!(private_file(&at.state_root), Some(before));
    nothing_private(&printed(&exit));
}

#[test]
fn f2_t5_no_row_answer_or_file_row_of_this_slice_has_a_member_for_an_identifier() {
    // The members an identifier of a phone could hide in: none of them is ever a member here.
    let forbidden = names(&[
        "udid",
        "serial",
        "serial_number",
        "identifier",
        "name",
        "device_name",
        "unique_device_id",
        "address",
    ]);
    let disjoint = |shown: &BTreeSet<String>| {
        assert!(shown.is_disjoint(&forbidden), "{shown:?}");
    };

    // The private file's iOS row, and its Android row as it was.
    let ios = serde_json::to_value(DeviceBinding::ios(
        IPHONE,
        fingerprint(SECRET, IDENTIFIER),
        1,
        DeviceBoundVia::Cli,
    ))
    .expect("the row's JSON");
    assert_eq!(
        members(&ios),
        names(&[
            "alias",
            "system",
            "device_key",
            "bound_at_unix_ms",
            "bound_via"
        ])
    );
    disjoint(&members(&ios));
    let android = serde_json::to_value(DeviceBinding::new(PHONE, SERIAL, 1, DeviceBoundVia::Cli))
        .expect("the row's JSON");
    assert_eq!(
        members(&android),
        names(&["alias", "serial", "bound_at_unix_ms", "bound_via"])
    );
    let shown = format!(
        "{:?}",
        DeviceBinding::ios(
            IPHONE,
            fingerprint(SECRET, IDENTIFIER),
            1,
            DeviceBoundVia::Cli
        )
    );
    assert!(!shown.contains("sha256:"), "{shown}");

    // A listing's phone row, a bind's answer and the member of a listing without phones.
    let at = roots(&mixed(), true);
    let calls = RefCell::new(Vec::new());
    let acib = scripted(&calls);
    let exit = run(&at, &["devices", "list"], &[], &acib, &nothing);
    let (listed, _) = answered(&exit);
    let phone = rows(&listed)[1].clone();
    assert!(
        members(&phone).is_subset(&names(&[
            "device_key",
            "system",
            "transport",
            "bound_alias"
        ])),
        "{phone}"
    );
    disjoint(&members(&phone));
    let key = fingerprint(SECRET, IDENTIFIER);
    let exit = run(
        &at,
        &["devices", "bind", IPHONE],
        &["--device-key", key.as_str()],
        &acib,
        &nothing,
    );
    let (answer, _) = answered(&exit);
    assert_eq!(
        members(&answer),
        names(&[
            "alias",
            "device_key",
            "system",
            "restart_required",
            "config"
        ])
    );
    let failing = |_: &Path, _: &AcibCall| {
        AcibRun::Refused(br#"{"command":"devices","problem":"usbmuxd_unreachable"}"#.to_vec())
    };
    let exit = run(&at, &["devices", "list"], &[], &failing, &nothing);
    let (listed, _) = answered(&exit);
    assert_eq!(members(&listed["ios_devices_unlisted"]), names(&["why"]));

    // A call's Debug names its variables and none of their values.
    let call = AcibCall {
        arguments: vec!["devices".to_owned()],
        environment: vec![(ACIB_KEY_SALT_VARIABLE, SALT_A.to_owned())],
        removed: Vec::new(),
    };
    let shown = format!("{call:?}");
    assert!(
        shown.contains(ACIB_KEY_SALT_VARIABLE) && !shown.contains(SALT_A),
        "{shown}"
    );

    // The status row of an iOS handset: kind, system and whether it is bound, nothing else; an
    // instance without the facts has none.
    let ids = actingcommand_contract::IdentifierIssuer::new().expect("issuer");
    let mint = || -> InstanceId { *ids.mint_instance_id().expect("instance id").transport() };
    let (iphone, other) = (mint(), mint());
    let record = |key: &str, value: actingcommand_contract::FactValue| RuntimeFactRecord {
        scope: RuntimeFactScope::Instance {
            instance_id: iphone,
        },
        key: key.to_owned(),
        value,
        observed_at_unix_ms: 1,
        source: OriginModule::Runtime,
        ttl_ms: None,
    };
    let snapshot = RuntimeFactSnapshot {
        schema_version: "neutral".to_owned(),
        ledger_position: 1,
        taken_at_unix_ms: 1,
        snapshot_id: 1,
        part: 1,
        parts: 1,
        records: vec![
            record(
                "device.system",
                actingcommand_contract::FactValue::String("ios".to_owned()),
            ),
            record(
                "device.bound",
                actingcommand_contract::FactValue::Boolean(true),
            ),
        ],
    };
    let device = crate::overview::handset_device(&snapshot, iphone).expect("the row's device");
    assert_eq!(
        device,
        json!({"kind": "handset", "system": "ios", "bound": true})
    );
    disjoint(&members(&device));
    assert_eq!(crate::overview::handset_device(&snapshot, other), None);
}

#[test]
fn f2_t6_the_private_file_reads_version_1_as_before_and_version_2_with_ios_rows() {
    let key = fingerprint(SECRET, IDENTIFIER);
    let android = json!({
        "alias": PHONE, "serial": SERIAL, "bound_at_unix_ms": 1, "bound_via": "cli"
    });
    let ios = |alias: &str, key: &str| {
        json!({
            "alias": alias, "system": "ios", "device_key": key,
            "bound_at_unix_ms": 1, "bound_via": "cli"
        })
    };
    let file = |schema: &str, rows: Vec<Value>| -> Vec<u8> {
        serde_json::to_vec(&json!({"schema": schema, "bindings": rows})).expect("the file")
    };
    let v1 = "actingcommand.device-bindings.v1";
    let v2 = "actingcommand.device-bindings.v2";

    // A file an earlier build wrote loads, and is written back byte for byte.
    let before = file(v1, vec![android.clone()]);
    let read = parse_device_bindings(&before).expect("a version-1 file");
    let rows = read.iter().cloned().collect::<Vec<_>>();
    assert_eq!(rows[0].serial(), Some(SERIAL));
    assert_eq!(rows[0].system(), DeviceSystem::Android);
    let again = serde_json::to_vec(&DeviceBindingsFile {
        schema: device_bindings_schema(&rows).to_owned(),
        bindings: rows,
    })
    .expect("written again");
    assert_eq!(again, before);

    // Version 2 holds both kinds of row.
    let both = parse_device_bindings(&file(v2, vec![android.clone(), ios(IPHONE, &key)]))
        .expect("a version-2 file");
    let phone = both.get(IPHONE).expect("the iOS row");
    assert_eq!(
        (phone.system(), phone.device_key(), phone.serial()),
        (DeviceSystem::Ios, Some(key.as_str()), None)
    );
    assert_eq!(
        device_bindings_schema(&both.iter().cloned().collect::<Vec<_>>()),
        v2
    );

    // Each rule of the new kind of row.
    let mut wrong_shape = ios(IPHONE, &key);
    wrong_shape["serial"] = json!(SERIAL);
    let mut unknown_member = ios(IPHONE, &key);
    unknown_member["udid"] = json!(IDENTIFIER);
    let mut android_with_system = android.clone();
    android_with_system["system"] = json!("android");
    // Fix round 1: a version-1 row is read exactly as before this slice: a `system` member, even
    // `null`, is no Android row's (nor is a `device_key` one).
    let mut android_with_null_system = android.clone();
    android_with_null_system["system"] = Value::Null;
    let mut android_with_null_key = android.clone();
    android_with_null_key["device_key"] = Value::Null;
    for (bytes, problem) in [
        (
            file(v1, vec![android_with_null_system]),
            DeviceBindingProblem::Malformed,
        ),
        (
            file(v1, vec![android_with_null_key]),
            DeviceBindingProblem::Malformed,
        ),
        (
            file(v1, vec![ios(IPHONE, &key)]),
            DeviceBindingProblem::Malformed,
        ),
        (
            file(v2, vec![ios(IPHONE, "sha256:NOT-A-KEY")]),
            DeviceBindingProblem::DeviceKeyInvalid,
        ),
        (
            file(v2, vec![ios(IPHONE, &key), ios(IPHONE_B, &key)]),
            DeviceBindingProblem::DuplicateSerial,
        ),
        (file(v2, vec![wrong_shape]), DeviceBindingProblem::Malformed),
        (
            file(v2, vec![unknown_member]),
            DeviceBindingProblem::Malformed,
        ),
        (
            file(v2, vec![android_with_system]),
            DeviceBindingProblem::Malformed,
        ),
        (
            file("actingcommand.device-bindings.v3", vec![android]),
            DeviceBindingProblem::Malformed,
        ),
    ] {
        assert_eq!(
            parse_device_bindings(&bytes).err(),
            Some(problem),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

/// A scripted `acib` child: it ends with status 0 at its look number `ends_at` (never without
/// one); ending it fails when `end_fails` is set. It counts the times it is ended.
struct ScriptedChild {
    looks: usize,
    ends_at: Option<usize>,
    end_fails: bool,
    ends: usize,
}

impl ScriptedChild {
    fn new(ends_at: Option<usize>, end_fails: bool) -> Self {
        Self {
            looks: 0,
            ends_at,
            end_fails,
            ends: 0,
        }
    }
}

impl AcibChild for ScriptedChild {
    fn ended(&mut self) -> std::io::Result<Option<bool>> {
        self.looks += 1;
        Ok(self
            .ends_at
            .is_some_and(|at| self.looks >= at)
            .then_some(true))
    }

    fn end(&mut self) -> std::io::Result<()> {
        self.ends += 1;
        if self.end_fails {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        } else {
            Ok(())
        }
    }
}

/// A clock that stands still and moves on by each pause it is asked for, waiting no time.
struct StepClock(std::cell::Cell<std::time::Duration>);

impl StepClock {
    fn new() -> Self {
        Self(std::cell::Cell::new(std::time::Duration::ZERO))
    }
}

impl AcibClock for StepClock {
    fn elapsed(&self) -> std::time::Duration {
        self.0.get()
    }

    fn pause(&self, span: std::time::Duration) {
        self.0.set(self.0.get().saturating_add(span));
        std::thread::yield_now();
    }
}

/// A stream that has ended.
fn ended_stream() -> Box<dyn std::io::Read + Send> {
    Box::new(std::io::empty())
}

/// A stream that prints without end.
fn endless_stream() -> Box<dyn std::io::Read + Send> {
    Box::new(std::io::repeat(b'x'))
}

/// The Lab's bounds with a wait that never passes, so only the child's end or a stream decides.
fn no_wait() -> AcibBounds {
    AcibBounds {
        wait: std::time::Duration::MAX,
        ..AcibBounds::lab()
    }
}

#[test]
fn f2_t7_acib_is_called_with_its_deadline_and_ended_past_the_labs_wait_or_its_cap() {
    // The deadline the Lab gives `acib` is under its own wait, which is well under the 30 s the
    // installer gives a Lab call (register §1.475).
    let lab = AcibBounds::lab();
    assert_eq!(devices::ACIB_TIMEOUT_MS, 10_000);
    assert!(lab.wait > std::time::Duration::from_millis(devices::ACIB_TIMEOUT_MS));
    assert!(lab.wait < std::time::Duration::from_secs(30));

    // A child that never ends: ended once the Lab's wait passed, `no_answer_in_time` below.
    let mut child = ScriptedChild::new(None, false);
    let clock = StepClock::new();
    let waited = devices::wait_acib(&mut child, ended_stream(), ended_stream(), lab, &clock);
    assert!(matches!(waited, AcibRun::Overdue));
    assert_eq!(child.ends, 1);
    assert!(clock.elapsed() >= lab.wait);

    // A child that prints without end, on its output or on its errors: ended past the cap.
    for (output, errors) in [
        (endless_stream(), ended_stream()),
        (ended_stream(), endless_stream()),
    ] {
        let mut child = ScriptedChild::new(None, false);
        let waited = devices::wait_acib(&mut child, output, errors, no_wait(), &StepClock::new());
        assert!(matches!(waited, AcibRun::TooLong));
        assert_eq!(child.ends, 1);
    }

    // A child that ends gives its line and is not ended by the Lab.
    let line = br#"{"command":"devices","devices":[]}"#.to_vec();
    let mut child = ScriptedChild::new(Some(3), false);
    let waited = devices::wait_acib(
        &mut child,
        Box::new(std::io::Cursor::new(line.clone())),
        ended_stream(),
        no_wait(),
        &StepClock::new(),
    );
    assert!(matches!(waited, AcibRun::Answered(answer) if answer == line));
    assert_eq!(child.ends, 0);

    // A child that cannot be ended.
    let mut child = ScriptedChild::new(None, true);
    let waited = devices::wait_acib(
        &mut child,
        ended_stream(),
        ended_stream(),
        lab,
        &StepClock::new(),
    );
    assert!(matches!(waited, AcibRun::NotEnded));

    // A listing answers adb's rows and the word for each.
    let at = roots(&mixed(), true);
    let overdue = |_: &Path, _: &AcibCall| AcibRun::Overdue;
    let too_long = |_: &Path, _: &AcibCall| AcibRun::TooLong;
    let not_ended = |_: &Path, _: &AcibCall| AcibRun::NotEnded;
    let cases: [(ScriptedAcib<'_>, &str); 3] = [
        (&overdue, "no_answer_in_time"),
        (&too_long, "answer_too_long"),
        (&not_ended, "not_ended"),
    ];
    for (acib, word) in cases {
        let exit = run_devices(&at, acib);
        let (listed, code) = answered(&exit);
        assert_eq!(code, "device_list_reported", "{word}");
        assert_eq!(rows(&listed), [android_row(SECRET)], "{word}");
        assert_eq!(listed["ios_devices_unlisted"], json!({"why": word}));
    }
}

/// `devices list` against the candidate of `at`, `acib` run by `acib`, no salt given.
fn run_devices(at: &Roots, acib: &dyn Fn(&Path, &AcibCall) -> AcibRun) -> Exit {
    run(at, &["devices", "list"], &[], acib, &nothing)
}
