// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 A-1 (test plan v2.2 §3.7), in-process.
// G10 (apps 13): the documented forms of pause / resume (contracts/scheduling-pause.md, "CLI"),
// emulator (contracts/emulator-control.md: status/start/stop/restart with --instance, discover
// without), selfcheck (README.md, "actingctl selfcheck <alias>") and install-transition
// (distribution/windows/INSTALL.md, "Host installation control": the five actions, deadlines
// defaulting to 60 s and bounded at 600 s, `--wait` on commit_shutdown), and the shape of the
// README's monitor-set example line, parse to their commands. Their usage errors, reported before
// any connection, are tests/gate/cli_grammar.rs.
// G11 (apps 18): `request-shutdown --wait` with an injected poller and clock (Workflow
// 191-batch4-models.md s1): every 250 ms the wait reads the owner journal; it is done when the
// target owner epoch's last record is inactive and the target process is not alive, giving
// `{state, closed_at_unix_ms, elapsed_ms}`; past its time it is `shutdown_wait_timeout` with the
// last state it saw; another epoch's close never counts.

use crate::shutdown_wait::{Poller, wait_with};
use crate::{ActingctlError, Command, Invocation, pause_scope};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EmulatorInstanceAction, IdentifierIssuer, InstallTransitionAction, InstallTransitionTicket,
    OWNER_JOURNAL_SCHEMA, OwnerEpoch, RuntimeShutdownTarget, SchedulingPauseScope,
};
use actingcommand_ledger::owner_journal::RuntimeOwnerRecord;
use serde_json::json;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const ALIAS: &str = "neutral.instance";
const TRANSITION_ID: &str = "transition-grammar";
/// The s1 model's poll interval.
const POLL: Duration = Duration::from_millis(250);

