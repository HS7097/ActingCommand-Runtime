// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-7c Q (rd-1026/ln7c/final-query.md §1, §2, §4, §7 row Q; brief rd-1026/brief-ln7c-q.md;
// Alice's ruling 70 and her rule for evidence frames, the 14-day question answered A): the frame
// cleaner by ledger query, on the ledger's side.
// - T-eq (production-shaped): on a history of runs closed by their summaries and terminals, with
//   marker chains, run-linked Warnings that keep windows, resource readings, Lab runs, runs cut
//   short by a start and an open run, each of the fixed set of about 2 000 live frames gets from
//   the query the full view's class with E1's due time, and for a frame it moves the full view's
//   kept folder; at about 10 000 records here, and at 1 000 000 records on demand (ignored: the
//   history is written through the writer). The case fixture is `gate_ln7.rs`'s.
// - T-evidence (the ledger's half): a kept frame no one confirmed stays at 13.9 and 14.5 days and
//   is removed at 15.0 days; a frame whose reading an agent confirmed 2 days after its capture is
//   removed (in `kept\` and still live); a run's confirmation covers the frames captured before it
//   and not a later one; a confirmation recorded before the capture covers nothing; reading the
//   run's records (`QueryEvents`, as `ac_run_evidence` reads them) is no confirmation.
// - T-open: the writer open reads no record of a frame (`LedgerReadCounts`: none of their event or
//   link rows and no artifact row) and does not list `artifacts\` (here a file stands there).
// - T-pace (the ledger's half): a raised stop interrupts a statement in progress, and a statement
//   asked after the stop does not run.

use super::gate_ln7::{
    CHUNK_FRAMES, Corpus, DAY_MS, EVIDENCE_MAX_AGE_MS, Frame, RunLinks, SWITCHES, at,
    expected_action, query_decisions, settings,
};
use super::missing_material::{formal_root, material_store};
use crate::global::{
    FrameQueryAction, FrameQueryOutcome, FrameRetentionClass, GlobalLedger, LedgerFailurePlan,
    LedgerMaintenance, LedgerOpenTiming, LedgerWritePolicy,
};
use crate::{GlobalLedgerEvidenceConfig, LedgerReadCounter, PersistedEvent};
use actingcommand_contract::{
    ArtifactId, AuditInput, ClientActionKind, ClientActionRecord, ClientActionValue,
    ClientPayloadDraft, CommandPayloadDraft, EVIDENCE_READ_FRAME_CONTROL,
    EVIDENCE_READ_RUN_CONTROL, EventAction, EventActor, EventLinksDraft, EventOrigin, EventQuery,
    EventSeverity, EventSource, IdentifierIssuer, OriginModule,
};
use actingcommand_runtime_database::MaintenanceLimits;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// T-eq: the live frames of the production-shaped history, four per run.
const SHAPED_RUNS: u64 = 500;
const FRAMES_PER_RUN: u64 = 4;
// The history's lengths: about 10 000 records with the live frames alone, and 1 000 000.
const SHAPED_RECORDS: u64 = 10_000;
const SCALED_RECORDS: u64 = 1_000_000;
// T-open: the open authenticates this tail, plain records only, and reads pages of this size.
const TAIL: usize = 16;
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;

/// A plain record of the instance (`command.received` from the command line), no frame.
fn plain(corpus: &Corpus<'_>, at_ms: u64) -> PersistedEvent {
    corpus.append(
        at_ms,
        EventSeverity::Info,
        EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
        corpus.scoped(),
        CommandPayloadDraft::received(EventAction::RuntimeStart, AuditInput::new()).into(),
    )
}

/// A read confirmation (`client.action`, control `evidence_read_frame` or `evidence_read_run`)
/// recorded at `at_ms` as MCP records an agent's (Agent, Adapter) or the CLI the command line's.
fn confirm(corpus: &Corpus<'_>, control: &str, value: String, agent: bool, at_ms: u64) {
    let (surface, origin) = if agent {
        (
            "mcp",
            EventOrigin::new(
                EventSource::Adapter,
                OriginModule::Governance,
                EventActor::Agent,
            ),
        )
    } else {
        (
            "cli",
            EventOrigin::new(EventSource::Cli, OriginModule::Governance, EventActor::Cli),
        )
    };
    let record = ClientActionRecord::new(
        surface,
        control,
        ClientActionKind::Command,
        None,
        Some(ClientActionValue::PathSafeString(value)),
    )
    .expect("a read confirmation");
    corpus.append(
        at_ms,
        EventSeverity::Info,
        origin,
        corpus.scoped(),
        ClientPayloadDraft::action(record, AuditInput::new()).into(),
    );
}

