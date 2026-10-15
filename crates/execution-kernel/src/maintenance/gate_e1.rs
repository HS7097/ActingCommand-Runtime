// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 E-1 G4a (exec key 12): pure tables for the shared `PrerequisiteChain` predicate
// and `validate_bundle_maintenance`, as contracts/package-reference.md "Maintenance declarations"
// states them (bundle v4 since Workflow #336 PKG-S5): the chain owns game/server/resolution compatibility, cycle
// detection, three declared layers and the 1,000-step aggregate bound; a return-home fallback is
// outside the three declared layers, and only a linear package with a resolved first-step page
// and no declared prerequisite selects it; the bundle check matches every indexed identity
// `(package_id, server, entry_task_id)` and the index's game, checks each declared use (a
// return-home package declares no prerequisite of its own) and every complete chain against the
// bundle's own packs (Workflow #373 NAV-3a: every pack is a prerequisite package of the registry
// the bundle becomes). Requirement basis: rd-1009/tests/req-basis-el.md section E-1,
// rows "G4a PrerequisiteChain" and "G4a validate_bundle_maintenance". Descriptions are built
// directly; nothing is loaded.

use super::*;
use actingcommand_contract::{BundleIndexVersion, BundleMaintenance, BundlePack};

const ROOT: &str = "neutral.test.root";
const HOME: &str = "neutral.test.home";
const UNBOUND: &str = "contained_task_prerequisite_unbound";
const CYCLE: &str = "contained_task_prerequisite_cycle";
const DEPTH: &str = "contained_task_prerequisite_depth_exceeded";
const MISMATCH: &str = "contained_task_prerequisite_mismatch";
const INCOMPATIBLE: &str = "contained_task_prerequisite_incompatible";
const STEP_LIMIT: &str = "contained_task_prerequisite_step_limit";
const BUNDLE_INVALID: &str = "bundle_maintenance_invalid";

// A page-graph package of game `neutral`, server `test`, 2x1, one step, that qualifies for every
// use.
fn package(package_id: &str) -> TaskPackageDescriptor {
    TaskPackageDescriptor {
        package_id: package_id.to_owned(),
        entry_task_id: "task".to_owned(),
        execution_mode: "navigable_route".to_owned(),
        game: "neutral".to_owned(),
        server: "test".to_owned(),
        resolution: (2, 1),
        prerequisite_package_id: None,
        entry_pages: None,
        maximum_executed_steps: 1,
        startup_incompatibility: None,
        prerequisite_incompatibility: None,
        recognition_incompatibility: None,
        reading_keys: Vec::new(),
    }
}

// A linear package whose first step's page resolved.
fn linear(package_id: &str) -> TaskPackageDescriptor {
    TaskPackageDescriptor {
        execution_mode: "linear_steps".to_owned(),
        entry_pages: Some(vec!["neutral/home".to_owned()]),
        ..package(package_id)
    }
}

// `package` declaring the prerequisite package `prerequisite`.
fn declaring(package: TaskPackageDescriptor, prerequisite: &str) -> TaskPackageDescriptor {
    TaskPackageDescriptor {
        prerequisite_package_id: Some(prerequisite.to_owned()),
        ..package
    }
}

// The chain `ids[0] -> ids[1] -> ...` of page-graph packages, each declaring the next.
fn declared_chain(ids: &[&str]) -> Vec<TaskPackageDescriptor> {
    ids.iter()
        .enumerate()
        .map(|(index, id)| match ids.get(index + 1) {
            Some(next) => declaring(package(id), next),
            None => package(id),
        })
        .collect()
}

// Walks the chain of `root` the way Host and the bundle check consume the predicate: each link is
// begun with its binding (`bound` names the bound packages), admitted with the description of the
// package it names, and the chain is finished. Returns `(package id, return home)` per link.
fn walk(
    root: &TaskPackageDescriptor,
    packages: &[TaskPackageDescriptor],
    bound: &[&str],
    return_home: Option<&str>,
) -> Result<Vec<(String, bool)>, ContainedTaskError> {
    let mut chain = PrerequisiteChain::new(root.clone());
    let mut links = Vec::new();
    while let Some(link) = chain.next(return_home) {
        chain.begin(&link, bound.contains(&link.package_id.as_str()))?;
        let described = packages
            .iter()
            .find(|candidate| candidate.package_id == link.package_id)
            .unwrap_or_else(|| panic!("the fixture describes {}", link.package_id))
            .clone();
        chain.admit(&link, described)?;
        links.push((link.package_id.clone(), link.return_home));
    }
    chain.finish()?;
    Ok(links)
}

