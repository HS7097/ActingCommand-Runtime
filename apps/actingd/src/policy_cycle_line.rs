// SPDX-License-Identifier: AGPL-3.0-only

//! #486 ASLEEP-5 (register §1.524, the coordinator's ruling 4): one line of the daemon's log per
//! evaluated policy cycle, saying where its time went and what woke the driver for it. A cycle
//! that did not evaluate writes no line; nothing is written to the ledger and nothing is stored.
//!
//! The line names the cycle's number (`seq`, the fact refresh recorder's count of evaluated
//! cycles, which `actingctl overview` shows as `cycle_seq`), the kind of its trigger
//! (`triggers`), what woke the driver since the line before (`wakes`: each type of committed
//! record that raised the host's business notification, and the driver's own wakes, each with its
//! number), and the milliseconds of the projection of the cycle's inputs, the evaluation itself,
//! the clock slots' preview (with the task pairs it read, its calls of `clock_slot_live` and its
//! reads of the planning-signal store), the observation of the holds for the held records, what
//! follows the capped record up to the cycle's end, and the whole cycle.
//!
//! When cycles follow one another faster than one a second, one line a second at most: a cycle
//! less than a second after the last line is kept, and the next line (that of a later cycle, or
//! the driver's next look at the clock at least a second after the last line) stands for every
//! cycle kept, with their number (`cycles`) and their summed times and counts.

use actingcommand_contract::EventType;
use actingcommand_runtime_host::{PolicyEvaluationMeasurement, PolicyTrigger};
use std::time::Duration;

/// The least time between two lines.
const LINE_INTERVAL_MS: u64 = 1_000;

/// The driver's own wakes, as the line names them beside the host's: a scheduled run that ended
/// (its settlement, its failure or its drop).
pub(crate) const RUN_ENDED_WAKE: &str = "run_ended";
/// A cycle that left intents unattempted or whose run failed, evaluated again at once.
pub(crate) const CYCLE_FOLLOW_UP_WAKE: &str = "cycle_follow_up";
/// The time the last evaluation or the host named for the driver to look again came.
pub(crate) const NEXT_WAKE: &str = "next_wake";
/// The reconciliation's interval came.
pub(crate) const RECONCILIATION_WAKE: &str = "reconciliation";
/// The clock jumped.
pub(crate) const CLOCK_JUMP_WAKE: &str = "clock_jump";
/// Work a cool-down or a debounce deferred came due.
pub(crate) const DEFERRED_WAKE: &str = "deferred";

/// Adds `count` to `key`'s entry of `tally`, in the order each key first came.
pub(crate) fn add_count<K: PartialEq>(tally: &mut Vec<(K, u64)>, key: K, count: u64) {
    match tally.iter_mut().find(|(known, _)| *known == key) {
        Some((_, total)) => *total = total.saturating_add(count),
        None => tally.push((key, count)),
    }
}

/// The spelling of a committed record's type, as the ledger writes it.
pub(crate) fn event_type_token(event_type: EventType) -> String {
    serde_json::to_value(event_type)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{event_type:?}"))
}

/// The word of a trigger's kind on the line.
const fn trigger_word(trigger: PolicyTrigger) -> &'static str {
    match trigger {
        PolicyTrigger::FactsChanged => "facts_changed",
        PolicyTrigger::ResourcesChanged => "resources_changed",
        PolicyTrigger::CatalogChanged => "catalog_changed",
        PolicyTrigger::Reconciliation => "reconciliation",
        PolicyTrigger::Recovery => "recovery",
        PolicyTrigger::ClockObserved { .. } => "clock_observed",
    }
}

/// Microseconds as milliseconds with three decimals.
fn milliseconds(micros: u64) -> String {
    format!("{}.{:03}", micros / 1_000, micros % 1_000)
}

