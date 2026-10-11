// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan v2.2 §3.6 H-5 G14 (inventory-host key 38; Workflow #382 H-5 G14): when more than
//! 16 frames are due, the frame cleaner (Workflow #375 R5d) cleans them in rounds of at most 16
//! actions, over as many rounds as the sweep needs, and acts on each due frame once a sweep
//! (`contracts/ledger-store.md`, "Frame cleaner"). Between rounds the cleaner re-reads its view.
//! It records nothing, so the unchanged view still lists every frame it already removed or moved
//! as due. The 1 s round budget is wall-clock and no logic test asserts it (coordinator answer
//! CD-25), so every assertion here holds however many frames a round reaches within it.

use super::*;

/// One table row: settled frames of each class, all entered at `T0`, every file written.
struct Case {
    name: &'static str,
    /// Default frames, due 1 day after their entry.
    defaults: usize,
    /// Duplicate frames, due at their entry.
    duplicates: usize,
    /// Error frames, kept in their error folder.
    errors: usize,
    /// Lab frames, kept in their Lab folder.
    labs: usize,
}

const CASES: [Case; 2] = [
    // The boundary just above 16: removals only.
    Case {
        name: "a",
        defaults: 9,
        duplicates: 8,
        errors: 0,
        labs: 0,
    },
    // At least three rounds, and moves count as actions too.
    Case {
        name: "b",
        defaults: 12,
        duplicates: 12,
        errors: 8,
        labs: 8,
    },
];

/// The frames of one case in a fresh state root. Every frame is due at `NOW`.
struct Due {
    root: Root,
    /// Frames due for removal.
    removals: Vec<usize>,
    /// Frames due for a move into their kept folder.
    moves: Vec<usize>,
}

impl Due {
    fn new(case: &Case) -> Self {
        let mut root = Root::new();
        let mut removals = Vec::new();
        let mut moves = Vec::new();
        for _ in 0..case.defaults {
            removals.push(root.frame(FrameRetentionClass::Default, Some(T0 + DAY_MS), None, true));
        }
        for _ in 0..case.duplicates {
            removals.push(root.frame(FrameRetentionClass::Duplicate, Some(T0), None, true));
        }
        for _ in 0..case.errors {
            moves.push(root.frame(FrameRetentionClass::Error, None, Some(ERROR_LEAF), true));
        }
        for _ in 0..case.labs {
            moves.push(root.frame(FrameRetentionClass::Lab, None, Some(LAB_LEAF), true));
        }
        Self {
            root,
            removals,
            moves,
        }
    }

    /// N, the number of due frames.
    fn count(&self) -> usize {
        self.removals.len() + self.moves.len()
    }
}

/// One sweep, driven round by round.
struct Driven {
    /// The first round left the sweep open: `round` returned `None`.
    first_round_open: bool,
    /// Per round, the frames whose file left its object key during that round.
    acted: Vec<usize>,
    report: SweepReport,
    warnings: Vec<FrameRetentionWarning>,
}

/// `maintain` at an unchanged head, without its ledger read: one sweep from the case's view,
/// then rounds, deriving the queued actions again from the full, unchanged view between rounds,
/// until a round returns the sweep's report. Bounded at 2 × N rounds so that a defect fails
/// instead of hanging; not N, because a round whose budget is spent before its first action
/// acts on no frame.
fn drive(retention: &mut FrameRetention, due: &Due, case: &Case) -> Driven {
    let root = &due.root;
    let bound = 2 * due.count();
    retention.begin_sweep(&root.view());
    let mut first_round_open = false;
    let mut acted = Vec::new();
    let mut warnings = Vec::new();
    let mut gone_before = 0;
    loop {
        assert!(
            acted.len() < bound,
            "case {}: the sweep is not complete after {bound} rounds",
            case.name
        );
        let report = retention
            .round(&root.path, &|| false, &mut |warning| {
                warnings.push(warning.clone());
                Ok(())
            })
            .expect("a round never fails on a file");
        let gone = (0..root.frames.len())
            .filter(|index| !root.object(*index).exists())
            .count();
        acted.push(gone - gone_before);
        gone_before = gone;
        if acted.len() == 1 {
            first_round_open = report.is_none();
        }
        if let Some(report) = report {
            return Driven {
                first_round_open,
                acted,
                report,
                warnings,
            };
        }
        retention.rederive(&root.view());
    }
}

/// Model v4.1 §3.1: at most 16 actions a round, a sweep over as many rounds as it needs; with
/// more than 16 due frames the sweep takes several rounds and ends with every due frame acted
/// on.
#[test]
fn gate_more_than_sixteen_due_frames_are_cleaned_in_rounds_of_at_most_sixteen_actions() {
    for case in &CASES {
        let name = case.name;
        let due = Due::new(case);
        let mut retention = cleaner();
        let driven = drive(&mut retention, &due, case);
        assert!(
            driven.first_round_open,
            "case {name}: the sweep is still open after its first round"
        );
        for (round, acted) in driven.acted.iter().enumerate() {
            // The requirement's bound of 16 actions a round.
            assert!(
                *acted <= 16,
                "case {name}: round {} acted on {acted} frames",
                round + 1
            );
        }
        assert!(
            driven.acted.len() >= 2,
            "case {name}: the sweep took {} round(s)",
            driven.acted.len()
        );
        for index in due.removals.iter().chain(&due.moves) {
            assert!(
                !due.root.object(*index).exists(),
                "case {name}: frame {index} has left its object key"
            );
        }
        for index in &due.moves {
            assert!(
                due.root.kept(*index).is_file(),
                "case {name}: frame {index} is at its kept path"
            );
        }
        assert_eq!(
            driven.report.rounds,
            driven.acted.len() as u64,
            "case {name}: the report counts the rounds the sweep took"
        );
        assert!(
            driven.warnings.is_empty(),
            "case {name}: no Warning: {:?}",
            driven.warnings
        );
    }
}

/// Model §2.1 and the v4.3 note: the cleaner acts on each frame once. Re-reading the unchanged
/// view between rounds must not make it act on a frame again: every due frame is removed or
/// moved exactly once over the sweep, none skipped and none repeated.
#[test]
fn gate_a_sweep_over_several_rounds_acts_on_each_due_frame_once() {
    for case in &CASES {
        let name = case.name;
        let due = Due::new(case);
        let mut retention = cleaner();
        let driven = drive(&mut retention, &due, case);
        let report = &driven.report;
        let count = due.count() as u64;
        assert_eq!(report.frames, count, "case {name}: due frames");
        assert_eq!(
            report.deleted,
            due.removals.len() as u64,
            "case {name}: removed frames"
        );
        assert_eq!(
            report.moved,
            due.moves.len() as u64,
            "case {name}: moved frames"
        );
        assert_eq!(
            report.deleted + report.moved,
            count,
            "case {name}: no due frame skipped"
        );
        // A second visit to a frame already removed or moved finds nothing at its object key
        // and would count here.
        assert_eq!(
            (report.absent, report.rescanned_absent),
            (0, 0),
            "case {name}: no due frame repeated"
        );
        assert_eq!(
            (report.busy, report.failed),
            (0, 0),
            "case {name}: nothing busy or failed"
        );
        assert!(
            driven.warnings.is_empty(),
            "case {name}: no Warning: {:?}",
            driven.warnings
        );
        assert!(
            retention.sweep.is_none(),
            "case {name}: the sweep is complete"
        );
    }
}