fn text<T: serde::Serialize>(id: &T) -> String {
    match serde_json::to_value(id) {
        Ok(serde_json::Value::String(text)) => text,
        other => panic!("identifier text: {other:?}"),
    }
}

/// The production-shaped history of T-eq on `corpus` (one instance, one owner), from `first` s
/// after `T0`: `SHAPED_RUNS` runs 60 s apart of `FRAMES_PER_RUN` frames 2 s apart, each closed by
/// its summary and terminal, except every 17th run, left without a terminal and cut short by the
/// start after it, and the last, open in the current epoch. Every 5th run's frames 0, 1 and 2 are
/// a marker chain (frame 1 nearly repeats frame 0, frame 2 frame 1); every 9th run has a
/// run-linked Warning naming its frame 2; every 11th run has a resource reading on its frame 0;
/// every 13th run has a Lab use in its correlation. Returns the frames.
fn shaped(corpus: &Corpus<'_>, first: u64) -> Vec<Frame> {
    let mut frames = Vec::new();
    for index in 0..SHAPED_RUNS {
        let begin = first + index * 60;
        let run = corpus.run(at(begin));
        let mut own = Vec::new();
        for frame in 0..FRAMES_PER_RUN {
            own.push(corpus.run_frame(&run, at(begin + 1 + 2 * frame)));
        }
        if index % 5 == 0 {
            corpus.marker(&own[0], &own[1], at(begin + 4));
            corpus.marker(&own[1], &own[2], at(begin + 6));
        }
        if index % 9 == 0 {
            corpus.warning_naming(&own[2], at(begin + 6));
        }
        if index % 13 == 0 {
            corpus.lab_anchor(
                EventLinksDraft::default()
                    .with_request_id(corpus.request())
                    .with_correlation_id(run.correlation),
                at(begin + 8),
            );
        }
        let last = index + 1 == SHAPED_RUNS;
        if index % 17 == 0 && !last {
            // Cut short: no terminal, and a start ends its owner epoch.
            corpus.start(at(begin + 30));
        } else if !last {
            let refs = own.iter().collect::<Vec<_>>();
            corpus.close(&run.links, &refs, at(begin + 9));
            if index % 11 == 0 {
                corpus.reading(&run, &own[0], at(begin + 10));
            }
        }
        frames.extend(own);
    }
    frames
}

/// T-eq on the production-shaped history of `records` records: the query's answers equal the
/// full view's classes, with E1's due times, and its folders the full view's.
fn shaped_equivalence(records: u64) {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    let frames = shaped(&corpus, 100);
    let live = frames.iter().map(Frame::id).collect::<BTreeSet<_>>();
    assert_eq!(live.len() as u64, SHAPED_RUNS * FRAMES_PER_RUN);
    // The rest of the history: plain records after the runs.
    let end = 100 + SHAPED_RUNS * 60;
    let head = ledger.latest_sequence().expect("head");
    for record in head..records {
        plain(&corpus, at(end) + record);
    }
    let head = ledger.latest_sequence().expect("head");
    assert!(
        head >= records,
        "the history holds {records} records: {head}"
    );
    let now = at(end) + 2 * DAY_MS;
    let decided = query_decisions(&ledger, &live, now);
    ledger.close().expect("close the writer");
    let full = GlobalLedger::open_evidence(
        GlobalLedgerEvidenceConfig::new(root.path()).sqlite_material_not_read(),
        |_| None,
    )
    .expect("the whole opening")
    .frame_retention_view(now, SWITCHES)
    .expect("the full view");

    let mut classes = BTreeMap::<FrameRetentionClass, u64>::new();
    let mut compared = 0;
    for frame in full
        .frames
        .iter()
        .filter(|frame| live.contains(&frame.reference.artifact_id))
    {
        let id = frame.reference.artifact_id;
        *classes.entry(frame.class).or_default() += 1;
        compared += 1;
        let Some((answer, folder)) = decided.get(&id) else {
            panic!("the query answers no row for {id:?} ({:?})", frame.class);
        };
        assert_eq!(answer.class, frame.class, "the class of {id:?}");
        assert_eq!(
            answer.action,
            expected_action(frame, now),
            "the action of {id:?} ({:?})",
            frame.class
        );
        assert_eq!(folder, &frame.folder, "the kept folder of {id:?}");
    }
    assert_eq!(compared, live.len(), "the full view lists every live frame");
    // The shape is not vacuous: every class but resource-after-7-days occurs.
    for class in [
        FrameRetentionClass::Lab,
        FrameRetentionClass::Error,
        FrameRetentionClass::Resource,
        FrameRetentionClass::Duplicate,
        FrameRetentionClass::Default,
        FrameRetentionClass::Running,
    ] {
        assert!(
            classes.get(&class).is_some_and(|count| *count > 0),
            "no {class:?} frame: {classes:?}"
        );
    }
}