/// What a line stands for: one evaluated cycle, or the sums of several.
#[derive(Debug, Default)]
struct CycleSums {
    first_seq: u64,
    last_seq: u64,
    cycles: u64,
    triggers: Vec<(&'static str, u64)>,
    wakes: Vec<(String, u64)>,
    projection_micros: u64,
    evaluation_micros: u64,
    clock_slots_micros: u64,
    clock_slot_pairs: u64,
    clock_slot_live_calls: u64,
    clock_slot_store_reads: u64,
    held_micros: u64,
    after_capped_micros: u64,
    total_micros: u64,
}

impl CycleSums {
    fn line(&self) -> String {
        let tally = |entries: Vec<String>| {
            if entries.is_empty() {
                "none".to_owned()
            } else {
                entries.join(",")
            }
        };
        let triggers = tally(
            self.triggers
                .iter()
                .map(|(word, count)| format!("{word}:{count}"))
                .collect(),
        );
        let wakes = tally(
            self.wakes
                .iter()
                .map(|(cause, count)| format!("{cause}:{count}"))
                .collect(),
        );
        format!(
            "actingd policy_cycle seq={} first_seq={} cycles={} triggers={triggers} \
             wakes={wakes} projection_ms={} evaluation_ms={} clock_slots_ms={} \
             clock_slot_pairs={} clock_slot_live_calls={} clock_slot_store_reads={} held_ms={} \
             after_capped_ms={} total_ms={}",
            self.last_seq,
            self.first_seq,
            self.cycles,
            milliseconds(self.projection_micros),
            milliseconds(self.evaluation_micros),
            milliseconds(self.clock_slots_micros),
            self.clock_slot_pairs,
            self.clock_slot_live_calls,
            self.clock_slot_store_reads,
            milliseconds(self.held_micros),
            milliseconds(self.after_capped_micros),
            milliseconds(self.total_micros),
        )
    }
}

/// The driver's lines of its evaluated cycles: what woke it since the last evaluated cycle, and
/// the cycles kept since the last line.
#[derive(Debug, Default)]
pub(crate) struct PolicyCycleLines {
    last_line_unix_ms: Option<u64>,
    pending: Option<CycleSums>,
    wakes: Vec<(String, u64)>,
}

impl PolicyCycleLines {
    /// What woke the driver, each cause with its number; the next evaluated cycle's line names
    /// them.
    pub(crate) fn note_wakes(&mut self, wakes: impl IntoIterator<Item = (String, u64)>) {
        for (cause, count) in wakes {
            add_count(&mut self.wakes, cause, count);
        }
    }

    /// One evaluated cycle at `now_unix_ms`: its trigger, the host's measurement of it and the
    /// whole cycle's time. The line to write now, if one is due.
    pub(crate) fn record(
        &mut self,
        now_unix_ms: u64,
        trigger: PolicyTrigger,
        measurement: &PolicyEvaluationMeasurement,
        total: Duration,
    ) -> Option<String> {
        let total_micros = u64::try_from(total.as_micros()).unwrap_or(u64::MAX);
        let sums = self.pending.get_or_insert_with(|| CycleSums {
            first_seq: measurement.cycle_seq,
            ..CycleSums::default()
        });
        sums.last_seq = measurement.cycle_seq;
        sums.cycles = sums.cycles.saturating_add(1);
        add_count(&mut sums.triggers, trigger_word(trigger), 1);
        for (cause, count) in std::mem::take(&mut self.wakes) {
            add_count(&mut sums.wakes, cause, count);
        }
        let clock_slots = measurement.clock_slots;
        for (sum, value) in [
            (&mut sums.projection_micros, measurement.projection_micros),
            (&mut sums.evaluation_micros, measurement.elapsed_micros),
            (&mut sums.clock_slots_micros, clock_slots.micros),
            (&mut sums.clock_slot_pairs, clock_slots.pairs),
            (&mut sums.clock_slot_live_calls, clock_slots.live_calls),
            (&mut sums.clock_slot_store_reads, clock_slots.store_reads),
            (&mut sums.held_micros, measurement.held_observe_micros),
            (
                &mut sums.after_capped_micros,
                total_micros.saturating_sub(measurement.until_capped_micros),
            ),
            (&mut sums.total_micros, total_micros),
        ] {
            *sum = sum.saturating_add(value);
        }
        self.flush(now_unix_ms)
    }

    /// The line of the cycles kept, when a second has passed since the last line at
    /// `now_unix_ms`; none when no cycle is kept or the second has not passed.
    pub(crate) fn flush(&mut self, now_unix_ms: u64) -> Option<String> {
        if self
            .last_line_unix_ms
            .is_some_and(|last| now_unix_ms.saturating_sub(last) < LINE_INTERVAL_MS)
        {
            return None;
        }
        let sums = self.pending.take()?;
        self.last_line_unix_ms = Some(now_unix_ms);
        Some(sums.line())
    }
}
