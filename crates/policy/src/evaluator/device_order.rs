// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #437 ORD-1b (`contracts/scheduling/README.md`, "Order within one device"; the owner's
//! rulings on board #437 as the coordinator recorded them, not the owner's words): the selection
//! among the members of one device.
//!
//! With the order inputs ([`super::DeviceOrder`]), a device whose key the map names for two or
//! more aliases selects at the place of its first contender in the allocation order, step by
//! step, over that order: nothing is sorted anew.
//!
//! - **Contenders.** Every candidate of the device's members from that place on, but one the
//!   Runtime holds whole ([`super::InstanceHolds`]), one whose member is held as the cycle starts
//!   (`FactRefreshInputs::instance_holds`) and one the device's chain handover holds. Those take
//!   their turns as before.
//! - **Each contender** has its expected duration `E`, its limit `L` (its task's
//!   `max_queue_wait_ms`, else the order's default), its start `S` (the Runtime's, else the
//!   evaluation's time) and its deadline `S + L`.
//! - **The binding waiter** is, of the contenders of every member but the one in front (all
//!   contenders when none is in front), the one with the earliest deadline, then the earlier in
//!   the allocation order.
//! - **The pick.** The front member's first contender when it is the device's chain successor,
//!   or when no other member has a contender; else its first contender, in its own order, that
//!   ends by the binding waiter's deadline (`now + E ≤ deadline`); else the binding waiter.
//! - **Tried** with the steps of any candidate. Selected, it wins the device and every other
//!   contender waits for it; deferred, it keeps its own deferral and the device picks again from
//!   the contenders left.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::outcome::Outcome;

use super::chain_handover::Handover;
use super::{
    ConsumptionAmounts, DecisionReason, DeviceGroups, DeviceOrder, FACT_REFRESH_TASK_ID,
    InstanceHold, InstanceHolds, PlacementCandidate, PolicyEvaluationError, PolicyEvaluationResult,
    RefreshGate, TaskSpec, TaskWork, reason_with,
};
use crate::codes::{DeviceGroupWait, PolicyCode};

/// One evaluation's order inputs, read against its device map.
pub(super) struct DeviceOrdering<'a> {
    groups: &'a DeviceGroups,
    order: &'a DeviceOrder,
    /// The member in front of each device of several, by device key.
    fronts: BTreeMap<&'a str, &'a str>,
    /// The device keys the map names for two or more aliases.
    shared: BTreeSet<&'a str>,
    /// Each catalog task by id, for its declared limit.
    tasks: &'a BTreeMap<&'a str, (usize, &'a TaskSpec)>,
    now: u64,
}

impl<'a> DeviceOrdering<'a> {
    /// `None` without order inputs: the device map alone selects as MAPP-1 does. Two members of
    /// one device named in front refuse the evaluation; a name in front of an instance alone on
    /// its device orders nothing.
    pub(super) fn new(
        groups: &'a DeviceGroups,
        tasks: &'a BTreeMap<&'a str, (usize, &'a TaskSpec)>,
        now: u64,
    ) -> PolicyEvaluationResult<Option<Self>> {
        let Some(order) = groups.order.as_ref() else {
            return Ok(None);
        };
        let mut members = BTreeMap::<&str, usize>::new();
        for device in groups.devices.values() {
            *members.entry(device.as_str()).or_default() += 1;
        }
        let shared = members
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(device, _)| device)
            .collect::<BTreeSet<_>>();
        let mut fronts = BTreeMap::new();
        for alias in &order.fronts {
            let device = groups.device(alias);
            if !shared.contains(device) {
                continue;
            }
            if let Some(other) = fronts.insert(device, alias.as_str()) {
                return Err(PolicyEvaluationError::invalid(format!(
                    "the members '{other}' and '{alias}' of one device are both named in front"
                )));
            }
        }
        Ok(Some(Self {
            groups,
            order,
            fronts,
            shared,
            tasks,
            now,
        }))
    }

    /// The device key of `candidate`'s device when its selection starts there: a device of
    /// several, not ordered yet this round nor won, and the candidate one of its contenders.
    pub(super) fn starts(
        &self,
        candidate: &PlacementCandidate,
        contenders: &Contenders<'_>,
        ordered: &BTreeSet<String>,
        selected_devices: &BTreeMap<String, (String, String)>,
    ) -> Option<String> {
        let device = self.groups.device(&candidate.instance_id);
        (self.shared.contains(device)
            && !ordered.contains(device)
            && !selected_devices.contains_key(device)
            && contenders.admits(candidate, device))
        .then(|| device.to_owned())
    }

    /// The contender at `place` of the allocation order with its duration, limit and deadline.
    fn seat(&self, place: usize, candidate: &PlacementCandidate) -> Seat {
        let limit_ms = self
            .tasks
            .get(candidate.task_id.as_str())
            .and_then(|(_, task)| task.max_queue_wait_ms)
            .unwrap_or(self.order.default_max_queue_wait_ms);
        let start = self
            .order
            .wait_starts
            .get(&(candidate.task_id.clone(), candidate.instance_id.clone()))
            .copied()
            .unwrap_or(self.now);
        Seat {
            place,
            work_index: candidate.work_index,
            instance_id: candidate.instance_id.clone(),
            expected_ms: candidate.expected_duration_ms,
            limit_ms,
            deadline: start.saturating_add(limit_ms),
        }
    }
}