#[test]
fn ln7c_q_teq_the_query_decides_the_live_frames_of_a_production_shaped_history_as_the_full_view() {
    shaped_equivalence(SHAPED_RECORDS);
}

#[test]
#[ignore = "1 000 000 records written through the writer: run on demand"]
fn ln7c_q_teq_the_same_live_frames_over_a_million_records() {
    shaped_equivalence(SCALED_RECORDS);
}

/// K and L's answers for one frame at `now`.
fn kept_action(
    ledger: &GlobalLedger,
    frame: ArtifactId,
    now: u64,
    max_age_ms: u64,
) -> Option<FrameQueryAction> {
    let mut query = ledger
        .frame_query(Arc::new(|| false))
        .expect("the writer's frame query");
    let mut settings = settings(now);
    settings.evidence_max_age_ms = max_age_ms;
    match query.kept(&[frame], &settings).expect("statement K") {
        FrameQueryOutcome::Answered { value, .. } => {
            value.into_iter().next().map(|answer| answer.action)
        }
        other => panic!("statement K: {other:?}"),
    }
}

fn live_action(ledger: &GlobalLedger, frame: ArtifactId, now: u64) -> Option<FrameQueryAction> {
    let live = BTreeSet::from([frame]);
    query_decisions(ledger, &live, now)
        .remove(&frame)
        .map(|(answer, _)| answer.action)
}

