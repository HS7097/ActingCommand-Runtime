// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-2 (model-398 v1.1 §4.2, §4.3, §4.6 and §12 rows RD-2-T2, T3, T5, T6, T7,
// T8, T10): the link state machine of the bound handsets and their watch, driven by hand-given
// times and lists; no device, no process, no host. Every expected value comes from the model:
// the state table of §4.2, the restore steps and watch restarts of §4.3 and the keys of §4.6,
// each timing row run under two configurations so the behaviour follows the configuration.

use super::*;

const ALIAS_A: &str = "handset-a";
const ALIAS_B: &str = "handset-b";

fn instance() -> InstanceId {
    *actingcommand_contract::IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

// The defaults of model-398 §4.6, with the settle wait and the offline action given.
fn config(settle_ms: u64, offline_action: OfflineAction) -> DeviceLinkConfig {
    DeviceLinkConfig {
        watch_restart_backoff_ms: vec![1_000, 5_000, 30_000],
        watch_restart_limit: 5,
        watch_restart_window_ms: 600_000,
        settle_ms,
        offline_action,
        restore_retry_ms: vec![5_000, 15_000, 60_000],
    }
}

fn links(states: &[(&str, DeviceLinkState)]) -> BTreeMap<String, DeviceLinkState> {
    states
        .iter()
        .map(|(alias, state)| ((*alias).to_owned(), *state))
        .collect()
}

// A machine of one handset whose first list found it prepared at `device`.
fn linked_machine(handset: InstanceId, config: &DeviceLinkConfig) -> LinkMachine {
    let mut machine = LinkMachine::new([(handset, ALIAS_A.to_owned())]);
    machine.watch_opened();
    let first = machine.on_links(
        0,
        config,
        &links(&[(ALIAS_A, DeviceLinkState::Device)]),
        &BTreeSet::from([handset]),
    );
    assert_eq!(
        first.links,
        [LinkAction::Linked {
            instance_id: handset
        }]
    );
    machine
}

fn prepares(actions: &LinkActions) -> usize {
    actions
        .links
        .iter()
        .filter(|action| matches!(action, LinkAction::Prepare { .. }))
        .count()
}

#[test]
fn rd2_t2_leaving_device_waits_once_per_state_and_holds_the_handset() {
    let config = config(3_000, OfflineAction::Wait);
    let handset = instance();
    let mut machine = linked_machine(handset, &config);
    assert_eq!(machine.hold(handset), None);
    let absent = links(&[(ALIAS_A, DeviceLinkState::Absent)]);
    assert_eq!(
        machine
            .on_links(10, &config, &absent, &BTreeSet::new())
            .links,
        [LinkAction::Waiting {
            instance_id: handset,
            state: DeviceLinkState::Absent
        }]
    );
    assert_eq!(machine.hold(handset), Some(DeviceLinkState::Absent));
    // The same state again records nothing.
    assert!(
        machine
            .on_links(20, &config, &absent, &BTreeSet::new())
            .links
            .is_empty()
    );
    // Another state while away is its own wait.
    assert_eq!(
        machine
            .on_links(
                30,
                &config,
                &links(&[(ALIAS_A, DeviceLinkState::NoPermissions)]),
                &BTreeSet::new()
            )
            .links,
        [LinkAction::Waiting {
            instance_id: handset,
            state: DeviceLinkState::NoPermissions
        }]
    );
    // Another instance is never held.
    assert_eq!(machine.hold(instance()), None);
}

#[test]
fn rd2_t3_the_preparation_is_queued_only_after_the_settle_wait() {
    for settle_ms in [0, 3_000] {
        let config = config(settle_ms, OfflineAction::Wait);
        let handset = instance();
        let mut machine = linked_machine(handset, &config);
        machine.on_links(
            100,
            &config,
            &links(&[(ALIAS_A, DeviceLinkState::Absent)]),
            &BTreeSet::new(),
        );
        let back = machine.on_links(
            1_000,
            &config,
            &links(&[(ALIAS_A, DeviceLinkState::Device)]),
            &BTreeSet::from([handset]),
        );
        assert_eq!(
            back.links,
            [LinkAction::LinkState {
                instance_id: handset,
                state: DeviceLinkState::Device
            }],
            "settle {settle_ms}"
        );
        if settle_ms > 0 {
            assert_eq!(
                prepares(&machine.on_tick(1_000 + settle_ms - 1, &config)),
                0,
                "settle {settle_ms}: not before the wait"
            );
        }
        assert_eq!(
            machine.on_tick(1_000 + settle_ms, &config).links,
            [LinkAction::Prepare {
                instance_id: handset
            }],
            "settle {settle_ms}"
        );
        // Queued once; the preparation passed: restored, nothing holds the handset.
        assert_eq!(prepares(&machine.on_tick(1_000_000, &config)), 0);
        assert_eq!(
            machine.on_prepared(1_000_001, &config, handset, true).links,
            [LinkAction::Restored {
                instance_id: handset
            }]
        );
        assert_eq!(machine.hold(handset), None);
    }
}

#[test]
fn rd2_t5_offline_sends_one_reconnect_per_episode_only_under_kick_once() {
    for (action, expected) in [(OfflineAction::KickOnce, 2), (OfflineAction::Wait, 0)] {
        let config = config(3_000, action);
        let handset = instance();
        let mut machine = linked_machine(handset, &config);
        let offline = links(&[(ALIAS_A, DeviceLinkState::Offline)]);
        let device = links(&[(ALIAS_A, DeviceLinkState::Device)]);
        let mut reconnects = 0;
        for (at, list) in [
            (10, &offline),
            (20, &offline),
            (30, &device),
            (40, &offline),
            (50, &offline),
        ] {
            reconnects += machine
                .on_links(at, &config, list, &BTreeSet::new())
                .links
                .iter()
                .filter(|action| matches!(action, LinkAction::Reconnect { .. }))
                .count();
        }
        assert_eq!(reconnects, expected, "{action:?}");
    }
}

#[test]
fn rd2_t6_unauthorized_only_waits() {
    let config = config(3_000, OfflineAction::KickOnce);
    let handset = instance();
    let mut machine = linked_machine(handset, &config);
    let actions = machine.on_links(
        10,
        &config,
        &links(&[(ALIAS_A, DeviceLinkState::Unauthorized)]),
        &BTreeSet::new(),
    );
    assert_eq!(
        actions.links,
        [LinkAction::Waiting {
            instance_id: handset,
            state: DeviceLinkState::Unauthorized
        }]
    );
    assert!(machine.on_tick(10_000_000, &config).links.is_empty());
    assert_eq!(machine.hold(handset), Some(DeviceLinkState::Unauthorized));
}

#[test]
fn rd2_t7_a_link_that_flaps_faster_than_the_settle_wait_is_prepared_once() {
    for settle_ms in [500, 3_000] {
        let config = config(settle_ms, OfflineAction::Wait);
        let handset = instance();
        let mut machine = linked_machine(handset, &config);
        let mut queued = 0;
        let mut at = 1_000;
        for state in [
            DeviceLinkState::Absent,
            DeviceLinkState::Device,
            DeviceLinkState::Offline,
            DeviceLinkState::Device,
            DeviceLinkState::Absent,
            DeviceLinkState::Device,
        ] {
            machine.on_links(at, &config, &links(&[(ALIAS_A, state)]), &BTreeSet::new());
            at += settle_ms / 2;
            queued += prepares(&machine.on_tick(at, &config));
        }
        assert_eq!(queued, 0, "settle {settle_ms}: nothing while it flaps");
        // Stable at `device` since the last list: one preparation once the wait has passed.
        let stable_since = at - settle_ms / 2;
        queued += prepares(&machine.on_tick(stable_since + settle_ms, &config));
        queued += prepares(&machine.on_tick(stable_since + 10 * settle_ms, &config));
        assert_eq!(queued, 1, "settle {settle_ms}");
    }
}

#[test]
fn rd2_t8_the_watch_restarts_by_its_backoff_until_the_window_s_limit() {
    for limit in [2, 3] {
        let mut config = config(3_000, OfflineAction::Wait);
        config.watch_restart_limit = limit;
        let handset_a = instance();
        let handset_b = instance();
        let mut machine = LinkMachine::new([
            (handset_a, ALIAS_A.to_owned()),
            (handset_b, ALIAS_B.to_owned()),
        ]);
        machine.watch_opened();
        let mut now = 0;
        for restart in 0..limit {
            let ended = machine.on_watch_ended(now, &config, DeviceWatchProblem::Exited);
            assert_eq!(
                ended,
                LinkActions::default(),
                "limit {limit} restart {restart}"
            );
            let wait = config.restart_backoff_ms(usize::try_from(restart).expect("index"));
            assert_eq!(
                machine.on_tick(now + wait - 1, &config).watch,
                None,
                "limit {limit}: not before its wait"
            );
            now += wait;
            assert_eq!(
                machine.on_tick(now, &config).watch,
                Some(WatchAction::Restart {
                    problem: DeviceWatchProblem::Exited
                }),
                "limit {limit} restart {restart}"
            );
        }
        // One end more within the window than the limit: failed, every handset unwatched.
        let failed = machine.on_watch_ended(now, &config, DeviceWatchProblem::Exited);
        assert_eq!(
            failed.watch,
            Some(WatchAction::Failed {
                problem: DeviceWatchProblem::RestartBudgetSpent
            }),
            "limit {limit}"
        );
        assert_eq!(
            failed.links,
            [
                LinkAction::Waiting {
                    instance_id: handset_a.min(handset_b),
                    state: DeviceLinkState::Unwatched
                },
                LinkAction::Waiting {
                    instance_id: handset_a.max(handset_b),
                    state: DeviceLinkState::Unwatched
                },
            ],
            "limit {limit}"
        );
        assert_eq!(machine.hold(handset_a), Some(DeviceLinkState::Unwatched));
        assert_eq!(machine.on_tick(now + 3_600_000, &config).watch, None);
        // A person's self-check starts it again at the next tick.
        machine.rewatch(now + 10);
        assert_eq!(
            machine.on_tick(now + 10, &config).watch,
            Some(WatchAction::Restart {
                problem: DeviceWatchProblem::RestartBudgetSpent
            })
        );
        assert_eq!(machine.hold(handset_a), None);
    }
    // An output of unknown shape fails the watch at once.
    let config = config(3_000, OfflineAction::Wait);
    let handset = instance();
    let mut machine = LinkMachine::new([(handset, ALIAS_A.to_owned())]);
    machine.watch_opened();
    assert_eq!(
        machine
            .on_watch_ended(0, &config, DeviceWatchProblem::ProtocolInvalid)
            .watch,
        Some(WatchAction::Failed {
            problem: DeviceWatchProblem::ProtocolInvalid
        })
    );
}

#[test]
fn rd2_t10_a_failed_preparation_is_tried_again_after_each_retry_wait_until_they_are_used() {
    for retries in [vec![5_000, 15_000, 60_000], vec![1_000]] {
        let mut config = config(0, OfflineAction::Wait);
        config.restore_retry_ms.clone_from(&retries);
        let handset = instance();
        let mut machine = linked_machine(handset, &config);
        machine.on_links(
            10,
            &config,
            &links(&[(ALIAS_A, DeviceLinkState::Absent)]),
            &BTreeSet::new(),
        );
        machine.on_links(
            20,
            &config,
            &links(&[(ALIAS_A, DeviceLinkState::Device)]),
            &BTreeSet::new(),
        );
        let mut now = 20;
        assert_eq!(prepares(&machine.on_tick(now, &config)), 1);
        for wait in &retries {
            assert!(
                machine
                    .on_prepared(now, &config, handset, false)
                    .links
                    .is_empty()
            );
            assert_eq!(prepares(&machine.on_tick(now + wait - 1, &config)), 0);
            now += wait;
            assert_eq!(prepares(&machine.on_tick(now, &config)), 1, "{retries:?}");
        }
        // The waits are used up: the Runtime waits for the link or a person.
        assert!(
            machine
                .on_prepared(now, &config, handset, false)
                .links
                .is_empty()
        );
        assert_eq!(prepares(&machine.on_tick(now + 3_600_000, &config)), 0);
        assert_eq!(machine.hold(handset), None);
    }
}
