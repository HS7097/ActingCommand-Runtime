// SPDX-License-Identifier: AGPL-3.0-only

//! The daily summary (Workflow #432 AGT-2b; `contracts/agent-wake.md`, "The daily summary"): when a
//! `daily_summary` rule's summary is due, the date its message names, and the parts of its body the
//! rows of `host.agent_wake` give. Pure: the worker hands the rule's time, the rows and what
//! `RoutineSummary` answers for each instance.
//!
//! A rule's day is the local day of its own fixed offset from UTC, not the host's: the summary of a
//! day is due from that day's `at_minute_of_day` until the day ends, and is made once.

use crate::agent_wake::guards::{DAY_MS, local_day};
use crate::agent_wake::rows::WakeRow;
use actingcommand_contract::RoutineSummaryPage;
use actingcommand_runtime_config::agent_wake::{KIND_DAILY_SUMMARY, SummaryAt, WakeRule};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const MINUTE_MS: i64 = 60_000;
/// Days from 0000-03-01 to 1970-01-01 in the proleptic Gregorian calendar.
const EPOCH_FROM_MARCH_ZERO: i64 = 719_468;

fn offset_ms(at: SummaryAt) -> i64 {
    i64::from(at.utc_offset_minutes) * MINUTE_MS
}

/// The rule's local day of `unix_ms`: whole days since the epoch in its offset.
pub(crate) fn day_of(at: SummaryAt, unix_ms: u64) -> u64 {
    local_day(unix_ms, offset_ms(at))
}

/// When the summary of the rule's local day `day` is due.
pub(crate) fn due_at(at: SummaryAt, day: u64) -> u64 {
    let due = i128::from(day) * i128::from(DAY_MS)
        + i128::from(at.minute_of_day) * i128::from(MINUTE_MS)
        - i128::from(offset_ms(at));
    u64::try_from(due).unwrap_or(0)
}

/// The instants of the rule's local day `day`, `[from, until)`: its midnight to the next, by the
/// rule's offset.
pub(crate) fn day_span(at: SummaryAt, day: u64) -> (u64, u64) {
    let midnight = |day: u64| {
        u64::try_from(i128::from(day) * i128::from(DAY_MS) - i128::from(offset_ms(at))).unwrap_or(0)
    };
    (midnight(day), midnight(day.saturating_add(1)))
}

/// Workflow #429 R-12: `page`, an instance's `RoutineSummary` page by day, with only the buckets
/// that overlap `span`, the summary's own local day. The page's other members stay as the read
/// answered them.
pub(crate) fn within_day(mut page: RoutineSummaryPage, span: (u64, u64)) -> RoutineSummaryPage {
    let (from, until) = span;
    for instance in &mut page.instances {
        instance
            .buckets
            .retain(|bucket| bucket.start_unix_ms < until && bucket.end_unix_ms > from);
    }
    page
}

/// `YYYY-MM-DD` of a local day, days since 1970-01-01 (the proleptic Gregorian calendar).
pub(crate) fn date_text(day: u64) -> String {
    let days = i64::try_from(day)
        .unwrap_or(i64::MAX)
        .saturating_add(EPOCH_FROM_MARCH_ZERO);
    let era = days.div_euclid(146_097);
    let of_era = days.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * of_year + 2) / 153;
    let day_of_month = of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day_of_month:02}")
}

/// Whether `row` is a summary the rule `rule_id` made.
fn is_summary_of(row: &WakeRow, rule_id: &str) -> bool {
    !row.test && row.kind == KIND_DAILY_SUMMARY && row.rule.as_deref() == Some(rule_id)
}

/// Whether the rows hold the rule's summary of its local day `day`, whatever became of it.
pub(crate) fn made_on(rows: &[WakeRow], rule_id: &str, at: SummaryAt, day: u64) -> bool {
    rows.iter()
        .any(|row| is_summary_of(row, rule_id) && day_of(at, row.created_at_unix_ms) == day)
}

/// The date a summary's message names where an alias stands: its rule's local day, by the rule in
/// effect; `None` when no `daily_summary` rule of that id is configured now, and the message then
/// names none.
pub(crate) fn row_date(row: &WakeRow, rules: &[WakeRule]) -> Option<String> {
    let at = rules
        .iter()
        .find(|rule| row.rule.as_deref() == Some(rule.id.as_str()))?
        .summary_at?;
    Some(date_text(day_of(at, row.created_at_unix_ms)))
}

/// What a summary's `summary` member is made of.
pub(crate) struct SummaryInput<'a> {
    pub(crate) rule_id: &'a str,
    pub(crate) at: SummaryAt,
    pub(crate) day: u64,
    /// The Runtime's owner epoch, as its identifier is written.
    pub(crate) owner_epoch: String,
    pub(crate) started_at_unix_ms: u64,
    /// One entry per instance of the rule: what `RoutineSummary` answers for it.
    pub(crate) instances: Vec<Value>,
    /// The rows of `host.agent_wake` before this summary's own.
    pub(crate) rows: &'a [WakeRow],
}

/// The body's `summary` member: the date, the Runtime's epoch and start, each instance's answer,
/// the day's wakes by kind and the state each is in now, the rule's newest earlier summary the
/// rows hold (`null` when they hold none), and the days after it that have none (`null` when
/// there are none, or nothing to count from).
pub(crate) fn summary_member(input: SummaryInput<'_>) -> Value {
    let SummaryInput {
        rule_id,
        at,
        day,
        owner_epoch,
        started_at_unix_ms,
        instances,
        rows,
    } = input;
    let mut by_kind = BTreeMap::<(&str, &str), u64>::new();
    for row in rows
        .iter()
        .filter(|row| day_of(at, row.created_at_unix_ms) == day)
    {
        *by_kind
            .entry((row.kind.as_str(), row.state.as_str()))
            .or_default() += 1;
    }
    let wakes = by_kind
        .into_iter()
        .map(|((kind, state), count)| json!({"kind": kind, "state": state, "count": count}))
        .collect::<Vec<_>>();
    let previous = rows
        .iter()
        .filter(|row| is_summary_of(row, rule_id))
        .map(|row| (day_of(at, row.created_at_unix_ms), row))
        .filter(|(earlier, _)| *earlier < day)
        .max_by_key(|(earlier, row)| (*earlier, row.created_at_unix_ms));
    let skipped = previous.and_then(|(earlier, _)| {
        let first = earlier + 1;
        (first < day).then(|| {
            json!({
                "from": date_text(first),
                "through": date_text(day - 1),
                "days": day - first,
            })
        })
    });
    let previous = previous
        .map(|(earlier, row)| json!({"date": date_text(earlier), "state": row.state.as_str()}));
    json!({
        "date": date_text(day),
        "at_minute_of_day": at.minute_of_day,
        "utc_offset_minutes": at.utc_offset_minutes,
        "runtime": {"owner_epoch": owner_epoch, "started_at_unix_ms": started_at_unix_ms},
        "instances": instances,
        "wakes": wakes,
        "previous": previous,
        "skipped": skipped,
    })
}