fn refused(result: Result<impl std::fmt::Debug, ContainedTaskError>) -> (String, String) {
    let error = result.expect_err("the chain is refused");
    (
        error.code().as_str().to_owned(),
        error.detail().unwrap_or_default().to_owned(),
    )
}

fn links(ids: &[(&str, bool)]) -> Vec<(String, bool)> {
    ids.iter()
        .map(|(id, return_home)| ((*id).to_owned(), *return_home))
        .collect()
}

// package-reference.md: the chain owns "three declared layers". Three declared prerequisite
// packages are run; a fourth is refused when its link begins, naming its layer.
#[test]
fn gate_e1_chain_takes_three_declared_layers_and_refuses_a_fourth() {
    let three = declared_chain(&[ROOT, "p1", "p2", "p3"]);
    assert_eq!(
        walk(&three[0], &three, &["p1", "p2", "p3"], None).expect("three declared layers"),
        links(&[("p1", false), ("p2", false), ("p3", false)])
    );

    let four = declared_chain(&[ROOT, "p1", "p2", "p3", "p4"]);
    assert_eq!(
        refused(walk(&four[0], &four, &["p1", "p2", "p3", "p4"], None)),
        (DEPTH.to_owned(), "layer=4 package_id=p4".to_owned())
    );
}

// package-reference.md: "A return-home fallback is outside the three declared layers." The last
// declared package, linear with a resolved first-step page and no prerequisite of its own, falls
// back to the return-home package after three declared layers.
#[test]
fn gate_e1_chain_return_home_fallback_is_outside_the_three_declared_layers() {
    let mut packages = declared_chain(&[ROOT, "p1", "p2", "p3"]);
    packages[3] = linear("p3");
    packages.push(package(HOME));
    assert_eq!(
        walk(
            &packages[0],
            &packages,
            &["p1", "p2", "p3", HOME],
            Some(HOME)
        )
        .expect("three declared layers and the fallback"),
        links(&[("p1", false), ("p2", false), ("p3", false), (HOME, true)])
    );
}

// package-reference.md: "Only a linear package with a resolved first-step page and no declared
// prerequisite selects that fallback; an application entry or page-graph package does not." A
// package that declares a prerequisite runs that one; without a configured return-home package
// there is no fallback.
#[test]
fn gate_e1_chain_only_a_linear_page_entry_without_a_prerequisite_selects_the_fallback() {
    let next = |root: TaskPackageDescriptor, return_home: Option<&str>| {
        PrerequisiteChain::new(root)
            .next(return_home)
            .map(|link| (link.package_id, link.return_home))
    };
    assert_eq!(
        next(linear(ROOT), Some(HOME)),
        Some((HOME.to_owned(), true))
    );
    assert_eq!(next(linear(ROOT), None), None);
    assert_eq!(
        next(declaring(linear(ROOT), "p1"), Some(HOME)),
        Some(("p1".to_owned(), false))
    );
    // An application entry resolves no first-step page.
    let application_entry = TaskPackageDescriptor {
        entry_pages: None,
        ..linear(ROOT)
    };
    assert_eq!(next(application_entry, Some(HOME)), None);
    assert_eq!(next(package(ROOT), Some(HOME)), None);
}

// package-reference.md: the chain owns "cycle detection". A package that is already in the chain,
// the dependent itself included, is refused when its link begins.
#[test]
fn gate_e1_chain_refuses_a_cycle() {
    let back_to_root = [
        declaring(package(ROOT), "p1"),
        declaring(package("p1"), ROOT),
    ];
    assert_eq!(
        refused(walk(&back_to_root[0], &back_to_root, &["p1", ROOT], None)),
        (CYCLE.to_owned(), format!("layer=2 package_id={ROOT}"))
    );

    let itself = [declaring(package(ROOT), ROOT)];
    assert_eq!(
        refused(walk(&itself[0], &itself, &[ROOT], None)),
        (CYCLE.to_owned(), format!("layer=1 package_id={ROOT}"))
    );
}

