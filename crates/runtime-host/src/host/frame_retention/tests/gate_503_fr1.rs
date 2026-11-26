// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #503 FR-1 (the cleaner's half; register §1.527): a pass costs what is due. On the fake
//! ledger query of `tests.rs`, whose statements W, C, L and K record what each was asked:
//! - T1: a second pass over an unchanged store runs statement W and one statement C only: no
//!   statement L and no statement K, so it reads no live frame and no kept frame again;
//! - T2: a kept frame is asked about at its maximum age and removed then, or when statement C
//!   names a read confirmation of it, and not before; a kept file a person removed is forgotten at
//!   the next pass start, with no statement about it; a kept file the ledger does not know is
//!   looked at once;
//! - T3: a running frame an open run holds is not asked about while the run stays open, even when
//!   a record closes its own capture, and is asked about at the first pass after the run ended; a
//!   running frame no open run holds is asked about when statement C names a close of its capture;
//!   the line names the open runs, the oldest and its age, and counts the frames left waiting;
//! - T4: a live frame statement L gives no row is asked about at two pass starts ten minutes apart,
//!   then no more by this process.

use super::*;

/// One pass at `now` over the live frames `live` and the root's kept listing, its rounds until it
/// ends (every statement's steps are the fake's, zero here, so all of it runs in its first round),
/// and the line it printed.
fn pass_over(
    retention: &mut FrameRetention,
    root: &Root,
    fake: &mut Fake,
    live: BTreeSet<ArtifactId>,
    now: u64,
) -> Option<SweepReport> {
    let started = retention
        .begin_pass(fake, live, root.kept_listing(), clock(now, 0), SETTINGS)
        .expect("a pass begins");
    assert_eq!(started, PassStart::Started);
    for _ in 0..100 {
        let ended = retention
            .round(
                fake,
                &root.path,
                clock(now, 0),
                SETTINGS,
                &|| false,
                &mut |warning| panic!("the cleaner warns about no frame: {warning:?}"),
            )
            .expect("a round never fails here");
        if retention.pass.is_none() {
            return match ended {
                Some(PassEnd::Completed(report)) => Some(report),
                Some(PassEnd::Skipped) => panic!("the fake covers the ledger"),
                None => None,
            };
        }
    }
    panic!("the pass ends within 100 rounds");
}

/// Writes frame `index`'s file into its kept folder, as an earlier move left it.
fn keep_file(root: &Root, index: usize) {
    let path = root.kept(index);
    std::fs::create_dir_all(path.parent().expect("its leaf")).expect("the kept leaf");
    std::fs::write(&path, b"a kept frame").expect("the kept file");
}

/// A kept file of a frame this ledger has no record of, in the Lab leaf.
fn stranger(root: &Root) -> PathBuf {
    let path = root
        .path
        .join(KEPT_DIRECTORY)
        .join(DATE)
        .join(LAB_LEAF)
        .join("061502-117_artifact_000000000000000000000000000000aa.png");
    std::fs::create_dir_all(path.parent().expect("its leaf")).expect("the kept leaf");
    std::fs::write(&path, b"a frame of another ledger").expect("the stranger");
    path
}

/// The frames the first statement L of the fake was asked about.
fn first_asked(fake: &Fake) -> BTreeSet<ArtifactId> {
    fake.asked_live
        .first()
        .map(|(_, frames)| frames.iter().copied().collect())
        .unwrap_or_default()
}

#[test]
fn fr1_t1_a_second_pass_over_an_unchanged_store_reads_no_frame_again() {
    let mut root = Root::new();
    // Twenty minutes before the pass: a default frame and a reading, kept by their age; a frame
    // whose own run no record closed; a Lab frame already in `kept\`. An open run's first frame
    // five minutes before the pass, and a frame it holds, captured 10 s before that.
    let early = NOW - 20 * 60_000;
    let first_at = NOW - 5 * 60_000;
    let ordinary = root.frame_at(
        FrameRetentionClass::Default,
        early,
        Some(early + DAY_MS),
        None,
        true,
    );
    let reading = root.frame_at(
        FrameRetentionClass::Resource,
        early,
        Some(early + 7 * DAY_MS),
        None,
        true,
    );
    let open = root.frame_at(FrameRetentionClass::Running, early, None, None, true);
    let held_at = first_at - 10_000;
    let held = root.frame_at(
        FrameRetentionClass::Default,
        held_at,
        Some(held_at + DAY_MS),
        None,
        true,
    );
    let kept = root.frame_at(FrameRetentionClass::Lab, early, None, Some(LAB_LEAF), false);
    keep_file(&root, kept);
    let unknown = stranger(&root);
    let live = [ordinary, reading, open, held]
        .map(|index| root.id(index))
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut fake = root.answers();
    fake.open_runs.push(first_at);
    fake.head = 41;
    let mut retention = cleaner();

    let first = pass_over(&mut retention, &root, &mut fake, live.clone(), NOW)
        .expect("the first pass prints its line");
    assert_eq!(
        (first.running, first.kept_examined, first.kept_unknown),
        (2, 2, 1),
        "{first:?}"
    );
    assert_eq!(
        first_asked(&fake),
        live,
        "the first pass asks about every live frame"
    );
    assert!(
        fake.asked_changes.is_empty(),
        "a process's first pass has no head to read after"
    );

    // Ten minutes later, over the same store and the same ledger.
    fake.asked_live.clear();
    fake.asked_kept.clear();
    let second = pass_over(&mut retention, &root, &mut fake, live, NOW + 10 * 60_000);
    assert_eq!(
        second, None,
        "a pass after the first that queues nothing prints nothing"
    );
    assert!(
        fake.asked_live.is_empty(),
        "no statement L: no live frame is read again"
    );
    assert!(
        fake.asked_kept.is_empty(),
        "no statement K: no kept frame is read again"
    );
    assert_eq!(
        fake.asked_changes,
        vec![41_u64],
        "one statement C, after the first pass's head"
    );
    for index in [ordinary, reading, open, held] {
        assert!(root.object(index).is_file(), "frame {index} stays");
    }
    assert!(root.kept(kept).is_file());
    assert!(unknown.is_file());
}

