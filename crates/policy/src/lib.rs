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

pub use codes::{PolicyCode, PolicyInput, PolicyInputProblem, RefreshLimit};
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

    // SS-1 (0.12.30; Alice's rulings 100 and 102): a pool's base valuation counts on every
    // instance, a goal only adds its shortfall, and the resource layer reads no task state.
    mod gate_ss1;
    // SS-3 (register §1.212): a catalog that writes every run-count bound evaluates identically
    // to before the bounds became optional.
    mod gate_ss3;
}
