// SPDX-License-Identifier: AGPL-3.0-only

// #398 DB-2 (register §1.467; contracts/device-binding.md, "Choosing a device"; the coordinator's
// rulings in work order rd-1117/brief-398-db2-devices-before-install.md and its addition of
// `--key-salt-env`): `devices` lists for a root that has nothing selected, a row carries a device
// key that outlives the adb session, `devices bind` takes the key in place of the handle, and every
// bind refuses a row that cannot be a handset's device. The rows hand their command lines as the
// Lab's row builds them (`core_ops`), `devices` runs them with a scripted `adb devices -l` listing
// (`devices::dispatch_in`, whose environment the test gives: the test process's own is neither
// read nor written), and a binding is read back by a listing. No device, no adb, no process, no
// clock read here; the serials are neutral fakes.
// - db2_t1: a root with only its tools: the Lab's row on a place that locates no state root hands
//   `devices` a line without the module's state root and root and is served by it, not refused by
//   the place; the rows are answered with no `bound_alias` and `bindings_unread`
//   `state_root_unknown`; with `config` of a candidate whose state root holds a binding, that row
//   carries its alias, the answer has no `bindings_unread`, and every row the key the candidate's
//   secret makes (the contract's fingerprint of its serial).
// - db2_t2: with a salt every row carries `device_key`; two listings whose handles differ give the
//   same keys; another salt gives other keys; the salt read from a variable (`--key-salt-env`)
//   gives the keys of the same salt given as text; an unset variable, both forms, and a salt too
//   short are refused by their words; without a salt and without an installation no row has a
//   key; a key is `sha256:` and 64 lowercase hexadecimal digits, holds no serial, and neither the
//   salt nor the variable's value is in any line.
// - db2_t3: a bind by key after the handles changed binds the device the key was listed for (the
//   answer names its new handle, a listing finds the alias on that row, as a bind by its handle
//   finds it); an unknown key and a key under another salt are `device_key_unknown`, the handle
//   and the key both `--device-key` `conflicting`, and none writes anything.
// - db2_t4: an `unauthorized` row, an `offline` row and a loopback `tcp` row are each refused
//   `device_row_unbindable` with their word, by handle and by key, naming no serial and writing
//   nothing.
// - db2_t6: an installed root without the new options: the listing answers today's members, each
//   row with the key the installation's own secret makes, and no `bindings_unread`; the bind by
//   handle answers today's members exactly. RD-1c's tests (`devices::tests`) stay as they are: none
//   binds a row ruling 4 refuses.

use crate::cli_line::Exit;
use crate::codes::ActingctlCode;
use crate::core_ops::{self, Caller, Place};
use crate::devices::{self, AdbRun, Bind};
use crate::events::CursorBinding;
use crate::front::Connection;
use crate::table;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{DeviceBoundVia, SecretField, SecretFingerprinter};
use actingcommand_ledger::Sha256SecretFingerprinter;
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const ALIAS: &str = "neutral.phone";
/// The four rows' serials, in the listing's order: a handset on USB, an emulator's loopback row, a
/// handset that has not allowed debugging and one that is offline.
const SERIALS: [&str; 4] = [
    "TESTSERIAL0201",
    "127.0.0.1:5555",
    "TESTSERIAL0202",
    "TESTSERIAL0203",
];
/// The candidate configuration's own secret, and two salts a caller gives.
const SECRET: &str = "neutral-secret-0398-db2";
const SALT_A: &str = "neutral-salt-a-0398-0001";
const SALT_B: &str = "neutral-salt-b-0398-0002";
/// The variable `--key-salt-env` names, as the test's environment holds it.
const SALT_VARIABLE: &str = "NEUTRAL_DEVICE_KEY_SALT";
/// The rows' transport ids in a first listing, and after a replug and an adb restart.
const FIRST: [u16; 4] = [7, 3, 8, 9];
const REPLUGGED: [u16; 4] = [17, 13, 18, 19];

/// An installer's root that holds only its tools, a candidate configuration beside it naming a
/// state root that does not exist yet, all in `temp`.
struct Roots {
    temp: TempDir,
    root: PathBuf,
    candidate: PathBuf,
    state_root: PathBuf,
}

