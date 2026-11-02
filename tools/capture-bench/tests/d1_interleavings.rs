// SPDX-License-Identifier: AGPL-3.0-only

//! D1 item 1 (plan v2.1 §4.4 确定性交错): every feasible interleaving of a preview sample
//! (barriers g0-g5 and the pump) with a registration (r0-r2), a gate signal (s0-s1), a
//! dispatch with its bounded retry (d0-d1) and a whole-run STOP, each logical thread a step
//! machine driven without blocking on one thread. Every interleaving is checked for I1-I4 and the
//! lock order; the counts and classes are printed (the CI report step runs this with
//! `--nocapture`).

mod hub_fixture;

use actingcommand_capture_bench::hub::{
    DispatchOp, GateOp, GateTarget, Hub, Mode, Op, RecordKind, SampleOp, SeqRecord, Step,
};
use actingcommand_capture_bench::locks::LockOwner;
use actingcommand_capture_bench::model::{GateKind, GlobalGate, PathKind};
use hub_fixture::{Fixture, fixture, instant_script};
use std::collections::BTreeMap;
use std::sync::Arc;

trait Logical {
    /// One step; false when it is blocked (nothing changed).
    fn step(&mut self, hub: &Hub) -> bool;
    fn done(&self) -> bool;
}

struct Wrap<O: Op> {
    op: O,
    out: Option<O::Output>,
}

impl<O: Op> Logical for Wrap<O> {
    fn step(&mut self, hub: &Hub) -> bool {
        if self.out.is_some() {
            return false;
        }
        match self.op.step(hub, Mode::NonBlocking) {
            Step::Progress => true,
            Step::Blocked => false,
            Step::Done(output) => {
                self.out = Some(output);
                true
            }
        }
    }

    fn done(&self) -> bool {
        self.out.is_some()
    }
}

fn wrap<O: Op + 'static>(op: O) -> Box<dyn Logical> {
    Box::new(Wrap { op, out: None })
}

type Make = dyn Fn(&Fixture) -> Vec<Box<dyn Logical>>;
type Classify = dyn Fn(&[SeqRecord]) -> String;

/// Explores every feasible schedule by replaying choice prefixes (stateless enumeration).
fn explore(pair: &str, make: &Make, classify: &Classify) -> (usize, BTreeMap<String, usize>) {
    let mut pending: Vec<Vec<usize>> = vec![Vec::new()];
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut schedules = 0;
    while let Some(prefix) = pending.pop() {
        let fx = fixture(instant_script(), false);
        let mut ops = make(&fx);
        let mut trace: Vec<usize> = Vec::new();
        let mut feasible = true;
        loop {
            if ops.iter().all(|op| op.done()) {
                break;
            }
            if trace.len() < prefix.len() {
                let choice = prefix[trace.len()];
                if ops[choice].done() || !ops[choice].step(&fx.hub) {
                    feasible = false;
                    break;
                }
                trace.push(choice);
                continue;
            }
            let candidates: Vec<usize> =
                (0..ops.len()).filter(|index| !ops[*index].done()).collect();
            let mut chosen = None;
            for (position, candidate) in candidates.iter().enumerate() {
                if ops[*candidate].step(&fx.hub) {
                    chosen = Some((*candidate, position));
                    break;
                }
            }
            let Some((choice, position)) = chosen else {
                panic!("{pair}: every logical thread is blocked after {trace:?} (a deadlock)");
            };
            for alternative in &candidates[position + 1..] {
                let mut next = trace.clone();
                next.push(*alternative);
                pending.push(next);
            }
            trace.push(choice);
        }
        if !feasible {
            continue;
        }
        schedules += 1;
        let checker = LockOwner::new("checker");
        let violations = fx.hub.violations(&checker);
        assert!(
            violations.is_empty(),
            "{pair}: schedule {trace:?} violates the invariants: {violations:?}"
        );
        *classes.entry(classify(&fx.sink.records())).or_default() += 1;
    }
    println!("{pair}: {schedules} interleavings, classes {classes:?}");
    (schedules, classes)
}

fn first(records: &[SeqRecord], matches: impl Fn(&RecordKind) -> bool) -> Option<u64> {
    records
        .iter()
        .find(|record| matches(&record.kind))
        .map(|record| record.seq)
}

fn sample(_fx: &Fixture) -> Box<dyn Logical> {
    wrap(SampleOp::new(0, PathKind::Nemu, LockOwner::new("sampler")))
}

