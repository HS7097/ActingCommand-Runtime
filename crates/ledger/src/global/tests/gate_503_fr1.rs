// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #503 FR-1 (the ledger's half; register §1.527): the frame cleaner's statements on a
// real formal root, with the case fixture of `gate_ln7.rs`.
// - A run of an owner epoch that ended holds none of its frames and none of its instance's: its
//   frames are settled at the epoch's end; an open run of the current epoch holds its own frames
//   and the frames of its instance captured after it, and statement W names it, with its
//   instance and its first frame time, and names no run of the ended epoch.
// - Statement L answers each frame's own links: its frame, instance, run and lease.
// - Statement C answers only what was recorded after its head: the runs a terminal or a capture
//   summary names, the frames whose capture completed, the frames and runs a read confirmation
//   names, and a start; after the latest head it answers nothing, and its steps after the head
//   do not grow when the history before the head does.

use super::gate_ln7::{Corpus, DAY_MS, at, query_decisions};
use super::missing_material::{formal_root, material_store};
use crate::global::{FrameChanges, FrameQueryAction, FrameQueryOutcome, FrameRetentionClass};
use actingcommand_contract::{
    AuditInput, ClientActionKind, ClientActionRecord, ClientActionValue, ClientPayloadDraft,
    CommandPayloadDraft, EVIDENCE_READ_FRAME_CONTROL, EVIDENCE_READ_RUN_CONTROL, EventAction,
    EventActor, EventOrigin, EventSeverity, EventSource, OriginModule,
};
use std::collections::BTreeSet;
use std::sync::Arc;

fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

/// A read confirmation (`client.action`, control `evidence_read_frame` or `evidence_read_run`)
/// recorded at `at_ms` from the command line.
fn confirm(corpus: &Corpus<'_>, control: &str, value: String, at_ms: u64) {
    let record = ClientActionRecord::new(
        "cli",
        control,
        ClientActionKind::Command,
        None,
        Some(ClientActionValue::PathSafeString(value)),
    )
    .expect("a read confirmation");
    corpus.append(
        at_ms,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Governance, EventActor::Cli),
        corpus.scoped(),
        ClientPayloadDraft::action(record, AuditInput::new()).into(),
    );
}

/// A plain record of the instance (`command.received` from the command line), no frame.
fn plain(corpus: &Corpus<'_>, at_ms: u64) {
    corpus.append(
        at_ms,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        corpus.scoped(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    );
}

/// Statement C after `after`: its answer and its steps.
fn changes_after(ledger: &crate::GlobalLedger, after: u64) -> (FrameChanges, u64) {
    let mut query = ledger
        .frame_query(Arc::new(|| false))
        .expect("the writer's frame query");
    match query.changes(after).expect("statement C") {
        FrameQueryOutcome::Answered { value, steps } => (value, steps),
        other => panic!("statement C: {other:?}"),
    }
}

#[test]
fn fr1_a_run_open_at_its_epochs_end_holds_nothing_an_open_run_of_this_epoch_holds() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    // A run of the first epoch with two frames and no terminal; a start ends its epoch.
    let ended = corpus.run(at(100));
    let ended_frames = [
        corpus.run_frame(&ended, at(101)),
        corpus.run_frame(&ended, at(103)),
    ];
    corpus.start(at(200));
    // A run-less frame of the current epoch, captured before the open run below.
    let before = corpus.observe(at(300));
    // A run of the current epoch with a frame and no terminal, and a run-less frame after it.
    let open = corpus.run(at(400));
    let open_frame = corpus.run_frame(&open, at(401));
    let after = corpus.observe(at(500));

    let mut query = ledger
        .frame_query(Arc::new(|| false))
        .expect("the writer's frame query");
    let FrameQueryOutcome::Answered { value: waits, .. } = query.waits().expect("statement W")
    else {
        panic!("statement W answered nothing");
    };
    drop(query);
    let open_runs = waits.open_runs();
    assert_eq!(open_runs.len(), 1, "one open run: {open_runs:?}");
    assert_eq!(open_runs[0].run_id, *open.id.transport());
    assert_eq!(open_runs[0].instance_id, Some(*corpus.instance.transport()));
    assert_eq!(open_runs[0].first_at_unix_ms, at(401));
    assert_eq!(
        waits.head(),
        ledger.latest_sequence().expect("head"),
        "W reads at the ledger's head"
    );

    let live = ended_frames
        .iter()
        .chain([&before, &open_frame, &after])
        .map(|frame| frame.id())
        .collect::<BTreeSet<_>>();
    let now = at(500) + 2 * DAY_MS;
    let decided = query_decisions(&ledger, &live, now);
    for frame in &ended_frames {
        let (answer, _) = decided
            .get(&frame.id())
            .expect("a frame of the ended epoch is answered");
        assert_ne!(
            answer.class,
            FrameRetentionClass::Running,
            "a run left open by an ended epoch holds none of its frames"
        );
        assert_ne!(answer.action, FrameQueryAction::Running);
        assert_eq!(answer.run_id, Some(*ended.id.transport()));
    }
    let (answer, _) = decided.get(&before.id()).expect("the earlier frame");
    assert_ne!(
        answer.class,
        FrameRetentionClass::Running,
        "a frame captured more than 30 s before the open run's first frame is not held"
    );
    for (frame, run) in [(&open_frame, Some(*open.id.transport())), (&after, None)] {
        let (answer, _) = decided
            .get(&frame.id())
            .expect("a frame of the open run's instance");
        assert_eq!(
            (answer.class, answer.action),
            (FrameRetentionClass::Running, FrameQueryAction::Running),
            "an open run of the current epoch holds its instance's frames"
        );
        assert_eq!(answer.run_id, run);
        assert_eq!(answer.instance_id, Some(*corpus.instance.transport()));
        assert_eq!(answer.frame_id, frame.reference.frame_id);
        assert!(waits.holds(answer.instance_id.as_ref(), answer.created_at_unix_ms));
    }
    ledger.close().expect("close the writer");
}

