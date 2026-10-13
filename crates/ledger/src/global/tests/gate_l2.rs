// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 L-2, G1 (inventory-ledger key 14): the error points of the frame retention
//! view, from real events appended to an I6 formal root (I7 drafts) and read through the
//! writer's `frame_retention_view`. `contracts/ledger-store.md`, "Error points": a
//! non-retention, non-`perf.*` event on a known instance (its own link, or its run's instance)
//! that is a `TerminalCommitted` with outcome `Failure` at any severity, a run-linked event at
//! Warning or higher, or an instance-linked event without a run at Error or higher (frozen
//! model #375 §2.1: a `Cancelled` terminal counts only at Warning or higher; an Error event
//! without an instance keeps nothing). Points depend only on outcome, severity and links, never
//! on code text. Every timestamp is a fixed instant after `T0`.

use super::fixture::FormalRoot;
use super::retention_events::{RetentionEvents, code_of};
use crate::global::{FrameErrorPoint, FrameRetentionSwitches, FrameRetentionView};
use actingcommand_contract::{EventLinksDraft, EventSeverity, TaskOutcome};

/// 2026-01-01 00:00:00 UTC.
const T0: u64 = 1_767_225_600_000;
const DAY_MS: u64 = 86_400_000;
/// The view is read two days after `T0`, with the switches' defaults.
const NOW: u64 = T0 + 2 * DAY_MS;
const SWITCHES: FrameRetentionSwitches = FrameRetentionSwitches {
    dedup_error: true,
    dedup_lab: false,
};
/// Two registered failure codes (ledger revision 3: a terminal's failure is an outcome of a
/// registered code; an unregistered spelling would name no leaf of its own).
const FAILURE_FIRST: &str = "capture_failed";
const FAILURE_RENAMED: &str = "recognition_failed";

fn view(root: &FormalRoot) -> FrameRetentionView {
    root.ledger()
        .frame_retention_view(NOW, SWITCHES)
        .expect("frame retention view")
}

/// The view's error points at `sequence`.
fn points_at(view: &FrameRetentionView, sequence: u64) -> Vec<&FrameErrorPoint> {
    view.error_points
        .iter()
        .filter(|point| point.sequence == sequence)
        .collect()
}

/// The row's event (I7 items 5, 6 and 7).
#[derive(Clone, Copy)]
enum Event {
    Terminal(TaskOutcome),
    RuntimeFailed,
    MonitorDegraded,
}

/// The row event's links. A run's first event, a `task.started`, always carries the instance.
#[derive(Clone, Copy)]
enum Links {
    InstanceAndRun,
    RunOnly,
    InstanceOnly,
}

#[test]
fn gate_error_points_depend_only_on_outcome_severity_and_links() {
    use Event::{MonitorDegraded, RuntimeFailed, Terminal};
    use EventSeverity::{Error, Info, Warning};
    use Links::{InstanceAndRun, InstanceOnly, RunOnly};
    use TaskOutcome::{Cancelled, Failure, Success};
    // (row, event, links, severity, error point). Pairs that differ in one input: outcome 1/2
    // and 1/4; severity 2/3 and 7/8; links 5/7, and 6 (the run's instance stands in for a
    // missing instance link); family 8/10.
    let rows = [
        (1, Terminal(Failure), InstanceAndRun, Info, true),
        (2, Terminal(Cancelled), InstanceAndRun, Info, false),
        (3, Terminal(Cancelled), InstanceAndRun, Warning, true),
        (4, Terminal(Success), InstanceAndRun, Info, false),
        (5, RuntimeFailed, InstanceAndRun, Warning, true),
        (6, RuntimeFailed, RunOnly, Warning, true),
        (7, RuntimeFailed, InstanceOnly, Warning, false),
        (8, RuntimeFailed, InstanceOnly, Error, true),
        (10, MonitorDegraded, InstanceOnly, Error, false),
    ];
    let root = FormalRoot::create();
    let events = RetentionEvents::new(&root);
    let instance = events.instance();
    let mut at = T0;
    let mut appended = Vec::new();
    for (row, event, shape, severity, expected) in rows {
        let (run, links) = match shape {
            InstanceOnly => (None, EventLinksDraft::default().with_instance_id(instance)),
            InstanceAndRun | RunOnly => {
                let run = events.run(instance);
                at += 1_000;
                events.task_started(&run, at);
                let links = match shape {
                    RunOnly => EventLinksDraft::default().with_run_id(run.id),
                    _ => run.links.clone(),
                };
                (Some(*run.id.transport()), links)
            }
        };
        at += 1_000;
        let persisted = match event {
            Terminal(outcome) => {
                // Ledger revision 3: the terminal's severity is its failure's category.
                let failure_code = (outcome != Success).then(|| code_of(severity));
                events.terminal(links, outcome, failure_code, severity, at)
            }
            RuntimeFailed => events.runtime_failed(links, severity, at),
            MonitorDegraded => events.monitor_degraded(links, severity, at),
        };
        appended.push((row, persisted, run, at, expected));
    }

    let view = view(&root);
    for (row, event, run, at, expected) in appended {
        let points = points_at(&view, event.sequence());
        if !expected {
            assert!(points.is_empty(), "row {row}: no error point: {points:?}");
            continue;
        }
        let [point] = points.as_slice() else {
            panic!("row {row}: exactly one error point: {points:?}");
        };
        assert_eq!(point.instance_id, *instance.transport(), "row {row}");
        assert_eq!(point.run_id, run, "row {row}");
        assert!(!point.epoch_end, "row {row}");
        assert_eq!(point.event_type, event.event_type(), "row {row}");
        assert_eq!(point.at_unix_ms, at, "row {row}");
    }
}

