// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 CLI-HELP (register §1.313; contracts/lab-two-entries.md §9.1 and §9.3; coordinator
// work order rd-1111/brief-cli-help.md, the coordinator's rulings of 2026-11-11, not Alice's
// words): `actinglab help` prints a text a person reads, and `--help` on any line asks for it.
// - `cli_help_the_list_has_every_offered_row_and_own_command_once`: `help`'s list holds every row
//   this build offers (a rescue row only on Windows, where it is offered; no row it does not offer)
//   and every own command without a row, each once with a sentence that is not empty; those own
//   commands are the commands of the inventory that no offered row names, serves or replaces, and
//   `serve`; the text for people prints each entry once with its sentence, every global option
//   but the retired `--require-session` with its phrase, in plain ASCII of at most 100 columns.
// - `cli_help_the_detail_of_each_kind_by_help_and_by_the_flag`: one row of each kind register
//   §1.307 names (a CLI-module row with a required instance, one with an optional instance, an
//   own command under other words, an own command under the row's name, a row whose instance is an
//   option) by `help <words>` and by `<words> --help`, the two giving the same text: its usage
//   with the instance as the row's note says, its sentence, the table's three facts and a line
//   for each parameter.
// - `cli_help_groups_the_bare_line_and_a_flagged_line_that_runs_nothing`: an own group's `help`
//   and `--help` list its commands; the bare `actinglab` prints the list for people; `--help` on
//   a line with other flags runs nothing: on a scripted Runtime with no connection scripted, none
//   arrives.
// - `cli_help_json_keeps_todays_members_and_adds_the_list`: `--json help` answers every member it
//   answered before, unchanged, and the list as data (name, module, help, params and the three
//   facts, the table's for a row and none for an own command without one).
// - `cli_help_the_refusals_that_are_gone_and_status_help_unchanged`: lines refused before
//   (`--help`, `help <words>`, `<words> --help`, `<group> help`) answer; `status help` still
//   names the instance `help`; an unknown word after `help` is refused as a read list refuses one;
//   the module's refusal names `actinglab help`, not a contract file.
// Fakes only: the operation table this build embeds, the command inventory, neutral aliases and a
// scripted Runtime that answers nothing; no device, no clock, no sleep.

use super::gate_434_lab5b4::{Isolated, developer, printed};
use super::*;
use crate::cli_help::{self, OWN_WITHOUT_ROW};
use crate::cli_module::{self, Route, words};
use actingcommand_lab_ops::table::{OperationTable, RuntimeNeed};
use actingcommand_runtime_client::test_support::ScriptedRuntime;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The instance the lines name.
const ALIAS: &str = "neutral.a";
/// The note of a row whose command line takes `--instance` in place of its positional (§1.307).
const ALSO: &str = "on the command line also --instance <x>, in place of the positional";
/// The note of a row whose own command takes only `--instance` (§1.307).
const ONLY: &str = "on the command line only --instance <x>";

/// A row of one kind of register §1.307: its name, the words its command line takes, the
/// beginning of its usage and how its instance is written.
type Kind = (
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
);

/// One row of each kind of register §1.307.
const KINDS: [Kind; 5] = [
    // A row of the CLI module, its instance required.
    ("frame", &["frame"], "frame <instance>", "<instance>"),
    // A row of the CLI module, its instance optional.
    ("pause", &["pause"], "pause [<instance>]", "<instance>"),
    // A row actinglab's own command serves under other words (`session app launch`).
    (
        "app launch",
        &["session", "app", "launch"],
        "app launch <instance>",
        "<instance>",
    ),
    // A row actinglab's own command serves under the row's name: the instance only as the flag.
    (
        "tap",
        &["tap"],
        "tap --instance <instance> <x> <y>",
        "--instance <instance>",
    ),
    // A row whose instance is an option of its own.
    (
        "remedy set",
        &["remedy", "set"],
        "remedy set [<file>]",
        "--instance <instance>",
    ),
];

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

fn table() -> &'static OperationTable {
    cli_module::table().expect("the operation table")
}

