// SPDX-License-Identifier: AGPL-3.0-only

//! The landing history of single-region presses (Workflow #406 T12; plan Q15, Q16;
//! `contracts/resource-declarations.md`, "Touch sampling"). Per instance and per box (the
//! sampling region's `x`, `y`, `width` and `height`, in device pixels), the points its presses
//! landed on, each with the instance's use counter at its last use; "used" is the same pixel in
//! the same box. A press with several centre-hotspot candidates lands on the one the kernel's
//! least-recent rule picks ([`least_recent_candidate`]), and that choice is recorded as the
//! newest use. A box keeps at most `touch.history.points_per_box` points (the newest; a
//! forgotten point counts as unused) and an instance at most `touch.history.max_boxes` boxes
//! (the box unused longest is forgotten).
//!
//! Memory only: never read from or written to the ledger, so a restart starts with an empty
//! history. The policy runs and the direct runs of an instance share its history.

use actingcommand_contract::InputSamplingRegion;
use actingcommand_execution_kernel::least_recent_candidate;
use std::collections::BTreeMap;

/// The bounds of the history in effect (`touch.history.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TouchHistoryLimits {
    /// The most points one box keeps.
    pub(crate) points_per_box: usize,
    /// The most boxes one instance keeps.
    pub(crate) max_boxes: usize,
}

/// A box: the sampling region's `x`, `y`, `width` and `height`.
type BoxKey = (i32, i32, i32, i32);

fn box_key(region: &InputSamplingRegion) -> BoxKey {
    (region.x(), region.y(), region.width(), region.height())
}

/// One box's landing points.
#[derive(Debug, Default)]
struct BoxHistory {
    /// The instance's use counter at the box's newest use.
    last_use: u64,
    /// Each kept point with the instance's use counter at its last use.
    points: BTreeMap<(i32, i32), u64>,
}

/// One instance's boxes and its use counter.
#[derive(Debug, Default)]
struct InstanceHistory {
    uses: u64,
    boxes: BTreeMap<BoxKey, BoxHistory>,
}

impl InstanceHistory {
    /// Forgets what the bounds do not keep: in each box its oldest points beyond
    /// `points_per_box`, then the boxes unused longest beyond `max_boxes`.
    fn bound(&mut self, limits: TouchHistoryLimits) {
        let points_per_box = limits.points_per_box.max(1);
        for kept in self.boxes.values_mut() {
            while kept.points.len() > points_per_box {
                let Some(oldest) = kept
                    .points
                    .iter()
                    .min_by_key(|(_, used)| **used)
                    .map(|(point, _)| *point)
                else {
                    break;
                };
                kept.points.remove(&oldest);
            }
        }
        let max_boxes = limits.max_boxes.max(1);
        while self.boxes.len() > max_boxes {
            let Some(oldest) = self
                .boxes
                .iter()
                .min_by_key(|(_, kept)| kept.last_use)
                .map(|(key, _)| *key)
            else {
                break;
            };
            self.boxes.remove(&oldest);
        }
    }

    /// Records `point` of the box `key` as the instance's newest use.
    fn record(&mut self, key: BoxKey, point: (i32, i32)) {
        self.uses = self.uses.saturating_add(1);
        let kept = self.boxes.entry(key).or_default();
        kept.last_use = self.uses;
        kept.points.insert(point, self.uses);
    }
}

/// Every instance's landing history (see the module header).
#[derive(Debug, Default)]
pub(crate) struct TouchHistory {
    instances: BTreeMap<String, InstanceHistory>,
}

impl TouchHistory {
    /// The candidate a press in `region` on `instance` lands on, among `candidates` in draw
    /// order, by the least-recent rule over the box's history; the chosen point is recorded as
    /// the instance's newest use, within `limits`.
    pub(crate) fn choose(
        &mut self,
        instance: &str,
        region: &InputSamplingRegion,
        candidates: &[(i32, i32)],
        limits: TouchHistoryLimits,
    ) -> usize {
        let key = box_key(region);
        let history = self.instances.entry(instance.to_owned()).or_default();
        history.bound(limits);
        let kept = history.boxes.get(&key);
        let chosen = least_recent_candidate(candidates, |point| {
            kept.and_then(|kept| kept.points.get(&point).copied())
        });
        if let Some(&point) = candidates.get(chosen) {
            history.record(key, point);
            history.bound(limits);
        }
        chosen
    }

    /// Tests only: the points `instance` keeps for the box `region`, oldest use first.
    #[cfg(test)]
    pub(crate) fn points(&self, instance: &str, region: &InputSamplingRegion) -> Vec<(i32, i32)> {
        let Some(kept) = self
            .instances
            .get(instance)
            .and_then(|history| history.boxes.get(&box_key(region)))
        else {
            return Vec::new();
        };
        let mut points = kept
            .points
            .iter()
            .map(|(point, used)| (*used, *point))
            .collect::<Vec<_>>();
        points.sort_unstable();
        points.into_iter().map(|(_, point)| point).collect()
    }
}
