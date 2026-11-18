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

    /// The stages before the application service answers anything, which the three commands
    /// share, in the order the code takes them.
    const OPENING: [Stage; 5] = [
        Stage::Session,
        Stage::ProxyTunnel,
        Stage::DiscoveryHandshake,
        Stage::ServiceConnection,
        Stage::ServiceXpcHandshake,
    ];

    /// The stages of each command of the application service, in the order its code takes
    /// them, the longest way through (a bundle installed and, for `stop`, running).
    fn paths() -> [(CommandName, Vec<Stage>); 3] {
        let after = |rest: &[Stage]| {
            let mut path = OPENING.to_vec();
            path.extend_from_slice(rest);
            path
        };
        [
            (
                CommandName::Running,
                after(&[Stage::ApplicationList, Stage::ProcessList]),
            ),
            (
                CommandName::Launch,
                after(&[Stage::ApplicationList, Stage::LaunchRequest]),
            ),
            (
                CommandName::Stop,
                after(&[
                    Stage::ApplicationList,
                    Stage::ProcessList,
                    Stage::KillSignals,
                    Stage::LooksAfterSignals,
                ]),
            ),
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
            r#"{"command":"running","device":3,"problem":"deadline_passed","stage":"discovery_handshake","elapsed_ms":{"session":812,"proxy_tunnel":88}}"#
        );
        assert!(!line.answered());
        let times = steps.end(ms(1_000));
        assert_eq!(steps.current(), None);
        assert_eq!(
            serde_json::to_string(&times).expect("times serialize"),
            r#"{"session":812,"proxy_tunnel":88,"discovery_handshake":99}"#
        );
    }

    /// Each stage of the application service's commands is reached on a command's way, and a
    /// deadline in it names it, with the times of the stages before it on that way and of no
    /// other; every stage word of these commands is reached so, and none of the screenshot's
    /// words that means something else here (`tunnel`, `service_handshake`) is.
    #[test]
    fn each_stage_of_the_application_service_is_reached_and_named_by_its_deadline() {
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
                let named: BTreeSet<String> = value["elapsed_ms"]
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