#[test]
fn fr1_statement_c_answers_only_what_was_recorded_after_its_head() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    let early = corpus.run(at(10));
    let early_frame = corpus.run_frame(&early, at(11));
    corpus.close(&early.links, &[&early_frame], at(12));
    let early_observe = corpus.observe(at(13));
    let head = ledger.latest_sequence().expect("head");
    let (nothing_yet, steps_after_a_short_history) = changes_after(&ledger, head);
    assert_eq!(nothing_yet, FrameChanges::default());

    // A longer history before a later head, with nothing C reads: the same steps after it.
    for second in 0..2_000 {
        plain(&corpus, at(20) + second);
    }
    let later_head = ledger.latest_sequence().expect("head");
    let (nothing_still, steps_after_a_long_history) = changes_after(&ledger, later_head);
    assert_eq!(nothing_still, FrameChanges::default());
    assert_eq!(
        steps_after_a_long_history, steps_after_a_short_history,
        "statement C's steps after its head do not grow with the history before it"
    );

    // After the head: a run closed by its summary and its terminal, a run-less capture, read
    // confirmations of a frame and of a run recorded before the head, and a start.
    let later = corpus.run(at(100));
    let later_frame = corpus.run_frame(&later, at(101));
    corpus.close(&later.links, &[&later_frame], at(102));
    let observed = corpus.observe(at(103));
    confirm(
        &corpus,
        EVIDENCE_READ_FRAME_CONTROL,
        text(&early_frame.id()),
        at(104),
    );
    confirm(
        &corpus,
        EVIDENCE_READ_RUN_CONTROL,
        text(early.id.transport()),
        at(105),
    );
    corpus.start(at(106));
    let (changes, _) = changes_after(&ledger, later_head);
    assert_eq!(
        changes,
        FrameChanges {
            started: true,
            runs: BTreeSet::from([*later.id.transport()]),
            leases: BTreeSet::new(),
            frames: BTreeSet::from([observed.reference.frame_id.expect("a frame id")]),
            confirmed_artifacts: BTreeSet::from([early_frame.id()]),
            confirmed_runs: BTreeSet::from([*early.id.transport()]),
        },
        "only what was recorded after the head"
    );
    assert!(
        !changes
            .frames
            .contains(&early_observe.reference.frame_id.expect("a frame id")),
        "a capture completed before the head is not named"
    );
    // After the latest head: nothing.
    let latest = ledger.latest_sequence().expect("head");
    assert_eq!(changes_after(&ledger, latest).0, FrameChanges::default());
    ledger.close().expect("close the writer");
}