/// `help`'s text for people of the command line `line`, through the route `main` takes.
fn people_of(line: &[&str]) -> String {
    let exit = printed(line);
    cli_help::people(&exit)
        .unwrap_or_else(|| panic!("{line:?} printed no text for people: {:?}", exit.line))
}

/// What a person reads in `text`: its words, one space between each, its line breaks aside.
fn read(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every line of `text` is plain ASCII without a control character, of at most 100 columns.
fn assert_printable(text: &str) {
    for line in text.lines() {
        assert!(
            line.len() <= cli_help::WIDTH,
            "a line of {} columns: {line}",
            line.len()
        );
        assert!(
            line.chars()
                .all(|character| character.is_ascii() && !character.is_ascii_control()),
            "a line that is not plain ASCII: {line:?}"
        );
    }
}

/// The commands of the inventory, `execute`'s, which the architecture guard keeps equal to it.
fn inventory() -> Vec<String> {
    let snapshot: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ratchet/actinglab_commands.json"),
        )
        .expect("the command inventory"),
    )
    .expect("the command inventory parses");
    snapshot["commands"]
        .as_array()
        .expect("its commands")
        .iter()
        .map(|command| command.as_str().expect("a command").to_owned())
        .collect()
}

#[test]
fn cli_help_the_list_has_every_offered_row_and_own_command_once() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    let table = table();
    let line = printed(&["--json", "help"]).line.expect("help's line");
    let list = line["list"].as_array().expect("help's list");
    let names = list
        .iter()
        .map(|entry| entry["name"].as_str().expect("an entry's name"))
        .collect::<Vec<_>>();

    // Every offered row and every own command without a row, once, each with its sentence.
    let offered = table
        .operations
        .iter()
        .filter(|operation| words::offered(operation))
        .collect::<Vec<_>>();
    let expected = offered
        .iter()
        .map(|operation| operation.name.as_str())
        .chain(OWN_WITHOUT_ROW.iter().map(|own| own.words))
        .collect::<Vec<_>>();
    for name in &expected {
        assert_eq!(
            names.iter().filter(|listed| *listed == name).count(),
            1,
            "{name} is listed once"
        );
    }
    assert_eq!(names.len(), expected.len(), "the list holds nothing else");
    for entry in list {
        assert!(
            entry["help"]
                .as_str()
                .is_some_and(|help| !help.trim().is_empty()),
            "an entry without a sentence: {entry}"
        );
    }
    // A row this platform does not offer is not listed.
    for operation in table
        .operations
        .iter()
        .filter(|operation| !words::offered(operation))
    {
        assert!(
            !names.contains(&operation.name.as_str()),
            "{} is not offered here, and listed",
            operation.name
        );
    }
    let rescue = list
        .iter()
        .filter(|entry| entry["module"] == "rescue")
        .count();
    #[cfg(windows)]
    assert!(rescue > 0, "the rescue rows are offered on Windows");
    #[cfg(not(windows))]
    assert_eq!(rescue, 0, "no rescue row is offered off Windows");

    // The own commands without a row are the inventory's commands that no offered row names,
    // serves or replaces (the survey's 34), and `serve`, the CLI module's resident mode.
    let covered = |command: &str| {
        let under = format!("{command} ");
        offered.iter().any(|operation| {
            cli_help::row_names(operation)
                .iter()
                .any(|name| name == command || name.starts_with(&under))
        })
    };
    let rest = inventory()
        .into_iter()
        .filter(|command| !covered(command))
        .collect::<BTreeSet<_>>();
    let own = OWN_WITHOUT_ROW
        .iter()
        .map(|own| own.words)
        .filter(|spelled| *spelled != cli_help::SERVE)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        rest, own,
        "the own commands without a row are the inventory's commands no row names"
    );
    // Workflow #434 CLI-COMP (register §1.318): its two own commands, `completion` and
    // `__complete`, have no row either.
    assert_eq!(own.len(), 36, "the survey's 34 and CLI-COMP's two");

    // The text for people: each entry once, its sentence beside it; plain ASCII, 100 columns.
    let text = people_of(&["help"]);
    assert_printable(&text);
    assert!(text.starts_with("usage: actinglab "), "{text}");
    assert!(
        !text.contains(" items]") && !text.contains(" members}"),
        "help prints its entries, not their counts"
    );
    for entry in list {
        let mut block = Vec::new();
        cli_help::columns(
            &mut block,
            entry["usage"].as_str().expect("its usage"),
            entry["help"].as_str().expect("its sentence"),
        );
        let block = block.join("\n");
        assert_eq!(
            text.matches(block.as_str()).count(),
            1,
            "{} is printed once with its sentence:\n{block}",
            entry["name"]
        );
    }
    // Every global option with its phrase, but the retired one.
    let mut options = words::GLOBAL_OPTIONS.to_vec();
    options.extend([cli_module::JSON, cli_help::HELP_FLAG]);
    for option in &options {
        let phrases = cli_help::GLOBAL_OPTION_PHRASES
            .iter()
            .filter(|(shown, _)| shown.split(' ').next() == Some(*option))
            .count();
        assert_eq!(
            phrases,
            usize::from(*option != "--require-session"),
            "{option}"
        );
    }
    assert_eq!(
        cli_help::GLOBAL_OPTION_PHRASES.len(),
        options.len() - 1,
        "the phrases are the options'"
    );
    for (shown, phrase) in cli_help::GLOBAL_OPTION_PHRASES {
        let mut block = Vec::new();
        cli_help::columns(&mut block, shown, phrase);
        assert!(text.contains(&block.join("\n")), "{shown}");
    }
    assert!(
        !text.contains("--require-session"),
        "the retired option is not listed"
    );
}

