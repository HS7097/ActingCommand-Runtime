// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 CLI-COMP (register §1.318; contracts/lab-two-entries.md §9.3, "Completion";
// coordinator work order rd-1111/brief-cli-completion.md, the coordinator's eight rulings of
// 2026-11-11, not Alice's words): `actinglab completion powershell|bash` prints a thin script, and
// `actinglab __complete`, the callback it calls, prints the candidates of the word being typed.
// - `cli_comp_command_words_by_prefix_at_each_position`: the first word (every offered row's and
//   every own command's but the callback's), by prefix; the second word of a group; `help <words>`
//   as the words; the global options at the first word and after an own command without a row,
//   which completes nothing else.
// - `cli_comp_a_row_s_flags_and_an_enumeration_s_words`: after a row's words its options and
//   switches with the global options its line takes, one already on the line left out and a
//   repeating one kept; an enumeration's words after its option, by prefix; nothing after an
//   option of another type.
// - `cli_comp_instances_from_the_runtime_else_the_configuration`: at an instance's place, a
//   scripted Runtime's aliases, by prefix, asked with one Status after the handshake; a Runtime
//   that does not answer within the wait, and none at all, give the configuration's aliases, or
//   none.
// - `cli_comp_a_row_whose_instance_is_only_the_flag_offers_the_flag`: `tap` (register §1.307's
//   note "only --instance") offers `--instance` and its aliases, no alias in a positional's place;
//   `pause` takes either, never both.
// - `cli_comp_anything_wrong_gives_no_candidate`: an index beyond the words, the program's own
//   word, a word that is no command, and lines the callback cannot read print nothing (with
//   `--json`, no candidate); a line that reaches the dispatch answers the same candidates as data.
//   That the operation table this build embeds does not read is not reachable in a test.
// - `cli_comp_the_scripts_are_ascii_and_pass_the_words_marked`: each script is plain ASCII, names
//   the callback and its shell's registration, passes the words quoted and marked (asserted on the
//   text; no shell runs), and `completion <shell>` prints it; words written so, with a space, a
//   quote, a percent sign or a tab, read back whole; a missing or unknown shell is refused.
// The exit code and stderr of the callback's process are `completion_callback`'s (tests/).
// Fakes only: the operation table this build embeds, neutral aliases, a configuration in a
// temporary folder and the runtime client's scripted Runtime on loopback; no device, no clock, no
// sleep.

use super::gate_434_lab5b4::{Isolated, printed};
use super::*;
use crate::cli_completion::{self, BASH, CALLBACK, POWERSHELL, Sources};
use crate::cli_help::{self, OWN_WITHOUT_ROW};
use crate::cli_module::{self, words};
use actingcommand_contract::{
    IdentifierIssuer, OwnerEpoch, RuntimeControlPlaneStatus, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeRequest, RuntimeResult,
};
use actingcommand_lab_ops::packs::ROOT_CONFIG_FILE;
use actingcommand_lab_ops::table::{Operation, OperationTable, ParamForm, ParamType};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;

fn table() -> &'static OperationTable {
    cli_module::table().expect("the operation table")
}

fn owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// `word` in the word form, as both scripts write it: the mark, then `%`, `"`, a space and a tab
/// as `%25`, `%22`, `%20` and `%09`.
fn marked(word: &str) -> String {
    format!(
        ":{}",
        word.replace('%', "%25")
            .replace('"', "%22")
            .replace(' ', "%20")
            .replace('\t', "%09")
    )
}

/// The callback's arguments for `typed` (the program's word first; the last word the one being
/// completed, its index the last), after `more`.
fn line(more: &[&str], typed: &[&str]) -> Vec<String> {
    let mut args = owned(more);
    args.push((typed.len() - 1).to_string());
    args.extend(typed.iter().map(|word| marked(word)));
    args
}

