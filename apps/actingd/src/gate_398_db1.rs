// SPDX-License-Identifier: AGPL-3.0-only

// #398 DB-1 (register §1.460; contracts/device-binding.md, "Choosing a device"; the coordinator's
// ruling on board #398, comment 6940490302; coordinator work orders
// rd-1117/brief-398-db1-bind-candidate-config.md and, fix round 1, rd-1117/brief-398-db1-fix1.md):
// `devices bind <alias> <handle> --config <candidate>` checks the alias in the candidate
// configuration and binds into the state root the candidate names, which check-config of the
// candidate reads; `--bound-via ui` writes the channel `ui`. The Lab's row hands the command line
// `lab-ops` builds for it (`core_ops::handed_words`), `devices` runs it with the test's own
// `adb devices -l` listing (`devices::dispatch_with`), and the binding is read as the Runtime
// reads it: by the start's assembly of the candidate, by check-config and by the first start. No
// device, no adb, no process, no clock read here. The serials are neutral fakes, distinct from the
// other actingd tests' so the process-wide label of each stays its own.
// - db1_t1: on a root without a selected configuration the row's line against a candidate
//   carries neither the module's state root nor its root (without `config` it is today's), a
//   `--root` the command line gives goes on, and the bind answers today's members and `config`,
//   the device's serial and the channel `cli` in the candidate's state root as the start reads it,
//   nothing else made in that state root, the located one untouched.
// - db1_t2 (fix round 1): in the installer's order (the state root created empty, the programs
//   laid, the candidate written), check-config with `--install-root`, as the installer runs it,
//   refuses the candidate `device_binding_missing` (its alias) before the bind and passes whole
//   after it (`check_config_passed`), writing nothing; the bind is the installer's (`bound_via`
//   `ui`), its adb listing answered only for the install root's adb.
// - db1_t3: the candidate's refusals, the root's own configuration having the alias as a handset:
//   an alias the candidate lacks `instance_unknown`, one that is no handset there
//   `device_kind_unsupported`, a file that cannot be read `config_read_failed`, one that does not
//   decode `config_decode_failed` (each with its `path`), one without a state root
//   `install_state_root_unresolved`; none writes; and the Lab's row on a place that locates no state
//   root is served against the candidate, while without `config` it is the place's refusal.
// - db1_t5: `bound_via ui` records `ui`, without it `cli`; another word is refused
//   `not_recognised`, nothing written; the row declares it as the table's enum of the one word.
// - db1_fix1_a_candidate_bind_reads_no_install_selection (fix round 1, item 4): the install
//   root's adb comes from `--root`, else from the program's place, never from a selection: a
//   program of another installation, whose root's selection cannot even be read, binds with
//   `--root`; a program in an A/B slot of a root that has no `install\active.json` binds without
//   it; each with the adb of the root it names.
// - db1_fix1_the_first_start_claims_the_state_root_the_bind_made (the coordinator's addition):
//   after the bind the state root holds exactly its private folder with the one binding file (the
//   architecture guard keeps that file's name to its reader and writer); adopted, the root
//   is an installation root and check-config finds the state root `unclaimed`; the first start of
//   the configuration switched in (`RuntimeHost::start`, in this process) accepts it, claims it
//   (`installation.json`, the identity minted) and leaves the private file as it was.
// The slice's test 4 is RD-1c's tests (`lab_ops::devices::tests`), unchanged: without `--config`
// `devices bind` answers and writes as before. The MCP part of test 5 is actinglab-mcp's
// `gate_398_db1`.