/// What keeps a candidate of a device of several out of its device's selection.
pub(super) struct Contenders<'a> {
    /// The Runtime's whole-instance holds (UPD-1, a switch cooldown).
    pub(super) instance_holds: &'a InstanceHolds,
    /// The members held as the cycle starts (a pause, a fence, a person's hold, a holder of the
    /// device, a takeover cooldown); `None` without the fact refresh inputs.
    pub(super) cycle_holds: Option<&'a BTreeMap<String, InstanceHold>>,
    /// The round's chain handovers, by device key.
    pub(super) handovers: &'a BTreeMap<String, Handover>,
}

impl Contenders<'_> {
    /// Whether `candidate` of `device` contends: held by none of the three.
    fn admits(&self, candidate: &PlacementCandidate, device: &str) -> bool {
        self.instance_holds
            .held(&candidate.task_id, &candidate.instance_id)
            .is_none()
            && !self
                .cycle_holds
                .is_some_and(|holds| holds.contains_key(&candidate.instance_id))
            && !self
                .handovers
                .get(device)
                .is_some_and(|handover| handover.holds(candidate.work_index))
    }

    /// Whether `work_index` is `device`'s chain successor.
    fn is_successor(&self, device: &str, work_index: usize) -> bool {
        self.handovers
            .get(device)
            .is_some_and(|handover| handover.is_successor(work_index))
    }
}

/// One contender of a pass.
#[derive(Debug)]
struct Seat {
    /// Its place in the allocation order.
    place: usize,
    work_index: usize,
    instance_id: String,
    /// `E`, the duration its dispatch reserves.
    expected_ms: u64,
    /// `L`.
    limit_ms: u64,
    /// `S + L`.
    deadline: u64,
}

/// One pass of a device's selection: its contenders and its pick.
#[derive(Debug)]
struct Pass {
    /// The member in front of the device, if any.
    front: Option<String>,
    seats: Vec<Seat>,
    /// The binding waiter: its index in `seats`.
    binding: Option<usize>,
    /// The places of the front's contenders walked before a front pick: none of them fits.
    walked: BTreeSet<usize>,
    /// The place of the pick.
    pick: usize,
}

/// The selection of one device of several in a round.
#[derive(Debug)]
pub(super) struct DeviceRound {
    device: String,
    /// The place of its first contender in the allocation order.
    start: usize,
    /// The latest pass, while its pick is tried and once it won the device.
    pass: Option<Pass>,
}

impl DeviceRound {
    pub(super) fn new(device: String, start: usize) -> Self {
        Self {
            device,
            start,
            pass: None,
        }
    }

    /// The place of the device's next pick in `queue`, the allocation order with every candidate
    /// taken so far gone; `None` once the device is won or no contender is left.
    pub(super) fn next(
        &mut self,
        ordering: &DeviceOrdering<'_>,
        contenders: &Contenders<'_>,
        queue: &[Option<PlacementCandidate>],
        selected_devices: &BTreeMap<String, (String, String)>,
    ) -> Option<usize> {
        if selected_devices.contains_key(&self.device) {
            return None;
        }
        let seats = queue
            .iter()
            .enumerate()
            .skip(self.start)
            .filter_map(|(place, slot)| slot.as_ref().map(|candidate| (place, candidate)))
            .filter(|(_, candidate)| {
                ordering.groups.device(&candidate.instance_id) == self.device
                    && contenders.admits(candidate, &self.device)
            })
            .map(|(place, candidate)| ordering.seat(place, candidate))
            .collect::<Vec<_>>();
        let front = ordering
            .fronts
            .get(self.device.as_str())
            .map(|alias| (*alias).to_owned());
        let binding = seats
            .iter()
            .enumerate()
            .filter(|(_, seat)| front.as_deref() != Some(seat.instance_id.as_str()))
            .min_by_key(|(_, seat)| (seat.deadline, seat.place))
            .map(|(index, _)| index);
        let mut walked = BTreeSet::new();
        let mut pick = None;
        if let Some(front) = front.as_deref() {
            let own = seats
                .iter()
                .filter(|seat| seat.instance_id == front)
                .collect::<Vec<_>>();
            match (own.first(), binding) {
                (None, _) => {}
                (Some(first), None) => pick = Some(first.place),
                // Amendment 1: the device's chain successor goes first whether or not it fits.
                (Some(first), Some(_))
                    if contenders.is_successor(&self.device, first.work_index) =>
                {
                    pick = Some(first.place);
                }
                (Some(_), Some(index)) => {
                    let deadline = seats[index].deadline;
                    for seat in &own {
                        if ordering.now.saturating_add(seat.expected_ms) <= deadline {
                            pick = Some(seat.place);
                            break;
                        }
                        walked.insert(seat.place);
                    }
                }
            }
        }
        let Some(pick) = pick.or_else(|| binding.map(|index| seats[index].place)) else {
            self.pass = None;
            return None;
        };
        self.pass = Some(Pass {
            front,
            seats,
            binding,
            walked,
            pick,
        });
        Some(pick)
    }

