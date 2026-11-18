// SPDX-License-Identifier: AGPL-3.0-only

//! Where a command of the application service is (board #479, ACIB-3b): the stage `running`,
//! `launch` or `stop` is in, and how long each stage it finished took, timed from the command's
//! start. The command enters each stage as its code takes the step ([`crate::bridge`]); when
//! its deadline passes, its line names the stage it was waiting in and the times of those that
//! finished before it, and when it ends, the times of all of them. Nothing but a stage's word
//! and a duration is kept: no identifier of the phone or of an application.

use crate::answer::{Stage, StageTimes};
use std::time::Duration;

/// The stages a command has gone through so far, each timed from the command's start.
#[derive(Debug, Default)]
pub struct Steps {
    /// The stage the command is in and when it entered it.
    current: Option<(Stage, Duration)>,
    /// Each stage it finished and how long it took, in the order taken.
    finished: Vec<(Stage, Duration)>,
}

impl Steps {
    /// The command enters `stage` at `at` after its start; the stage it was in is finished then.
    pub fn enter(&mut self, stage: Stage, at: Duration) {
        self.close(at);
        self.current = Some((stage, at));
    }

    /// The stage the command is in, if it entered one.
    pub fn current(&self) -> Option<Stage> {
        self.current.map(|(stage, _)| stage)
    }

    /// The times of the stages finished so far, not of the one the command is in; `None` when
    /// it entered none (a command that is not of the application service).
    pub fn finished(&self) -> Option<StageTimes> {
        (self.current.is_some() || !self.finished.is_empty())
            .then(|| StageTimes::of(&self.finished))
    }

    /// The command ends at `at`: the stage it is in is finished then. The times of every stage
    /// it went through.
    pub fn end(&mut self, at: Duration) -> StageTimes {
        self.close(at);
        StageTimes::of(&self.finished)
    }