#[test]
fn cli_help_the_detail_of_each_kind_by_help_and_by_the_flag() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    let yes = |fact: bool| if fact { "yes" } else { "no" };
    for (name, spoken, begins, instance) in KINDS {
        let row = table()
            .operation(name)
            .unwrap_or_else(|| panic!("the row {name}"));
        let text = people_of(&[&["help"][..], spoken].concat());
        assert_eq!(
            people_of(&[spoken, &["--help"][..]].concat()),
            text,
            "{name}: help <words> and <words> --help"
        );
        let named = name.split(' ').collect::<Vec<_>>();
        assert_eq!(
            people_of(&[&["help"][..], &named[..]].concat()),
            text,
            "{name}: by the row's name"
        );
        assert_printable(&text);
        let first = text.lines().next().unwrap_or("");
        assert!(
            first.starts_with(&format!("actinglab {begins}")),
            "{name}: {first}"
        );
        let seen = read(&text);
        let sentence = row.help.as_deref().expect("an offered row's sentence");
        assert!(seen.contains(sentence), "{name}: its sentence\n{text}");
        let facts = format!(
            "Writes: {}. Irreversible: {}.",
            yes(row.writes),
            yes(row.irreversible)
        );
        assert!(seen.contains(&facts), "{name}: {facts}\n{text}");
        let runtime = match row.runtime {
            RuntimeNeed::Required => "Runtime: required",
            RuntimeNeed::Optional => "Runtime: optional",
            RuntimeNeed::Offline => "Runtime: not needed",
        };
        assert!(seen.contains(runtime), "{name}: {runtime}\n{text}");

        // The same detail as data: every parameter, each with a line of its own in the text.
        let line = printed(&[&["--json", "help"][..], spoken].concat())
            .line
            .expect("the detail's line");
        let command = &line["command"];
        assert_eq!(command["name"], name);
        assert_eq!(command["writes"], row.writes, "{name}");
        assert_eq!(command["irreversible"], row.irreversible, "{name}");
        assert_eq!(command["help"], sentence, "{name}");
        let params = command["params"].as_array().expect("its parameters");
        assert_eq!(params.len(), row.params.len(), "{name}");
        for param in params {
            let written = param["written"].as_str().expect("how it is written");
            assert!(
                text.lines()
                    .any(|line| line.starts_with(&format!("  {written}"))),
                "{name}: {written} has a line\n{text}"
            );
        }
        let declared = params
            .iter()
            .find(|param| param["name"] == "instance")
            .unwrap_or_else(|| panic!("{name}'s instance"));
        assert_eq!(declared["written"], instance, "{name}");
        match name {
            "frame" | "pause" => assert!(seen.contains(ALSO), "{name}: {ALSO}\n{text}"),
            "tap" => assert!(seen.contains(ONLY), "{name}: {ONLY}\n{text}"),
            "app launch" => assert!(
                seen.contains(
                    "Also: actinglab session app launch, actinglab's own command that serves this \
                     row."
                ),
                "{name}: both spellings, the row's name first\n{text}"
            ),
            _ => {}
        }
    }
}

