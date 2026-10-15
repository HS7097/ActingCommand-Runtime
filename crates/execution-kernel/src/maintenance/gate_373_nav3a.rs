// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #373 NAV-3a (model M-373 v1.1 §4.2; test row N3a-3, exec key 67): the shared
// prerequisite chain goes on through page-graph layers. A page-graph layer with a page entry
// leads to its own declared prerequisite package and, declaring none, falls back to the
// return-home package; three declared layers, the return-home layer outside them, cycles and the
// 1,000-step bound keep their codes. Descriptions are built directly; nothing is loaded.

use super::*;
use actingcommand_contract::{ContentDirectory, ContentDirectoryVersion};

const ROOT: &str = "neutral.test.root";
const HOME: &str = "neutral.test.home";

// A `navigable_route` package of game `neutral`, server `test`, 16x9, whose entry is its page
// `neutral/<package_id>`, running at most `steps` steps.
fn page_graph(package_id: &str, prerequisite: Option<&str>, steps: u32) -> TaskPackageDescriptor {
    TaskPackageDescriptor {
        package_id: package_id.to_owned(),
        entry_task_id: "task".to_owned(),
        reference: PackageRef::ContentDirectory(ContentDirectory {
            schema_version: ContentDirectoryVersion::V1,
            sha256: "0".repeat(64),
        }),
        execution_mode: "navigable_route".to_owned(),
        game: "neutral".to_owned(),
        server: "test".to_owned(),
        resolution: (16, 9),
        prerequisite_package_id: prerequisite.map(str::to_owned),
        entry_pages: Some(vec![format!("neutral/{package_id}")]),
        maximum_executed_steps: steps,
        startup_incompatibility: None,
        prerequisite_incompatibility: None,
        recognition_incompatibility: None,
        reading_keys: Vec::new(),
    }
}

// Walks the chain of `root` over `packages` (every one bound) with the return-home package
// `return_home`: the links it took, `(package id, return-home)`, or its refusal's code.
fn walk(
    root: &TaskPackageDescriptor,
    packages: &[TaskPackageDescriptor],
    return_home: Option<&str>,
) -> Result<Vec<(String, bool)>, String> {
    let refused = |error: ContainedTaskError| error.code().as_str().to_owned();
    let mut chain = PrerequisiteChain::new(root.clone());
    let mut links = Vec::new();
    while let Some(link) = chain.next(return_home) {
        chain.begin(&link, true).map_err(refused)?;
        let package = packages
            .iter()
            .find(|package| package.package_id == link.package_id)
            .expect("a package of the set")
            .clone();
        links.push((link.package_id.clone(), link.return_home));
        chain.admit(&link, package).map_err(refused)?;
    }
    chain.finish().map_err(refused)?;
    Ok(links)
}

// N3a-3: page-graph layers lead the chain on: three declared layers, then the return-home
// package below the last one, which declares none and has a page entry. A fourth declared layer
// exceeds the depth, a layer that leads back to one already in the chain is a cycle, and steps
// beyond 1,000 over the whole chain are the step limit, each with today's code.
#[test]
fn n3a_3_the_chain_goes_on_through_page_graph_layers_with_its_limits() {
    let declared = [
        page_graph("neutral.test.first", Some("neutral.test.second"), 1),
        page_graph("neutral.test.second", Some("neutral.test.third"), 1),
        page_graph("neutral.test.third", None, 1),
        page_graph(HOME, None, 1),
    ];
    let root = page_graph(ROOT, Some("neutral.test.first"), 1);
    assert_eq!(
        walk(&root, &declared, Some(HOME)),
        Ok(vec![
            ("neutral.test.first".to_owned(), false),
            ("neutral.test.second".to_owned(), false),
            ("neutral.test.third".to_owned(), false),
            (HOME.to_owned(), true),
        ])
    );

    let deeper = [
        page_graph("neutral.test.first", Some("neutral.test.second"), 1),
        page_graph("neutral.test.second", Some("neutral.test.third"), 1),
        page_graph("neutral.test.third", Some("neutral.test.fourth"), 1),
        page_graph("neutral.test.fourth", None, 1),
    ];
    assert_eq!(
        walk(&root, &deeper, None),
        Err("contained_task_prerequisite_depth_exceeded".to_owned())
    );

    let back = [page_graph("neutral.test.first", Some(ROOT), 1)];
    assert_eq!(
        walk(&root, &back, None),
        Err("contained_task_prerequisite_cycle".to_owned())
    );

    let long = [
        page_graph("neutral.test.first", None, 400),
        page_graph(HOME, None, 300),
    ];
    let root = page_graph(ROOT, Some("neutral.test.first"), 400);
    assert_eq!(
        walk(&root, &long, Some(HOME)),
        Err("contained_task_prerequisite_step_limit".to_owned())
    );
}
