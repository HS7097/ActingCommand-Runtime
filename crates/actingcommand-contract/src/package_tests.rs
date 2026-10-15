// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S5, model M-PKG v1.1 §4.6 (with the coordinator's 10-15 lines from M-357a
// v1.0 §11.2 A3) and M-357a §2.1, §2.8, §7 TB12: the bundle index v4 a game root's `bundle.json`
// holds. Tests S5-T2 and S5-T6; the game is the neutral fixture game.

use super::*;
use crate::{
    STANDARD_PACK_BUNDLE_FILE, StandardPackProblem, StandardPackRegistry, StandardPackRootIssue,
};
use serde_json::{Value, json};

const GAME: &str = "neutral";
const PACKAGE: &str = "neutral.test.task";

// One bundle v4 pack entry: its directory is `packs/<package_id>`.
fn pack(package_id: &str) -> Value {
    json!({
        "package_id": package_id,
        "server": "test",
        "entry_task_id": "task",
        "path": format!("packs/{package_id}"),
        "file_count": 3,
        "byte_count": 64
    })
}

fn bundle(packs: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packs.iter().map(|package_id| pack(package_id)).collect::<Vec<_>>()
    })
}

// The game root's registry, as the Runtime, check-config and `resource validate` read it.
fn read(value: &Value) -> Result<StandardPackRegistry, StandardPackRootIssue> {
    StandardPackRegistry::from_bundle_bytes(&serde_json::to_vec(value).expect("bundle JSON"), GAME)
}

fn refused(problem: StandardPackProblem) -> Result<(), StandardPackRootIssue> {
    Err(StandardPackRootIssue {
        file: STANDARD_PACK_BUNDLE_FILE,
        problem,
        pointer: String::new(),
    })
}

// S5-T2: a bundle index v2 or v3 (the digest-named shapes) is refused outright, with no
// compatibility reading (M-PKG §7): it does not decode as the bundle index, and a game root
// holding it is `malformed`. The same pack as bundle v4 is read, its directory its package_id.
#[test]
fn s5_t2_a_bundle_index_v2_or_v3_is_refused() {
    let digest = "a".repeat(64);
    let v2 = json!({
        "schema_version": "actingcommand.bundle.v2",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "server": "test",
            "entry_task_id": "task",
            "digest": digest,
            "path": format!("packs/{digest}"),
            "file_count": 3,
            "byte_count": 64
        }]
    });
    let mut v3 = v2.clone();
    v3["schema_version"] = json!("actingcommand.bundle.v3");
    v3["maintenance"] = json!([]);
    for (case, value) in [("v2", &v2), ("v3", &v3)] {
        assert!(
            serde_json::from_value::<BundleIndex>(value.clone()).is_err(),
            "{case} decodes as the bundle index"
        );
        assert_eq!(
            read(value).map(|_| ()),
            refused(StandardPackProblem::Malformed),
            "{case}"
        );
    }

    let registry = read(&bundle(&[PACKAGE])).expect("a bundle v4 is read");
    assert_eq!(registry.bundle_version(), 4);
    assert_eq!(
        registry.get(PACKAGE).map(|entry| entry.directory.as_str()),
        Some(PACKAGE)
    );
}

// S5-T6 (M-357a TB12, §2.8): bundle v4's optional `tables` list, at most 64 unique table ids of
// `[a-z][a-z0-9_]{0,63}`, is read and kept for the updater; a list that breaks those rules is
// `bundle_invalid`. A pack whose `package_id` is `tables` (the game root's reserved directory,
// M-357a §2.1, in any ASCII case on a case-insensitive file system) is `bundle_invalid`.
#[test]
fn s5_t6_bundle_v4_reads_its_tables_list_and_refuses_a_pack_named_tables() {
    let tables = (0..MAX_BUNDLE_TABLES)
        .map(|index| format!("table_{index:02}"))
        .collect::<Vec<_>>();
    let mut listed = bundle(&[PACKAGE]);
    listed["tables"] = json!(tables);
    read(&listed).expect("a bundle with its tables list is read");
    let index: BundleIndex = serde_json::from_value(listed.clone()).expect("bundle v4");
    assert_eq!(index.tables.as_deref(), Some(tables.as_slice()));
    let mut unlisted = listed.clone();
    unlisted
        .as_object_mut()
        .expect("bundle object")
        .remove("tables");
    let index: BundleIndex = serde_json::from_value(unlisted).expect("bundle v4 without tables");
    assert_eq!(index.tables, None);

    let mut too_many = listed.clone();
    too_many["tables"]
        .as_array_mut()
        .expect("tables")
        .push(json!("table_64"));
    let with_tables = |tables: Value| {
        let mut value = bundle(&[PACKAGE]);
        value["tables"] = tables;
        value
    };
    for (case, value) in [
        ("65 tables", too_many),
        (
            "a repeated table id",
            with_tables(json!(["unit_scores", "unit_scores"])),
        ),
        (
            "an upper-case table id",
            with_tables(json!(["Unit_scores"])),
        ),
        ("a hyphenated table id", with_tables(json!(["unit-scores"]))),
        (
            "a table id starting with a digit",
            with_tables(json!(["1_scores"])),
        ),
    ] {
        assert_eq!(
            read(&value).map(|_| ()),
            refused(StandardPackProblem::BundleInvalid),
            "{case}"
        );
    }

    for package_id in ["tables", "Tables"] {
        assert_eq!(
            read(&bundle(&[PACKAGE, package_id])).map(|_| ()),
            refused(StandardPackProblem::BundleInvalid),
            "a pack named {package_id}"
        );
    }
}

// Workflow #373 NAV-3a, test row N3a-9 (M-373 v1.1 §3.8; wave 14 ruling NAV-3a item 5; exec key
// 71): bundle v4's maintenance uses lose `prerequisite`, since every pack of a game root's
// `bundle.json` is a prerequisite package of that game's registry, and v4 refuses an entry that
// still names it, with no compatibility reading: the bundle index does not decode, a game root
// holding it is `malformed`, and the source declaration array (`maintenance.json`, `package
// bundle --maintenance`), which shares the entry shape, does not decode either. The same entry
// with `return_home` alone is read and names the server's return-home package.
#[test]
fn n3a_9_bundle_v4_refuses_a_maintenance_entry_with_the_prerequisite_use() {
    const HOME: &str = "neutral.test.home";
    let declaring = |uses: Value| {
        let mut value = bundle(&[PACKAGE, HOME]);
        value["maintenance"] = json!([{"package_id": HOME, "server": "test", "uses": uses}]);
        value
    };
    for uses in [
        json!(["prerequisite"]),
        json!(["prerequisite", "return_home"]),
    ] {
        let value = declaring(uses.clone());
        assert!(
            serde_json::from_value::<BundleIndex>(value.clone()).is_err(),
            "{uses}: decodes as the bundle index"
        );
        assert_eq!(
            read(&value).map(|_| ()),
            refused(StandardPackProblem::Malformed),
            "{uses}"
        );
        assert!(
            serde_json::from_value::<Vec<BundleMaintenance>>(value["maintenance"].clone()).is_err(),
            "{uses}: decodes as a source declaration"
        );
    }

    let registry = read(&declaring(json!(["return_home"]))).expect("a return-home entry is read");
    assert_eq!(registry.return_home_package("test"), Some(HOME));
    assert!(
        registry.get(HOME).is_some(),
        "the return-home package is listed"
    );
}
