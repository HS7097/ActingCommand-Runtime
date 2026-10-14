// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-13a: the parity of MCP `ac_app` and its command-line original, actinglab
// `session app <launch|restart|stop|force-stop> --instance <alias>` (MCP-0 command table v1.1
// §2.3, §4.14; #389 R8: an option tool is one actinglab command, run as `<root>\tools\
// actinglab.exe --json`). For the same action and instance, the tool runs exactly the command
// an operator types, after `--json`, against the Runtime of this server's state root, without
// `--record`; its answer is that command's envelope data unchanged, as for every Lab tool
// (gate 43, `interpret`). Workflow #389 U-2: MCP forwards the work, so `--via mcp` comes
// after `--json`, before the operator's command.

use super::super::{APP, APP_ACTIONS, Program, command_line};
use serde_json::{Map, json};
use std::ffi::OsStr;
use std::path::PathBuf;

const ALIAS: &str = "neutral.instance";
const STATE_ROOT_ENV: &str = "ACTINGCOMMAND_RUNTIME_STATE_ROOT";

#[test]
fn gate_ac_app_runs_actinglab_session_app_with_the_same_action_and_instance() {
    let state_root = PathBuf::from("neutral-state");
    let program = Program {
        actinglab: PathBuf::from("neutral-install")
            .join("tools")
            .join("actinglab.exe"),
        root: PathBuf::from("neutral-install"),
        config: PathBuf::from("neutral-install").join("actingd.config.json"),
        installation: None,
        state_root: Ok(state_root.clone()),
    };
    for action in APP_ACTIONS {
        let mut arguments = Map::new();
        arguments.insert("action".to_owned(), json!(action));
        arguments.insert("instance".to_owned(), json!(ALIAS));
        let line = match command_line(&APP, &arguments) {
            Ok(line) => line,
            Err(error) => panic!(
                "ac_app with action {action} was refused: {}",
                error.to_value()
            ),
        };
        let command = program.command(&line);
        assert_eq!(command.get_program(), program.actinglab.as_os_str());
        let words = command
            .get_args()
            .map(|word| word.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            words,
            [
                "--json",
                "--via",
                "mcp",
                "session",
                "app",
                action,
                "--instance",
                ALIAS
            ],
            "ac_app with action {action}"
        );
        assert!(
            !words.iter().any(|word| word == "--record"),
            "ac_app records no Lab recording step: {words:?}"
        );
        let runtime = command
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(STATE_ROOT_ENV))
            .and_then(|(_, value)| value);
        assert_eq!(
            runtime,
            Some(state_root.as_os_str()),
            "ac_app with action {action} talks to another Runtime"
        );
    }
}
