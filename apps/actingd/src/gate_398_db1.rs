// SPDX-License-Identifier: AGPL-3.0-only

// #398 DB-1 (register §1.459; contracts/device-binding.md, "Choosing a device"; the coordinator's
// ruling on board #398, comment 6940490302; coordinator work order
// rd-1117/brief-398-db1-bind-candidate-config.md): `devices bind <alias> <handle> --config
// <candidate>` checks the alias in the candidate configuration and binds into the state root the
// candidate names, which check-config of the candidate reads; `--bound-via ui` writes the channel
// `ui`. The Lab's row hands the command line `lab-ops` builds for it (`core_ops::handed_words`),
// `devices` runs it with the test's own `adb devices -l` listing (`devices::dispatch_with`), and
// the binding is read as the Runtime reads it: by the start's assembly of the candidate and by
// check-config. No device, no adb, no Runtime, no clock. The serials are neutral fakes, distinct
// from the other actingd tests' so the process-wide label of each stays its own.
// - db1_t1: on a root without a selected configuration the row's line against a candidate
//   carries no module's state root (without `config` it is today's), and binds: today's members
//   and `config`, the device's serial and the channel `cli` in the candidate's state root as the
//   start reads it, nothing else made in that state root, the located one untouched.
// - db1_t2: check-config refuses the same candidate `device_binding_missing` (its alias) before
//   the bind and not for it after.
// - db1_t3: the candidate's refusals, the root's own configuration having the alias as a handset:
//   an alias the candidate lacks `instance_unknown`, one that is no handset there
//   `device_kind_unsupported`, a file that cannot be read `config_read_failed`, one that does not
//   decode `config_decode_failed` (each with its `path`), one without a state root
//   `install_state_root_unresolved`; none writes; and the Lab's row on a place that locates no state
//   root is served against the candidate, while without `config` it is the place's refusal.
// - db1_t5: `bound_via ui` records `ui`, without it `cli`; another word is refused
//   `not_recognised`, nothing written; the row declares it as the table's enum of the one word.
// The slice's test 4 is RD-1c's tests (`lab_ops::devices::tests`), unchanged: without `--config`
// `devices bind` answers and writes as before. The MCP part of test 5 is actinglab-mcp's
// `gate_398_db1`.

use super::{check_config, config};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{DeviceBoundVia, InstallConfigPurpose};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::codes::ActingctlCode;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::devices::{self, AdbRun, Bind};
use actingcommand_lab_ops::events::CursorBinding;
use actingcommand_lab_ops::front::Connection;
use actingcommand_lab_ops::table::{self, Operation, OperationTable, ParamType};
use actingcommand_runtime_host::{HandsetEndpoint, ResolvedInstanceEndpoint};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const ALIAS: &str = "neutral.phone";
const HANDLE: &str = "usb:7";
const MODEL: &str = "Neutral_Phone";
const INSTANCE_ID: &str = "instance_0000000000000000000000000000398d";

/// What makes a candidate's bytes for its state root (none: no file).
type MakeCandidate = Box<dyn FnOnce(&Path) -> Option<Vec<u8>>>;

/// An install root without a selected configuration of its own (but where a test gives one): its
/// programs' manifest, no `install\active.json`; a candidate configuration beside it, whose state
/// root does not exist yet; and the state root a place would locate.
struct Installation {
    _temp: TempDir,
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
        _temp: temp,
        root,
        candidate: file,
        state_root,
        located,
    }
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

/// The handset `ALIAS` on USB, with the backends a handset has.
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

/// The command line the row `devices bind` hands `devices` for `members`.
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

/// `actingd check-config --config <candidate>`: the outcome it refuses with, none when it passes.
fn refused_check(candidate: &Path) -> Option<Outcome> {
    check_config::run(vec![
        OsString::from("check-config"),
        OsString::from("--config"),
        candidate.as_os_str().to_owned(),
    ])
    .err()
    .map(|error| error.outcome())
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
    // With it, no module's state root: the candidate names its own.
    let candidate = text(&at.candidate);
    let argv = handed(
        row,
        json!({"instance": ALIAS, "handle": HANDLE, "config": candidate}),
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
    let exit = devices::dispatch_with(&argv, &listing("TESTSERIAL0031")).expect("devices answers");
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
fn db1_t2_check_config_refuses_the_candidate_before_the_bind_and_not_for_it_after() {
    let at = installation(candidate_with(json!([handset()])), None);
    let before =
        refused_check(&at.candidate).expect("check-config refuses the candidate before the bind");
    assert_eq!(before.code().as_str(), "device_binding_missing", "{before}");
    assert_eq!(before.value("alias"), Some(&json!(ALIAS)), "{before}");
    devices::bind_candidate(
        &at.candidate,
        Some(at.root.as_path()),
        &Bind {
            alias: ALIAS,
            handle: HANDLE,
            via: DeviceBoundVia::Cli,
        },
        &listing("TESTSERIAL0032"),
    )
    .expect("the bind");
    if let Some(after) = refused_check(&at.candidate) {
        assert!(
            !matches!(
                after.code().as_str(),
                "device_binding_missing" | "device_binding_file_invalid"
            ),
            "check-config reads the binding the bind wrote: {after}"
        );
    }
    assert_eq!(
        assembled(&at.candidate),
        ("TESTSERIAL0032".to_owned(), DeviceBoundVia::Cli)
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
    // `config` the same call is the place's refusal.
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
        code(json!({"instance": ALIAS, "handle": HANDLE, "config": text(&at.candidate)})),
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

    // `ui`, as the Lab's row hands it.
    let ui = installation(candidate_with(json!([handset()])), None);
    let candidate = text(&ui.candidate);
    let root = text(&ui.root);
    let argv = handed(
        row,
        json!({"instance": ALIAS, "handle": HANDLE, "config": candidate, "bound_via": "ui"}),
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