/// What the callback prints for `typed`, one candidate per line, the instances asked from `here`.
fn completed_with(
    more: &[&str],
    typed: &[&str],
    here: &dyn Fn(Duration) -> Sources,
) -> Vec<String> {
    cli_completion::callback_text(&line(more, typed), here)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The same for a line with no instance's place: asking for the instances fails the test.
fn completed(typed: &[&str]) -> Vec<String> {
    completed_with(&[], typed, &unasked)
}

fn unasked(_: Duration) -> Sources {
    panic!("no instance is completed at this place")
}

fn has(found: &[String], word: &str) -> bool {
    found.iter().any(|candidate| candidate == word)
}

/// The command words of every command this build lists: each offered row's name and other
/// spellings, and each own command without a row.
fn listed() -> Vec<Vec<String>> {
    let mut names = Vec::new();
    for operation in table()
        .operations
        .iter()
        .filter(|operation| words::offered(operation))
    {
        for name in cli_help::row_names(operation) {
            names.push(owned(&name.split(' ').collect::<Vec<_>>()));
        }
    }
    for own in &OWN_WITHOUT_ROW {
        names.push(owned(&own.words.split(' ').collect::<Vec<_>>()));
    }
    names
}

/// A selected configuration of an installation root `root` that binds the instances `aliases`.
fn configured(root: &Path, aliases: &[&str]) {
    let instances = aliases
        .iter()
        .enumerate()
        .map(|(index, alias)| json!({"alias": alias, "instance_id": format!("neutral-{index}")}))
        .collect::<Vec<_>>();
    fs::write(
        root.join(ROOT_CONFIG_FILE),
        json!({"instances": instances}).to_string(),
    )
    .unwrap();
}

/// The scripted Runtime's answer to the read that resolves an instance: the instances `aliases`.
fn status(
    owner_epoch: OwnerEpoch,
    aliases: &'static [&'static str],
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        assert!(
            matches!(request.operation(), RuntimeOperation::Status),
            "the read that resolves an instance is Status, got {:?}",
            request.operation()
        );
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instances = aliases
            .iter()
            .map(|alias| {
                let instance_id = *issuer
                    .mint_instance_id()
                    .expect("an instance id")
                    .transport();
                RuntimeInstanceStatus::new(*alias, instance_id, false, 0, false, false, false)
                    .expect("an instance's status")
            })
            .collect();
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, instances).expect("the scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

/// The flags of `row`'s options and switches, `--` and the name with `_` as `-`.
fn flags_of(row: &Operation) -> BTreeSet<String> {
    row.params
        .iter()
        .filter(|param| param.form != ParamForm::Positional)
        .map(|param| format!("--{}", param.name.replace('_', "-")))
        .collect()
}

#[test]
fn cli_comp_command_words_by_prefix_at_each_position() {
    let _guard = env_lock();
    let names = listed();

    // The first word: every offered row's and every own command's, but the callback's.
    let first = completed(&["actinglab", ""]);
    for name in names.iter().filter(|name| name[0] != CALLBACK) {
        assert!(has(&first, &name[0]), "{name:?} is not offered: {first:?}");
    }
    assert!(!has(&first, CALLBACK), "the callback is not for people");
    for word in &first {
        assert!(
            names.iter().any(|name| &name[0] == word),
            "{word} begins no command"
        );
    }
    for word in [
        "pause",
        "events",
        "tap",
        "help",
        "version",
        "completion",
        "lab",
        "session",
        "serve",
    ] {
        assert!(has(&first, word), "{word}: {first:?}");
    }
    // By prefix.
    let pa = completed(&["actinglab", "pa"]);
    let expected = first
        .iter()
        .filter(|word| word.starts_with("pa"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(pa, expected);
    assert!(has(&pa, "pause") && has(&pa, "packs"), "{pa:?}");

    // The second word of a group: the words under it, by prefix.
    let lab = completed(&["actinglab", "lab", ""]);
    for word in ["watch", "validate", "status", "arbitrator"] {
        assert!(has(&lab, word), "lab {word}: {lab:?}");
    }
    for word in &lab {
        assert!(
            names
                .iter()
                .any(|name| name.len() > 1 && name[0] == "lab" && &name[1] == word),
            "lab {word} names no command"
        );
    }
    let v = lab
        .iter()
        .filter(|word| word.starts_with('v'))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(completed(&["actinglab", "lab", "v"]), v);
    assert!(
        has(&v, "validate") && has(&v, "vendor-stdio-selftest"),
        "{v:?}"
    );

    // `help <words>` completes as the words themselves do.
    assert_eq!(completed(&["actinglab", "help", "pa"]), pa);
    assert_eq!(completed(&["actinglab", "help", "lab", ""]), lab);

    // The global options a person gives, at the first word.
    let options = completed(&["actinglab", "--"]);
    for option in [
        "--json",
        "--help",
        "--instance",
        "--run-root",
        "--resource-root",
        "--game",
        "--server",
        "--dry-run",
        "--verbose",
        "--version",
    ] {
        assert!(has(&options, option), "{option}: {options:?}");
    }
    for option in &options {
        assert!(
            option == cli_module::JSON
                || option == cli_help::HELP_FLAG
                || words::GLOBAL_OPTIONS.contains(&option.as_str()),
            "{option} is no global option"
        );
    }
    for option in ["--via", "--require-session"] {
        assert!(!has(&options, option), "{option} is not a person's");
    }
    // An own command without a row: its words, then the global options only.
    for own in ["version", "completion"] {
        assert_eq!(completed(&["actinglab", own, "--"]), options, "{own}");
        assert!(completed(&["actinglab", own, ""]).is_empty(), "{own}");
    }
}

#[test]
fn cli_comp_a_row_s_flags_and_an_enumeration_s_words() {
    let _guard = env_lock();
    let events = table().operation("events").expect("the row events");

    // After a row of the CLI module: its options and switches, `--json`, `--help` and
    // `--instance`, which may name its instance.
    let mut expected = flags_of(events);
    expected.extend(["--json", "--help", "--instance"].map(str::to_owned));
    let found = completed(&["actinglab", "events", "--"]);
    assert_eq!(found.iter().cloned().collect::<BTreeSet<_>>(), expected);
    // One already on the line is left out, and so is `--instance` once the instance is given.
    let given = completed(&["actinglab", "events", "--limit", "5", "--"]);
    assert!(
        !has(&given, "--limit") && has(&given, "--view"),
        "{given:?}"
    );
    let named = completed(&["actinglab", "events", "neutral.a", "--"]);
    assert!(
        !has(&named, "--instance") && has(&named, "--view"),
        "{named:?}"
    );

    // A repeating one is kept: `ensure`'s `--scene`, while its `--to` is given once.
    let ensure_row = table().operation("ensure").expect("the row ensure");
    let scene = ensure_row
        .params
        .iter()
        .find(|param| param.name == "scene")
        .expect("ensure's scene");
    assert!(scene.repeat && scene.form == ParamForm::Option);
    let ensure = completed(&[
        "actinglab",
        "ensure",
        "--scene",
        "a.png",
        "--to",
        "home",
        "--",
    ]);
    assert!(has(&ensure, "--scene"), "a repeating option: {ensure:?}");
    assert!(!has(&ensure, "--to"), "an option given: {ensure:?}");
    assert!(has(&ensure, "--page"), "{ensure:?}");
    // Its command takes actinglab's global options as written, so a person's are offered too.
    assert!(
        has(&ensure, "--run-root") && has(&ensure, "--json"),
        "{ensure:?}"
    );

    // An enumeration's words after its option, by prefix.
    let view = events
        .params
        .iter()
        .find(|param| param.name == "view")
        .expect("events' view");
    let ParamType::Enum(choices) = &view.value_type else {
        panic!("events' view is an enumeration");
    };
    let mut choices = choices.clone();
    choices.sort();
    assert_eq!(completed(&["actinglab", "events", "--view", ""]), choices);
    assert_eq!(
        completed(&["actinglab", "events", "--min-severity", "w"]),
        ["warning"]
    );
    // An option of another type: nothing, the shell's own completion.
    assert!(completed(&["actinglab", "events", "--limit", ""]).is_empty());
}

#[test]
fn cli_comp_instances_from_the_runtime_else_the_configuration() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let install = temp.path().join("install");
    fs::create_dir_all(&install).unwrap();
    configured(&install, &["neutral.config-a", "neutral.config-b"]);
    const ALIASES: &[&str] = &["neutral.a", "neutral.b", "other.c"];
    let runtime = ScriptedRuntime::start(&state, |owner_epoch| {
        vec![
            ScriptedConnection::new().request(status(owner_epoch, ALIASES)),
            ScriptedConnection::new().request(status(owner_epoch, ALIASES)),
            ScriptedConnection::new().request(|_| ScriptedAnswer::StallUntilClosed),
        ]
    });
    let here = |wait| Sources {
        state_root: Some(state.clone()),
        install_root: Some(install.clone()),
        wait,
    };

    // A Runtime that answers: its aliases, by prefix, in a positional instance's place and after
    // `--instance`.
    assert_eq!(
        completed_with(
            &["--wait-ms", "60000"],
            &["actinglab", "pause", "neutral."],
            &here
        ),
        ["neutral.a", "neutral.b"]
    );
    assert_eq!(
        completed_with(
            &["--wait-ms", "60000"],
            &["actinglab", "events", "--instance", ""],
            &here
        ),
        ALIASES
    );
    // A Runtime that does not answer within the wait: the configuration's aliases.
    assert_eq!(
        completed_with(&["--wait-ms", "2000"], &["actinglab", "pause", ""], &here),
        ["neutral.config-a", "neutral.config-b"]
    );
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond one per read"
    );
    for connection in &record.connections {
        assert_eq!(connection.requests.len(), 3, "the handshake and one read");
        assert_eq!(
            connection.count(|operation| matches!(operation, RuntimeOperation::Status)),
            1
        );
    }

    // No Runtime at all: the configuration's aliases, by prefix; with no configuration either,
    // none. The wait 0 asks no Runtime.
    let nowhere = temp.path().join("nowhere");
    fs::create_dir_all(&nowhere).unwrap();
    let configured_only = |wait| Sources {
        state_root: Some(nowhere.clone()),
        install_root: Some(install.clone()),
        wait,
    };
    assert_eq!(
        completed_with(
            &[],
            &["actinglab", "pause", "neutral.config-b"],
            &configured_only
        ),
        ["neutral.config-b"]
    );
    assert_eq!(
        completed_with(&["--wait-ms", "0"], &["actinglab", "pause", ""], &here),
        ["neutral.config-a", "neutral.config-b"]
    );
    let neither = |wait| Sources {
        state_root: Some(nowhere.clone()),
        install_root: Some(nowhere.clone()),
        wait,
    };
    assert!(completed_with(&[], &["actinglab", "pause", ""], &neither).is_empty());
}

#[test]
fn cli_comp_a_row_whose_instance_is_only_the_flag_offers_the_flag() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    configured(temp.path(), &["neutral.a", "neutral.b"]);
    let here = |wait| Sources {
        state_root: None,
        install_root: Some(temp.path().to_path_buf()),
        wait,
    };
    let tap = table().operation("tap").expect("the row tap");
    assert!(cli_help::own_named(tap) && words::positional_instance(tap).is_some());
    assert!(
        tap.params
            .iter()
            .find(|param| param.name == "instance")
            .and_then(|param| param.note.as_deref())
            .is_some_and(|note| note.contains("only --instance")),
        "CLI-IN's note"
    );

    // No alias in a positional's place: the first positional of tap's line is its point.
    assert!(completed(&["actinglab", "tap", ""]).is_empty());
    assert!(completed(&["actinglab", "tap", "neu"]).is_empty());
    // The flag, and the aliases after it; given once, it is not offered again.
    assert!(has(&completed(&["actinglab", "tap", "--"]), "--instance"));
    assert_eq!(
        completed_with(&[], &["actinglab", "tap", "--instance", "neutral."], &here),
        ["neutral.a", "neutral.b"]
    );
    assert!(!has(
        &completed(&["actinglab", "tap", "--instance", "neutral.a", "--"]),
        "--instance"
    ));

    // A row of the CLI module takes the instance in its positional's place or as the flag, and
    // once it is given the flag is not offered.
    assert_eq!(
        completed_with(&[], &["actinglab", "pause", ""], &here),
        ["neutral.a", "neutral.b"]
    );
    assert!(has(&completed(&["actinglab", "pause", "--"]), "--instance"));
    assert!(!has(
        &completed(&["actinglab", "pause", "neutral.a", "--"]),
        "--instance"
    ));
    // Named by the flag before the row's words, it is not asked for again: the row's flags.
    let flagged_first = completed(&["actinglab", "--instance", "neutral.a", "pause", ""]);
    assert!(
        has(&flagged_first, "--reason") && !has(&flagged_first, "--instance"),
        "{flagged_first:?}"
    );
}