/// What a documented invocation parses to.
enum Parsed {
    /// The pause scope, reason and drain timeout.
    Pause(SchedulingPauseScope, &'static str, u64),
    Resume(SchedulingPauseScope),
    /// `emulator status` of `ALIAS`.
    EmulatorStatus,
    /// `emulator start|stop|restart` of `ALIAS`.
    EmulatorControl(EmulatorInstanceAction),
    EmulatorDiscover,
    SelfCheck(&'static str),
    /// The action and the `--wait` after it.
    InstallTransition(InstallTransitionAction, Option<Duration>),
    /// The monitor policy of `ALIAS`: interval, expected page, recovery.
    MonitorSet(u64, &'static str, bool),
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn parsed_as(invocation: Invocation, expected: &Parsed) -> bool {
    let Invocation {
        instance,
        shutdown_wait,
        command,
        ..
    } = invocation;
    match (command, expected) {
        (
            Command::Pause {
                pause_reason,
                drain_timeout_ms,
            },
            Parsed::Pause(scope, reason, timeout_ms),
        ) => {
            pause_scope(instance) == *scope
                && pause_reason.as_str() == *reason
                && drain_timeout_ms == *timeout_ms
        }
        (Command::Resume, Parsed::Resume(scope)) => pause_scope(instance) == *scope,
        (Command::EmulatorStatus, Parsed::EmulatorStatus) => instance.as_deref() == Some(ALIAS),
        (Command::EmulatorControl { action }, Parsed::EmulatorControl(expected)) => {
            action == *expected && instance.as_deref() == Some(ALIAS)
        }
        (Command::EmulatorDiscover, Parsed::EmulatorDiscover) => instance.is_none(),
        (Command::SelfCheck { instance_alias }, Parsed::SelfCheck(alias)) => {
            instance_alias == *alias && instance.is_none()
        }
        (Command::InstallTransition { action }, Parsed::InstallTransition(expected, wait)) => {
            action == *expected && shutdown_wait == *wait
        }
        (Command::MonitorSet { policy }, Parsed::MonitorSet(interval_ms, page, recovery)) => {
            instance.as_deref() == Some(ALIAS)
                && policy.interval_ms() == *interval_ms
                && policy.expected_page() == *page
                && policy.recovery_enabled() == *recovery
        }
        _ => false,
    }
}

/// G10: each documented form, as its contract or the README writes it, parses to its command.
#[test]
fn gate_the_documented_forms_parse_to_their_commands() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let ticket = InstallTransitionTicket {
        target: RuntimeShutdownTarget {
            owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
            pid: 4_242,
            started_at_unix_ms: 1_800_000_000_000,
        },
        transition_id: TRANSITION_ID.to_owned(),
        request_id: *issuer.mint_request_id().expect("request id").transport(),
    };
    let action_json =
        |action: &InstallTransitionAction| serde_json::to_string(action).expect("action JSON");
    let begin_drain = |timeout_ms| InstallTransitionAction::BeginDrain {
        transition_id: TRANSITION_ID.to_owned(),
        timeout_ms,
    };
    let query = InstallTransitionAction::Query {
        transition_id: TRANSITION_ID.to_owned(),
    };
    let abort = InstallTransitionAction::Abort {
        ticket: ticket.clone(),
    };
    let release = InstallTransitionAction::Release {
        ticket: ticket.clone(),
        timeout_ms: 600_000,
    };
    let commit = InstallTransitionAction::CommitShutdown { ticket };
    let at_instance = || SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    };
    let install = |json: String, wait: Option<&str>| {
        let mut arguments = words(&[
            "install-transition",
            "--state-root",
            "state",
            "--action-json",
        ]);
        arguments.push(json);
        if let Some(seconds) = wait {
            arguments.extend(words(&["--wait", seconds]));
        }
        arguments
    };
    let rows: Vec<(Vec<String>, Parsed)> = vec![
        (
            words(&["pause", "--state-root", "state"]),
            Parsed::Pause(SchedulingPauseScope::Global, "operator", 60_000),
        ),
        (
            words(&[
                "pause",
                "--state-root",
                "state",
                "--instance",
                ALIAS,
                "--reason",
                "maintenance",
                "--drain-timeout-ms",
                "1000",
            ]),
            Parsed::Pause(at_instance(), "maintenance", 1_000),
        ),
        (
            words(&[
                "pause",
                "--state-root",
                "state",
                "--drain-timeout-ms",
                "600000",
            ]),
            Parsed::Pause(SchedulingPauseScope::Global, "operator", 600_000),
        ),
        (
            words(&["resume", "--state-root", "state"]),
            Parsed::Resume(SchedulingPauseScope::Global),
        ),
        (
            words(&["resume", "--state-root", "state", "--instance", ALIAS]),
            Parsed::Resume(at_instance()),
        ),
        (
            words(&[
                "emulator",
                "status",
                "--state-root",
                "state",
                "--instance",
                ALIAS,
            ]),
            Parsed::EmulatorStatus,
        ),
        (
            words(&[
                "emulator",
                "start",
                "--state-root",
                "state",
                "--instance",
                ALIAS,
            ]),
            Parsed::EmulatorControl(EmulatorInstanceAction::Start),
        ),
        (
            words(&[
                "emulator",
                "stop",
                "--state-root",
                "state",
                "--instance",
                ALIAS,
            ]),
            Parsed::EmulatorControl(EmulatorInstanceAction::Stop),
        ),
        (
            words(&[
                "emulator",
                "restart",
                "--state-root",
                "state",
                "--instance",
                ALIAS,
            ]),
            Parsed::EmulatorControl(EmulatorInstanceAction::Restart),
        ),
        (
            words(&["emulator", "discover", "--state-root", "state"]),
            Parsed::EmulatorDiscover,
        ),
        (
            words(&["selfcheck", ALIAS, "--state-root", "state"]),
            Parsed::SelfCheck(ALIAS),
        ),
        // begin_drain without timeout_ms: the 60 s default.
        (
            install(
                format!(r#"{{"action":"begin_drain","transition_id":"{TRANSITION_ID}"}}"#),
                None,
            ),
            Parsed::InstallTransition(begin_drain(60_000), None),
        ),
        (
            install(action_json(&begin_drain(600_000)), None),
            Parsed::InstallTransition(begin_drain(600_000), None),
        ),
        (
            install(action_json(&query), None),
            Parsed::InstallTransition(query, None),
        ),
        (
            install(action_json(&abort), None),
            Parsed::InstallTransition(abort, None),
        ),
        (
            install(action_json(&release), None),
            Parsed::InstallTransition(release, None),
        ),
        (
            install(action_json(&commit), Some("60")),
            Parsed::InstallTransition(commit, Some(Duration::from_secs(60))),
        ),
        // README.md, the monitor-set example line.
        (
            words(&[
                "monitor-set",
                "--state-root",
                "state",
                "--instance",
                ALIAS,
                "--interval-ms",
                "30000",
                "--expect",
                "home",
                "--recover",
            ]),
            Parsed::MonitorSet(30_000, "home", true),
        ),
    ];
    for (arguments, expected) in rows {
        let invocation = Invocation::parse(arguments.iter().map(OsString::from).collect())
            .unwrap_or_else(|error| panic!("{arguments:?} is refused: {error}"));
        assert!(
            parsed_as(invocation, &expected),
            "{arguments:?} does not parse to its documented command"
        );
    }
}

/// One owner guard record of the journal (`contracts/owner-journal.md`): active, or closed at
/// `closed_at_unix_ms`.
fn record(
    revision: u64,
    owner_epoch: OwnerEpoch,
    pid: u32,
    closed_at_unix_ms: Option<u64>,
) -> RuntimeOwnerRecord {
    RuntimeOwnerRecord {
        schema_version: OWNER_JOURNAL_SCHEMA.to_owned(),
        revision,
        owner_epoch,
        pid,
        started_at_unix_ms: 1_800_000_000_000,
        active: closed_at_unix_ms.is_none(),
        active_instances: Vec::new(),
        closed_at_unix_ms,
        resource_disposition: None,
        closed_by: None,
    }
}

/// Appends `records` to the owner journal as the owner guard does, one JSON line each.
fn append(journal: &Path, records: &[RuntimeOwnerRecord]) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal)
        .expect("open the owner journal");
    for record in records {
        let line = serde_json::to_string(record).expect("record JSON");
        writeln!(file, "{line}").expect("append the record");
    }
}

/// The injected poller and clock: virtual time moves only by the waits the loop asks for; at
/// each wait the journal gets the records the Runtime appended meanwhile; the liveness answers
/// come in order and the last one repeats.
struct ScriptedPoller {
    journal: PathBuf,
    elapsed: Duration,
    waits: Vec<Duration>,
    appended: VecDeque<Vec<RuntimeOwnerRecord>>,
    alive: VecDeque<bool>,
    probed: Vec<u32>,
}

impl ScriptedPoller {
    fn new(journal: &Path, appended: Vec<Vec<RuntimeOwnerRecord>>, alive: &[bool]) -> Self {
        Self {
            journal: journal.to_path_buf(),
            elapsed: Duration::ZERO,
            waits: Vec::new(),
            appended: appended.into(),
            alive: alive.iter().copied().collect(),
            probed: Vec::new(),
        }
    }
}

impl Poller for ScriptedPoller {
    fn elapsed(&self) -> Duration {
        self.elapsed
    }