#[test]
fn ln7c_q_t_evidence_a_kept_frame_goes_once_confirmed_or_at_its_maximum_age_only() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    // Each frame in its own closed run, its run's correlation used by the Lab, so the frame is
    // Lab output (an evidence frame) while it is live.
    let lab_run = |begin: u64| -> RunLinks {
        let run = corpus.run(at(begin));
        corpus.lab_anchor(
            EventLinksDraft::default()
                .with_request_id(corpus.request())
                .with_correlation_id(run.correlation),
            at(begin),
        );
        run
    };
    let unconfirmed_run = lab_run(10);
    let unconfirmed = corpus.run_frame(&unconfirmed_run, at(11));
    corpus.close(&unconfirmed_run.links, &[&unconfirmed], at(12));
    let read_run = lab_run(20);
    let read = corpus.run_frame(&read_run, at(21));
    corpus.close(&read_run.links, &[&read], at(22));
    // A run whose reading the command line confirmed: one frame before, one after.
    let confirmed_run = lab_run(30);
    let before = corpus.run_frame(&confirmed_run, at(31));
    let after = corpus.run_frame(&confirmed_run, at(50));
    corpus.close(&confirmed_run.links, &[&before, &after], at(51));
    let early_run = lab_run(60);
    let early = corpus.run_frame(&early_run, at(61));
    corpus.close(&early_run.links, &[&early], at(62));
    let looked_run = lab_run(70);
    let looked = corpus.run_frame(&looked_run, at(71));
    corpus.close(&looked_run.links, &[&looked], at(72));

    // The confirmations: the run's at 40 s (between its frames), the early frame's recorded at
    // 59 s (before its capture at 61 s), the read frame's by an agent 2 days after its capture.
    confirm(
        &corpus,
        EVIDENCE_READ_RUN_CONTROL,
        text(confirmed_run.id.transport()),
        false,
        at(40),
    );
    confirm(
        &corpus,
        EVIDENCE_READ_FRAME_CONTROL,
        text(&early.id()),
        true,
        at(59),
    );
    let read_at = at(21) + 2 * DAY_MS;
    confirm(
        &corpus,
        EVIDENCE_READ_FRAME_CONTROL,
        text(&read.id()),
        true,
        read_at,
    );
    // Reading the run's records, as `ac_run_evidence` does, records nothing.
    let head = ledger.latest_sequence().expect("head");
    let records = ledger
        .query(EventQuery {
            run_id: Some(*looked_run.id.transport()),
            ..EventQuery::default()
        })
        .expect("the run's records");
    assert!(!records.is_empty(), "the run has records");
    assert_eq!(
        ledger.latest_sequence().expect("head"),
        head,
        "a read records nothing"
    );

    // The next pass after the frame confirmation (2 days after its capture).
    let next = read_at + 1_000;
    let max = EVIDENCE_MAX_AGE_MS;
    use FrameQueryAction::{Keep, MoveLab, RemoveConfirmed, RemoveExpired};
    for (frame, expected, what) in [
        (&read, RemoveConfirmed, "confirmed 2 days after its capture"),
        (
            &before,
            RemoveConfirmed,
            "captured before its run's confirmation",
        ),
        (&after, Keep, "captured after its run's confirmation"),
        (&early, Keep, "confirmed before its capture"),
        (&looked, Keep, "read through its run's records only"),
        (&unconfirmed, Keep, "never confirmed"),
    ] {
        assert_eq!(
            kept_action(&ledger, frame.id(), next, max),
            Some(expected),
            "in kept: the frame {what}"
        );
    }
    // Still live, a confirmed evidence frame is removed too; an unconfirmed one is moved.
    assert_eq!(live_action(&ledger, read.id(), next), Some(RemoveConfirmed));
    assert_eq!(
        live_action(&ledger, before.id(), next),
        Some(RemoveConfirmed)
    );
    assert_eq!(live_action(&ledger, after.id(), next), Some(MoveLab));
    assert_eq!(live_action(&ledger, unconfirmed.id(), next), Some(MoveLab));

    // Never confirmed: kept at 13.9 and 14.5 days, removed at 15.0 days (answer A: nothing
    // happens at 14 days).
    let captured = unconfirmed.reference.created_at_unix_ms;
    for (age_tenths, expected) in [(139, Keep), (145, Keep), (150, RemoveExpired)] {
        assert_eq!(
            kept_action(
                &ledger,
                unconfirmed.id(),
                captured + age_tenths * DAY_MS / 10,
                max
            ),
            Some(expected),
            "unconfirmed at {age_tenths} tenths of a day"
        );
    }
    // The maximum age is the one the pass reads: 10 days removes it at 10 days.
    assert_eq!(
        kept_action(
            &ledger,
            unconfirmed.id(),
            captured + 10 * DAY_MS,
            10 * DAY_MS
        ),
        Some(RemoveExpired)
    );
    // A kept file the ledger has no record of gives no row (the cleaner counts it kept_unknown).
    let unknown = *IdentifierIssuer::new()
        .expect("issuer")
        .mint_artifact_id()
        .expect("artifact")
        .transport();
    assert_eq!(kept_action(&ledger, unknown, next, max), None);
    ledger.close().expect("close the writer");
}