#[test]
fn cli_comp_anything_wrong_gives_no_candidate() {
    let _guard = env_lock();
    let lines: [&[&str]; 12] = [
        // An index beyond the words, and the program's own word.
        &["5", ":actinglab", ":pa"],
        &["0", ":actinglab"],
        // A word that is no command, and a beginning no command has.
        &["2", ":actinglab", ":frobnicate", ":"],
        &["1", ":actinglab", ":frobn"],
        // Lines the callback cannot read: no index, two, a flag it does not read, words without
        // the mark, a wait that is no number or has no value, a word that is no UTF-8, nothing.
        &[":actinglab", ":pa"],
        &["1", "1", ":actinglab", ":pa"],
        &["--frobnicate", "1", ":actinglab", ":pa"],
        &["1", "actinglab", "pa"],
        &["--wait-ms", "soon", "1", ":actinglab", ":pa"],
        &["1", ":actinglab", ":pa", "--wait-ms"],
        &["1", ":actinglab", ":%FF"],
        &[],
    ];
    for args in lines {
        assert_eq!(
            cli_completion::callback_text(&owned(args), &unasked),
            "",
            "{args:?}"
        );
    }
    // With `--json`: the line of an own command, with no candidate.
    let text = cli_completion::callback_text(&owned(&["--json", "5", ":actinglab"]), &unasked);
    let printed_line: Value = serde_json::from_str(text.trim_end()).expect("one JSON line");
    assert_eq!(printed_line["candidates"], json!([]));
    assert_eq!(
        printed_line["outcome"]["code"], "lab_operation_completed",
        "{printed_line}"
    );
    // A line that reaches the dispatch answers the same candidates as data.
    let result = run_cli(["--json", CALLBACK, "1", ":actinglab", ":pa"], true);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let data = result.envelope.data.clone().expect("the callback's data");
    assert_eq!(data["candidates"], json!(completed(&["actinglab", "pa"])));
}

