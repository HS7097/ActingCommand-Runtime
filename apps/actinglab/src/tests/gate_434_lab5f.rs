// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5F (register §1.287, "Slice LAB-5F"; contracts/lab-two-entries.md §8.3; the
// coordinator's ruling of 2026-11-10 on #434, point 15; work order
// rd-1108/arch-013/brief-lab5f.md): the authoring option decides whether the authoring module's
// operations are listed; fix 2 (rd-1108/arch-013/brief-lab5f-fix2.md; the coordinator's ruling of
// 2026-11-11): only that. A build with the authoring code serves them everywhere.
// - `lab5f_authoring_rows_follow_the_installed_module`: on a made-up installation root (a
//   single-location root in a temporary folder, `runtime\BUILD-MANIFEST.json` beside the root's
//   `actingd.config.json`, the program read as the root's `runtime\actinglab.exe`), with
//   `lab_module_authoring: true` the authoring rows are listed (`help`, its options, `list
//   commands`, `capabilities`); with the key absent, `false`, a value that is no boolean, a
//   configuration that is no JSON and none at all they are not listed, and where the option cannot
//   be read the listing's developer line names the failure with the configuration's path. Outside
//   any installation root they are listed. Fix 2: in every place a call of an authoring row
//   reaches its command, with the answer it has outside any root (`package fmt` writes, also
//   through MCP's `--via` child; `regression`, given by its short words, answers its command's own
//   refusal; `record start` makes its state directory; `observe --capture` and `standard-pack
//   compose` answer their commands); `lab_command_option_disabled` is never the answer. A row of
//   each other module (`codes`, core; `user-config`, debugging; `op status`, rescue, on Windows)
//   answers the same in every case. Fix 1 (rd-1108/arch-013/brief-lab5f-fix1.md; the
//   coordinator's ruling of 2026-11-11): `standard-pack check` is a row of the core module, which
//   the rescue module's `place` runs as its post-check: in every place it is listed and its call
//   reaches its command (its own refusal of a game folder with no root file); `standard-pack
//   compose` is listed only where the option lists the authoring rows.
// - `lab5f_serve_answers_as_the_command_line`: `serve --stdio` answers an authoring row's call in
//   every place with the line the command line's `--json` prints, its command's own line (fix 2);
//   the other modules' rows answer as before. `serve --stdio` lists nothing. Fix 1:
//   `standard-pack check` answers the command line's line in every place, its command's own
//   refusal; fix 2: `standard-pack compose` reaches its command in every place.
// Fakes only: made-up roots in a temporary folder; no Runtime, no device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, authoring, own_line, serve};
use actingcommand_contract::outcome::Outcome;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_lab_ops::locate::AuthoringModule;
use std::collections::BTreeMap;

/// The refusal LAB-5F first gave a call where the authoring module was not installed, which no
/// call answers since fix 2.
const DISABLED: &str = "lab_command_option_disabled";
/// A non-canonical JSON file of the package `package fmt` formats.
const COMPACT: &[u8] = b"{\"kept\":false}";
/// The same file in the canonical serialization.
const CANONICAL: &str = "{\n  \"kept\": false\n}\n";

/// What the made-up place says of the authoring module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Outside any installation root.
    Unrooted,
    /// A root whose configuration says `lab_module_authoring: true`.
    True,
    /// A root whose configuration names no `lab_module_authoring`.
    Absent,
    /// A root whose configuration says `lab_module_authoring: false`.
    False,
    /// A root whose configuration says a `lab_module_authoring` that is no boolean.
    NotBoolean,
    /// A root whose configuration is no JSON.
    NotJson,
    /// A root without its configuration.
    Missing,
}

/// The places, the first outside any root: what a call answers there every other place answers.
const PLACES: [Place; 7] = [
    Place::Unrooted,
    Place::True,
    Place::Absent,
    Place::False,
    Place::NotBoolean,
    Place::NotJson,
    Place::Missing,
];

impl Place {
    /// Whether the authoring rows are listed here.
    fn listed(self) -> bool {
        matches!(self, Self::Unrooted | Self::True)
    }
}

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