    /// Once the device is won, every contender of the last pass but its pick waits for it: one
    /// of the winner's own member as behind its instance's winner (with its fit when it was
    /// walked), one of the member in front, after a switch, with the time it needs and the time
    /// the binding waiter has left, and every other one with its limit and its own time left.
    pub(super) fn finish(
        self,
        ordering: &DeviceOrdering<'_>,
        queue: &mut [Option<PlacementCandidate>],
        rows: &mut Rows<'_>,
    ) {
        let Some(pass) = self.pass else {
            return;
        };
        let selected_devices = rows.selected_devices;
        let Some((winner, winner_task)) = selected_devices.get(&self.device) else {
            return;
        };
        let now = ordering.now;
        let binding_left = pass
            .binding
            .map(|index| pass.seats[index].deadline.saturating_sub(now));
        for seat in &pass.seats {
            if seat.place == pass.pick {
                continue;
            }
            let Some(candidate) = queue.get_mut(seat.place).and_then(Option::take) else {
                continue;
            };
            if candidate.instance_id == *winner {
                let fit = binding_left
                    .filter(|_| pass.walked.contains(&seat.place))
                    .map(|left| (seat.expected_ms, left));
                super::settle_behind_instance(
                    &candidate,
                    fit,
                    rows.selected_blocks,
                    rows.amounts,
                    rows.work,
                    rows.refresh_gates,
                );
                continue;
            }
            let wait = match binding_left {
                Some(left) if pass.front.as_deref() == Some(candidate.instance_id.as_str()) => {
                    Wait::Fit {
                        required_ms: seat.expected_ms,
                        remaining_ms: left,
                    }
                }
                _ => Wait::Limit {
                    limit_ms: seat.limit_ms,
                    remaining_ms: seat.deadline.saturating_sub(now),
                },
            };
            rows.work[candidate.work_index].reasons.push(waiting_reason(
                &candidate.instance_id,
                winner,
                winner_task,
                wait,
            ));
            if candidate.task_id == FACT_REFRESH_TASK_ID {
                rows.refresh_gates.insert(
                    candidate.work_index,
                    RefreshGate::Temporary {
                        gate: Some(PolicyCode::DeviceGroupWaiting.code()),
                        wake: None,
                    },
                );
            }
        }
    }
}

/// What [`DeviceRound::finish`] writes to.
pub(super) struct Rows<'r> {
    pub(super) work: &'r mut [TaskWork],
    pub(super) refresh_gates: &'r mut BTreeMap<usize, RefreshGate>,
    pub(super) selected_devices: &'r BTreeMap<String, (String, String)>,
    pub(super) selected_blocks: &'r BTreeMap<String, (String, String)>,
    pub(super) amounts: &'r ConsumptionAmounts,
}

/// The values a waiting contender's `device_group_waiting` carries.
enum Wait {
    /// A contender of a member not in front: its limit and its own time left.
    Limit { limit_ms: u64, remaining_ms: u64 },
    /// A contender of the member in front when another member's pick switches the device: the
    /// time its run needs and the time the binding waiter has left.
    Fit { required_ms: u64, remaining_ms: u64 },
}

/// `device_group_waiting` `selected` with the order's values: the candidate of `instance_id`
/// waits for `winner`'s candidate of `winner_task`.
fn waiting_reason(
    instance_id: &str,
    winner: &str,
    winner_task: &str,
    wait: Wait,
) -> DecisionReason {
    let outcome = Outcome::new(PolicyCode::DeviceGroupWaiting)
        .instance_id(instance_id)
        .alias(winner)
        .device_group_wait(DeviceGroupWait::Selected)
        .task_id(winner_task);
    let (outcome, values) = match wait {
        Wait::Limit {
            limit_ms,
            remaining_ms,
        } => (
            outcome.limit_ms(limit_ms).remaining_ms(remaining_ms),
            format!("limit_ms={limit_ms} remaining_ms={remaining_ms}"),
        ),
        Wait::Fit {
            required_ms,
            remaining_ms,
        } => (
            outcome.required_ms(required_ms).remaining_ms(remaining_ms),
            format!("required_ms={required_ms} remaining_ms={remaining_ms}"),
        ),
    };
    reason_with(
        outcome,
        format!(
            "device_group_wait=selected alias={winner} task_id={winner_task} {values}: another \
             member of the device won it for the evaluation round"
        ),
    )
}

/// `instance_already_selected` of a contender of the member in front walked before the pick: its
/// run needs `required_ms`, and the binding waiter has `remaining_ms` left.
pub(super) fn walked_reason(required_ms: u64, remaining_ms: u64) -> DecisionReason {
    reason_with(
        Outcome::new(PolicyCode::InstanceAlreadySelected)
            .required_ms(required_ms)
            .remaining_ms(remaining_ms),
        format!(
            "required_ms={required_ms} remaining_ms={remaining_ms}: a task of the instance that \
             ends before the waiting task's limit runs first; this one would not"
        ),
    )
}
