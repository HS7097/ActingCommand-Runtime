// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #382 L-3 G8 with LED-I8 and LED-I3: the subscription contracts of ledger key 7, run on
// the SQLite store (and on the Segment store where it costs nothing). The requirements:
// - `contracts/ledger-store.md` "Reads, projections and subscriptions": the subscription owner
//   registers the subscriber and captures the high-water mark in one writer command; history is
//   fetched in bounded pages; committed events above the mark are delivered live; future cursors
//   suppress earlier events; a reconnect replays from its last observed sequence; live queues are
//   bounded; lag reports `subscription_lagged` and removes that subscriber; timeout, clean
//   closure, dropped receivers and writer failure keep distinct behaviour.
// - `contracts/ledger-store.md` "Close": a successful explicit close yields `subscription_closed`.
// - Test plan v2.2 row L-3 and inventory gap G8: the subscription suite runs on SQLite through a
//   backend-parameterized runner (I8); positive waits get a 30 s budget that returns on arrival;
//   a "nothing arrived" check uses `try_recv` after a commit barrier; I3 stalls one commit and
//   the test sees no spurious timeout and no gap.
//
// Each contract is a function over an `open` closure, as in `store_contract.rs`. The commit
// barrier is a writer command (`TestSubscriberCount`): the writer flushes and delivers every
// captured record before it answers a command, so its reply comes after every earlier append
// reached the live queues. Nothing waits on a clock: `GATE_WAIT` bounds a positive wait only.
// `store_contract::subscription_reports_terminal_writer_failure` already runs on SQLite and is
// not repeated here.

use super::{config, event, sqlite_contract};
use crate::PersistedEvent;
use crate::WriterStallGate;
use crate::global::{
    GlobalLedger, GlobalLedgerConfig, GlobalLedgerError, GlobalLedgerResult, LedgerSubscription,
    SubscriptionOptions, WriterCommand,
};
use actingcommand_contract::SubscriptionCursor;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const GATE_WAIT: Duration = Duration::from_secs(30);
const OWNER: &str = "subscription-writer";

// The test configuration; a writer reply is a positive wait too.
fn gate_config(temp: &TempDir) -> GlobalLedgerConfig {
    config(temp, OWNER).with_reply_timeout(GATE_WAIT)
}

// The next `count` events of the subscription, each a positive wait.
fn receive(subscription: &mut LedgerSubscription, count: usize) -> Vec<PersistedEvent> {
    (0..count)
        .map(|_| {
            subscription
                .recv_timeout(GATE_WAIT)
                .expect("the next subscribed event")
        })
        .collect()
}

// The commit barrier: the writer answers only after it delivered every committed event.
fn subscriber_count(ledger: &GlobalLedger) -> usize {
    let (response, count) = mpsc::sync_channel(1);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestSubscriberCount { response })
        .expect("request the subscriber count");
    count.recv_timeout(GATE_WAIT).expect("the subscriber count")
}

fn append_history(ledger: &GlobalLedger, prefix: &str, count: usize) -> Vec<PersistedEvent> {
    (0..count)
        .map(|index| {
            ledger
                .append(event(&format!("{prefix}-{index}")))
                .expect("append history")
        })
        .collect()
}

// Key 7: paged replay, then live, with no gap and in sequence order.
fn paged_replay_then_live_is_gap_free_and_ordered(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp)).expect("ledger");
    let mut expected = append_history(&ledger, "evt-ordered", 5);
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor { after_sequence: 1 },
            SubscriptionOptions::new(2).expect("two-event replay pages"),
        )
        .expect("subscribe");
    assert_eq!(subscription.replay_through_sequence(), 5);
    expected.push(
        ledger
            .append(event("evt-ordered-live"))
            .expect("append live"),
    );

    assert_eq!(receive(&mut subscription, 5), expected[1..]);
    assert_eq!(subscription.resume_cursor().after_sequence, 6);
    ledger.close().expect("close");
}