fn roots() -> Roots {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("tools").join("platform-tools")).expect("the root's tools");
    let state_root = temp.path().join("state");
    let candidate = temp.path().join("candidate.json");
    fs::write(
        &candidate,
        json!({
            "schema_version": "actingcommand.actingd.config.v1",
            "state_root": state_root,
            "secret_fingerprint_salt": SECRET,
            "instances": [{"alias": ALIAS, "device": {"kind": "handset", "transport": "usb"}}]
        })
        .to_string(),
    )
    .expect("the candidate");
    Roots {
        temp,
        root,
        candidate,
        state_root,
    }
}

/// `adb devices -l` printing the four rows with the transport ids `ids`.
fn listing_text(ids: [u16; 4]) -> String {
    format!(
        "List of devices attached\n\
         {}         device usb:1-1 product:neutral model:Neutral_Phone device:neutral transport_id:{}\n\
         {}         device product:neutral_emu model:Neutral_Emulator device:neutral transport_id:{}\n\
         {}         unauthorized usb:1-2 transport_id:{}\n\
         {}         offline usb:1-3 transport_id:{}\n\n",
        SERIALS[0], ids[0], SERIALS[1], ids[1], SERIALS[2], ids[2], SERIALS[3], ids[3]
    )
}

/// The listing of `ids`, answered only for the adb at `adb`; any other adb is not started.
fn listing_of(adb: PathBuf, ids: [u16; 4]) -> impl Fn(&Path) -> AdbRun {
    move |asked: &Path| {
        if asked == adb.as_path() {
            AdbRun::Printed(listing_text(ids))
        } else {
            AdbRun::NotStarted(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }
}

/// The adb of the install root `root`.
fn adb_of(root: &Path) -> PathBuf {
    root.join("tools").join("platform-tools").join("adb.exe")
}

/// An environment without any variable.
fn nothing(_: &str) -> Option<OsString> {
    None
}

/// An environment whose one variable [`SALT_VARIABLE`] holds [`SALT_A`].
fn salted(name: &str) -> Option<OsString> {
    (name == SALT_VARIABLE).then(|| OsString::from(SALT_A))
}

fn words(words: &[&str]) -> Vec<OsString> {
    words.iter().map(OsString::from).collect()
}

fn text(path: &Path) -> String {
    path.to_str()
        .expect("the temporary folder is UTF-8")
        .to_owned()
}

fn params(members: Value) -> Map<String, Value> {
    match members {
        Value::Object(members) => members,
        other => panic!("not a JSON object: {other}"),
    }
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

/// The outcome of the refused line `exit`, which has no other member.
fn refused(exit: &Exit) -> Value {
    assert_eq!(exit.code, 1, "{exit:?}");
    let line = exit.line.clone().expect("the line encodes");
    assert_eq!(line.as_object().map(Map::len), Some(1), "{line}");
    line["outcome"].clone()
}

/// The `argument` and `argument_problem` of the usage refusal `exit`.
fn usage(exit: &Exit) -> (Value, Value) {
    let outcome = refused(exit);
    assert_eq!(outcome["code"], "actingctl_usage_invalid", "{outcome}");
    (
        outcome["values"]["argument"].clone(),
        outcome["values"]["argument_problem"].clone(),
    )
}

/// What `exit` prints, its line and its developer line.
fn printed(exit: &Exit) -> String {
    let line = exit.line.as_ref().map(Value::to_string).unwrap_or_default();
    let developer = exit
        .developer
        .as_ref()
        .map(|developer| developer.line("actingctl"))
        .unwrap_or_default();
    format!("{line}\n{developer}")
}

/// No serial in `printed`.
fn no_serial(printed: &str) {
    for serial in SERIALS
        .iter()
        .filter(|serial| serial.starts_with("TESTSERIAL"))
    {
        assert!(!printed.contains(serial), "{serial} in {printed}");
    }
}

/// The rows of a listing's members.
fn rows(members: &Value) -> Vec<Value> {
    members["devices"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("a device list: {members}"))
}

/// The device keys of a listing's members, row by row.
fn keys(members: &Value) -> Vec<String> {
    rows(members)
        .iter()
        .map(|row| {
            row["device_key"]
                .as_str()
                .unwrap_or_else(|| panic!("a row with its key: {row}"))
                .to_owned()
        })
        .collect()
}

/// The contract's fingerprint of `serial` under `salt`: the ledger's one function.
fn fingerprint(salt: &str, serial: &str) -> String {
    Sha256SecretFingerprinter::new(salt)
        .expect("a fingerprinter")
        .fingerprint(SecretField::DeviceSerial, serial)
        .expect("a fingerprint")
        .as_str()
        .to_owned()
}

/// The names in the folder `folder`.
fn names(folder: &Path) -> Vec<String> {
    let mut names = fs::read_dir(folder)
        .expect("list the folder")
        .map(|entry| {
            entry
                .expect("a folder entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

/// `devices list --root <root>` with `extra`, the adb of `at`'s root listing `ids`, in
/// `environment`.
fn list(
    at: &Roots,
    ids: [u16; 4],
    extra: &[&str],
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Exit {
    let root = text(&at.root);
    let mut argv = words(&["devices", "list", "--root", root.as_str()]);
    argv.extend(words(extra));
    devices::dispatch_in(&argv, &listing_of(adb_of(&at.root), ids), environment)
        .expect("devices answers")
}

/// `devices bind <alias>`, then `device` (the handle, or the key and its salt), then the
/// candidate of `at` and its root and the channel `ui`, as the installer binds; the adb of `at`'s
/// root listing `ids`, in `environment`.
fn bind(
    at: &Roots,
    device: &[&str],
    ids: [u16; 4],
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Exit {
    let (candidate, root) = (text(&at.candidate), text(&at.root));
    let mut argv = words(&["devices", "bind", ALIAS]);
    argv.extend(words(device));
    argv.extend(words(&[
        "--config",
        candidate.as_str(),
        "--root",
        root.as_str(),
        "--bound-via",
        "ui",
    ]));
    devices::dispatch_in(&argv, &listing_of(adb_of(&at.root), ids), environment)
        .expect("devices answers")
}

/// The four rows without a key and without a binding, their transport ids `ids`.
fn bare_rows(ids: [u16; 4]) -> Value {
    json!([
        {"handle": format!("usb:{}", ids[0]), "transport": "usb", "state": "device", "model": "Neutral_Phone"},
        {"handle": format!("tcp:{}", ids[1]), "transport": "tcp", "state": "device", "model": "Neutral_Emulator", "endpoint": SERIALS[1]},
        {"handle": format!("usb:{}", ids[2]), "transport": "usb", "state": "unauthorized"},
        {"handle": format!("usb:{}", ids[3]), "transport": "usb", "state": "offline"},
    ])
}

#[test]
fn db2_t1_a_root_with_only_its_tools_lists_its_rows_and_says_no_binding_was_read() {
    let at = roots();
    assert_eq!(names(&at.root), ["tools"], "setup: only the tools are laid");
    let table = table::operation_table().expect("the operation table");
    let row = table.operation("devices").expect("the row devices");
    let root = text(&at.root);

    // The Lab's row where the place locates no state root: neither the module's state root nor
    // its root, the root the call names.
    let argv = core_ops::unlocated_words(row, &params(json!({"root": root})))
        .expect("devices lists where nothing is located")
        .expect("its words are built");
    assert_eq!(argv, words(&["devices", "list", "--root", root.as_str()]));
    let exit = devices::dispatch_in(&argv, &listing_of(adb_of(&at.root), FIRST), &nothing)
        .expect("devices answers");
    let (members, code) = answered(&exit);
    assert_eq!(code, "device_list_reported", "{exit:?}");
    assert_eq!(
        members,
        json!({"devices": bare_rows(FIRST), "bindings_unread": "state_root_unknown"})
    );
    no_serial(&printed(&exit));
    assert!(!at.state_root.exists(), "nothing is written");

    // The row is served by `devices`, not refused by the place: a salt too short is the
    // command's refusal, where the place's would be install_state_root_unresolved.
    let place = || Place {
        state_root: Err(Outcome::new(ActingctlCode::InstallStateRootUnresolved)),
        root: None,
    };
    let caller = Caller {
        place: &place,
        unlocated: "the test's place locates no state root",
        connection: Connection::per_call(),
        budget: None,
        cursor: CursorBinding {
            session: 1,
            generation: 1,
        },
    };
    let served = core_ops::answer(
        row,
        &params(json!({"root": root, "key_salt": "neutral-short"})),
        &caller,
    );
    assert_eq!(
        usage(&served),
        (json!("--key-salt"), json!("out_of_range")),
        "{served:?}"
    );
    assert!(!printed(&served).contains("neutral-short"));

    // With the candidate whose state root holds a binding, that row carries its alias.
    devices::bind_candidate_from(
        &at.candidate,
        Some(at.root.as_path()),
        None,
        &Bind {
            alias: ALIAS,
            handle: "usb:7",
            via: DeviceBoundVia::Ui,
        },
        &listing_of(adb_of(&at.root), FIRST),
    )
    .unwrap_or_else(|refused| panic!("the candidate's bind: {refused}"));
    let candidate = text(&at.candidate);
    let located = at.temp.path().join("located");
    let argv = core_ops::handed_words(
        row,
        &params(json!({"config": candidate, "root": root})),
        Some(at.root.as_path()),
        &located,
    )
    .expect("the row hands words")
    .expect("its words are built");
    assert_eq!(
        argv,
        words(&[
            "devices",
            "list",
            "--config",
            candidate.as_str(),
            "--root",
            root.as_str()
        ])
    );
    let exit = devices::dispatch_in(&argv, &listing_of(adb_of(&at.root), FIRST), &nothing)
        .expect("devices answers");
    let (members, _) = answered(&exit);
    assert!(members.get("bindings_unread").is_none(), "{members}");
    let listed = rows(&members);
    assert_eq!(listed[0]["bound_alias"], ALIAS, "{members}");
    assert!(
        listed[1..]
            .iter()
            .all(|row| row.get("bound_alias").is_none()),
        "{members}"
    );
    let secret_keys = SERIALS
        .iter()
        .map(|serial| fingerprint(SECRET, serial))
        .collect::<Vec<_>>();
    assert_eq!(keys(&members), secret_keys);
    no_serial(&printed(&exit));
    assert!(!located.exists(), "the located state root is not made");
}

#[test]
fn db2_t2_a_device_key_holds_across_handles_and_differs_by_salt() {
    let at = roots();
    let first = list(&at, FIRST, &["--key-salt", SALT_A], &nothing);
    let (members, _) = answered(&first);
    let under_a = keys(&members);
    no_serial(&printed(&first));
    assert!(!printed(&first).contains(SALT_A), "the salt is not printed");

    // Other handles, the same keys.
    let replugged = list(&at, REPLUGGED, &["--key-salt", SALT_A], &nothing);
    let (members, _) = answered(&replugged);
    let handles = rows(&members)
        .iter()
        .map(|row| row["handle"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        handles,
        [
            json!("usb:17"),
            json!("tcp:13"),
            json!("usb:18"),
            json!("usb:19")
        ]
    );
    assert_eq!(keys(&members), under_a);

    // Another salt, other keys: no key of one salt is a key of the other.
    let (members, _) = answered(&list(&at, FIRST, &["--key-salt", SALT_B], &nothing));
    let under_b = keys(&members);
    for key in &under_b {
        assert!(!under_a.contains(key), "{key} under both salts");
    }

    // The salt read from a variable of the process: the keys of the same salt given as text.
    let variable = list(&at, REPLUGGED, &["--key-salt-env", SALT_VARIABLE], &salted);
    let (members, _) = answered(&variable);
    assert_eq!(keys(&members), under_a);
    assert!(
        !printed(&variable).contains(SALT_A),
        "the variable's value is not printed"
    );

    // An unset variable, both forms, a salt too short (16 to 1024 bytes).
    assert_eq!(
        usage(&list(
            &at,
            FIRST,
            &["--key-salt-env", SALT_VARIABLE],
            &nothing
        )),
        (json!("--key-salt-env"), json!("unusable"))
    );
    assert_eq!(
        usage(&list(
            &at,
            FIRST,
            &["--key-salt", SALT_A, "--key-salt-env", SALT_VARIABLE],
            &salted
        )),
        (json!("--key-salt-env"), json!("conflicting"))
    );
    let short = list(&at, FIRST, &["--key-salt", "neutral-short"], &nothing);
    let outcome = refused(&short);
    assert_eq!(
        (
            &outcome["values"]["argument"],
            &outcome["values"]["argument_problem"],
            &outcome["values"]["range_min"],
            &outcome["values"]["range_max"]
        ),
        (
            &json!("--key-salt"),
            &json!("out_of_range"),
            &json!(16),
            &json!(1024)
        ),
        "{outcome}"
    );
    assert!(!printed(&short).contains("neutral-short"));

    // Without a salt and without an installation: no key.
    let (members, _) = answered(&list(&at, FIRST, &[], &nothing));
    assert_eq!(
        members,
        json!({"devices": bare_rows(FIRST), "bindings_unread": "state_root_unknown"})
    );

    // A key's length and alphabet are fixed, and it is no serial.
    for (key, serial) in under_a.iter().chain(&under_b).zip(SERIALS.iter().cycle()) {
        assert_eq!(key.len(), 71, "{key}");
        let digits = key
            .strip_prefix("sha256:")
            .unwrap_or_else(|| panic!("{key} begins with sha256:"));
        assert!(
            digits.len() == 64
                && digits
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "{key}"
        );
        assert!(!key.contains(serial), "{key} holds {serial}");
    }
    assert!(!at.state_root.exists(), "nothing is written");
}

#[test]
fn db2_t3_a_bind_by_key_after_the_handles_changed_binds_the_device_it_was_listed_for() {
    let at = roots();
    // The installer's listing, its salt in a variable of the process.
    let (members, _) = answered(&list(
        &at,
        FIRST,
        &["--key-salt-env", SALT_VARIABLE],
        &salted,
    ));
    assert_eq!(rows(&members)[0]["handle"], "usb:7");
    let key = keys(&members)[0].clone();
    let by_key = [
        "--device-key",
        key.as_str(),
        "--key-salt-env",
        SALT_VARIABLE,
    ];

    // Refused, each writing nothing: an unknown key, the key under another salt, the handle and
    // the key both.
    let unknown = format!("sha256:{}", "0".repeat(64));
    let refusals = [
        bind(
            &at,
            &["--device-key", unknown.as_str(), "--key-salt", SALT_A],
            REPLUGGED,
            &nothing,
        ),
        bind(
            &at,
            &["--device-key", key.as_str(), "--key-salt", SALT_B],
            REPLUGGED,
            &nothing,
        ),
    ];
    for exit in &refusals {
        assert_eq!(refused(exit)["code"], "device_key_unknown", "{exit:?}");
        no_serial(&printed(exit));
    }
    let both = bind(
        &at,
        &[&["usb:17"][..], &by_key[..]].concat(),
        REPLUGGED,
        &salted,
    );
    assert_eq!(usage(&both), (json!("--device-key"), json!("conflicting")));
    assert!(!at.state_root.exists(), "no refusal writes anything");

    // After a replug and an adb restart, the bind by key finds the device.
    let exit = bind(&at, &by_key, REPLUGGED, &salted);
    let (members, code) = answered(&exit);
    assert_eq!(code, "device_binding_changed", "{exit:?}");
    assert_eq!(
        members,
        json!({
            "alias": ALIAS,
            "device_key": key,
            "handle": "usb:17",
            "model": "Neutral_Phone",
            "restart_required": true,
            "config": text(&at.candidate)
        })
    );
    no_serial(&printed(&exit));
    assert!(!printed(&exit).contains(SALT_A));
    assert_eq!(names(&at.state_root), ["private"]);
    assert_eq!(names(&at.state_root.join("private")).len(), 1);

    // As a bind by its handle binds it: a listing of either candidate finds the alias on the same
    // row and nowhere else.
    let by_handle = roots();
    let (members, _) = answered(&bind(&by_handle, &["usb:17"], REPLUGGED, &nothing));
    assert_eq!(members["handle"], "usb:17");
    let listed = |of: &Roots| {
        let (candidate, root) = (text(&of.candidate), text(&of.root));
        let argv = words(&[
            "devices",
            "list",
            "--config",
            candidate.as_str(),
            "--root",
            root.as_str(),
        ]);
        let exit = devices::dispatch_in(&argv, &listing_of(adb_of(&of.root), REPLUGGED), &nothing)
            .expect("devices answers");
        answered(&exit).0
    };
    let bound = listed(&at);
    assert_eq!(rows(&bound)[0]["bound_alias"], ALIAS, "{bound}");
    assert!(
        rows(&bound)[1..]
            .iter()
            .all(|row| row.get("bound_alias").is_none()),
        "{bound}"
    );
    assert_eq!(bound, listed(&by_handle));
}

#[test]
fn db2_t4_an_unauthorized_an_offline_and_a_loopback_tcp_row_are_refused_by_handle_and_by_key() {
    // Each row, its place in the listing, its handle and the word of its refusal.
    let cases = [
        (2, "usb:8", "state_not_device"),
        (3, "usb:9", "state_not_device"),
        (1, "tcp:3", "transport_mismatch"),
    ];
    for (place, handle, problem) in cases {
        // By the handle, and by the key the candidate's own secret makes (no salt given).
        let key = fingerprint(SECRET, SERIALS[place]);
        let by_key = ["--device-key", key.as_str()];
        for device in [&[handle][..], &by_key[..]] {
            let at = roots();
            let exit = bind(&at, device, FIRST, &nothing);
            let outcome = refused(&exit);
            assert_eq!(
                (
                    &outcome["code"],
                    &outcome["values"]["alias"],
                    &outcome["values"]["device_row_problem"],
                    &outcome["values"]["handle"]
                ),
                (
                    &json!("device_row_unbindable"),
                    &json!(ALIAS),
                    &json!(problem),
                    &json!(handle)
                ),
                "{device:?}: {outcome}"
            );
            assert_eq!(outcome["category"], "error", "{outcome}");
            no_serial(&printed(&exit));
            assert!(!printed(&exit).contains("127.0.0.1"), "{outcome}");
            assert!(!at.state_root.exists(), "{device:?}: nothing is written");
        }
    }
}

#[test]
fn db2_t6_an_installed_root_without_the_new_options_answers_todays_members() {
    let install = TempDir::new().expect("install root");
    fs::write(
        install.path().join("actingd.config.json"),
        json!({
            "secret_fingerprint_salt": SECRET,
            "instances": [{"alias": ALIAS, "device": {"kind": "handset", "transport": "usb"}}]
        })
        .to_string(),
    )
    .expect("the configuration");
    let state = TempDir::new().expect("state root");
    let run = |_: &Path| AdbRun::Printed(listing_text(FIRST));
    let root = Some(install.path());

    // Today's members, and each row's key under the installation's own secret.
    let keyed = |rows: Value| {
        let mut rows = rows.as_array().cloned().expect("rows");
        for (row, serial) in rows.iter_mut().zip(SERIALS) {
            row["device_key"] = json!(fingerprint(SECRET, serial));
        }
        Value::Array(rows)
    };
    let listed = devices::list(state.path(), root, &run).expect("the list");
    assert_eq!(listed, json!({"devices": keyed(bare_rows(FIRST))}));

    // The bind by handle answers today's members exactly.
    let bound = devices::bind(
        state.path(),
        root,
        &Bind {
            alias: ALIAS,
            handle: "usb:7",
            via: DeviceBoundVia::Cli,
        },
        &run,
    )
    .expect("the bind");
    assert_eq!(
        bound,
        json!({"alias": ALIAS, "handle": "usb:7", "model": "Neutral_Phone", "restart_required": true})
    );
    let mut expected = keyed(bare_rows(FIRST));
    expected[0]["bound_alias"] = json!(ALIAS);
    assert_eq!(
        devices::list(state.path(), root, &run).expect("the list"),
        json!({"devices": expected})
    );
}