    fn wait(&mut self, duration: Duration) {
        self.waits.push(duration);
        self.elapsed += duration;
        if let Some(records) = self.appended.pop_front() {
            append(&self.journal, &records);
        }
    }

    fn pid_alive(&mut self, pid: u32) -> Result<bool, Outcome> {
        self.probed.push(pid);
        let alive = if self.alive.len() > 1 {
            self.alive.pop_front()
        } else {
            self.alive.front().copied()
        };
        Ok(alive.expect("a scripted liveness answer"))
    }
}

/// The accepted shutdown's target and the epoch of another owner of the same root.
fn owners() -> (RuntimeShutdownTarget, OwnerEpoch) {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let target = RuntimeShutdownTarget {
        owner_epoch: *issuer.mint_owner_epoch().expect("owner epoch").transport(),
        pid: 4_242,
        started_at_unix_ms: 1_800_000_000_000,
    };
    let other = *issuer.mint_owner_epoch().expect("owner epoch").transport();
    (target, other)
}

/// The timeout a wait ended in; anything else fails the test.
fn timed_out(result: Result<serde_json::Value, ActingctlError>) -> Outcome {
    match result {
        Err(ActingctlError::ShutdownWait(outcome)) => outcome,
        other => panic!("the wait did not time out: {other:?}"),
    }
}

/// G11: the target epoch's record closes at the second wait and its process is still alive at
/// the first probe after it; the wait completes at the next poll with the record's close time.
/// An earlier owner's close, already in the journal, never completes it.
#[test]
fn gate_the_wait_completes_when_the_target_epoch_closed_and_its_process_exited() {
    let root = TempDir::new().expect("tempdir");
    let journal = root.path().join("owner.lock");
    let (target, earlier) = owners();
    append(
        &journal,
        &[
            record(1, earlier, 1_111, None),
            record(2, earlier, 1_111, Some(1_800_000_000_500)),
            record(3, target.owner_epoch, target.pid, None),
        ],
    );
    let mut poller = ScriptedPoller::new(
        &journal,
        vec![
            Vec::new(),
            vec![record(
                4,
                target.owner_epoch,
                target.pid,
                Some(1_800_000_009_000),
            )],
        ],
        &[true, false],
    );

    let done = wait_with(root.path(), &target, Duration::from_secs(60), &mut poller)
        .expect("the wait completes");

    assert_eq!(
        done,
        json!({
            "state": "completed",
            "closed_at_unix_ms": 1_800_000_009_000_u64,
            "elapsed_ms": 750_u64,
        })
    );
    // The process is probed only once the target's record closed, until it is gone.
    assert_eq!(poller.probed, [target.pid, target.pid]);
    assert_eq!(poller.waits, [POLL; 3]);
}

/// G11: the target owner stops without closing its record, and a later owner whose process got
/// the same pid starts and closes meanwhile. That close is another epoch's: the wait runs to its
/// end and times out with what it last saw of the target (record open, process gone).
#[test]
fn gate_another_epochs_close_does_not_end_the_wait_which_times_out_with_the_last_state() {
    let root = TempDir::new().expect("tempdir");
    let journal = root.path().join("owner.lock");
    let (target, later) = owners();
    append(&journal, &[record(1, target.owner_epoch, target.pid, None)]);
    let mut poller = ScriptedPoller::new(
        &journal,
        vec![
            Vec::new(),
            vec![
                record(2, later, target.pid, None),
                record(3, later, target.pid, Some(1_800_000_009_000)),
            ],
        ],
        &[false],
    );
    let wait = Duration::from_secs(1);

    let outcome = timed_out(wait_with(root.path(), &target, wait, &mut poller));

    assert_eq!(outcome.code().as_str(), "shutdown_wait_timeout");
    assert_eq!(
        outcome.value("owner_closed"),
        Some(&json!(false)),
        "{outcome}"
    );
    assert_eq!(
        outcome.value("process_alive"),
        Some(&json!(false)),
        "{outcome}"
    );
    assert_eq!(outcome.value("pid"), Some(&json!(target.pid)), "{outcome}");
    assert_eq!(
        outcome.value("owner_epoch"),
        Some(&serde_json::to_value(target.owner_epoch).expect("epoch JSON")),
        "{outcome}"
    );
    assert_eq!(
        outcome.value("timeout_ms"),
        Some(&json!(1_000_u64)),
        "{outcome}"
    );
    // It polled every 250 ms to its end and probed the process once, for the timeout's state.
    assert_eq!(poller.waits, [POLL; 4]);
    assert_eq!(poller.elapsed, wait);
    assert_eq!(poller.probed, [target.pid]);
}

/// G11: the target epoch's record closes, but its process outlives the wait: not done; the
/// timeout carries the last state (record closed, process alive).
#[test]
fn gate_a_closed_epoch_whose_process_outlives_the_wait_times_out_with_the_last_state() {
    let root = TempDir::new().expect("tempdir");
    let journal = root.path().join("owner.lock");
    let (target, _) = owners();
    append(&journal, &[record(1, target.owner_epoch, target.pid, None)]);
    let mut poller = ScriptedPoller::new(
        &journal,
        vec![vec![record(
            2,
            target.owner_epoch,
            target.pid,
            Some(1_800_000_009_000),
        )]],
        &[true],
    );

    let outcome = timed_out(wait_with(
        root.path(),
        &target,
        Duration::from_millis(500),
        &mut poller,
    ));

    assert_eq!(outcome.code().as_str(), "shutdown_wait_timeout");
    assert_eq!(
        outcome.value("owner_closed"),
        Some(&json!(true)),
        "{outcome}"
    );
    assert_eq!(
        outcome.value("process_alive"),
        Some(&json!(true)),
        "{outcome}"
    );
    assert_eq!(poller.waits, [POLL; 2]);
}
