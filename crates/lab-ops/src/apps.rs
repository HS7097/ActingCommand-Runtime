// SPDX-License-Identifier: AGPL-3.0-only

//! `app launch`, `app restart` and `app stop` (Workflow #434 LAB-1b; register §1.287), moved
//! from MCP's `ac_app` (Workflow #389 MCP-13a; MCP-0 command table v1.1 §4.14): the game of one
//! instance started, restarted or stopped by actinglab's `session app <action> --instance
//! <alias>`, which sends `ApplicationLifecycle`. The operation runs that command as a child
//! ([`run`], through [`crate::lab_child`]) and answers its envelope's data; the session command
//! itself is actinglab's own (LAB-2).

use crate::lab_child::{self, Actinglab, Answer};
use std::process::Command;

/// actinglab's command words.
const SESSION_APP: [&str; 2] = ["session", "app"];
/// actinglab's option naming the instance.
const INSTANCE: &str = "--instance";
/// The actions of actinglab's `session app` (`session_management.rs`); `force-stop` is the same
/// action as `stop`.
pub const ACTIONS: [&str; 4] = ["launch", "restart", "stop", "force-stop"];

/// One `app` operation: an action of [`ACTIONS`] on an instance alias.
pub struct App {
    action: &'static str,
    instance: String,
}

impl App {
    /// The action `action` on the instance alias `instance`; `None` for an action that is none of
    /// [`ACTIONS`], refused before actinglab runs.
    pub fn new(action: &str, instance: &str) -> Option<Self> {
        let action = ACTIONS.into_iter().find(|known| *known == action)?;
        Some(Self {
            action,
            instance: instance.to_owned(),
        })
    }

    /// actinglab's arguments: `session app <action> --instance <instance>`, never with `--record`
    /// (no recording step).
    fn line(&self) -> Vec<String> {
        let mut line = SESSION_APP
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>();
        line.extend([
            self.action.to_owned(),
            INSTANCE.to_owned(),
            self.instance.clone(),
        ]);
        line
    }
}

/// The child that serves `app`: `lab`'s actinglab with `app`'s arguments.
pub fn command(lab: &Actinglab<'_>, app: &App) -> Command {
    lab.command(&app.line())
}

/// `app launch|restart|stop`: `app` run by `lab`'s actinglab to its end (its `session app` sends
/// `ApplicationLifecycle`), answering the envelope's data or the failure it names.
pub fn run(lab: &Actinglab<'_>, app: &App) -> Answer {
    lab_child::run(command(lab, app))
}
