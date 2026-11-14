// SPDX-License-Identifier: AGPL-3.0-only

//! Pure scheduling-policy contracts shared by the catalog compiler and evaluator.

#![forbid(unsafe_code)]

mod canonical;
mod codes;
mod compiler;
// ROUT-L RL-1a: the composer of the standard packs' schedule rules.
mod compose;
mod evaluator;
mod forward;
mod remedy;
mod resource_targets;
mod schema;
mod source;
mod strategy;
mod validation;

pub use codes::{
    DeviceGroupWait, PolicyCode, PolicyInput, PolicyInputProblem, RefreshLimit,
    SchedulingRulePolicyCode,
};
pub use compiler::*;
pub use compose::*;
pub use evaluator::*;
pub use forward::*;
pub use remedy::*;
pub use resource_targets::*;
pub use schema::*;
pub use source::{CatalogDocumentSource, CatalogSources, validate_catalog_declaration};
pub use strategy::*;

/// Gate tests that need the crate's private items (test plan §2.1), and their shared fixtures.
#[cfg(test)]
mod tests {
    // Workflow #361 R7, test row R7-T7: the S-08 day replay with and without `missed`.
    mod gate_361r7;
    mod gate_390_pool1;
    // Workflow #405 W1: the catalog predicate `fact_time_reached`.
    mod gate_405_w1;
    // SCH-M CF-1: the edge members `since_own_run` and `once_per_reading`.
    mod gate_405_cf1;
    // Workflow #411 A26: a dispatch decision record says why each listed candidate did not run.
    mod gate_411_a26;
    // Workflow #411 B15 R2: the evaluator's plan of declared remedies.
    mod gate_411_b15e;
    // Workflow #411 B8 (policy key 59): a dispatch's schedule values for a task that opts in.
    mod gate_411_b8;
    mod gate_d2_s1;
    mod gate_d2_s4;
    // Workflow #386 SR-6b, MCP-19a (policy key 56): each decision's own wake, never serialized.
    mod gate_mcp19a;
    mod gate_p1;
    mod gate_p2a;
    mod gate_p2b;
    mod selection_catalog;

    // Workflow #411 B41 (policy keys 60 to 63): a chain step that hands over on its page goes
    // first.
    mod gate_411_b41;

    // Workflow #411 B16 part 2, slice BT-B16b (policy keys 64 to 66): a task's reserves gate it,
    // and of two tasks that fit alone but not together the higher score runs first.
    mod gate_411_bt_b16b;

    // Workflow #386 SR-4a1 (SCH-M §3.2 D-1): the pure blackout judgement over its three sources.
    mod gate_sr4a1;

    // SCH-M ED-1 (Workflow #405; SCH-M §3.4 D-12, D-15): the expected duration E.
    mod gate_ed1;

    // ROUT-L RL-1a (frozen model v1.0 §6.1): `schedule.json` v1 and the pure composer.
    mod gate_rl1a;

    // #386 CF-4: a `fact` predicate compares with another fact of its scope plus an offset.
    mod gate_386_cf4;

    // #386 CF-5: a pool's capacity and a projection's comparand can be facts.
    mod gate_386_cf5;

    // Workflow #427-B: no in-round CPU/GPU/IO deduction; the heavy-scene slots and the optional
    // maximum of concurrent runs, with their numbers.
    mod gate_427b;

    // Workflow #411 P19: a reset may exclude key prefixes and never clears a task's own last
    // result.
    mod gate_411_p19;

    // Workflow #386 SR-2b-2 (test ①): an owed rerun is triggered past its cooldown with
    // `policy_rerun_owed` and ranks first only when it promotes.
    mod gate_386_sr2b2;

    // UPD-1 fix round 5 (review L3; register §1.215 UPD-13): the whole-instance holds of
    // `evaluate_with_instance_holds` change nothing when they hold nothing, and defer a held
    // candidate the round settles before it asks the eligibility closure.
    mod gate_upd1_holds;