// package-reference.md: the chain owns "game/server/resolution compatibility", and prerequisites
// "retain the existing exclusions" a package's own qualification reports. Each prerequisite that
// differs from its dependent in one of them is refused with that reason.
#[test]
fn gate_e1_chain_refuses_an_incompatible_prerequisite() {
    let cases = [
        (
            "game",
            TaskPackageDescriptor {
                game: "other".to_owned(),
                ..package("p1")
            },
        ),
        (
            "server",
            TaskPackageDescriptor {
                server: "other".to_owned(),
                ..package("p1")
            },
        ),
        (
            "resolution",
            TaskPackageDescriptor {
                resolution: (4, 2),
                ..package("p1")
            },
        ),
        (
            "recognize_only",
            TaskPackageDescriptor {
                prerequisite_incompatibility: Some("recognize_only"),
                ..package("p1")
            },
        ),
    ];
    for (reason, prerequisite) in cases {
        let packages = [declaring(package(ROOT), "p1"), prerequisite];
        assert_eq!(
            refused(walk(&packages[0], &packages, &["p1"], None)),
            (
                INCOMPATIBLE.to_owned(),
                format!("layer=1 package_id=p1 reason={reason}")
            ),
            "{reason}"
        );
    }
}

// package-reference.md: the chain owns "the 1,000-step aggregate bound". A chain whose packages
// declare 1,000 steps together is run; 1,001 is refused when the chain is finished.
#[test]
fn gate_e1_chain_refuses_more_than_1000_steps_in_total() {
    let steps = |root: u32, prerequisite: u32| {
        [
            TaskPackageDescriptor {
                maximum_executed_steps: root,
                ..declaring(package(ROOT), "p1")
            },
            TaskPackageDescriptor {
                maximum_executed_steps: prerequisite,
                ..package("p1")
            },
        ]
    };
    let at_bound = steps(400, 600);
    assert_eq!(
        walk(&at_bound[0], &at_bound, &["p1"], None).expect("1,000 steps in total"),
        links(&[("p1", false)])
    );
    let above = steps(400, 601);
    assert_eq!(
        refused(walk(&above[0], &above, &["p1"], None)),
        (
            STEP_LIMIT.to_owned(),
            "maximum_executed_steps=1001".to_owned()
        )
    );
}

// package-reference.md: the chain is checked "against the bindings supplied", and a package's
// identity is never inferred from a path or a label. An unbound prerequisite is refused when its
// link begins; a package whose description names another package ID than the link is refused.
#[test]
fn gate_e1_chain_refuses_an_unbound_or_misidentified_prerequisite() {
    let packages = [declaring(package(ROOT), "p1"), package("p1")];
    assert_eq!(
        refused(walk(&packages[0], &packages, &[], None)),
        (UNBOUND.to_owned(), "layer=1 package_id=p1".to_owned())
    );

    let mut chain = PrerequisiteChain::new(packages[0].clone());
    let link = chain.next(None).expect("the declared prerequisite");
    chain
        .begin(&link, true)
        .expect("a bound prerequisite begins");
    assert_eq!(
        refused(chain.admit(&link, package("p9"))),
        (
            MISMATCH.to_owned(),
            "layer=1 package_id=p1 declared_package_id=p9".to_owned()
        )
    );
}

// A v4 index of game `neutral` over `packages`, each on server `test` with entry task `task`,
// declaring `maintenance`.
fn index(packages: &[TaskPackageDescriptor], maintenance: Vec<BundleMaintenance>) -> BundleIndex {
    BundleIndex {
        schema_version: BundleIndexVersion::V4,
        game: "neutral".to_owned(),
        source: None,
        packs: packages
            .iter()
            .map(|package| BundlePack {
                package_id: package.package_id.clone(),
                server: "test".to_owned(),
                entry_task_id: "task".to_owned(),
                path: format!("packs/{}", package.package_id),
                file_count: 3,
                byte_count: 64,
            })
            .collect(),
        maintenance: Some(maintenance),
        tables: None,
    }
}

fn uses(package_id: &str, uses: &[MaintenanceUse]) -> BundleMaintenance {
    BundleMaintenance {
        package_id: package_id.to_owned(),
        server: "test".to_owned(),
        uses: uses.to_vec(),
    }
}

fn bundle_refusal(index: &BundleIndex, packages: &[TaskPackageDescriptor]) -> (String, String) {
    refused(validate_bundle_maintenance(index, packages))
}

