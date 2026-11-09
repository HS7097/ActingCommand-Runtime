// SPDX-License-Identifier: AGPL-3.0-only

//! `app launch`, `app restart` and `app stop` (Workflow #434 LAB-1b; register §1.287), moved
//! from MCP's `ac_app` (Workflow #389 MCP-13a; MCP-0 command table v1.1 §4.14): the game of one
//! instance started, restarted or stopped by actinglab's `session app <action> --instance
//! <alias>`, which sends `ApplicationLifecycle` and answers its data. That session command is
//! actinglab's own (LAB-2); this is the command line the operation runs it with.

/// actinglab's command words.
pub const SESSION_APP: [&str; 2] = ["session", "app"];
/// The actions of actinglab's `session app` (`session_management.rs`); `force-stop` is the same
/// action as `stop`.
pub const ACTIONS: [&str; 4] = ["launch", "restart", "stop", "force-stop"];

/// actinglab's arguments for the action `action` on the instance alias `instance`, never with
/// `--record` (no recording step): `session app <action> --instance <instance>`. `None` for an
/// action that is none of [`ACTIONS`], refused before actinglab runs.
pub fn session_app(action: &str, instance: &str) -> Option<Vec<String>> {
    if !ACTIONS.contains(&action) {
        return None;
    }
    let mut line = SESSION_APP
        .iter()
        .map(|word| (*word).to_owned())
        .collect::<Vec<_>>();
    line.extend([
        action.to_owned(),
        "--instance".to_owned(),
        instance.to_owned(),
    ]);
    Some(line)
}