// Key 7: registration copies no history, replay pages stay within their size, and the writer
// commits while a replay is unfinished.
fn replay_pages_are_bounded_and_the_writer_stays_responsive(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp)).expect("ledger");
    let mut expected = append_history(&ledger, "evt-page", 8);
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor::default(),
            SubscriptionOptions::new(2).expect("two-event replay pages"),
        )
        .expect("subscribe");
    assert!(
        subscription.replay.is_empty(),
        "registration copies no history"
    );
    assert_eq!(subscription.replay_through_sequence(), 8);

    let mut received = receive(&mut subscription, 1);
    assert!(
        subscription.replay.len() <= 1,
        "a page of two holds at most one undelivered event"
    );
    expected.push(
        ledger
            .append(event("evt-writer-responsive"))
            .expect("the writer commits while the replay is unfinished"),
    );
    while received.len() < expected.len() {
        received.extend(receive(&mut subscription, 1));
        assert!(subscription.replay.len() <= 1);
    }
    assert_eq!(received, expected);
    ledger.close().expect("close");
}

// Key 7: a resume cursor recovers every event the subscriber missed, including one committed
// while no subscriber read.
fn a_resume_cursor_recovers_every_missing_event(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp)).expect("ledger");
    let mut expected = append_history(&ledger, "evt-resume", 6);
    let options = SubscriptionOptions::new(4).expect("four-event replay pages");
    let mut first = ledger
        .subscribe_with_options(SubscriptionCursor::default(), options)
        .expect("first subscription");
    assert_eq!(receive(&mut first, 2), expected[..2]);
    let resume = first.resume_cursor();
    assert_eq!(resume.after_sequence, 2);
    drop(first);
    expected.push(
        ledger
            .append(event("evt-resume-while-away"))
            .expect("append while no subscriber reads"),
    );

    let mut resumed = ledger
        .subscribe_with_options(resume, options)
        .expect("resumed subscription");
    assert_eq!(receive(&mut resumed, 5), expected[2..]);
    ledger.close().expect("close");
}

// Key 7: a future cursor suppresses the events at or before it and then delivers without a gap.
fn a_future_cursor_suppresses_earlier_events_and_stays_gap_free(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp)).expect("ledger");
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor { after_sequence: 3 },
            SubscriptionOptions::new(1).expect("one-event replay pages"),
        )
        .expect("subscribe");
    append_history(&ledger, "evt-future-suppressed", 3);

    assert_eq!(
        subscriber_count(&ledger),
        1,
        "the future-cursor subscriber stays registered"
    );
    assert_eq!(
        subscription.live.try_recv(),
        Err(TryRecvError::Empty),
        "events at or before the cursor are not delivered"
    );
    assert_eq!(
        subscription.terminal.try_recv(),
        Err(TryRecvError::Empty),
        "suppression reports nothing"
    );

    let visible = append_history(&ledger, "evt-future-visible", 2);
    assert_eq!(visible[0].sequence(), 4);
    assert_eq!(receive(&mut subscription, 2), visible);
    assert_eq!(subscription.resume_cursor().after_sequence, 5);
    ledger.close().expect("close");
}

// Key 7: an overflowing live queue reports `subscription_lagged` before any replay or buffered
// event, removes the subscriber, and the report stays the same on every later receive.
fn lag_is_absorbing(open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp).with_subscription_capacity(1)).expect("ledger");
    ledger
        .append(event("evt-lag-replay"))
        .expect("append replay");
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor::default(),
            SubscriptionOptions::new(1).expect("one-event replay pages"),
        )
        .expect("subscribe");
    ledger
        .append(event("evt-lag-one"))
        .expect("first live event");
    ledger
        .append(event("evt-lag-two"))
        .expect("second live event overflows a queue of one");

    assert_eq!(subscriber_count(&ledger), 0, "lag removes the subscriber");
    let lagged = subscription
        .recv_timeout(GATE_WAIT)
        .expect_err("lag preempts the replay and the buffered event");
    assert_eq!(lagged.code(), "subscription_lagged");
    assert!(subscription.replay.is_empty());
    ledger
        .append(event("evt-after-lag"))
        .expect("the writer keeps committing");
    assert_eq!(
        subscription
            .recv_timeout(GATE_WAIT)
            .expect_err("lag stays terminal"),
        lagged
    );
    ledger.close().expect("close");
}