    /// The stage the command is in, if any, is finished at `at`.
    fn close(&mut self, at: Duration) {
        if let Some((stage, since)) = self.current.take() {
            self.finished.push((stage, at.saturating_sub(since)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::answer::{CommandName, Line};
    use std::collections::BTreeSet;

    /// The code of the commands, where each enters its stages (`bridge.rs`).
    const BRIDGE: &str = include_str!("bridge.rs");

    /// Every stage of [`Stage`], to read a stage's name in the code by.
    const ALL_STAGES: [Stage; 14] = [
        Stage::Session,
        Stage::Tunnel,
        Stage::ServiceHandshake,
        Stage::ServerFirstMessage,
        Stage::PictureRequest,
        Stage::ProxyTunnel,
        Stage::DiscoveryHandshake,
        Stage::ServiceConnection,
        Stage::ServiceXpcHandshake,
        Stage::ApplicationList,
        Stage::ProcessList,
        Stage::LaunchRequest,
        Stage::KillSignals,
        Stage::LooksAfterSignals,
    ];

    /// The stage the code names `Stage::<name>`: the one whose word is `name` in snake case.
    fn named(name: &str) -> Stage {
        let mut snake = String::new();
        for (index, character) in name.chars().enumerate() {
            if character.is_ascii_uppercase() && index > 0 {
                snake.push('_');
            }
            snake.push(character.to_ascii_lowercase());
        }
        ALL_STAGES
            .into_iter()
            .find(|stage| word(*stage) == snake)
            .unwrap_or_else(|| panic!("no stage is named {name}"))
    }

    /// The code of the function `name` of `bridge.rs`, from its `fn` to the brace that closes it
    /// (rustfmt closes a function of the module at the start of a line).
    fn body(name: &str) -> &'static str {
        let start = BRIDGE
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("bridge.rs has no fn {name}"));
        let rest = &BRIDGE[start..];
        let end = rest
            .find("\n}")
            .unwrap_or_else(|| panic!("fn {name} does not end"));
        &rest[..end]
    }

    /// The stages the function `name` of `bridge.rs` enters, in the order of its code: each
    /// `context.enter(Stage::<name>)` in it and, where it calls `app_service`, the stages that
    /// function enters. Each of these steps is taken in the order the code is written (no
    /// branch or loop of these functions enters a stage before one written above it).
    fn entered(name: &str) -> Vec<Stage> {
        const ENTER: &str = "context.enter(Stage::";
        const OPENING: &str = "app_service(target, context)";
        let mut rest = body(name);
        let mut stages = Vec::new();
        loop {
            let enter = rest.find(ENTER).unwrap_or(usize::MAX);
            let opening = rest.find(OPENING).unwrap_or(usize::MAX);
            if enter == usize::MAX && opening == usize::MAX {
                break;
            }
            if enter < opening {
                let after = &rest[enter + ENTER.len()..];
                let end = after.find(')').expect("a stage's name ends");
                stages.push(named(&after[..end]));
                rest = &after[end..];
            } else {
                stages.extend(entered("app_service"));
                rest = &rest[opening + OPENING.len()..];
            }
        }
        stages
    }

    /// The stages of each command of the application service, the longest way through (a
    /// bundle installed and, for `stop`, running), as its code in `bridge.rs` enters them.
    fn paths() -> [(CommandName, Vec<Stage>); 3] {
        [
            (CommandName::Running, entered("running")),
            (CommandName::Launch, entered("launch")),
            (CommandName::Stop, entered("stop")),
        ]
    }

    /// The word a stage serializes to.
    fn word(stage: Stage) -> String {
        serde_json::to_value(stage)
            .expect("a stage serializes")
            .as_str()
            .expect("a stage is a word")
            .to_owned()
    }

    fn ms(milliseconds: u64) -> Duration {
        Duration::from_millis(milliseconds)
    }

    /// A command in a stage names it; each stage it finished before has the time from its entry
    /// to the next stage's, the one it is in has none; at its end each stage has its time.
    #[test]
    fn a_command_names_the_stage_it_waits_in_and_the_time_of_each_finished_one() {
        let mut steps = Steps::default();
        assert_eq!(steps.current(), None);
        assert_eq!(steps.finished(), None);
        steps.enter(Stage::Session, ms(0));
        assert_eq!(steps.current(), Some(Stage::Session));
        assert_eq!(steps.finished(), Some(StageTimes::of(&[])));
        steps.enter(Stage::ProxyTunnel, ms(812));
        steps.enter(Stage::DiscoveryHandshake, Duration::from_micros(900_600));
        assert_eq!(steps.current(), Some(Stage::DiscoveryHandshake));
        assert_eq!(
            steps.finished(),
            Some(StageTimes::of(&[
                (Stage::Session, ms(812)),
                (Stage::ProxyTunnel, Duration::from_micros(88_600)),
            ]))
        );
        let line = Line::deadline_passed(
            CommandName::Running,
            Some(3),
            None,
            steps.current(),
            steps.finished(),
        );
        assert_eq!(
            serde_json::to_string(&line).expect("a line serializes"),
            r#"{"command":"running","device":3,"problem":"deadline_passed","stage":"discovery_handshake","stage_ms":{"session":812,"proxy_tunnel":88}}"#
        );
        assert!(!line.answered());
        let times = steps.end(ms(1_000));
        assert_eq!(steps.current(), None);
        assert_eq!(
            serde_json::to_string(&times).expect("times serialize"),
            r#"{"session":812,"proxy_tunnel":88,"discovery_handshake":99}"#
        );
    }

    /// Each stage of the application service's commands is reached on a command's way, as the
    /// code in `bridge.rs` enters them (ACIB-3b, second round: read from the code, not from a
    /// table of its own), and a deadline in it names it, with the times of the stages before it
    /// on that way and of no other; every stage word of these commands is reached so, each
    /// command's way opens with the same five stages, and none of the screenshot's words that
    /// means something else here (`tunnel`, `service_handshake`) is.
    #[test]
    fn each_stage_of_the_application_service_is_reached_and_named_by_its_deadline() {
        let opening = entered("app_service");
        assert_eq!(opening.len(), 5, "{opening:?}");
        for (command, path) in paths() {
            assert_eq!(path[..5], opening[..], "{command:?}");
        }
        let mut reached = BTreeSet::new();
        for (command, path) in paths() {
            let mut steps = Steps::default();
            for (index, stage) in path.iter().enumerate() {
                let at = ms(100 * u64::try_from(index).expect("a short path"));
                steps.enter(*stage, at);
                reached.insert(word(*stage));
                let line = Line::deadline_passed(
                    command,
                    Some(3),
                    None,
                    steps.current(),
                    steps.finished(),
                );
                let value = serde_json::to_value(&line).expect("a line serializes");
                assert_eq!(value["stage"], serde_json::json!(word(*stage)), "{value}");
                // The members are compared as a set (a JSON value need not keep their order);
                // the order is the string's, below.
                let before: BTreeSet<String> =
                    path[..index].iter().map(|done| word(*done)).collect();
                let named: BTreeSet<String> = value["stage_ms"]
                    .as_object()
                    .expect("the times of the stages before")
                    .iter()
                    .map(|(key, took)| {
                        assert_eq!(took, &serde_json::json!(100), "{key} in {value}");
                        key.clone()
                    })
                    .collect();
                assert_eq!(named, before, "{value}");
            }
            let times = serde_json::to_string(&steps.end(ms(5_000))).expect("times serialize");
            let all = path
                .iter()
                .enumerate()
                .map(|(index, stage)| {
                    let took = if index + 1 == path.len() {
                        5_000 - 100 * index
                    } else {
                        100
                    };
                    format!(r#""{}":{took}"#, word(*stage))
                })
                .collect::<Vec<_>>()
                .join(",");
            assert_eq!(times, format!("{{{all}}}"), "{command:?}");
        }
        let expected: BTreeSet<String> = [
            "session",
            "proxy_tunnel",
            "discovery_handshake",
            "service_connection",
            "service_xpc_handshake",
            "application_list",
            "process_list",
            "launch_request",
            "kill_signals",
            "looks_after_signals",
        ]
        .iter()
        .map(|stage| (*stage).to_owned())
        .collect();
        assert_eq!(reached, expected);
    }

    /// The README names, for each command of the application service, its stages in the order
    /// its code takes them, as one list of words written `` `word` `` and joined by ", ", and
    /// names each stage's word.
    #[test]
    fn the_readme_lists_each_commands_stages_in_their_order() {
        let readme = include_str!("../README.md").replace("\r\n", "\n");
        let flat = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        for (command, path) in paths() {
            let list = path
                .iter()
                .map(|stage| format!("`{}`", word(*stage)))
                .collect::<Vec<_>>()
                .join(", ");
            let command = serde_json::to_value(command).expect("a command serializes");
            let sentence = format!(
                "`{}` goes through {list}.",
                command.as_str().expect("a command is a word")
            );
            assert!(flat.contains(&sentence), "the README lacks: {sentence}");
        }
    }
}
