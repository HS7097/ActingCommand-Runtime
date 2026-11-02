// SPDX-License-Identifier: AGPL-3.0-only

//! The guards of an automatic wake (Workflow #432 AGT-2; `contracts/agent-wake.md`, "Rules and
//! guards"): a cool-down per rule, instance and code, and a day cap per rule and over all rules,
//! each day being the host's local day. Every count is read from the rows of `host.agent_wake`,
//! which a start restores, so a restart forgets none of them. A person's test is held by none and
//! counts toward none. Pure: the worker hands the rows, the time and the day of an instant.

use crate::agent_wake::rows::WakeRow;
use actingcommand_contract::outcome::CodeStr;
use actingcommand_runtime_config::agent_wake::WakeRule;

/// One day in milliseconds.
pub(crate) const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

/// The local day of `unix_ms` in a zone `offset_ms` east of UTC: whole days since the epoch.
pub(crate) fn local_day(unix_ms: u64, offset_ms: i64) -> u64 {
    let shifted = i128::from(unix_ms) + i128::from(offset_ms);
    u64::try_from(shifted.div_euclid(i128::from(DAY_MS))).unwrap_or(0)
}

/// The wakes of one local day the rows hold: of one rule, and of every rule together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DayUsage {
    pub(crate) rule: u64,
    pub(crate) all: u64,
}

/// Why a wake a rule chose is not made. Each is counted in the status, never recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    /// The same rule woke for the same instance and code within its `cooldown_ms`.
    Cooldown,
    /// The rule's `daily_cap` is used up today.
    RuleCap,
    /// `budget.daily_cap` is used up today.
    BudgetCap,
}

/// A wake a rule chose: the rule, the instance and the trigger's code.
pub(crate) struct Prospect<'a> {
    pub(crate) rule: &'a WakeRule,
    pub(crate) instance_alias: Option<&'a str>,
    pub(crate) spelling: &'a str,
}

/// The automatic rows of the local day `today`; `day_of` gives an instant's local day.
fn of_today<'a>(
    rows: &'a [WakeRow],
    today: u64,
    day_of: &'a dyn Fn(u64) -> Option<u64>,
) -> impl Iterator<Item = &'a WakeRow> + 'a {
    rows.iter()
        .filter(move |row| !row.test && day_of(row.created_at_unix_ms) == Some(today))
}

fn count<T>(rows: impl Iterator<Item = T>) -> u64 {
    u64::try_from(rows.count()).unwrap_or(u64::MAX)
}

/// How many wakes of `today` the rows hold, of the rule `rule_id` and of every rule.
pub(crate) fn day_usage(
    rows: &[WakeRow],
    rule_id: &str,
    today: u64,
    day_of: &dyn Fn(u64) -> Option<u64>,
) -> DayUsage {
    DayUsage {
        rule: count(
            of_today(rows, today, day_of).filter(|row| row.rule.as_deref() == Some(rule_id)),
        ),
        all: count(of_today(rows, today, day_of)),
    }
}

/// What holds `prospect` back at `now_unix_ms`, `None` when the wake may be made: its cool-down,
/// then its rule's cap, then the budget's.
pub(crate) fn hold(
    rows: &[WakeRow],
    prospect: &Prospect<'_>,
    budget_daily_cap: u64,
    now_unix_ms: u64,
    today: u64,
    day_of: &dyn Fn(u64) -> Option<u64>,
) -> Option<Hold> {
    let rule = prospect.rule;
    let cooling = rows.iter().any(|row| {
        !row.test
            && row.rule.as_deref() == Some(rule.id.as_str())
            && row.instance_alias.as_deref() == prospect.instance_alias
            && row.code.as_ref().map(CodeStr::as_str) == Some(prospect.spelling)
            && now_unix_ms < row.created_at_unix_ms.saturating_add(rule.cooldown_ms)
    });
    if cooling {
        return Some(Hold::Cooldown);
    }
    let used = day_usage(rows, &rule.id, today, day_of);
    if used.rule >= rule.daily_cap {
        return Some(Hold::RuleCap);
    }
    if used.all >= budget_daily_cap {
        return Some(Hold::BudgetCap);
    }
    None
}

/// Whether a cap is used up on `today`: the budget's, or any rule's.
pub(crate) fn exhausted_today(
    rows: &[WakeRow],
    rules: &[WakeRule],
    budget_daily_cap: u64,
    today: u64,
    day_of: &dyn Fn(u64) -> Option<u64>,
) -> bool {
    count(of_today(rows, today, day_of)) >= budget_daily_cap
        || rules
            .iter()
            .any(|rule| day_usage(rows, &rule.id, today, day_of).rule >= rule.daily_cap)
}
