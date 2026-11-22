// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-1 (0.12.79; register §1.507; contracts/pack-catalog.md, "Interface";
// contracts/component-interfaces.md, the `package` row 24): a bundle zip that carries a
// `pack-catalog.json` of version 2, which the reader of every earlier build takes as `malformed`,
// declares `package` 24, so that an installer of the earlier range ([3, 23]) refuses it at its
// `package` edge; one that carries a version 1 file keeps 16, which that installer places. The zip
// entries are built in memory as the revision table (`standard-pack build`'s,
// `apps/actinglab/src/resource_verify/revision.rs`) reads them, and the edge is the contract
// crate's one rule, which the installer's `place` calls; every name is made up.

use actingcommand_contract::{
    CATALOG_PACKAGE_REVISION, CATALOG_V2_PACKAGE_REVISION, package_edge_holds,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// This build's own declaration of its interface ranges.
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");

/// The revision `content_revision` gives a zip that carries `catalog` as its `pack-catalog.json`.
fn revision_of(catalog: &Value) -> u64 {
    let zip = BTreeMap::from([(
        "pack-catalog.json".to_owned(),
        serde_json::to_vec(catalog).expect("the catalog's JSON"),
    )]);
    crate::resource_verify::revision::content_revision(&zip)
}

/// This build's own `package` range.
fn own_package_range() -> [u64; 2] {
    let declared: Value =
        serde_json::from_str(OWN_DECLARATION).expect("the build's declaration is JSON");
    match declared["speaks"]["package"].as_array().map(Vec::as_slice) {
        Some([min, max]) => [
            min.as_u64().expect("a minimum"),
            max.as_u64().expect("a maximum"),
        ],
        other => panic!("the build's package range: {other:?}"),
    }
}

/// A version 2 catalog needs 24 and a version 1 catalog 16; this build declares [3, 25] (Workflow
/// #477 LR-1, register §1.513, raises it to 25) and places both; an installer of [3, 23] refuses
/// the zip of 24 at its edge and places the zip of 16.
#[test]
fn cv1_a_zip_with_a_version_2_catalog_needs_package_24_and_one_with_version_1_keeps_16() {
    let version_1 =
        json!({"schema_version": "actingcommand.pack-catalog.v1", "game": "neutral", "packs": []});
    let version_2 =
        json!({"schema_version": "actingcommand.pack-catalog.v2", "game": "neutral", "packs": []});
    assert_eq!(revision_of(&version_1), 16);
    assert_eq!(revision_of(&version_1), CATALOG_PACKAGE_REVISION);
    assert_eq!(revision_of(&version_2), 24);
    assert_eq!(revision_of(&version_2), CATALOG_V2_PACKAGE_REVISION);

    let own = own_package_range();
    assert_eq!(own, [3, 25]);
    let earlier = [3, CATALOG_V2_PACKAGE_REVISION - 1];
    let carries_version_2 = [CATALOG_V2_PACKAGE_REVISION, CATALOG_V2_PACKAGE_REVISION];
    let carries_version_1 = [CATALOG_PACKAGE_REVISION, CATALOG_PACKAGE_REVISION];
    assert!(package_edge_holds(own, carries_version_2));
    assert!(package_edge_holds(own, carries_version_1));
    assert!(
        !package_edge_holds(earlier, carries_version_2),
        "an installer of the earlier range refuses the zip at its package edge"
    );
    assert!(package_edge_holds(earlier, carries_version_1));
}