fn text_of(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

/// Makes `place` under `base` and answers the program whose place it is, and the root's
/// configuration (none outside a root).
fn made_up(base: &Path, place: Place) -> (PathBuf, Option<PathBuf>) {
    if place == Place::Unrooted {
        let unpacked = base.join("unpacked-tools");
        fs::create_dir_all(&unpacked).unwrap();
        return (unpacked.join("actinglab.exe"), None);
    }
    let root = base.join("install-root");
    let runtime = root.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("BUILD-MANIFEST.json"), b"{}\n").unwrap();
    let config = root.join("actingd.config.json");
    let mut document = json!({"state_root": text_of(&base.join("state"))});
    match place {
        Place::True => document["lab_module_authoring"] = json!(true),
        Place::False => document["lab_module_authoring"] = json!(false),
        Place::NotBoolean => document["lab_module_authoring"] = json!("true"),
        _ => {}
    }
    match place {
        Place::NotJson => fs::write(&config, b"{\"lab_module_authoring\": true,\n").unwrap(),
        Place::Missing => {}
        _ => fs::write(&config, document.to_string()).unwrap(),
    }
    (runtime.join("actinglab.exe"), Some(config))
}

/// A content directory with one JSON file that is not canonical.
fn package(base: &Path) -> PathBuf {
    let package = base.join("neutral.package");
    fs::create_dir_all(package.join("data")).unwrap();
    fs::write(package.join("data").join("compact.json"), COMPACT).unwrap();
    package
}

/// What `main` prints for the command line `line` (no `--via`) with `--json`: its exit code and
/// its one JSON line.
fn printed(line: &[&str]) -> (u8, Value) {
    let exit = match entry(words_of(line), false) {
        Ok(invocation) => own_line::answer(invocation).0,
        Err(module) => cli_module::run_line(&cli_module::front_end(module).1),
    };
    let text = cli_module::stdout_text(&exit, true)
        .unwrap_or_else(|text| panic!("{line:?} printed no line: {text}"));
    let answered = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{line:?} printed no JSON line ({error}): {text}"));
    (exit.code, answered)
}

/// The exit and the outcome's code of the answer to each call in the first place.
type FirstAnswers = BTreeMap<&'static str, (u8, Value)>;

/// Fix 2: the answer `answered` (exit `code`) of the call `call` at `place` came from its command,
/// whatever the option says: never `lab_command_option_disabled`, and the same exit and outcome
/// code as in the first place, outside any root, where the rows are listed (`first` keeps them).
fn reached(first: &mut FirstAnswers, place: Place, call: &'static str, code: u8, answered: &Value) {
    assert_ne!(
        answered["outcome"]["code"], DISABLED,
        "{place:?} {call}: {answered}"
    );
    let seen = (code, answered["outcome"]["code"].clone());
    let expected = first.entry(call).or_insert_with(|| seen.clone());
    assert_eq!(seen, *expected, "{place:?} {call}: {answered}");
}

/// Fix 2: where the option cannot be read (`cause`), the developer line a listing writes is a
/// warning that names the failure with the configuration `config`.
fn assert_unread_line(place: Place, cause: &Outcome, config: Option<&Path>) {
    let line = authoring::unread_option_line(cause);
    assert!(
        line.starts_with(&format!("WARN actinglab: {}", cause.code().as_str())),
        "{place:?}: {line}"
    );
    assert!(
        line.contains(text_of(config.expect("a root's configuration"))),
        "{place:?}: {line}"
    );
}

/// Whether the command entries `entries` list `command`.
fn lists(entries: &Value, command: &str) -> bool {
    entries
        .as_array()
        .expect("a list of commands")
        .iter()
        .any(|entry| entry["command"] == command)
}