use super::{ActingdError, Reported, check_config, config};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{DeviceBoundVia, InstallConfigPurpose};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::devices::{self, AdbRun, Bind};
use actingcommand_lab_ops::events::CursorBinding;
use actingcommand_lab_ops::front::Connection;
use actingcommand_lab_ops::locate;
use actingcommand_lab_ops::table::{self, Operation, OperationTable, ParamType};
use actingcommand_runtime_host::{
    ExecutionBackendProvider, HandsetEndpoint, ResolvedInstanceEndpoint, RuntimeHost,
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

const ALIAS: &str = "neutral.phone";
const HANDLE: &str = "usb:7";
const MODEL: &str = "Neutral_Phone";
const INSTANCE_ID: &str = "instance_0000000000000000000000000000398d";

/// What makes a candidate's bytes for its state root (none: no file).
type MakeCandidate = Box<dyn FnOnce(&Path) -> Option<Vec<u8>>>;

/// An install root without a selected configuration of its own (but where a test gives one): its
/// programs' manifest, no `install\active.json`; a candidate configuration beside it, whose state
/// root does not exist yet; and the state root a place would locate; all in `temp`.
struct Installation {
    temp: TempDir,
    root: PathBuf,
    candidate: PathBuf,
    state_root: PathBuf,
    located: PathBuf,
}

/// The installation, its candidate the bytes `candidate` makes for its state root (none: no
/// file), and `selected` as the root's own `actingd.config.json` when given.
fn installation(
    candidate: impl FnOnce(&Path) -> Option<Vec<u8>>,
    selected: Option<Value>,
) -> Installation {
    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("runtime")).expect("the runtime folder");
    fs::write(root.join("runtime").join("BUILD-MANIFEST.json"), b"{}").expect("the manifest");
    if let Some(selected) = selected {
        fs::write(root.join("actingd.config.json"), selected.to_string())
            .expect("the root's own configuration");
    }
    let state_root = temp.path().join("state");
    let located = temp.path().join("located");
    let file = temp.path().join("candidate.json");
    if let Some(bytes) = candidate(&state_root) {
        fs::write(&file, bytes).expect("the candidate");
    }
    Installation {
        temp,
        root,
        candidate: file,
        state_root,
        located,
    }
}

/// Fix round 1: the installation as the installer has it when it binds, in its order (the
/// folders created, the state root empty; the programs laid; the candidate written): no
/// selection, no configuration of its own, and not adopted yet (no `install\state.json`, no
/// identity).
fn installer_root() -> Installation {
    let at = installation(candidate_with(json!([handset()])), None);
    fs::create_dir_all(at.state_root.as_path()).expect("the empty state root");
    at
}

/// Fix round 1: the installer's own state file of the root of `at`, which `install adopt` writes;
/// from then on check-config's `--install-root` and a start from the root take it as an
/// installation root (they read only that the file is there).
fn adopted(at: &Installation) {
    fs::create_dir_all(at.root.join("install")).expect("the installer's folder");
    fs::write(at.root.join("install").join("state.json"), b"{}")
        .expect("the installer's state file");
}

/// A candidate configuration, as actingd reads one, naming its state root and `instances`.
fn candidate_with(instances: Value) -> impl FnOnce(&Path) -> Option<Vec<u8>> {
    move |state_root: &Path| {
        Some(
            serde_json::to_vec(&json!({
                "schema_version": "actingcommand.actingd.config.v1",
                "state_root": state_root,
                "bind_host": "127.0.0.1",
                "bind_port": 0,
                "secret_fingerprint_salt": "0123456789abcdef",
                "instances": instances
            }))
            .expect("the candidate's JSON"),
        )
    }
}

/// The handset `ALIAS` on USB, with the backends a handset has. It names an adb of its own, not
/// the install root's, whose pinned files a test cannot lay.
fn handset() -> Value {
    json!({
        "alias": ALIAS,
        "instance_id": INSTANCE_ID,
        "application_id": "neutral.application",
        "adb_path": "neutral-adb",
        "touch_backend": "adb_shell_input",
        "capture_backend": "adb",
        "device": {"kind": "handset", "transport": "usb"}
    })
}

/// adb lists the handset on USB as `usb:7`, its serial `serial`.
fn listing(serial: &'static str) -> impl Fn(&Path) -> AdbRun {
    move |_: &Path| {
        AdbRun::Printed(format!(
            "List of devices attached\n{serial} device usb:1-1 product:neutral model:{MODEL} device:neutral transport_id:7\n"
        ))
    }
}