// Key 7: a terminal writer failure preempts an unfinished replay and is reported the same way on
// every later receive.
fn terminal_writer_failure_preempts_replay_and_is_stable(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let mut ledger = open(gate_config(&temp)).expect("ledger");
    append_history(&ledger, "evt-terminal", 3);
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor::default(),
            SubscriptionOptions::new(2).expect("two-event replay pages"),
        )
        .expect("subscribe");
    let terminal = GlobalLedgerError::fatal_untyped("test_terminal", "test_writer_failure");
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::TestTerminalFailure {
            error: terminal.clone(),
        })
        .expect("inject a terminal failure");

    ledger.sender.take();
    let writer = ledger.writer.take().expect("writer handle");
    assert_eq!(
        writer
            .join()
            .expect("the writer does not panic")
            .expect_err("the writer returns the terminal error"),
        terminal
    );
    assert_eq!(
        subscription
            .recv_timeout(GATE_WAIT)
            .expect_err("the terminal error preempts the replay"),
        terminal
    );
    assert!(subscription.replay.is_empty());
    assert_eq!(
        subscription
            .recv_timeout(GATE_WAIT)
            .expect_err("the terminal error is stable"),
        terminal
    );
}

// Key 7: a dropped subscriber, a dropped future-cursor subscriber and a dropped subscribe
// response harm no other subscriber, and none of them stays registered.
fn dropped_subscribers_harm_no_other_subscriber(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp).with_subscription_capacity(1)).expect("ledger");
    drop(
        ledger
            .subscribe(SubscriptionCursor::default())
            .expect("dropped subscription"),
    );
    drop(
        ledger
            .subscribe(SubscriptionCursor {
                after_sequence: 100,
            })
            .expect("dropped future-cursor subscription"),
    );
    let (response, dropped_response) = mpsc::sync_channel(1);
    drop(dropped_response);
    ledger
        .sender
        .as_ref()
        .expect("writer sender")
        .send(WriterCommand::Subscribe {
            cursor: SubscriptionCursor::default(),
            response,
        })
        .expect("enqueue a subscribe whose response is dropped");
    let mut active = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("active subscription");

    for label in ["evt-active-one", "evt-active-two", "evt-active-three"] {
        let appended = ledger.append(event(label)).expect("append");
        assert_eq!(
            active
                .recv_timeout(GATE_WAIT)
                .expect("the active subscriber's event"),
            appended
        );
    }
    assert_eq!(
        subscriber_count(&ledger),
        1,
        "only the active subscriber stays registered"
    );
    ledger.close().expect("close");
}

// Key 7 (clean close): a successful close reaches the subscriber as `subscription_closed`, not
// fatal (a writer failure is fatal), and the report stays latched.
fn a_clean_close_reaches_the_subscriber_and_stays_latched(
    open: impl Fn(GlobalLedgerConfig) -> GlobalLedgerResult<GlobalLedger>,
) {
    let temp = TempDir::new().expect("temp");
    let ledger = open(gate_config(&temp)).expect("ledger");
    let mut subscription = ledger
        .subscribe(SubscriptionCursor::default())
        .expect("subscribe");
    let appended = ledger.append(event("evt-before-close")).expect("append");
    assert_eq!(receive(&mut subscription, 1), [appended]);

    ledger.close().expect("close");
    let closed = subscription
        .recv_timeout(GATE_WAIT)
        .expect_err("a clean close reaches the subscriber");
    assert_eq!(closed.code(), "subscription_closed");
    assert!(!closed.is_fatal());
    assert_eq!(
        subscription
            .recv_timeout(GATE_WAIT)
            .expect_err("the clean close stays latched"),
        closed
    );
}