/// The commands of the authoring rows, and the groups of `record`, that the listings leave out
/// where the option does not list them.
const AUTHORING_COMMANDS: [&str; 9] = [
    "package fmt",
    "record start",
    "session record start",
    "record",
    "session record",
    "observe --capture --record",
    "recognition-regression",
    "resource verify",
    "standard-pack compose",
];
/// Commands of the other modules, listed everywhere (fix 1: `standard-pack check`, a row of the
/// core module since the coordinator's ruling of 2026-11-11).
const OTHER_COMMANDS: [&str; 9] = [
    "tap",
    "do",
    "do --capture --record",
    "capture --record",
    "session app",
    "lab validate",
    "run summary",
    "version",
    "standard-pack check",
];
/// Fix 1: the outcome `standard-pack check` answers for a game folder with no root file, its
/// command's own refusal.
const NO_ROOT_FILE: &str = "standard_pack_root_invalid";

/// Fix 1: a game folder of a standard-pack root that holds no root file.
fn empty_game_root(base: &Path) -> PathBuf {
    let game = base.join("standard-packs").join("neutral");
    fs::create_dir_all(&game).unwrap();
    game
}

#[test]
fn lab5f_authoring_rows_follow_the_installed_module() {
    let _guard = env_lock();
    set_missing_config_env();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let mut first = FirstAnswers::new();
    for place in PLACES {
        let temp = TempDir::new().unwrap();
        let (program, config) = made_up(temp.path(), place);
        let _place = authoring::test_place::set(program);
        // No Runtime runs: the state root `capabilities` and `observe` would read is not there.
        let _runtime_env = use_runtime_state_root(&temp.path().join("runtime-must-not-exist"));
        let listed = place.listed();

        // What decides: the place, and the selected configuration's key.
        match (place, authoring::module()) {
            (Place::Unrooted, Ok(AuthoringModule::Unrooted)) => {}
            (
                Place::True,
                Ok(AuthoringModule::Configured {
                    installed: true, ..
                }),
            )
            | (
                Place::Absent | Place::False,
                Ok(AuthoringModule::Configured {
                    installed: false, ..
                }),
            ) => {}
            (Place::NotBoolean | Place::NotJson, Err(outcome)) => {
                assert_eq!(outcome.code().as_str(), "config_decode_failed", "{place:?}");
                assert_unread_line(place, &outcome, config.as_deref());
            }
            (Place::Missing, Err(outcome)) => {
                assert_eq!(outcome.code().as_str(), "config_read_failed", "{place:?}");
                assert_unread_line(place, &outcome, config.as_deref());
            }
            (place, read) => panic!("{place:?} read as {read:?}"),
        }

        // The listings: `help` (its commands and its options), `list commands`, `capabilities`.
        let help = run_cli(["--json", "help"], true);
        assert_eq!(help.exit_code(), 0, "{place:?}: {}", help.envelope_json());
        let help = help.envelope.data.clone().expect("help's answer");
        for command in AUTHORING_COMMANDS {
            assert_eq!(
                lists(&help["commands"], command),
                listed,
                "{place:?}: help and {command}"
            );
        }
        for command in OTHER_COMMANDS {
            assert!(lists(&help["commands"], command), "{place:?}: {command}");
        }
        for command in [
            "package fmt",
            "recognition-regression",
            "standard-pack compose",
        ] {
            assert_eq!(
                help["command_options"].get(command).is_some(),
                listed,
                "{place:?}: help's options of {command}"
            );
        }
        // Fix 1: `standard-pack check` is the core module's, listed with its options everywhere.
        for command in ["lab watch", "standard-pack check"] {
            assert!(
                help["command_options"].get(command).is_some(),
                "{place:?}: help's options of {command}"
            );
        }
        for line in [
            &["--json", "list", "commands"][..],
            &["--json", "capabilities"][..],
        ] {
            let answered = run_cli(line.iter().copied(), true);
            assert_eq!(
                answered.exit_code(),
                0,
                "{place:?}: {}",
                answered.envelope_json()
            );
            let commands = &answered.envelope.data.as_ref().expect("an answer")["commands"];
            assert_eq!(lists(commands, "package fmt"), listed, "{place:?} {line:?}");
            assert_eq!(lists(commands, "record"), listed, "{place:?} {line:?}");
            assert!(lists(commands, "tap"), "{place:?} {line:?}");
            // Fix 1.
            assert!(lists(commands, "standard-pack check"), "{place:?} {line:?}");
            assert_eq!(
                lists(commands, "standard-pack compose"),
                listed,
                "{place:?} {line:?}"
            );
        }

        // Fix 1: `standard-pack check`, a row of the core module, reaches its command in every
        // place: its command's own refusal of a game folder with no root file.
        let game = empty_game_root(temp.path());
        let (code, answered) =
            printed(&["--json", "standard-pack", "check", "--root", text_of(&game)]);
        assert_eq!(code, 1, "{place:?}: {answered}");
        assert_eq!(
            answered["outcome"]["code"], NO_ROOT_FILE,
            "{place:?}: {answered}"
        );
        // Fix 2: `standard-pack compose`, an authoring row, reaches its command in every place,
        // listed or not.
        let composed = temp.path().join("composed");
        let (code, answered) = printed(&[
            "--json",
            "standard-pack",
            "compose",
            "--root",
            text_of(&temp.path().join("standard-packs")),
            "--instance",
            "neutral.a=neutral/test",
            "--out",
            text_of(&composed),
        ]);
        reached(&mut first, place, "standard-pack compose", code, &answered);

        // Fix 2: a call on actinglab's own words reaches its command in every place: `package fmt`
        // writes.
        let package = package(temp.path());
        let compact = package.join("data").join("compact.json");
        let (code, answered) =
            printed(&["--json", "package", "fmt", "--package", text_of(&package)]);
        assert_eq!(code, 0, "{place:?}: {answered}");
        assert_eq!(
            answered["outcome"]["code"], "lab_operation_completed",
            "{place:?}: {answered}"
        );
        assert_eq!(
            answered["reformatted"],
            json!(["data/compact.json"]),
            "{place:?}: {answered}"
        );
        assert_eq!(
            fs::read_to_string(&compact).unwrap(),
            CANONICAL,
            "{place:?}"
        );

        // A call on the row's short words, which `main`'s route gives to its own command
        // (`regression` is `recognition-regression`): its command's own refusal in every place
        // (the evidence is not there).
        let evidence = temp.path().join("page.json");
        let state = temp.path().join("state");
        let (code, answered) = printed(&[
            "--json",
            "regression",
            "--state-root",
            text_of(&state),
            "--evidence",
            text_of(&evidence),
            "--package",
            text_of(&package),
        ]);
        assert_eq!(code, 1, "{place:?}: {answered}");
        reached(&mut first, place, "regression", code, &answered);

        // Fix 2: `record start` reaches its command, which makes its state directory, in every
        // place; `observe --capture` reaches its command (no Runtime runs, so it answers as it
        // does outside any root).
        let record = temp.path().join("record");
        let (code, answered) = printed(&[
            "--json",
            "record",
            "start",
            "--state-dir",
            text_of(&record),
            "--task-id",
            "neutral",
        ]);
        reached(&mut first, place, "record start", code, &answered);
        assert!(
            record.exists(),
            "{place:?}: record start reached its command"
        );
        let (code, answered) =
            printed(&["--json", "observe", "--capture", "--instance", "neutral.a"]);
        reached(&mut first, place, "observe --capture", code, &answered);
        // MCP's `--via` child: the same call reaches its command in every place and writes.
        fs::write(&compact, COMPACT).unwrap();
        let via = run_cli(
            [
                "--json",
                "--via",
                "mcp",
                "package",
                "fmt",
                "--package",
                text_of(&package),
            ],
            true,
        );
        assert_eq!(via.exit_code(), 0, "{place:?}: {}", via.envelope_json());
        assert!(
            via.envelope.error.is_none(),
            "{place:?}: {}",
            via.envelope_json()
        );
        assert_eq!(
            fs::read_to_string(&compact).unwrap(),
            CANONICAL,
            "{place:?}: written"
        );

        // A row of each other module answers the same in every place.
        let (code, answered) = printed(&["--json", "codes", DISABLED]);
        assert_eq!(code, 0, "{place:?} codes: {answered}");
        let (code, answered) = printed(&["--json", "user-config"]);
        assert_eq!(code, 0, "{place:?} user-config: {answered}");
        assert_eq!(
            answered["outcome"]["code"], "lab_operation_completed",
            "{answered}"
        );
        // The rescue module locates the root of this test program, which is in none.
        #[cfg(windows)]
        {
            let (code, answered) = printed(&["--json", "op", "status"]);
            assert_eq!(code, 1, "{place:?} op status: {answered}");
            assert_eq!(
                answered["outcome"]["code"], "install_root_unresolved",
                "{place:?}: {answered}"
            );
        }
    }
}