/// Fix round 1: [`listing`] when the adb run is the one at `adb`; any other adb is not started,
/// so that a bind that lists the devices with another adb is refused `device_list_failed`.
fn listing_of(adb: PathBuf, serial: &'static str) -> impl Fn(&Path) -> AdbRun {
    let listed = listing(serial);
    move |asked: &Path| {
        if asked == adb.as_path() {
            listed(asked)
        } else {
            AdbRun::NotStarted(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }
}

/// The adb of the install root `root`.
fn adb_of(root: &Path) -> PathBuf {
    root.join("tools").join("platform-tools").join("adb.exe")
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

fn devices_bind(table: &OperationTable) -> &Operation {
    table
        .operation("devices bind")
        .expect("the table's row devices bind")
}

/// The command line the row `devices bind` hands `devices` for `members`, the module having
/// located the root and the state root of `at`.
fn handed(row: &Operation, members: Value, at: &Installation) -> Vec<OsString> {
    core_ops::handed_words(row, &params(members), Some(at.root.as_path()), &at.located)
        .expect("the row hands words")
        .expect("its words are built")
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

/// The code, `argument` and `argument_problem` of the refused line `exit`.
fn refusal(exit: &Exit) -> (Value, Value, Value) {
    assert_ne!(exit.code, 0, "{exit:?}");
    let line = exit.line.clone().expect("the line encodes");
    let outcome = &line["outcome"];
    (
        outcome["code"].clone(),
        outcome["values"]["argument"].clone(),
        outcome["values"]["argument_problem"].clone(),
    )
}

/// The serial and the channel the start's assembly of `candidate` binds the handset `ALIAS` to:
/// the private binding file as the Runtime reads it.
fn assembled(candidate: &Path) -> (String, DeviceBoundVia) {
    let file = config::load_for(candidate, InstallConfigPurpose::CandidateCheck)
        .unwrap_or_else(|code| panic!("the candidate loads: {}", code.as_str()));
    let assembly = file
        .assemble()
        .unwrap_or_else(|code| panic!("the candidate assembles: {}", code.as_str()));
    assert!(
        assembly.unbound_handsets.is_empty(),
        "the handset is bound: {:?}",
        assembly.unbound_handsets
    );
    let registry = assembly
        .provider
        .into_registry()
        .expect("configured registry");
    let resolved = registry.resolve(ALIAS).expect("the registered handset");
    let endpoint = resolved
        .adb_endpoint()
        .and_then(ResolvedInstanceEndpoint::bound)
        .expect("a bound endpoint");
    let bound = endpoint
        .handset()
        .and_then(HandsetEndpoint::binding)
        .expect("a bound handset");
    (bound.serial().adb_arg().to_owned(), bound.bound_via())
}

/// Fix round 1: `actingd check-config --config <candidate> --install-root <root>`, as the
/// installer runs it on its candidate (the Lab's rescue row `check-config <file>`).
fn check_with_install_root(at: &Installation) -> Result<Reported, ActingdError> {
    check_config::run(vec![
        OsString::from("check-config"),
        OsString::from("--config"),
        at.candidate.as_os_str().to_owned(),
        OsString::from("--install-root"),
        at.root.as_os_str().to_owned(),
    ])
}

/// Fix round 1: the installer's bind of the handset against the candidate of `at`, through the
/// Lab's row with the channel `ui`; its adb listing, of the serial `serial`, is answered only for
/// the install root's adb. The test's program runs from no installation, so the row is given the
/// root as a command line gives `--root`.
fn bind_as_installer(at: &Installation, serial: &'static str) {
    let table = table::operation_table().expect("the operation table");
    let argv = handed(
        devices_bind(&table),
        json!({
            "instance": ALIAS,
            "handle": HANDLE,
            "config": text(&at.candidate),
            "bound_via": "ui",
            "root": text(&at.root)
        }),
        at,
    );
    let exit = devices::dispatch_with(&argv, &listing_of(adb_of(&at.root), serial))
        .expect("devices answers");
    assert_eq!(answered(&exit).1, "device_binding_changed", "{exit:?}");
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

/// Fix round 1: each file of the folder `folder`, its name and its bytes, in name order.
fn contents(folder: &Path) -> Vec<(String, Vec<u8>)> {
    names(folder)
        .into_iter()
        .map(|name| {
            let bytes = fs::read(folder.join(&name)).expect("read a file of the folder");
            (name, bytes)
        })
        .collect()
}

#[test]
fn db1_t1_a_root_without_a_selected_configuration_binds_against_the_candidate() {
    let at = installation(candidate_with(json!([handset()])), None);
    assert!(
        !at.root.join("actingd.config.json").exists() && !at.state_root.exists(),
        "setup: no selected configuration, no state root"
    );
    let table = table::operation_table().expect("the operation table");
    let row = devices_bind(&table);
    let root = text(&at.root);
    let located = text(&at.located);
    // Without `config`, the line is today's: the module's state root and root.
    assert_eq!(
        handed(row, json!({"instance": ALIAS, "handle": HANDLE}), &at),
        words(&[
            "devices",
            "bind",
            ALIAS,
            HANDLE,
            "--state-root",
            located.as_str(),
            "--root",
            root.as_str()
        ])
    );
    // With it, neither the module's state root (the candidate names its own) nor the module's
    // root (no install selection decides the adb): `devices` takes the program's place.
    let candidate = text(&at.candidate);
    assert_eq!(
        handed(
            row,
            json!({"instance": ALIAS, "handle": HANDLE, "config": candidate}),
            &at,
        ),
        words(&[
            "devices",
            "bind",
            ALIAS,
            HANDLE,
            "--config",
            candidate.as_str()
        ])
    );
    // A `--root` the command line gives goes on.
    let argv = handed(
        row,
        json!({"instance": ALIAS, "handle": HANDLE, "config": candidate, "root": root}),
        &at,
    );
    assert_eq!(
        argv,
        words(&[
            "devices",
            "bind",
            ALIAS,
            HANDLE,
            "--config",
            candidate.as_str(),
            "--root",
            root.as_str()
        ])
    );
    let exit = devices::dispatch_with(&argv, &listing_of(adb_of(&at.root), "TESTSERIAL0031"))
        .expect("devices answers");
    let (members, code) = answered(&exit);
    assert_eq!(code, "device_binding_changed", "{exit:?}");
    assert_eq!(
        members,
        json!({
            "alias": ALIAS,
            "handle": HANDLE,
            "model": MODEL,
            "restart_required": true,
            "config": candidate
        })
    );
    assert!(!members.to_string().contains("TESTSERIAL"), "{members}");
    // The binding is in the candidate's state root, as the start reads it; the bind made its
    // private folder and that one file, nothing else; the located state root is untouched.
    assert_eq!(
        assembled(&at.candidate),
        ("TESTSERIAL0031".to_owned(), DeviceBoundVia::Cli)
    );
    assert_eq!(names(&at.state_root), ["private"]);
    assert_eq!(names(&at.state_root.join("private")).len(), 1);
    assert!(!at.located.exists(), "the located state root is not made");
}

#[test]
fn db1_t2_check_config_with_install_root_refuses_before_the_bind_and_passes_after() {
    let at = installer_root();
    let before = check_with_install_root(&at)
        .expect_err("check-config refuses the candidate before the bind")
        .outcome();
    assert_eq!(before.code().as_str(), "device_binding_missing", "{before}");
    assert_eq!(before.value("alias"), Some(&json!(ALIAS)), "{before}");
    assert!(names(&at.state_root).is_empty(), "the check writes nothing");

    bind_as_installer(&at, "TESTSERIAL0032");
    assert_eq!(
        assembled(&at.candidate),
        ("TESTSERIAL0032".to_owned(), DeviceBoundVia::Ui)
    );
    // The same check now passes whole.
    let after = check_with_install_root(&at)
        .unwrap_or_else(|error| panic!("check-config passes after the bind: {}", error.outcome()));
    assert_eq!(
        after.outcome.code().as_str(),
        "check_config_passed",
        "{}",
        after.outcome
    );
    assert!(after.data.contains_key("report"), "{:?}", after.data);
    assert_eq!(
        names(&at.state_root),
        ["private"],
        "the check writes nothing"
    );
}

#[test]
fn db1_t3_the_candidate_decides_its_refusals_whatever_the_selected_configuration_says() {
    // The root's own configuration has the alias as a handset; the candidate decides.
    let selected = json!({"instances": [{"alias": ALIAS, "device": {"kind": "handset"}}]});
    let request = Bind {
        alias: ALIAS,
        handle: HANDLE,
        via: DeviceBoundVia::Cli,
    };
    let other = json!([{"alias": "neutral.other", "device": {"kind": "handset"}}]);
    let cases: [(&str, MakeCandidate, &str); 5] = [
        (
            "an alias the candidate lacks",
            Box::new(candidate_with(other.clone())),
            "instance_unknown",
        ),
        (
            "an alias that is no handset in the candidate",
            Box::new(candidate_with(json!([{"alias": ALIAS, "port": 5555}]))),
            "device_kind_unsupported",
        ),
        (
            "a candidate that cannot be read",
            Box::new(|_: &Path| None),
            "config_read_failed",
        ),
        (
            "a candidate that does not decode",
            Box::new(|_: &Path| Some(b"{not json".to_vec())),
            "config_decode_failed",
        ),
        (
            "a candidate without a state root",
            Box::new(|_: &Path| {
                Some(
                    serde_json::to_vec(&json!({"instances": [handset()]}))
                        .expect("the candidate's JSON"),
                )
            }),
            "install_state_root_unresolved",
        ),
    ];
    for (case, candidate, code) in cases {
        let at = installation(candidate, Some(selected.clone()));
        let refused = devices::bind_candidate(
            &at.candidate,
            Some(at.root.as_path()),
            &request,
            &listing("TESTSERIAL0036"),
        )
        .expect_err(case);
        assert_eq!(refused.code().as_str(), code, "{case}: {refused}");
        let named = match code {
            "instance_unknown" => ("instance_selector", json!(ALIAS)),
            "device_kind_unsupported" => ("alias", json!(ALIAS)),
            "install_state_root_unresolved" => ("config_path", json!(text(&at.candidate))),
            _ => ("path", json!(text(&at.candidate))),
        };
        assert_eq!(refused.value(named.0), Some(&named.1), "{case}: {refused}");
        assert!(!at.state_root.exists(), "{case}: nothing is written");
    }

    // The Lab's row on a place that locates no state root (a root whose configuration is not
    // switched in yet) is served against the candidate, which refuses the alias it lacks; without
    // `config` the same call is the place's refusal. The test's program runs from no
    // installation: the root is given as a command line gives `--root`.
    let at = installation(candidate_with(other), Some(selected));
    let table = table::operation_table().expect("the operation table");
    let row = devices_bind(&table);
    let root = at.root.clone();
    let place = move || Place {
        state_root: Err(Outcome::new(ActingctlCode::InstallStateRootUnresolved)),
        root: Some(root.clone()),
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
    let code = |members: Value| refusal(&core_ops::answer(row, &params(members), &caller)).0;
    assert_eq!(
        code(json!({
            "instance": ALIAS,
            "handle": HANDLE,
            "config": text(&at.candidate),
            "root": text(&at.root)
        })),
        "instance_unknown"
    );
    assert_eq!(
        code(json!({"instance": ALIAS, "handle": HANDLE})),
        "install_state_root_unresolved"
    );
    assert!(!at.state_root.exists() && !at.located.exists());
}

#[test]
fn db1_t5_bound_via_ui_records_ui_without_it_cli_and_another_word_is_refused() {
    let table = table::operation_table().expect("the operation table");
    let row = devices_bind(&table);
    // The row declares the channel as the table's enum of one word: every front's type check
    // refuses another word before the row is served.
    let bound_via = row
        .params
        .iter()
        .find(|param| param.name == "bound_via")
        .expect("the row declares bound_via");
    assert_eq!(bound_via.value_type, ParamType::Enum(vec!["ui".to_owned()]));
    assert!(!bound_via.required);

    // `ui`, as the Lab's row hands it (the root given as a command line gives `--root`).
    let ui = installation(candidate_with(json!([handset()])), None);
    let candidate = text(&ui.candidate);
    let root = text(&ui.root);
    let argv = handed(
        row,
        json!({
            "instance": ALIAS,
            "handle": HANDLE,
            "config": candidate,
            "bound_via": "ui",
            "root": root
        }),
        &ui,
    );
    assert_eq!(
        argv,
        words(&[
            "devices",
            "bind",
            ALIAS,
            HANDLE,
            "--config",
            candidate.as_str(),
            "--bound-via",
            "ui",
            "--root",
            root.as_str()
        ])
    );
    let exit = devices::dispatch_with(&argv, &listing("TESTSERIAL0033")).expect("devices answers");
    assert_eq!(answered(&exit).1, "device_binding_changed", "{exit:?}");
    assert_eq!(
        assembled(&ui.candidate),
        ("TESTSERIAL0033".to_owned(), DeviceBoundVia::Ui)
    );

    // Without it, the command line's own channel.
    let cli = installation(candidate_with(json!([handset()])), None);
    let line = |at: &Installation, more: &[&str]| {
        let candidate = text(&at.candidate);
        let root = text(&at.root);
        let mut line = words(&[
            "devices",
            "bind",
            ALIAS,
            HANDLE,
            "--config",
            candidate.as_str(),
            "--root",
            root.as_str(),
        ]);
        line.extend(words(more));
        line
    };
    let exit = devices::dispatch_with(&line(&cli, &[]), &listing("TESTSERIAL0034"))
        .expect("devices answers");
    assert_eq!(answered(&exit).1, "device_binding_changed", "{exit:?}");
    assert_eq!(
        assembled(&cli.candidate),
        ("TESTSERIAL0034".to_owned(), DeviceBoundVia::Cli)
    );

    // Another word is refused before anything is read or written.
    let other = installation(candidate_with(json!([handset()])), None);
    let exit = devices::dispatch_with(
        &line(&other, &["--bound-via", "mcp"]),
        &listing("TESTSERIAL0035"),
    )
    .expect("devices answers");
    assert_eq!(
        refusal(&exit),
        (
            json!("actingctl_usage_invalid"),
            json!("--bound-via"),
            json!("not_recognised")
        )
    );
    assert!(!other.state_root.exists(), "nothing is written");
}

#[test]
fn db1_fix1_a_candidate_bind_reads_no_install_selection() {
    let request = Bind {
        alias: ALIAS,
        handle: HANDLE,
        via: DeviceBoundVia::Ui,
    };

    // A program of another installation, whose root holds a selection that cannot even be read
    // (as a process that inherited another installation's selection runs): with `--root` the
    // bind lists the devices with the adb of the root given and binds; nothing reads that
    // selection.
    let at = installer_root();
    let other = at.temp.path().join("other");
    fs::create_dir_all(other.join("runtime")).expect("the other root's runtime folder");
    fs::write(other.join("runtime").join("BUILD-MANIFEST.json"), b"{}")
        .expect("the other root's manifest");
    fs::create_dir_all(other.join("install")).expect("the other root's installer folder");
    fs::write(
        other.join("install").join("active.json"),
        b"{not a selection",
    )
    .expect("the other root's selection");
    let program = other.join("tools").join("actinglab.exe");
    assert_eq!(locate::root_of_place(&program), Some(other));
    let answer = devices::bind_candidate_from(
        &at.candidate,
        Some(at.root.as_path()),
        Some(program.as_path()),
        &request,
        &listing_of(adb_of(&at.root), "TESTSERIAL0037"),
    )
    .unwrap_or_else(|refused| panic!("the bind with --root of another installation: {refused}"));
    assert_eq!(answer["config"], json!(text(&at.candidate)), "{answer}");
    assert_eq!(
        assembled(&at.candidate),
        ("TESTSERIAL0037".to_owned(), DeviceBoundVia::Ui)
    );

    // A program in an A/B slot of a root that has no `install\active.json` yet (its slot's
    // manifest and the root's slot locator laid, nothing selected): without `--root` the bind
    // lists the devices with the adb of the slot's root, `<root>\tools`, and binds.
    let at = installer_root();
    let slotted = at.temp.path().join("slotted");
    fs::create_dir_all(slotted.join("A").join("runtime")).expect("the slot's runtime folder");
    fs::write(
        slotted
            .join("A")
            .join("runtime")
            .join("BUILD-MANIFEST.json"),
        b"{}",
    )
    .expect("the slot's manifest");
    fs::create_dir_all(slotted.join("install")).expect("the slotted root's installer folder");
    fs::write(slotted.join("install").join("slot-A.lock"), b"").expect("the slot's locator");
    assert!(!slotted.join("install").join("active.json").exists());
    let program = slotted.join("A").join("runtime").join("actingctl.exe");
    assert_eq!(locate::root_of_place(&program), Some(slotted.clone()));
    devices::bind_candidate_from(
        &at.candidate,
        None,
        Some(program.as_path()),
        &request,
        &listing_of(adb_of(&slotted), "TESTSERIAL0038"),
    )
    .unwrap_or_else(|refused| panic!("the bind from a slot of an unselected root: {refused}"));
    assert_eq!(
        assembled(&at.candidate),
        ("TESTSERIAL0038".to_owned(), DeviceBoundVia::Ui)
    );

    // Neither a root given nor a program in one: `install_root_unresolved`, nothing written.
    let at = installer_root();
    let elsewhere = at.temp.path().join("elsewhere").join("actinglab.exe");
    let refused = devices::bind_candidate_from(
        &at.candidate,
        None,
        Some(elsewhere.as_path()),
        &request,
        &listing("TESTSERIAL0039"),
    )
    .expect_err("no install root");
    assert_eq!(
        refused.code().as_str(),
        "install_root_unresolved",
        "{refused}"
    );
    assert!(names(&at.state_root).is_empty(), "nothing is written");
}

#[test]
fn db1_fix1_the_first_start_claims_the_state_root_the_bind_made() {
    let at = installer_root();
    bind_as_installer(&at, "TESTSERIAL0040");
    let private = at.state_root.join("private");
    assert_eq!(names(&at.state_root), ["private"]);
    let bound = contents(&private);
    assert_eq!(bound.len(), 1, "the one binding file");

    // Adopted, the root is an installation root: check-config of the candidate still passes, and
    // finds the state root unclaimed.
    adopted(&at);
    let checked = check_with_install_root(&at).unwrap_or_else(|error| {
        panic!(
            "check-config passes after the adoption: {}",
            error.outcome()
        )
    });
    let report = checked.data.get("report").expect("the report");
    assert_eq!(report["state_root_installation"], "unclaimed", "{report}");

    // The candidate switched in as the root's configuration; the start's installation root is the
    // directory of its `--config`, which holds the installer's own state file.
    let live = at.root.join("actingd.config.json");
    fs::copy(at.candidate.as_path(), &live).expect("the candidate switched in");
    let installation_root = actingcommand_runtime_host::installation_root_of_config(&live)
        .expect("the installer's state file is inspected")
        .expect("the root is an installation root");
    let file = config::load(&live)
        .unwrap_or_else(|code| panic!("the configuration loads: {}", code.as_str()));
    let assembly = file
        .assemble()
        .unwrap_or_else(|code| panic!("the configuration assembles: {}", code.as_str()));
    let registry = assembly
        .provider
        .into_registry()
        .expect("configured registry");
    let host = RuntimeHost::start(
        assembly
            .host
            .with_installation_root(installation_root.clone()),
        Arc::new(registry),
    )
    .expect("the first start accepts the state root the bind made");
    host.close().expect("close the host");

    // It claimed the state root for this installation, minting its identity, and left the private
    // folder as the bind wrote it.
    assert!(at.state_root.join("installation.json").is_file());
    assert!(at.root.join("install").join("identity.json").is_file());
    assert_eq!(
        actingcommand_runtime_host::check_state_root_installation(
            Some(installation_root.as_path()),
            &at.state_root
        )
        .expect("the state root's record reads")
        .as_str(),
        "claimed_by_this"
    );
    assert_eq!(contents(&private), bound);
}