#[test]
fn cli_help_groups_the_bare_line_and_a_flagged_line_that_runs_nothing() {
    let _guard = env_lock();
    let _isolated = Isolated::new();

    // An own group's `help` and `--help`, and `help <group>`: the group's commands.
    let lab = people_of(&["help", "lab"]);
    assert_eq!(people_of(&["lab", "help"]), lab, "lab help");
    assert_eq!(people_of(&["lab", "--help"]), lab, "lab --help");
    assert_printable(&lab);
    assert!(lab.starts_with("actinglab lab: "), "{lab}");
    for spoken in ["lab watch", "lab validate", "lab arbitrator", "lab status"] {
        assert!(
            lab.lines()
                .any(|line| line.starts_with(&format!("  {spoken}"))),
            "{spoken}\n{lab}"
        );
    }
    let line = printed(&["--json", "lab", "help"])
        .line
        .expect("the group's line");
    assert_eq!(line["group"], "lab");
    let members = line["members"]
        .as_array()
        .expect("its members")
        .iter()
        .map(|member| member["words"].as_str().expect("its words"))
        .collect::<Vec<_>>();
    for spoken in ["lab watch", "lab vendor-stdio-selftest", "lab receipt"] {
        assert!(members.contains(&spoken), "{spoken}: {members:?}");
    }
    for group in ["package", "control", "session", "run", "record"] {
        let text = people_of(&[group, "help"]);
        assert_eq!(people_of(&[group, "--help"]), text, "{group}");
        assert!(
            text.starts_with(&format!("actinglab {group}: ")),
            "{group}: {text}"
        );
        assert_printable(&text);
    }

    // The bare `actinglab`: the list, for people, as `help` prints it.
    assert!(cli_help::for_people(&parse_invocation(
        Vec::<String>::new(),
        false
    )));
    assert!(!cli_help::for_people(&parse_invocation(["--json"], false)));
    let bare = people_of(&[]);
    assert_eq!(bare, people_of(&["help"]));
    assert!(bare.starts_with("usage: actinglab "), "{bare}");

    // `--help` on a line with other flags runs nothing: no connection reaches the Runtime.
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let runtime = ScriptedRuntime::start(&state, |_| Vec::new());
    {
        let _state_root = use_runtime_state_root(&state);
        let lines: [(&[&str], &str); 4] = [
            (
                &[
                    "pause", ALIAS, "--reason", "drill", "--drain", "5s", "--help",
                ],
                "pause",
            ),
            (
                &[
                    "--instance",
                    ALIAS,
                    "tap",
                    "10",
                    "20",
                    "--touch-backend",
                    "adb_shell_input",
                    "--help",
                ],
                "tap",
            ),
            (
                &[
                    "run",
                    "start",
                    ALIAS,
                    "package-dir",
                    "--wait",
                    "1s",
                    "--help",
                ],
                "run start",
            ),
            (&["--json", "events", "--limit", "5", "--help"], "events"),
        ];
        for (line, topic) in lines {
            match cli_module::route(words_of(line)) {
                Route::Own(own) => assert!(
                    own.iter().any(|word| word == cli_help::HELP),
                    "{line:?}: {own:?}"
                ),
                other => panic!("{line:?} routed as {other:?}"),
            }
            let exit = printed(line);
            assert_eq!(exit.code, 0, "{line:?}: {:?}", exit.line);
            assert_eq!(
                exit.line.as_ref().expect("the line")["command"]["name"],
                topic,
                "{line:?}"
            );
        }
    }
    let record = runtime.finish();
    assert!(
        record.connections.is_empty() && record.unscripted.is_empty(),
        "a line with --help reached the Runtime: {record:?}"
    );
}