#[test]
fn lab5f_serve_answers_as_the_command_line() {
    let _guard = env_lock();
    set_missing_config_env();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let table = cli_module::table().expect("the operation table");
    // Fix 2: the code `standard-pack compose` answers in the first place, outside any root.
    let mut first_compose: Option<Value> = None;
    for place in PLACES {
        let temp = TempDir::new().unwrap();
        let (program, _) = made_up(temp.path(), place);
        let _place = authoring::test_place::set(program);
        let package = package(temp.path());
        // Fix 1: `standard-pack check` (core) and `standard-pack compose` (authoring).
        let game = empty_game_root(temp.path());
        let packs = temp.path().join("standard-packs");
        let composed = temp.path().join("composed");
        let input = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            json!({"op": "package fmt", "params": {"package": text_of(&package), "check": true}}),
            json!({"op": "codes", "params": {"code": DISABLED}}),
            json!({"op": "user-config", "params": {}}),
            json!({"op": "standard-pack check", "params": {"root": text_of(&game)}}),
            json!({"op": "standard-pack compose", "params": {
                "root": text_of(&packs),
                "instance": ["neutral.a=neutral/test"],
                "out": text_of(&composed),
            }}),
        );
        let mut output = Vec::new();
        let mut developer = Vec::new();
        serve::serve(table, input.as_bytes(), &mut output, &mut developer).expect("the loop ends");
        let output = String::from_utf8(output).unwrap();
        let served = output.lines().collect::<Vec<_>>();
        assert_eq!(served.len(), 5, "{place:?}: {output}");

        // The authoring row: in every place (fix 2), the line the command line's `--json` prints
        // for the same call, its command's own.
        let (code, line) = printed(&[
            "--json",
            "package",
            "fmt",
            "--package",
            text_of(&package),
            "--check",
        ]);
        assert_eq!(
            serde_json::from_str::<Value>(served[0]).unwrap(),
            line,
            "{place:?}"
        );
        assert_eq!(code, 1, "{place:?}: {line}");
        // `--check` reached the command, which names the file and writes nothing.
        assert_eq!(
            line["outcome"]["code"], "package_fmt_check_failed",
            "{place:?}: {line}"
        );
        assert_eq!(
            fs::read(package.join("data").join("compact.json")).unwrap(),
            COMPACT
        );

        // Fix 1: `standard-pack check`, a row of the core module, answers in every place the line
        // the command line's `--json` prints, its command's own refusal of a game folder with no
        // root file.
        let (code, line) = printed(&["--json", "standard-pack", "check", "--root", text_of(&game)]);
        assert_eq!(
            serde_json::from_str::<Value>(served[3]).unwrap(),
            line,
            "{place:?}"
        );
        assert_eq!(code, 1, "{place:?}: {line}");
        assert_eq!(line["outcome"]["code"], NO_ROOT_FILE, "{place:?}: {line}");
        // Fix 2: `standard-pack compose`, an authoring row, reaches its command in every place,
        // the code it answers outside any root.
        let compose = serde_json::from_str::<Value>(served[4]).unwrap();
        let code = compose["outcome"]["code"].clone();
        assert_ne!(code, DISABLED, "{place:?}: {compose}");
        let expected = first_compose.get_or_insert_with(|| code.clone());
        assert_eq!(*expected, code, "{place:?}: {compose}");

        // The other modules' rows answer as the command line does, in every place.
        for (served, words) in [
            (served[1], &["--json", "codes", DISABLED][..]),
            (served[2], &["--json", "user-config"][..]),
        ] {
            let (code, line) = printed(words);
            assert_eq!(code, 0, "{place:?} {words:?}: {line}");
            assert_eq!(
                serde_json::from_str::<Value>(served).unwrap(),
                line,
                "{place:?}"
            );
        }
    }
}
