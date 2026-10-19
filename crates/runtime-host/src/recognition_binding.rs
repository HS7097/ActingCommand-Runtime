// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` §3.2): the checks of an
//! instance's recognition maintenance package binding (`instances[].recognition_package`, a
//! location only) and the hint about realistic facts no reader marks. `check-config` refuses a
//! binding these checks refuse; a Runtime start runs without it and notes why (a package's error
//! is that package's alone).

use crate::codes::HostCode;
use actingcommand_contract::ReaderProblem;
use actingcommand_contract::outcome::Outcome;
use actingcommand_execution_kernel::TaskPackageDescriptor;
use actingcommand_policy::{CatalogBundle, ObservedFact, PredicateSpec, ScopeSelector};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// `fact_reader_binding_invalid` of the binding of `instance_alias` to `path`, for `problem`.
pub fn recognition_binding_invalid(
    instance_alias: &str,
    path: &Path,
    problem: ReaderProblem,
) -> Outcome {
    Outcome::new(HostCode::FactReaderBindingInvalid)
        .instance_id(instance_alias)
        .path(path)
        .reader_problem(problem)
}

/// Check 1 (D2 model §3.2, qualifications 1-3): the bound package is a reader. It only
/// navigates and reads: no `recognize_only` package, no stability termination, no designated
/// operation, at least one reading mark. Qualification 5 (no `cost_commit`) is the scheduling
/// declaration check's (SR-10).
pub fn recognition_binding_qualification(
    instance_alias: &str,
    path: &Path,
    descriptor: &TaskPackageDescriptor,
) -> Result<(), Outcome> {
    match descriptor.recognition_problem() {
        None => Ok(()),
        Some(problem) => Err(recognition_binding_invalid(instance_alias, path, problem)
            .package_id(descriptor.package_id())),
    }
}

/// Check 2 (D2 model §3.2): no mark writes a key the configuration's `policy.facts` declares for
/// the instance; the fact store would refuse every such reading (`contracts/resource-readings.md`,
/// Host). Check 3 is gone since Workflow #390 POOL-1: a reading of the key a `ledger_fact` pool
/// observes is published and resets the pool's base, so a mark may write it.
pub fn recognition_binding_key_conflict(
    instance_alias: &str,
    path: &Path,
    package_id: &str,
    reading_keys: &[String],
    policy_facts: &[ObservedFact],
) -> Result<(), Outcome> {
    let scope = ScopeSelector::Instance {
        instance_id: instance_alias.to_owned(),
    };
    for key in reading_keys {
        if policy_facts
            .iter()
            .any(|fact| fact.scope == scope && fact.fact_key == *key)
        {
            return Err(recognition_binding_invalid(
                instance_alias,
                path,
                ReaderProblem::PolicyFactAuthority,
            )
            .package_id(package_id)
            .fact_key(key.as_str()));
        }
    }
    Ok(())
}

/// The binding a Runtime start runs without: `fact_reader_binding_ignored`, caused by its
/// refusal `invalid`.
pub fn recognition_binding_ignored(
    instance_alias: &str,
    package_id: Option<&str>,
    invalid: Outcome,
) -> Outcome {
    let ignored = Outcome::new(HostCode::FactReaderBindingIgnored).instance_id(instance_alias);
    match package_id {
        Some(package_id) => ignored.package_id(package_id),
        None => ignored,
    }
    .caused_by(invalid)
}

/// `fact_reader_missing` (D2 model §3.2): each realistic fact (`resource.*`, `inventory.*`) a
/// catalog task's trigger or stop predicate reads in one instance's scope that the instance's
/// reader does not mark, or that no reader can refresh because the instance has none. `marked`
/// holds the keys each instance alias's reader marks. One warning per instance and key, naming
/// the first task that reads it, in catalog order.
pub fn missing_fact_readers(
    catalog: &CatalogBundle,
    marked: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<Outcome> {
    let mut seen = BTreeSet::new();
    let mut missing = Vec::new();
    for task in &catalog.tasks.tasks {
        let mut reads = Vec::new();
        instance_fact_reads(&task.trigger, &mut reads);
        instance_fact_reads(&task.feedback_stop, &mut reads);
        for (instance, key) in reads {
            if !(key.starts_with("resource.") || key.starts_with("inventory."))
                || marked.get(instance).is_some_and(|keys| keys.contains(key))
            {
                continue;
            }
            if seen.insert((instance, key)) {
                missing.push(
                    Outcome::new(HostCode::FactReaderMissing)
                        .instance_id(instance)
                        .fact_key(key)
                        .task_id(task.id.as_str()),
                );
            }
        }
    }
    missing
}

/// The `(instance alias, fact key)` pairs a predicate reads in an instance's scope.
fn instance_fact_reads<'a>(predicate: &'a PredicateSpec, reads: &mut Vec<(&'a str, &'a str)>) {
    match predicate {
        PredicateSpec::All { predicates } | PredicateSpec::Any { predicates } => {
            for predicate in predicates {
                instance_fact_reads(predicate, reads);
            }
        }
        PredicateSpec::Not { predicate } => instance_fact_reads(predicate, reads),
        PredicateSpec::Fact {
            scope: ScopeSelector::Instance { instance_id },
            fact_key,
            ..
        }
        | PredicateSpec::RecordDeadline {
            scope: ScopeSelector::Instance { instance_id },
            fact_key,
            ..
        }
        | PredicateSpec::FactTimeReached {
            scope: ScopeSelector::Instance { instance_id },
            fact_key,
            ..
        } => reads.push((instance_id.as_str(), fact_key.as_str())),
        PredicateSpec::Fact { .. }
        | PredicateSpec::RecordDeadline { .. }
        | PredicateSpec::FactTimeReached { .. }
        | PredicateSpec::Clock { .. }
        | PredicateSpec::TimelineActive { .. }
        | PredicateSpec::ResourceProjection { .. }
        | PredicateSpec::DependencyCompleted { .. }
        | PredicateSpec::Outcome { .. } => {}
    }
}