#[test]
fn cli_help_json_keeps_todays_members_and_adds_the_list() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    let result = run_cli(["--json", "help"], true);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.clone().expect("help's answer");
    let list = data.get("list").cloned().expect("the new list");
    // Every member it answered before, unchanged (this place lists every command).
    let before = data
        .as_object()
        .expect("help's members")
        .iter()
        .filter(|(member, _)| member.as_str() != "list")
        .map(|(member, value)| (member.clone(), value.clone()))
        .collect::<serde_json::Map<String, Value>>();
    for member in [
        "usage",
        "global_options",
        "command_options",
        "compatibility_notes",
        "commands",
    ] {
        assert!(before.contains_key(member), "{member}");
    }
    assert_eq!(
        Value::Object(before),
        help_data(),
        "today's members, unchanged"
    );

    // The list as data: each command's name, module, help, params and the three facts.
    let table = table();
    for entry in list.as_array().expect("the list") {
        for member in [
            "name",
            "module",
            "help",
            "params",
            "writes",
            "irreversible",
            "runtime",
        ] {
            assert!(entry.get(member).is_some(), "{member} of {entry}");
        }
        let name = entry["name"].as_str().expect("its name");
        match table
            .operation(name)
            .filter(|operation| words::offered(operation))
        {
            Some(row) => {
                assert_eq!(entry["help"], json!(row.help), "{name}");
                assert_eq!(entry["writes"], row.writes, "{name}");
                assert_eq!(entry["irreversible"], row.irreversible, "{name}");
                assert!(entry["runtime"].is_string(), "{name}");
                assert_eq!(
                    entry["params"].as_array().map(Vec::len),
                    Some(row.params.len()),
                    "{name}"
                );
            }
            None => {
                assert_eq!(entry["module"], cli_help::OWN, "{name}");
                for fact in ["writes", "irreversible", "runtime"] {
                    assert!(entry[fact].is_null(), "{name}: {fact}");
                }
            }
        }
    }
}

#[test]
fn cli_help_the_refusals_that_are_gone_and_status_help_unchanged() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    // Lines refused before this slice answer now.
    let answered: [&[&str]; 7] = [
        &["--help"],
        &["--json", "--help"],
        &["help", "--help"],
        &["help", "status"],
        &["status", "--help"],
        &["lab", "help"],
        &["session", "--help"],
    ];
    for line in answered {
        let exit = printed(line);
        assert_eq!(exit.code, 0, "{line:?}: {:?}", exit.line);
        assert_eq!(
            exit.line.as_ref().expect("the line")["outcome"]["code"],
            "lab_operation_completed",
            "{line:?}"
        );
    }

    // `status help` still reads `help` as the instance: the module's line, the call's instance.
    let status = words_of(&["status", "help"]);
    assert!(
        matches!(cli_module::route(status.clone()), Route::Module(_)),
        "status help is the module's line"
    );
    let call = words::parse(table(), &status).expect("status help");
    assert_eq!(call.name, "status");
    assert_eq!(call.params.get("instance"), Some(&json!("help")));

    // A word `help` does not know is refused as a read list refuses one.
    let refused = printed(&["help", "frobnicate"]);
    assert_eq!(refused.code, 1);
    let refusal = refused.line.expect("the refusal's line");
    assert_eq!(refusal["outcome"]["code"], "validation_failed");
    assert_eq!(refusal["message"], "help: unknown frobnicate");
    assert_eq!(
        refusal["details"],
        json!({"argument": "frobnicate", "argument_problem": "unknown"})
    );

    // The module's refusal of a row's words names `actinglab help`, not a contract file.
    let module = cli_module::run_line(&words_of(&["status", "--frobnicate"]));
    assert_eq!(module.code, 1);
    let line = developer(&module).expect("the developer line");
    assert!(
        line.contains("actinglab help lists the operations"),
        "{line}"
    );
    assert!(!line.contains("contracts/"), "{line}");
}