#[test]
fn gate_l3_g8_paged_replay_then_live_is_gap_free_and_ordered() {
    paged_replay_then_live_is_gap_free_and_ordered(sqlite_contract::open);
    paged_replay_then_live_is_gap_free_and_ordered(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_replay_pages_are_bounded_and_the_writer_stays_responsive() {
    replay_pages_are_bounded_and_the_writer_stays_responsive(sqlite_contract::open);
    replay_pages_are_bounded_and_the_writer_stays_responsive(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_a_resume_cursor_recovers_every_missing_event() {
    a_resume_cursor_recovers_every_missing_event(sqlite_contract::open);
    a_resume_cursor_recovers_every_missing_event(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_a_future_cursor_suppresses_earlier_events_and_stays_gap_free() {
    a_future_cursor_suppresses_earlier_events_and_stays_gap_free(sqlite_contract::open);
    a_future_cursor_suppresses_earlier_events_and_stays_gap_free(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_lag_is_absorbing() {
    lag_is_absorbing(sqlite_contract::open);
    lag_is_absorbing(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_terminal_writer_failure_preempts_replay_and_is_stable() {
    terminal_writer_failure_preempts_replay_and_is_stable(sqlite_contract::open);
    terminal_writer_failure_preempts_replay_and_is_stable(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_dropped_subscribers_harm_no_other_subscriber() {
    dropped_subscribers_harm_no_other_subscriber(sqlite_contract::open);
    dropped_subscribers_harm_no_other_subscriber(GlobalLedger::open);
}

#[test]
fn gate_l3_g8_a_clean_close_reaches_the_subscriber_and_stays_latched() {
    a_clean_close_reaches_the_subscriber_and_stays_latched(sqlite_contract::open);
    a_clean_close_reaches_the_subscriber_and_stays_latched(GlobalLedger::open);
}

// LED-I3: one commit is held before COMMIT while a subscriber waits. Nothing reaches the
// subscriber while the commit is held; once released, the append returns its sequence, the
// subscriber receives exactly that event next, and neither side reports a timeout or a failure.
// The SQLite store only: the gate holds the SQLite commit.
#[test]
fn gate_l3_g8_a_stalled_commit_reaches_the_subscriber_without_timeout_or_gap() {
    let temp = TempDir::new().expect("temp");
    let ledger = sqlite_contract::open(gate_config(&temp)).expect("ledger");
    let history = append_history(&ledger, "evt-before-stall", 2);
    let mut subscription = ledger
        .subscribe_with_options(
            SubscriptionCursor::default(),
            SubscriptionOptions::new(1).expect("one-event replay pages"),
        )
        .expect("subscribe");
    assert_eq!(receive(&mut subscription, 2), history);

    let stall = WriterStallGate::arm(temp.path()).expect("writer stall gate");
    let appended = thread::scope(|scope| {
        let appender = scope.spawn(|| ledger.append(event("evt-held")));
        assert!(
            stall.wait_held_commit(GATE_WAIT),
            "the append's commit is held"
        );
        assert!(!appender.is_finished(), "the append waits for its commit");
        assert_eq!(
            subscription.live.try_recv(),
            Err(TryRecvError::Empty),
            "a held commit delivers nothing"
        );
        assert_eq!(
            subscription.terminal.try_recv(),
            Err(TryRecvError::Empty),
            "a held commit reports nothing"
        );
        assert!(subscription.replay.is_empty());
        stall.release();
        appender.join().expect("the appending thread ends")
    })
    .expect("the held append commits once released");

    assert_eq!(appended.sequence(), 3);
    assert_eq!(
        subscription
            .recv_timeout(GATE_WAIT)
            .expect("the held event arrives once its commit finishes"),
        appended
    );
    assert_eq!(
        subscriber_count(&ledger),
        1,
        "the subscriber is still registered"
    );
    assert_eq!(
        subscription.terminal.try_recv(),
        Err(TryRecvError::Empty),
        "no failure is reported"
    );
    ledger
        .check_writer_health()
        .expect("the writer is healthy after the stall");
    let next = ledger
        .append(event("evt-after-stall"))
        .expect("append after the stall");
    assert_eq!(next.sequence(), 4);
    assert_eq!(receive(&mut subscription, 1), [next]);
    assert_eq!(subscription.resume_cursor().after_sequence, 4);
    ledger.close().expect("close");
}