#[test]
fn fr1_t2_a_kept_frame_is_asked_about_at_its_maximum_age_or_its_confirmation_an_unknown_once() {
    let mut root = Root::new();
    let aging = root.frame_at(FrameRetentionClass::Lab, T0, None, Some(LAB_LEAF), false);
    let confirmed = root.frame_at(
        FrameRetentionClass::Error,
        T0,
        None,
        Some(ERROR_LEAF),
        false,
    );
    let removed = root.frame_at(FrameRetentionClass::Lab, T0, None, Some(LAB_LEAF), false);
    for index in [aging, confirmed, removed] {
        keep_file(&root, index);
    }
    let unknown = stranger(&root);
    let max_age_ms = SETTINGS.evidence_max_age_days * DAY_MS;
    let mut fake = root.answers();
    let mut retention = cleaner();

    // The first pass looks at every kept file once.
    let first = pass_over(&mut retention, &root, &mut fake, BTreeSet::new(), NOW)
        .expect("the first pass prints its line");
    assert_eq!(
        (first.kept_examined, first.kept_unknown, first.deleted),
        (4, 1, 0),
        "{first:?}"
    );

    // A person removes one kept file; a millisecond before the maximum age no kept frame is asked
    // about, and the removed one is forgotten at the pass start.
    std::fs::remove_file(root.kept(removed)).expect("removed by hand");
    fake.asked_kept.clear();
    let before = T0 + max_age_ms - 1;
    assert_eq!(
        pass_over(&mut retention, &root, &mut fake, BTreeSet::new(), before),
        None
    );
    assert!(
        fake.asked_kept.is_empty(),
        "unconfirmed and younger than its maximum age, no kept frame is asked about"
    );
    assert!(
        !retention.kept_known.contains_key(&root.id(removed)),
        "a kept file removed by hand is forgotten at the next pass start"
    );

    // A read confirmation of one kept frame, which statement C names: that frame alone is asked
    // about, and removed.
    fake.frames
        .get_mut(&root.id(confirmed))
        .expect("the confirmed frame")
        .confirmed = true;
    fake.changes.confirmed_artifacts.insert(root.id(confirmed));
    let report = pass_over(&mut retention, &root, &mut fake, BTreeSet::new(), before)
        .expect("a pass that removes prints its line");
    assert_eq!(fake.asked_kept.first(), Some(&vec![root.id(confirmed)]));
    assert_eq!(
        (
            report.removed_confirmed,
            report.kept_examined,
            report.kept_unknown
        ),
        (1, 1, 0)
    );
    assert!(!root.kept(confirmed).exists());

    // At its maximum age the other kept frame is asked about and removed; the stranger never
    // again.
    fake.changes = FrameChanges::default();
    fake.asked_kept.clear();
    let report = pass_over(
        &mut retention,
        &root,
        &mut fake,
        BTreeSet::new(),
        T0 + max_age_ms,
    )
    .expect("a pass that removes prints its line");
    assert_eq!(fake.asked_kept.first(), Some(&vec![root.id(aging)]));
    assert_eq!(
        (
            report.removed_expired,
            report.kept_examined,
            report.kept_unknown
        ),
        (1, 1, 0)
    );
    assert!(!root.kept(aging).exists(), "removed at its maximum age");
    assert!(
        unknown.is_file(),
        "a kept file the ledger does not know stays"
    );
}