// package-reference.md: the bundle check "checks that the admitted material set matches every
// indexed identity `(package_id, server, entry_task_id)` and the index's game". Admitted material
// that differs from its index entry in one of them is refused, as is a material set of another
// size than the index.
#[test]
fn gate_e1_bundle_maintenance_matches_every_indexed_identity() {
    let packages = [package(ROOT)];
    validate_bundle_maintenance(&index(&packages, Vec::new()), &packages)
        .expect("material that matches its index");

    let cases = [
        TaskPackageDescriptor {
            game: "other".to_owned(),
            ..package(ROOT)
        },
        TaskPackageDescriptor {
            server: "other".to_owned(),
            ..package(ROOT)
        },
        TaskPackageDescriptor {
            entry_task_id: "other".to_owned(),
            ..package(ROOT)
        },
    ];
    for material in cases {
        let (code, detail) = bundle_refusal(
            &index(&packages, Vec::new()),
            std::slice::from_ref(&material),
        );
        assert_eq!(code, BUNDLE_INVALID, "{material:?}: {detail}");
    }
    let (code, detail) = bundle_refusal(
        &index(&packages, Vec::new()),
        &[package(ROOT), package("p1")],
    );
    assert_eq!(code, BUNDLE_INVALID, "{detail}");
}

// package-reference.md: "Each server has at most one startup, one return-home and one recognition
// entry", and a reference "must name a pack in this index with the same server". The bundle check
// reuses those structural rules.
#[test]
fn gate_e1_bundle_maintenance_refuses_a_duplicate_role_or_a_missing_reference() {
    let packages = [package(ROOT), package(HOME)];
    validate_bundle_maintenance(
        &index(
            &packages,
            vec![
                uses(ROOT, &[MaintenanceUse::Startup]),
                uses(HOME, &[MaintenanceUse::ReturnHome]),
            ],
        ),
        &packages,
    )
    .expect("one startup entry and one return-home entry");

    let two_startups = index(
        &packages,
        vec![
            uses(ROOT, &[MaintenanceUse::Startup]),
            uses(HOME, &[MaintenanceUse::Startup]),
        ],
    );
    assert_eq!(bundle_refusal(&two_startups, &packages).0, BUNDLE_INVALID);

    let missing = index(
        &packages,
        vec![uses("neutral.test.absent", &[MaintenanceUse::Startup])],
    );
    assert_eq!(bundle_refusal(&missing, &packages).0, BUNDLE_INVALID);
}

// package-reference.md: each declared use is checked on the actual material ("Startup uses
// ordinary task admission and rejects resource readings through
// `PreparedContainedTask::startup_incompatibility`"; "Return-home packages additionally declare no
// prerequisite of their own"). A package whose own qualification refuses the use, and a
// return-home package that declares a prerequisite, are refused with that reason.
#[test]
fn gate_e1_bundle_maintenance_checks_each_declared_use_on_the_material() {
    let reads = [TaskPackageDescriptor {
        startup_incompatibility: Some("resource_readings"),
        ..package(ROOT)
    }];
    let (code, detail) = bundle_refusal(
        &index(&reads, vec![uses(ROOT, &[MaintenanceUse::Startup])]),
        &reads,
    );
    assert_eq!(code, BUNDLE_INVALID, "{detail}");
    assert!(detail.contains("reason=resource_readings"), "{detail}");

    let home_with_prerequisite = [declaring(package(HOME), ROOT), package(ROOT)];
    let (code, detail) = bundle_refusal(
        &index(
            &home_with_prerequisite,
            vec![uses(HOME, &[MaintenanceUse::ReturnHome])],
        ),
        &home_with_prerequisite,
    );
    assert_eq!(code, BUNDLE_INVALID, "{detail}");
    assert!(
        detail.contains("reason=return_home_declares_prerequisite"),
        "{detail}"
    );
}

// package-reference.md: the bundle check "then checks each use and complete prerequisite chain"
// against the registry the bundle becomes (Workflow #373 NAV-3a, M-373 §3.8, §4.3: every pack of
// the bundle is a prerequisite package of its game's registry, no declaration binds one;
// `return_home` supplies the server's fallback). A declared prerequisite the bundle does not hold
// is refused by the chain under its own code; one it holds, with no maintenance entry naming it,
// is bound, and the chain and the fallback pass.
#[test]
fn gate_e1_bundle_maintenance_checks_every_chain_against_the_bundles_packs() {
    let outside = [declaring(linear(ROOT), "p9"), package(HOME)];
    let (code, detail) = bundle_refusal(
        &index(&outside, vec![uses(HOME, &[MaintenanceUse::ReturnHome])]),
        &outside,
    );
    assert_eq!(
        (code.as_str(), detail.as_str()),
        (UNBOUND, "layer=1 package_id=p9")
    );

    let packages = [declaring(linear(ROOT), "p1"), linear("p1"), package(HOME)];
    validate_bundle_maintenance(
        &index(&packages, vec![uses(HOME, &[MaintenanceUse::ReturnHome])]),
        &packages,
    )
    .expect("the bundle's prerequisite package and the return-home fallback are bound");
}
