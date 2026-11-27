// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 TK-1 (0.12.89; register §1.532; contracts/component-interfaces.md, the `package`
// row 26; contracts/resource-verify.md, "The `package` revision"): a variable of a pack's
// `control.json` that says who gives its value (`given_by`) is refused at the declaration by every
// reader before this revision, so a zip that holds one declares `package` 26, which
// `standard-pack build`'s revision table (`apps/actinglab/src/resource_verify/revision.rs`)
// counts for such a pack and for no other; this build declares [3, 26] and places it, and an
// installer of the earlier range ([3, 25]) refuses it at its `package` edge, the contract crate's
// one rule, which the installer's `place` calls. The zip entries are built in memory as the table
// reads them; every name is made up.

use actingcommand_contract::{
    GIVEN_BY_PACKAGE_REVISION, VARIABLES_PACKAGE_REVISION, package_edge_holds,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// This build's own declaration of its interface ranges.
const OWN_DECLARATION: &str =
    include_str!("../../../../distribution/windows/component-interfaces.json");

/// The revision `content_revision` gives a zip whose one pack's `control.json` declares
/// `variables`.
fn revision_of(variables: &Value) -> u64 {
    let zip = BTreeMap::from([(
        "packs/neutral.shop/control.json".to_owned(),
        serde_json::to_vec(&json!({"variables": variables})).expect("the control's JSON"),
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

/// Ruling 1: a pack whose variable writes `given_by` needs 26, whatever the member holds; the same
/// pack without the member needs 14, as before. This build's range is [3, 26] and holds both; an
/// installer of [3, 25] refuses the zip of 26 at its edge and places the zip of 14. Workflow #455
/// RR1 (register §1.535) raises this build's range to [3, 28], 26 within it.
#[test]
fn tk1_a_variable_a_rule_gives_needs_package_26_and_an_earlier_installer_refuses_it() {
    let choice = |given_by: Option<Value>| {
        let mut declared = json!({"name": "pick", "type": "choice", "title": {"en": "Item"},
                                  "options": [{"value": "item_a", "title": {"en": "Item A"}},
                                              {"value": "item_b", "title": {"en": "Item B"}}]});
        match given_by {
            Some(word) => declared["given_by"] = word,
            None => declared["default"] = json!("item_b"),
        }
        json!([declared])
    };
    assert_eq!(revision_of(&choice(Some(json!("rule")))), 26);
    assert_eq!(
        revision_of(&choice(Some(json!("rule")))),
        GIVEN_BY_PACKAGE_REVISION
    );
    assert_eq!(
        revision_of(&choice(Some(json!("another word")))),
        26,
        "whatever the member holds"
    );
    assert_eq!(revision_of(&choice(None)), VARIABLES_PACKAGE_REVISION);

    let own = own_package_range();
    assert_eq!(own, [3, 28]);
    let earlier = [3, GIVEN_BY_PACKAGE_REVISION - 1];
    let rule_given = [GIVEN_BY_PACKAGE_REVISION, GIVEN_BY_PACKAGE_REVISION];
    let person_s = [VARIABLES_PACKAGE_REVISION, VARIABLES_PACKAGE_REVISION];
    assert!(package_edge_holds(own, rule_given));
    assert!(package_edge_holds(own, person_s));
    assert!(
        !package_edge_holds(earlier, rule_given),
        "an installer of the earlier range refuses the zip at its package edge"
    );
    assert!(package_edge_holds(earlier, person_s));
}