#[test]
fn sample_x_register_never_grants_past_a_pending_registration() {
    let make = |fx: &Fixture| {
        vec![
            sample(fx),
            wrap(
                DispatchOp::new(0, LockOwner::new("registrar"), fx.hub.clock.clone(), None)
                    .register_only(),
            ),
        ]
    };
    let classify = |records: &[SeqRecord]| {
        let grant = first(records, |kind| matches!(kind, RecordKind::Grant { .. }));
        let register = first(records, |kind| matches!(kind, RecordKind::Register));
        match (grant, register) {
            (Some(grant), Some(register)) if grant < register => "inflight_before_register",
            (Some(_), Some(_)) => "grant_after_register",
            (None, Some(_)) => "skipped_demand",
            _ => "other",
        }
        .to_owned()
    };
    let (schedules, classes) = explore("sample x register", &make, &classify);
    assert!(schedules > 2);
    assert!(
        classes.contains_key("inflight_before_register"),
        "{classes:?}"
    );
    assert!(classes.contains_key("skipped_demand"), "{classes:?}");
    assert!(!classes.contains_key("grant_after_register"), "{classes:?}");
}

fn gate_classify(records: &[SeqRecord]) -> String {
    let grant = first(records, |kind| matches!(kind, RecordKind::Grant { .. }));
    let gate = first(records, |kind| matches!(kind, RecordKind::GateSet { .. }));
    let publish = first(records, |kind| matches!(kind, RecordKind::Publish { .. }));
    let discard = first(records, |kind| matches!(kind, RecordKind::Discard { .. }));
    match (grant, gate, publish, discard) {
        (None, Some(_), _, _) => "skipped_gate",
        (Some(_), Some(gate), Some(publish), _) if publish < gate => "published_before_gate",
        (Some(grant), Some(gate), None, Some(_)) if grant < gate => "inflight_then_discarded",
        _ => "other",
    }
    .to_owned()
}

#[test]
fn sample_x_gate_never_grants_after_the_gate_and_never_publishes_a_stale_sample() {
    let make = |fx: &Fixture| {
        vec![
            sample(fx),
            wrap(GateOp::with_owner(
                Arc::new(GateTarget::Instance(0, GateKind::Blackout)),
                true,
                LockOwner::new("gate"),
            )),
        ]
    };
    let (schedules, classes) = explore("sample x gate", &make, &gate_classify);
    assert!(schedules > 2);
    for class in [
        "skipped_gate",
        "published_before_gate",
        "inflight_then_discarded",
    ] {
        assert!(classes.contains_key(class), "{class} missing: {classes:?}");
    }
    assert!(!classes.contains_key("other"), "{classes:?}");
}

#[test]
fn sample_x_stop_behaves_as_a_gate_for_the_whole_run() {
    let make = |fx: &Fixture| {
        let _ = fx;
        vec![
            sample(fx),
            wrap(GateOp::with_owner(
                Arc::new(GateTarget::Global(GlobalGate::RunStop)),
                true,
                LockOwner::new("stop"),
            )),
        ]
    };
    let (schedules, classes) = explore("sample x stop", &make, &gate_classify);
    assert!(schedules > 2);
    for class in [
        "skipped_gate",
        "published_before_gate",
        "inflight_then_discarded",
    ] {
        assert!(classes.contains_key(class), "{class} missing: {classes:?}");
    }
    assert!(!classes.contains_key("other"), "{classes:?}");
}

#[test]
fn sample_x_dispatch_retry_waits_at_most_one_call_and_is_granted_by_the_pump_or_contended() {
    let make = |fx: &Fixture| {
        vec![
            sample(fx),
            wrap(
                DispatchOp::new(0, LockOwner::new("dispatch"), fx.hub.clock.clone(), None)
                    .with_max_attempts(2),
            ),
        ]
    };
    let classify = |records: &[SeqRecord]| {
        let pumped = records
            .iter()
            .any(|record| matches!(record.kind, RecordKind::FormalGrant { by_pump: true, .. }));
        let direct = records
            .iter()
            .any(|record| matches!(record.kind, RecordKind::FormalGrant { by_pump: false, .. }));
        let withdrawn = records
            .iter()
            .any(|record| matches!(record.kind, RecordKind::Withdraw { .. }));
        let granted = first(records, |kind| matches!(kind, RecordKind::Grant { .. }));
        let register = first(records, |kind| matches!(kind, RecordKind::Register));
        match (pumped, direct, withdrawn) {
            (true, false, false) => "granted_by_pump".to_owned(),
            (false, true, false) if granted.is_none() => {
                "sample_skipped_demand_then_direct".to_owned()
            }
            (false, true, false) => format!(
                "direct_{}",
                if granted < register {
                    "after_sample"
                } else {
                    "sample_later"
                }
            ),
            (false, false, true) => "dispatch_contended".to_owned(),
            other => format!("other {other:?}"),
        }
    };
    let (schedules, classes) = explore("sample x dispatch", &make, &classify);
    assert!(schedules > 2);
    assert!(classes.contains_key("granted_by_pump"), "{classes:?}");
    assert!(classes.contains_key("dispatch_contended"), "{classes:?}");
    assert!(
        classes.keys().all(|class| !class.starts_with("other")),
        "{classes:?}"
    );
}