    // SS-1 (0.12.30; Alice's rulings 100 and 102): a pool's base valuation counts on every
    // instance, a goal only adds its shortfall, and the resource layer reads no task state.
    mod gate_ss1;
    // SS-3 (register §1.212): a catalog that writes every run-count bound evaluates identically
    // to before the bounds became optional.
    mod gate_ss3;
    // SS-6 (0.12.31; register §1.218): a dispatch's spend allowance, remaining gap and run cap.
    mod gate_ss6;
    // SS-2 (0.12.32; register §1.222): chain successors rank with their chain head, and a tie
    // goes to the candidate closer to losing its eligibility, then more conditions, then declared
    // order.
    mod gate_ss2;
    // SS-5 (0.12.32; register §1.223): a self-read fact's `on_unknown: probe` admits its task once
    // per unknown period; a non-probe unknown still asks the maintenance package.
    mod gate_ss5;
    // SS-2b (0.12.32; register §1.225): business candidates rank by net benefit within their tier,
    // and a candidate whose value is unknown or not positive is not dispatched.
    mod gate_ss2b;
    // SS-2c (0.12.32; register §1.227): a `schedule.json` rule that declares itself maintenance is
    // not judged by net benefit.
    mod gate_ss2c;
    // SS-4 (0.12.34; register §1.231): after a successful run whose basis got no reading of
    // another value the pair waits with `policy_no_progress`; failures keep their retry path.
    mod gate_ss4;
    // SS-2g (0.12.34; register §1.232): a V2 consumption may declare its expected amount in
    // thousandths; the net benefit reads it exactly and every integer reader rounds it up.
    mod gate_ss2g;
    // SS-2d (0.12.35; register §1.235): a judged candidate within its rounding error is neither
    // positive nor negative; a model-verified realization task may run last in its tier.
    mod gate_ss2d;
    // SS-2f (0.12.46; register §1.288): the business tier across instances goes to the instance
    // least recently dispatched first; within an instance the order of before stays.
    mod gate_ss2f;
    // SS-2h (0.12.46; register §1.289): a pool's stop line and hard line, and the capped spending
    // order inside the scheduler.
    mod gate_ss2h;
    // #428 SCHED-C1a (0.12.52; register §1.299): a rule's fixed value written in its pack is its
    // known benefit, less the cost of what it consumes.
    mod gate_sched_c1a;
    // #428 SCHED-C1c (0.12.53; register §1.299): a pool's trigger line, and a pool its lines cannot
    // spend.
    mod gate_sched_c1c;
    // #428 SCHED-C1d (0.12.53; register §1.299 SC1-11): a capped episode survives a start, a
    // generation with the same consumers and an unknown reading.
    mod gate_sched_c1d;
    // #428 SCHED-C1e (0.12.54; register §1.299 SC1-12): a pool's lines given for one instance
    // come before the catalog's, and every reason that carries the lines names their source.
    mod gate_sched_c1e;
    // #428 SCHED-C1f1 (0.12.54; register §1.303): a run's consumption read from a fact, with a
    // fallback, resolved once per evaluation and read by every reader of the consumption.
    mod gate_sched_c1f1;
    // #428 SS-4b (0.12.55; register §1.310): a no-progress guard holds its pair only while the
    // pair is due for the cause the guard served.
    mod gate_ss4b;
    // #428 S168-LB1a (0.12.55; register §1.309 LB1-4, §1.311): a catalog that names a key under
    // `inventory.list.` to read is refused at compile.
    mod gate_428_s168_lb1a;
    // Workflow #437 MAPP-1 (0.12.56; register §1.328): one pick per device per round, the other
    // member waits with `device_group_waiting` `selected`.
    mod gate_mapp1;
    // Workflow #437 MAPP-1b (0.12.57; register §1.328 point 8, §1.330): a chain handover's
    // successor goes before the candidates of its device's other members too.
    mod gate_mapp1b;
    // Workflow #437 ORD-1 (0.12.60; register §1.352): a task's longest wait in its device's
    // queue.
    mod gate_ord1;
    // Workflow #455 G-R2a (0.12.60; register §1.361): the goal document's value store, a v3
    // record whose targets weigh as a v2 record's.
    mod gate_455_gr2a;
    // Workflow #455 G-R2s (0.12.60; register §1.363): a goal never raises the weight of a task
    // whose package can do something irreversible.
    mod gate_455_gr2s;

    /// Workflow #455 G-R2s: the composer's required input for `root` when every package its
    /// registry lists reads and provides nothing, so nothing is marked: the gate tests' roots hold
    /// no package files.
    fn readable_root(
        root: &actingcommand_contract::StandardPackRoot,
    ) -> actingcommand_contract::IrreversiblePackages {
        actingcommand_contract::IrreversiblePackages::new(
            root.registry
                .package_ids()
                .map(|package_id| {
                    (
                        package_id.to_owned(),
                        Some(std::collections::BTreeMap::new()),
                    )
                })
                .collect(),
        )
    }
}
