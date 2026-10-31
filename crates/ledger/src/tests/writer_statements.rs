// SPDX-License-Identifier: AGPL-3.0-only

// FLAKY-LNWS1: the LN-WS gate T1 (`global/tests/gate_ln_ws.rs`) once found a view page's record
// empty (PR #1473's CI, `statements: []`). Between two pages the test reads the head
// (`LatestSequence`), a read command the writer records and ends after its reply; the next page
// starts on a reader of the pool meanwhile. With one record per recorder, the reader's start
// replaced the writer's record and the writer's end then took the reader's, so the page's
// statements were lost. Here two recorded commands on two threads overlap in that order, one step
// at a time on a barrier (no sleep, no clock), and each keeps its own statements.

use super::*;
use std::sync::Barrier;

const WRITER_SQL: &str = "SELECT fixture_writer_statement";
const READER_FIRST_SQL: &str = "SELECT fixture_reader_statement_one";
const READER_SECOND_SQL: &str = "SELECT fixture_reader_statement_two";

fn run(sql: &str, vm_steps: u64, rows: u64) -> LedgerStatementRun {
    LedgerStatementRun {
        sql: sql.to_owned(),
        after: None,
        vm_steps,
        rows,
    }
}

#[test]
fn two_commands_recorded_at_once_on_two_threads_each_keep_their_own_statements() {
    let root = tempfile::tempdir().expect("state root");
    let recorder = WriterStatementRecorder::arm(root.path()).expect("the recorder");
    let canonical = recorder.recorder.root.clone();
    let step = Barrier::new(2);
    // Each thread only waits on the barrier and reports whether its command was recorded; the
    // assertions run after both have joined, so a failure never leaves the other thread waiting.
    let (writer_started, reader_started) = thread::scope(|scope| {
        // The writer's command: it starts and reads, and ends after the reader's command started
        // and read (the writer's `LatestSequence`, ended after its reply).
        let writer = scope.spawn(|| {
            let recording = start(&canonical);
            let started = recording.is_some();
            statement_done(WRITER_SQL, &[], 1, 1);
            step.wait(); // 1: the writer's command runs.
            step.wait(); // 2: the reader's command started and read.
            finish(
                recording,
                LedgerWriterCommandKind::LatestSequence,
                Duration::from_millis(1),
                (0, 0),
            );
            step.wait(); // 3: the writer's command ended.
            started
        });
        // The reader's command (a pool's `ProjectViewPage`): it starts while the writer's runs and
        // reads on after the writer's ended.
        let reader = scope.spawn(|| {
            step.wait(); // 1
            let recording = start(&canonical);
            let started = recording.is_some();
            statement_done(READER_FIRST_SQL, &[], 2, 3);
            step.wait(); // 2
            step.wait(); // 3
            statement_done(READER_SECOND_SQL, &[], 4, 5);
            finish(
                recording,
                LedgerWriterCommandKind::ProjectViewPage,
                Duration::from_millis(2),
                (6, 1),
            );
            started
        });
        (
            writer.join().expect("the writer's thread"),
            reader.join().expect("the reader's thread"),
        )
    });
    assert!(
        writer_started && reader_started,
        "both commands were recorded: the writer's {writer_started}, the reader's \
         {reader_started}"
    );
    assert_eq!(
        recorder.take(),
        vec![
            LedgerWriterCommandStatements {
                command: LedgerWriterCommandKind::LatestSequence,
                processing: Duration::from_millis(1),
                statements: vec![run(WRITER_SQL, 1, 1)],
                bound_callbacks: 0,
                late_callbacks: 0,
            },
            LedgerWriterCommandStatements {
                command: LedgerWriterCommandKind::ProjectViewPage,
                processing: Duration::from_millis(2),
                statements: vec![run(READER_FIRST_SQL, 2, 3), run(READER_SECOND_SQL, 4, 5)],
                bound_callbacks: 6,
                late_callbacks: 1,
            },
        ],
        "each command kept its own statements, in the order they ended"
    );
    drop(recorder);
}