#[test]
fn cli_comp_the_scripts_are_ascii_and_pass_the_words_marked() {
    let _guard = env_lock();
    let _isolated = Isolated::new();
    for (shell, script) in cli_completion::SHELLS {
        assert!(
            script
                .bytes()
                .all(|byte| byte == b'\n' || (b' '..=b'~').contains(&byte)),
            "{shell}: plain ASCII"
        );
        assert!(script.contains(CALLBACK), "{shell}: names the callback");
        // `completion <shell>` prints it as it is, and with `--json` as data.
        let exit = printed(&["completion", shell]);
        assert_eq!(
            cli_completion::script_text(&exit).as_deref(),
            Some(script.trim_end_matches('\n')),
            "{shell}"
        );
        let json_line = printed(&["--json", "completion", shell])
            .line
            .expect("the JSON line");
        assert_eq!(json_line["shell"], shell);
        assert_eq!(json_line["script"], script);
    }
    // PowerShell: the native registration, the words up to the cursor, each marked and escaped,
    // passed to the callback with the index of the last.
    for text in [
        "Register-ArgumentCompleter -Native -CommandName 'actinglab', 'actinglab.exe'",
        "foreach ($element in $commandAst.CommandElements)",
        "if ($wordToComplete -eq '') { $words += , '' }",
        r#"':' + $word.Replace('%', '%25').Replace('"', '%22').Replace(' ', '%20').Replace("`t", '%09')"#,
        "& $words[0] __complete ($words.Count - 1) $marked 2>$null",
    ] {
        assert!(POWERSHELL.contains(text), "PowerShell: {text}");
    }
    // bash: `complete -F`, with the shell's own file names where no candidate comes back; every
    // word quoted, marked and escaped.
    for text in [
        "complete -o default -F _actinglab_complete actinglab actinglab.exe",
        r#"for word in "${COMP_WORDS[@]:0:COMP_CWORD+1}"; do"#,
        r#"word=${word//\%/%25}"#,
        r#"word=${word//\"/%22}"#,
        "word=${word// /%20}",
        r#"word=${word//$'\t'/%09}"#,
        r#"marked+=(":$word")"#,
        r#""${COMP_WORDS[0]}" __complete "$COMP_CWORD" "${marked[@]}" 2>/dev/null"#,
    ] {
        assert!(BASH.contains(text), "bash: {text}");
    }

    // Words so written read back whole: a global option's value with a quote, a space, a tab and
    // a percent sign leaves the line as it is without it, and a typed alias with a space or a
    // quote is one word.
    let plain = completed(&["actinglab", "pa"]);
    assert!(!plain.is_empty());
    assert_eq!(
        completed(&["actinglab", "--game", "it's a \"fixture\"\t100%", "pa"]),
        plain
    );
    let temp = TempDir::new().unwrap();
    configured(temp.path(), &["neutral a", "neutral\"b", "neutral.c"]);
    let here = |wait| Sources {
        state_root: None,
        install_root: Some(temp.path().to_path_buf()),
        wait,
    };
    assert_eq!(
        completed_with(&[], &["actinglab", "pause", "neutral "], &here),
        ["neutral a"]
    );
    assert_eq!(
        completed_with(&[], &["actinglab", "pause", "neutral\""], &here),
        ["neutral\"b"]
    );

    // `completion` without a shell, or with another word, is refused as a read list refuses one.
    let refused: [(&[&str], &str, &str); 3] = [
        (&["completion"], "<shell>", "required"),
        (&["completion", "zsh"], "zsh", "not_recognised"),
        (
            &["completion", "bash", "--frobnicate"],
            "--frobnicate",
            "unknown",
        ),
    ];
    for (refused_line, argument, problem) in refused {
        let exit = printed(refused_line);
        assert_eq!(exit.code, 1, "{refused_line:?}");
        let refusal = exit.line.expect("the refusal's line");
        assert_eq!(
            refusal["outcome"]["code"], "validation_failed",
            "{refused_line:?}"
        );
        assert_eq!(
            refusal["details"],
            json!({"argument": argument, "argument_problem": problem}),
            "{refused_line:?}"
        );
    }
}