#[test]
fn gate_an_event_on_no_instance_keeps_nothing_and_changes_no_frame() {
    let root = FormalRoot::create();
    let events = RetentionEvents::new(&root);
    let instance = events.instance();
    let captured = [
        events.closed_capture(instance, b"first fixture frame", T0 + 1_000),
        events.closed_capture(instance, b"second fixture frame", T0 + 10_000),
    ];
    let before = view(&root);
    for reference in &captured {
        assert!(
            before
                .frames
                .iter()
                .any(|frame| frame.reference.artifact_id == reference.artifact_id),
            "each closed capture is a frame of the view"
        );
    }

    // An Error with no links: no instance, no run (the host's lifecycle failure shape).
    let unlinked = events.runtime_failed(
        EventLinksDraft::default(),
        EventSeverity::Error,
        T0 + 20_000,
    );
    let after = view(&root);

    assert!(points_at(&after, unlinked.sequence()).is_empty());
    assert_eq!(after.frames, before.frames);
    assert_eq!(after.error_points, before.error_points);
    assert_ne!(after.through_sequence, before.through_sequence);
    assert_eq!(
        FrameRetentionView {
            through_sequence: before.through_sequence,
            ..after
        },
        before,
        "only the head differs"
    );
}

#[test]
fn gate_a_renamed_failure_code_changes_only_the_leaf() {
    let root = FormalRoot::create();
    let events = RetentionEvents::new(&root);
    let instance = events.instance();
    let terminals = [(T0, FAILURE_FIRST), (T0 + 10_000, FAILURE_RENAMED)].map(|(at, code)| {
        let run = events.run(instance);
        events.task_started(&run, at + 1_000);
        let terminal = events.terminal(
            run.links,
            TaskOutcome::Failure,
            Some(code),
            EventSeverity::Info,
            at + 2_000,
        );
        (terminal, code)
    });

    let view = view(&root);
    let points = terminals.map(|(terminal, code)| {
        let points = points_at(&view, terminal.sequence());
        let [point] = points.as_slice() else {
            panic!("the {code} terminal is exactly one error point: {points:?}");
        };
        let suffix = format!("-{}-{code}", terminal.sequence());
        assert!(
            point.leaf.ends_with(&suffix),
            "leaf {} ends with {suffix}",
            point.leaf
        );
        FrameErrorPoint::clone(point)
    });
    let [first, renamed] = points;
    assert_eq!(first.event_type, renamed.event_type);
    assert_eq!(first.instance_id, *instance.transport());
    assert_eq!(renamed.instance_id, first.instance_id);
    assert!(!first.epoch_end);
    assert!(!renamed.epoch_end);
}
