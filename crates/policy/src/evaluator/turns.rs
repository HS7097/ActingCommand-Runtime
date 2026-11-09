// SPDX-License-Identifier: AGPL-3.0-only

//! SS-2f (register §1.288; Alice's answer K2 to question 137 of 11-02 19:4x; design
//! `rd-1031/design-acm-price-addendum2.1.md` §5.4): the business tier across instances.
//!
//! Each game values its candidates in its own unit, so a net benefit `B` compares only within one
//! instance. Across instances the business tier takes turns instead: the instance whose most recent
//! policy dispatch is the earliest goes first ([`super::allocation_order`]), and within an instance
//! the order of before stays. An instance's most recent policy dispatch is the latest
//! `last_dispatched_unix_ms` among its task snapshots, which every evaluation already holds (the
//! Runtime projects one per task and instance from the policy dispatches it admitted, the fact
//! refresh's included); nothing new is read.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::TaskRuntimeSnapshot;

/// Each instance's most recent policy dispatch, by instance id. An instance without one was never
/// dispatched.
#[derive(Debug)]
pub(super) struct InstanceTurns<'a> {
    latest: BTreeMap<&'a str, u64>,
}

impl<'a> InstanceTurns<'a> {
    /// The latest dispatch instant of each instance among `tasks`.
    pub(super) fn new(tasks: &'a [TaskRuntimeSnapshot]) -> Self {
        let mut latest = BTreeMap::<&str, u64>::new();
        for state in tasks {
            if let Some(dispatched) = state.last_dispatched_unix_ms {
                latest
                    .entry(state.instance_id.as_str())
                    .and_modify(|at| *at = (*at).max(dispatched))
                    .or_insert(dispatched);
            }
        }
        Self { latest }
    }

    /// The order of two instances' turns: the earlier most recent policy dispatch first, an
    /// instance never dispatched before every dispatched one; at the same instant (two instances
    /// never dispatched included) the smaller instance id first. Equal only for one instance.
    pub(super) fn order(&self, left: &str, right: &str) -> Ordering {
        self.latest
            .get(left)
            .cmp(&self.latest.get(right))
            .then_with(|| left.cmp(right))
    }
}