#[test]
fn ln7c_q_t_open_the_writer_open_reads_no_frame_and_does_not_list_the_frame_store() {
    let root = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    for index in 0..20 {
        let run = corpus.run(at(10 + index * 60));
        let one = corpus.run_frame(&run, at(11 + index * 60));
        let two = corpus.run_frame(&run, at(13 + index * 60));
        corpus.marker(&one, &two, at(13 + index * 60));
        corpus.close(&run.links, &[&one, &two], at(14 + index * 60));
    }
    // The open's tail: plain records only.
    for index in 0..2 * TAIL as u64 {
        plain(&corpus, at(2_000 + index));
    }
    let framed = ledger
        .query(EventQuery::default())
        .expect("the history")
        .into_iter()
        .filter(|event| event.links().frame_id().is_some() || !event.artifacts().is_empty())
        .map(|event| event.sequence())
        .collect::<BTreeSet<_>>();
    assert!(framed.len() >= 100, "the history has frame records");
    ledger.close().expect("close the writer");
    drop(artifacts);
    // A file where the live frame store's folder was: a listing of it fails.
    let store = root.path().join("artifacts");
    std::fs::rename(&store, root.path().join("artifacts-moved")).expect("move the frame store");
    std::fs::write(&store, b"not a folder").expect("a file at the frame store's path");

    let state_root = root.path().canonicalize().expect("state root");
    let epoch = *IdentifierIssuer::new()
        .expect("issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport();
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch,
        at(3_000),
    )
    .with_open_verify_tail_records(TAIL)
    .with_history_page_rows(TAIL);
    let counter = LedgerReadCounter::start();
    let opened = LedgerMaintenance::acquire(
        root.path(),
        false,
        MaintenanceLimits::default(),
        MaintenanceLimits::default().deadline().expect("deadline"),
    )
    .and_then(|maintenance| {
        maintenance.open_writer_timed(
            Arc::clone(&database),
            "ln7c-q-open".into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
    });
    let counts = counter.counts();
    let sequences = counter.sequences_read();
    drop(counter);
    let ledger = opened.expect("the writer opens without listing the frame store");
    ledger.close().expect("close the writer");
    let read = sequences
        .events
        .union(&sequences.links)
        .copied()
        .collect::<BTreeSet<_>>();
    assert!(
        read.is_disjoint(&framed),
        "the open read frame records: {:?}",
        read.intersection(&framed).collect::<Vec<_>>()
    );
    assert_eq!(counts.artifacts.rows, 0, "no artifact row: {counts:?}");
}

#[test]
fn ln7c_q_t_pace_a_raised_stop_interrupts_the_statement_in_progress() {
    let root = tempfile::tempdir().expect("root");
    let (_database, ledger) = formal_root(root.path());
    let artifacts = material_store(root.path());
    let corpus = Corpus::new(&ledger, &artifacts);
    corpus.start(at(0));
    corpus.owner_recorded(at(1));
    let run = corpus.run(at(10));
    let frame = corpus.run_frame(&run, at(11));
    corpus.close(&run.links, &[&frame], at(12));
    // Enough frames to ask about that the statement runs past several looks at the stop.
    let ids = IdentifierIssuer::new().expect("issuer");
    let mut asked = (0..20_000)
        .map(|_| *ids.mint_artifact_id().expect("artifact").transport())
        .collect::<Vec<_>>();
    asked.push(frame.id());
    let now = at(12) + 2 * DAY_MS;

    // The stop is raised at its third look: the run's own, then two inside the statement.
    let looks = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&looks);
    let mut query = ledger
        .frame_query(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst) >= 2
        }))
        .expect("the writer's frame query");
    let waits = crate::global::FrameWaits::none();
    assert_eq!(
        query
            .live(&asked, &waits, &settings(now))
            .expect("statement L"),
        FrameQueryOutcome::Stopped,
        "the statement in progress is interrupted"
    );
    assert!(
        looks.load(Ordering::SeqCst) >= 3,
        "the stop was looked at inside the statement"
    );
    // Asked after the stop, a statement does not run.
    let before = looks.load(Ordering::SeqCst);
    assert_eq!(
        query
            .kept(&[frame.id()], &settings(now))
            .expect("statement K"),
        FrameQueryOutcome::Stopped
    );
    assert_eq!(
        looks.load(Ordering::SeqCst),
        before + 1,
        "only the run's own look"
    );

    // Without the stop the same chunk answers.
    let mut query = ledger
        .frame_query(Arc::new(|| false))
        .expect("the writer's frame query");
    let FrameQueryOutcome::Answered { value, steps } = query
        .live(&asked[asked.len() - CHUNK_FRAMES..], &waits, &settings(now))
        .expect("statement L")
    else {
        panic!("statement L answered nothing");
    };
    assert!(steps > 0, "a statement reports its steps");
    assert_eq!(
        value
            .iter()
            .map(|answer| answer.artifact_id)
            .collect::<Vec<_>>(),
        vec![frame.id()]
    );
    ledger.close().expect("close the writer");
}