#[test]
fn fr1_t3_a_running_frame_waits_for_what_can_end_it_and_the_line_names_the_open_run() {
    let mut root = Root::new();
    let first_at = NOW - 10 * 60_000;
    let held_at = first_at - 10_000;
    let held = root.frame_at(
        FrameRetentionClass::Default,
        held_at,
        Some(held_at + DAY_MS),
        None,
        true,
    );
    let quiet = NOW - 60 * 60_000;
    let open = root.frame_at(FrameRetentionClass::Running, quiet, None, None, true);
    let duplicate = root.frame_at(
        FrameRetentionClass::Duplicate,
        quiet,
        Some(quiet),
        None,
        true,
    );
    // A default frame that turns one day old between the first pass and the second.
    let daily_at = NOW + 5 * 60_000 - DAY_MS;
    let daily = root.frame_at(
        FrameRetentionClass::Default,
        daily_at,
        Some(daily_at + DAY_MS),
        None,
        true,
    );
    let mut fake = root.answers();
    fake.open_runs.push(first_at);
    let run = fake.run_ids[0];
    let ids = |indexes: &[usize]| {
        indexes
            .iter()
            .map(|index| root.id(*index))
            .collect::<BTreeSet<_>>()
    };
    let frame_id = |index: usize| {
        root.frames[index]
            .frame
            .reference
            .frame_id
            .expect("a frame id")
    };
    let mut retention = cleaner();

    // The first pass: two running frames, the duplicate removed; the line names the open run.
    let first = pass_over(&mut retention, &root, &mut fake, root.live(), NOW)
        .expect("the first pass prints its line");
    assert_eq!(
        (first.running, first.deleted, first.waiting),
        (2, 1, 0),
        "{first:?}"
    );
    assert_eq!(
        (
            first.open_runs,
            first.oldest_open_run,
            first.oldest_open_age_ms
        ),
        (1, Some(run), NOW - first_at)
    );
    let line = first.to_string();
    assert!(
        line.contains(&format!(
            " pass_ms={} waiting=0 open_runs=1 oldest_open_run={} oldest_open_age_ms={} \
             live_unknown=0 kept_examined=",
            first.pass_ms,
            crate::failure_identity::identifier_text(&run),
            NOW - first_at
        )),
        "{line}"
    );
    assert!(root.object(open).is_file() && !root.object(duplicate).exists());

    // Ten minutes later the run is still open, and a record closes the held frame's capture: the
    // held frame is not asked about, nor the frame no record closed; the frame that turned one day
    // old is removed, and the line counts the two left waiting.
    fake.asked_live.clear();
    fake.changes.frames.insert(frame_id(held));
    let second = pass_over(
        &mut retention,
        &root,
        &mut fake,
        root.live(),
        NOW + 10 * 60_000,
    )
    .expect("a pass that removes prints its line");
    assert_eq!(first_asked(&fake), ids(&[daily]));
    assert_eq!(
        (
            second.running,
            second.waiting,
            second.deleted,
            second.open_runs
        ),
        (0, 2, 1, 1),
        "{second:?}"
    );

    // The run ends: the frame it held is asked about at the next pass, the other is not.
    fake.open_runs.clear();
    fake.changes = FrameChanges::default();
    fake.asked_live.clear();
    assert_eq!(
        pass_over(
            &mut retention,
            &root,
            &mut fake,
            root.live(),
            NOW + 20 * 60_000
        ),
        None
    );
    assert_eq!(first_asked(&fake), ids(&[held]));

    // A record closes the other frame's capture: it is asked about at the next pass.
    fake.changes.frames.insert(frame_id(open));
    fake.asked_live.clear();
    assert_eq!(
        pass_over(
            &mut retention,
            &root,
            &mut fake,
            root.live(),
            NOW + 30 * 60_000
        ),
        None
    );
    assert_eq!(first_asked(&fake), ids(&[open]));
    for index in [held, open] {
        assert!(root.object(index).is_file(), "frame {index} stays");
    }
}

#[test]
fn fr1_t4_a_live_frame_without_an_answer_is_asked_about_twice_ten_minutes_apart_then_no_more() {
    let mut root = Root::new();
    let orphan = root.frame_at(FrameRetentionClass::Default, T0, Some(T0), None, true);
    let mut fake = root.answers();
    // The ledger has no verified record of it: statement L gives no row.
    fake.frames.remove(&root.id(orphan));
    let mut retention = cleaner();
    let live = BTreeSet::from([root.id(orphan)]);
    for (now, what) in [
        (NOW, "at its first pass"),
        (NOW + 10 * 60_000, "ten minutes later"),
    ] {
        fake.asked_live.clear();
        pass_over(&mut retention, &root, &mut fake, live.clone(), now);
        assert_eq!(first_asked(&fake), live, "asked about {what}");
    }
    assert!(retention.unknown.contains(&root.id(orphan)));
    fake.asked_live.clear();
    assert_eq!(
        pass_over(&mut retention, &root, &mut fake, live, NOW + 20 * 60_000),
        None
    );
    assert!(fake.asked_live.is_empty(), "not asked about again");
    assert!(root.object(orphan).is_file(), "nothing touches it");
}
