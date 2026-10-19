// SPDX-License-Identifier: AGPL-3.0-only

//! The outcome guard on this workspace (G1, G2-G6 outside the allow list, G7, G8, G9, G10),
//! plus counterexamples that show each check catches a break.

use actingcommand_outcome_guard::catalog::{
    RELEASE_SCHEMA_VERSION, RELEASED_DIR, check_ladder_class, check_release,
    check_review as review_of, check_snapshot,
};
use actingcommand_outcome_guard::source::{ChannelScope, Check, inspect_channels, scan_file};
use actingcommand_outcome_guard::{
    ALLOW_LIST, ScannedFile, Workspace, check_channels, check_contract, check_keys, check_naming,
    check_persisted_prose, check_registries, check_review, load_workspace, persisted, release_gate,
    workspace_members, workspace_root,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

fn workspace() -> &'static Workspace {
    static WORKSPACE: OnceLock<Workspace> = OnceLock::new();
    WORKSPACE.get_or_init(|| load_workspace(&workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")))))
}

fn assert_none(check: &str, violations: Vec<String>) {
    assert!(
        violations.is_empty(),
        "{check} violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_member_source_is_read_and_parsed() {
    assert_none("source", workspace().errors.clone());
    assert!(
        workspace()
            .files
            .iter()
            .any(|file| file.path == "crates/actingcommand-contract/src/codes.rs"),
        "the module walk must reach the contract registry"
    );
}

#[test]
fn g1_registries_equal_the_catalog_fragments() {
    assert_none("G1", check_registries(workspace()));
}

#[test]
fn g2_to_g5_have_no_finding_outside_the_allow_list() {
    assert_none("G2-G5", check_channels(workspace()));
}

/// #378 A3 (model section 9, G6): ledger revision 3 persists no Runtime text.
#[test]
fn g6_persisted_contract_types_carry_no_text_member() {
    assert_none("G6", check_persisted_prose(workspace()));
}

/// #378 A3 (model section 9, G6): a text member is a finding exactly when a stored root reaches
/// its type, through any member type; an unreached type, a non-text member and test code are not.
#[test]
fn persisted_text_is_caught_where_a_stored_record_reaches_it() {
    let source = r#"
        #[derive(Serialize, Deserialize)]
        pub enum EventPayload { Failed(Box<Failure>), Other { note: Option<String> } }
        #[derive(Serialize)]
        pub struct Failure { pub message: String, pub detail: Option<Detail>, pub code: u32 }
        #[derive(Deserialize)]
        pub struct Detail { pub text: Box<str>, pub summary: u64 }
        #[derive(Serialize)]
        pub struct WireOnly { pub message: String }
        pub struct Unreached { pub reason: String }
        #[cfg(test)]
        mod tests { #[derive(Serialize)] pub struct EventPayload { pub error: String } }
    "#;
    let types = persisted::serde_types("contract.rs", source).expect("parse the counterexample");
    let mut items = persisted::persisted_prose(&types)
        .into_iter()
        .map(|(_, finding)| finding.item.unwrap_or_default())
        .collect::<Vec<_>>();
    items.sort_unstable();
    assert_eq!(
        items,
        ["Detail.text", "EventPayload::Other.note", "Failure.message"]
    );
}

#[test]
fn g7_key_table_and_vocabularies_equal_the_catalog() {
    assert_none("G7", check_keys(workspace()));
}

#[test]
fn g8_names_and_prefixes_follow_the_rules() {
    assert_none("G8", check_naming(workspace()));
}

#[test]
fn g9_merged_catalog_is_current_and_keeps_released_entries() {
    assert_none("G9", check_contract(workspace()));
}

#[test]
fn allow_list_names_members_once_with_a_reason() {
    let root = workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")));
    let members = workspace_members(&root).expect("read the workspace members");
    let mut seen = BTreeSet::new();
    for entry in ALLOW_LIST {
        assert!(
            !entry.reason.trim().is_empty(),
            "{} has no reason",
            entry.path
        );
        assert!(!entry.checks.is_empty(), "{} covers no check", entry.path);
        match entry.item {
            None => {
                assert!(
                    members.iter().any(|member| member.as_str() == entry.path),
                    "{} is not a workspace member",
                    entry.path
                );
                assert!(seen.insert(entry.path), "{} is listed twice", entry.path);
            }
            Some(item) => assert!(
                root.join(entry.path).is_file(),
                "{} (item {item}) is not a file",
                entry.path
            ),
        }
    }
}

#[test]
fn each_string_channel_is_caught() {
    let registered = BTreeSet::from(["lease_busy".to_owned()]);
    let source = r#"
        struct Refusal { code: String, failure_stage: &'static str, count: u64 }
        fn refuse(reason: impl Into<String>) -> Result<(), &'static str> { Err("lease_busy") }
        impl Refusal { fn code(&self) -> &str { "x" } }
        fn report() { let _ = serde_json::json!({"code": 1, "count": 2}); }
        impl From<String> for Code { fn from(text: String) -> Self { todo!() } }
        fn forge() { let _ = Location::__from_registry("forged_stage"); }
        fn compare(text: &str) -> bool { matches!(text, "lease_busy") }
        #[cfg(test)]
        mod tests { fn ignored(code: String) -> Result<(), String> { Ok(()) } }
    "#;
    let findings = inspect_channels(
        source,
        ChannelScope {
            registered: &registered,
            outcome_module: false,
        },
    )
    .expect("parse the counterexample");
    let count = |check: Check| {
        findings
            .iter()
            .filter(|finding| finding.check == check)
            .count()
    };
    // G2: two fields, one parameter, one Result error, one getter.
    assert_eq!(count(Check::G2), 5, "{findings:#?}");
    // G3: the json! key "code" (not "count").
    assert_eq!(count(Check::G3), 1, "{findings:#?}");
    // G4: From<String> for Code, and the registry constructor.
    assert_eq!(count(Check::G4), 2, "{findings:#?}");
    // G5: the Err literal and the matches! literal.
    assert_eq!(count(Check::G5), 2, "{findings:#?}");
}

fn changed_workspace() -> Workspace {
    workspace().clone()
}

fn assert_reported(check: &str, violations: &[String], expected: &str) {
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains(expected)),
        "{check} did not report {expected:?}:\n{}",
        violations.join("\n")
    );
}

#[test]
fn g1_reports_a_registry_entry_the_fragments_lack() {
    let mut changed = changed_workspace();
    let catalog = changed.catalog.as_mut().expect("the catalog loads");
    catalog.codes.remove("panic_caught");
    assert_reported(
        "G1",
        &check_registries(&changed),
        "code panic_caught is in no fragment's codes",
    );
}

#[test]
fn g7_reports_a_retyped_key_and_a_token_the_enum_lacks() {
    let mut changed = changed_workspace();
    let catalog = changed.catalog.as_mut().expect("the catalog loads");
    if let Some(entry) = catalog.keys.get_mut("path") {
        entry["type"] = serde_json::Value::from("name");
    }
    if let Some(tokens) = catalog
        .vocabularies
        .get_mut("raw_source")
        .and_then(|vocabulary| vocabulary.get_mut("tokens"))
        .and_then(serde_json::Value::as_object_mut)
    {
        tokens.insert(
            "satellite".to_owned(),
            serde_json::json!({"description": "A token no enum has."}),
        );
    }
    let violations = check_keys(&changed);
    assert_reported("G7", &violations, "key path is name in the catalog");
    assert_reported("G7", &violations, "token satellite is not in the enum");
}

#[test]
fn g8_reports_an_unowned_prefix_and_a_reserved_prefix() {
    let mut changed = changed_workspace();
    let catalog = changed.catalog.as_mut().expect("the catalog loads");
    let entry = catalog
        .codes
        .get("foreign_os_error")
        .cloned()
        .expect("the contract fragment lists foreign_os_error");
    catalog
        .codes
        .insert("orphan_thing_failed".to_owned(), entry.clone());
    catalog.codes.insert("setup_thing_failed".to_owned(), entry);
    let violations = check_naming(&changed);
    assert_reported("G8", &violations, "orphan_thing_failed: no domain");
    assert_reported("G8", &violations, "prefix setup_ is reserved for the UI");
}

/// #378 A2b (board comment 6108644961, spec amendment A-1..A-4: `setup_` and `console_` are the
/// UI's): a Runtime entry under a UI prefix is refused even when a longer domain that lists its
/// owner also matches it.
#[test]
fn gate_g8_reports_a_ui_prefix_under_a_longer_runtime_domain() {
    let mut changed = changed_workspace();
    let catalog = changed.catalog.as_mut().expect("the catalog loads");
    let mut entry = catalog
        .codes
        .get("runtime_restart_required")
        .cloned()
        .expect("the runtime-host fragment lists runtime_restart_required");
    entry.insert("owner".to_owned(), serde_json::Value::from("runtime-host"));
    catalog.domains.insert(
        "setup_wizard_".to_owned(),
        serde_json::json!(["runtime-host"]),
    );
    catalog
        .codes
        .insert("setup_wizard_step_failed".to_owned(), entry);
    assert_reported(
        "G8",
        &check_naming(&changed),
        "code setup_wizard_step_failed: the prefix setup_ is reserved for the UI",
    );
}

/// #378 A2b (board comment 6108644961, spec amendment A-1..A-4: UI-only keys live only in the
/// UI's table): a `setup_` or `console_` key is reported in the catalog key table and in
/// `outcome_keys!` alike, never skipped.
#[test]
fn gate_g7_reports_ui_prefixed_keys_on_either_side() {
    let mut changed = changed_workspace();
    let catalog = changed.catalog.as_mut().expect("the catalog loads");
    catalog.keys.insert(
        "setup_step".to_owned(),
        serde_json::json!({"type": "token", "description": "A UI-only key."}),
    );
    let table = changed
        .files
        .iter_mut()
        .filter(|file| file.member == "crates/actingcommand-contract")
        .find_map(|file| file.scan.facts.key_tables.first_mut())
        .expect("the contract holds the outcome_keys! table");
    table.push(actingcommand_outcome_guard::source::KeyLine {
        name: "console_view".to_owned(),
        kind: "token".to_owned(),
        fields: Vec::new(),
    });
    let violations = check_keys(&changed);
    assert_reported(
        "G7",
        &violations,
        "G7 catalog key setup_step: the setup_ and console_ prefixes are the UI's",
    );
    assert_reported(
        "G7",
        &violations,
        "G7 outcome_keys! key console_view: the setup_ and console_ prefixes are the UI's",
    );
}

#[test]
fn g9_reports_a_stale_merged_file_and_a_changed_settled_entry() {
    let mut changed = changed_workspace();
    let catalog = changed.catalog.as_mut().expect("the catalog loads");
    if let Some(entry) = catalog.codes.get_mut("foreign_os_error") {
        entry.insert(
            "description".to_owned(),
            serde_json::Value::from("Another description."),
        );
    }
    assert_reported(
        "G9",
        &check_contract(&changed),
        "is not the merge of the fragments",
    );

    let snapshot = serde_json::json!({
        "schema_version": RELEASE_SCHEMA_VERSION,
        "release": "0.12.0",
        "codes": {"foreign_os_error": {
            "category": "fatal", "review": "settled", "status": "active",
            "values": {"raw_text": {"type": "evidence", "required": true}}
        }}
    });
    let current = workspace().catalog.as_ref().expect("the catalog loads");
    let violations = check_snapshot(
        "released/0.12.0.json",
        snapshot.as_object().expect("a snapshot object"),
        current,
    );
    assert_reported(
        "G9",
        &violations,
        "settled foreign_os_error changed its category",
    );
    assert_reported("G9", &violations, "gained the required key raw_source");
}

/// #378 A2a T6 (a, b) (brief-a2a.md; model-378 §16 A-9 :631; A1 acceptance carry-over 3): a
/// `Result` whose error is a tuple or an array holding a string type is a G2 finding, and so is
/// a non-test `include!`; a tuple of numbers and an `include!` in a test module are not.
#[test]
fn gate_g2_reports_string_tuple_and_array_errors_and_a_non_test_include() {
    let registered = BTreeSet::new();
    let source = r#"
        fn pair() -> Result<(), (&'static str, &'static str)> { Ok(()) }
        fn listed() -> Result<(), [String; 2]> { Ok(()) }
        fn counted() -> Result<(), (u64, u64)> { Ok(()) }
        include!("generated.rs");
        #[cfg(test)]
        mod tests { include!("fixtures.rs"); }
    "#;
    let findings = inspect_channels(
        source,
        ChannelScope {
            registered: &registered,
            outcome_module: false,
        },
    )
    .expect("parse the counterexample");
    let mut details = findings
        .iter()
        .filter(|finding| finding.check == Check::G2)
        .map(|finding| finding.detail.as_str())
        .collect::<Vec<_>>();
    details.sort_unstable();
    assert_eq!(
        details,
        [
            "include! pulls in source the guard cannot see",
            "listed returns a Result whose error is a string",
            "pair returns a Result whose error is a string",
        ],
        "{findings:#?}"
    );
}

/// #378 A2a T6 (c) (brief-a2a.md; model-378 §16 A-9 :631; A1 acceptance carry-over 3): an
/// `outcome_codes!` invocation that does not parse registers nothing and is a G1 error, never
/// skipped.
#[test]
fn gate_g1_reports_a_registry_invocation_that_does_not_parse() {
    let source = r#"
        actingcommand_contract::outcome_codes! {
            pub enum BrokenCode {
                EntryMissing => "registry_entry_missing",
                EntryUncategorised => "registry_entry_uncategorised"
            }
        }
    "#;
    let scan = scan_file(source, None).expect("the file itself parses");
    assert!(scan.facts.registries.is_empty(), "{:#?}", scan.facts);
    assert_eq!(scan.facts.errors.len(), 1, "{:#?}", scan.facts);
    let path = "crates/actingcommand-contract/src/broken_registry.rs";
    let mut changed = changed_workspace();
    changed.files.push(ScannedFile {
        member: "crates/actingcommand-contract".to_owned(),
        path: path.to_owned(),
        scan,
    });
    assert_reported(
        "G1",
        &check_registries(&changed),
        &format!("G1 {path}: line 2: outcome_codes! does not parse"),
    );
}

/// #387 D1a C-5 G1b (model-387 v1.2 §8.2 row C-5 G1b, §3.2): every catalog code carries a
/// `ladder_class` that is a token of the vocabulary `ladder_class`, whose tokens are the seven
/// classes of the decision table; a `stability_warning` code is of category warning or error.
/// The guard refuses a missing or unknown class and a `stability_warning` of another category,
/// and a settled code whose class changed turns G9 red.
#[test]
fn gate_c5_g1b_every_code_has_a_ladder_class_and_a_settled_class_stays() {
    const CLASSES: [&str; 7] = [
        "planned_stop",
        "budget_or_permission",
        "declaration_error",
        "pack_logic_mismatch",
        "environment_fault",
        "stability_warning",
        "none",
    ];
    let catalog = workspace().catalog.as_ref().expect("the catalog loads");
    let tokens = catalog
        .vocabularies
        .get("ladder_class")
        .and_then(|vocabulary| vocabulary.get("tokens"))
        .and_then(serde_json::Value::as_object)
        .map(|tokens| tokens.keys().map(String::as_str).collect::<BTreeSet<_>>())
        .unwrap_or_default();
    assert_eq!(
        tokens,
        BTreeSet::from(CLASSES),
        "the vocabulary ladder_class"
    );
    for (name, entry) in &catalog.codes {
        let class = entry
            .get("ladder_class")
            .and_then(serde_json::Value::as_str);
        assert!(
            class.is_some_and(|class| tokens.contains(class)),
            "{name}: ladder_class {class:?} is not a token of the vocabulary ladder_class"
        );
        if class == Some("stability_warning") {
            let category = entry.get("category").and_then(serde_json::Value::as_str);
            assert!(
                matches!(category, Some("warning" | "error")),
                "{name}: a stability_warning code of category {category:?}"
            );
        }
    }

    // The guard's check of one code entry: (ladder_class, category, accepted).
    let probe = catalog
        .codes
        .get("foreign_os_error")
        .cloned()
        .expect("the contract fragment lists foreign_os_error");
    let cases: [(Option<&str>, &str, bool); 7] = [
        (None, "error", false),
        (Some("satellite"), "error", false),
        (Some("stability_warning"), "info", false),
        (Some("stability_warning"), "fatal", false),
        (Some("stability_warning"), "warning", true),
        (Some("stability_warning"), "error", true),
        (Some("pack_logic_mismatch"), "error", true),
    ];
    for (class, category, accepted) in cases {
        let mut entry = probe.clone();
        entry.remove("ladder_class");
        if let Some(class) = class {
            entry.insert("ladder_class".to_owned(), class.into());
        }
        entry.insert("category".to_owned(), category.into());
        let errors = check_ladder_class(&entry, &catalog.vocabularies, "probe");
        assert_eq!(
            errors.is_empty(),
            accepted,
            "ladder_class {class:?}, category {category}: {errors:?}"
        );
    }

    // G9: a released settled entry keeps its class.
    let current = probe
        .get("ladder_class")
        .and_then(serde_json::Value::as_str)
        .expect("foreign_os_error names its ladder_class");
    let other = CLASSES
        .into_iter()
        .find(|class| *class != current)
        .expect("another class");
    let values = probe
        .get("values")
        .and_then(serde_json::Value::as_object)
        .expect("foreign_os_error has values")
        .iter()
        .map(|(key, presence)| {
            let kind = catalog
                .keys
                .get(key)
                .and_then(|entry| entry.get("type"))
                .cloned()
                .expect("a value key is in the key table");
            let required = presence.as_str() == Some("required");
            (
                key.clone(),
                serde_json::json!({"type": kind, "required": required}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    for (class, changed) in [(current, false), (other, true)] {
        let snapshot = serde_json::json!({
            "schema_version": RELEASE_SCHEMA_VERSION,
            "release": "0.12.0",
            "codes": {"foreign_os_error": {
                "category": probe.get("category").cloned().expect("a category"),
                "ladder_class": class,
                "review": "settled",
                "status": "active",
                "values": values.clone()
            }}
        });
        let violations = check_snapshot(
            "released/0.12.0.json",
            snapshot.as_object().expect("a snapshot object"),
            catalog,
        );
        if changed {
            assert_reported(
                "G9",
                &violations,
                "G9 released/0.12.0.json: settled foreign_os_error changed its ladder_class",
            );
        } else {
            assert_none("G9", violations);
        }
    }
}

/// #378 A8b (model-378 §9 G10, §5.2, §5.3; runtime-outcomes.md, "Release snapshot"): every
/// settled entry of the catalog has its text, and the v0.12.0 release has its snapshot: the
/// `actingcommand.outcome-codes.release.v1` document of release 0.12.0, whose every code holds
/// exactly `category`, `ladder_class`, `review`, `status` and `values` (each key's `type` and
/// `required`) and every location exactly `review` and `status`. G10 holds for releasing
/// v0.12.0.
#[test]
fn gate_g10_settled_entries_have_text_and_v0120_has_its_snapshot() {
    assert_none("G10", check_review(workspace()));
    let root = workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert_none("G10 release 0.12.0", release_gate(&root, "0.12.0"));

    let path = root.join(RELEASED_DIR).join("0.12.0.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let snapshot: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    assert_eq!(
        members(&snapshot),
        BTreeSet::from(["codes", "locations", "release", "schema_version"]),
        "the members of the snapshot"
    );
    assert_eq!(snapshot["schema_version"], RELEASE_SCHEMA_VERSION);
    assert_eq!(snapshot["release"], "0.12.0");
    let codes = snapshot["codes"].as_object().expect("codes is an object");
    let locations = snapshot["locations"]
        .as_object()
        .expect("locations is an object");
    assert!(!codes.is_empty(), "the v0.12.0 snapshot lists codes");
    assert!(
        !locations.is_empty(),
        "the v0.12.0 snapshot lists locations"
    );
    let categories = ["success", "info", "warning", "error", "fatal"];
    let reviews = ["pending", "settled"];
    let statuses = ["active", "retired"];
    let token = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_owned();
    for (name, entry) in codes {
        assert_eq!(
            members(entry),
            BTreeSet::from(["category", "ladder_class", "review", "status", "values"]),
            "code {name}"
        );
        assert!(
            categories.contains(&token(&entry["category"]).as_str()),
            "code {name}: category"
        );
        assert!(
            entry["ladder_class"].is_string(),
            "code {name}: ladder_class"
        );
        assert!(
            reviews.contains(&token(&entry["review"]).as_str()),
            "code {name}: review"
        );
        assert!(
            statuses.contains(&token(&entry["status"]).as_str()),
            "code {name}: status"
        );
        let values = entry["values"].as_object().expect("values is an object");
        for (key, value) in values {
            assert_eq!(
                members(value),
                BTreeSet::from(["required", "type"]),
                "code {name} key {key}"
            );
            assert!(value["type"].is_string(), "code {name} key {key}: type");
            assert!(
                value["required"].is_boolean(),
                "code {name} key {key}: required"
            );
        }
    }
    for (name, entry) in locations {
        assert_eq!(
            members(entry),
            BTreeSet::from(["review", "status"]),
            "location {name}"
        );
        assert!(
            reviews.contains(&token(&entry["review"]).as_str()),
            "location {name}: review"
        );
        assert!(
            statuses.contains(&token(&entry["status"]).as_str()),
            "location {name}: status"
        );
    }
}

/// The member names of a JSON object (none for any other value).
fn members(value: &serde_json::Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// The name of the first entry of `table` whose `review` is `review`.
fn first_with_review(
    table: &BTreeMap<String, serde_json::Map<String, serde_json::Value>>,
    review: &str,
) -> Option<String> {
    table
        .iter()
        .find(|(_, entry)| entry.get("review").and_then(serde_json::Value::as_str) == Some(review))
        .map(|(name, _)| name.clone())
}

/// #378 A8b (model-378 §9 G10 "a settled entry without text"; §5.2: `description` is always
/// written, `common_causes` and `handling` once settled, `TBD` before): a settled code whose
/// description, common causes or handling is `TBD` or empty, and a settled location whose
/// description is `TBD`, are G10 violations; the same code still pending with `TBD` is not.
#[test]
fn gate_g10_reports_a_settled_entry_without_text() {
    let base = workspace().catalog.as_ref().expect("the catalog loads");
    let code = first_with_review(&base.codes, "settled")
        .expect("the v0.12.0 catalog holds settled codes (A7)");
    let location = first_with_review(&base.locations, "settled")
        .expect("the v0.12.0 catalog holds settled locations (A7)");
    for member in ["description", "common_causes", "handling"] {
        for unwritten in ["TBD", " "] {
            let mut changed = base.clone();
            if let Some(entry) = changed.codes.get_mut(&code) {
                entry.insert(member.to_owned(), unwritten.into());
            }
            assert_reported(
                "G10",
                &review_of(&changed),
                &format!("G10 settled code {code}: {member} is not written"),
            );
        }
    }
    let mut changed = base.clone();
    if let Some(entry) = changed.locations.get_mut(&location) {
        entry.insert("description".to_owned(), "TBD".into());
    }
    assert_reported(
        "G10",
        &review_of(&changed),
        &format!("G10 settled location {location}: description is not written"),
    );

    let mut pending = base.clone();
    if let Some(entry) = pending.codes.get_mut(&code) {
        entry.insert("review".to_owned(), "pending".into());
        entry.insert("handling".to_owned(), "TBD".into());
    }
    assert_none("G10", review_of(&pending));
}

/// #378 A8b (model-378 §9 G10, review L14: a release tag fails without
/// `released/<version>.json`; §5.3 "Settlement timing" as the coordinator's decision of
/// 2026-10-19 moves it: from v0.13.0 on the release gate fails while any entry is pending):
/// releasing X.Y.Z without its own snapshot is a G10 violation; v0.12.0 releases with pending
/// entries; v0.13.0 fails while any code or location is pending and passes once every entry is
/// settled with its text.
#[test]
fn gate_g10_a_release_needs_its_snapshot_and_v0130_needs_every_entry_settled() {
    let catalog = workspace().catalog.as_ref().expect("the catalog loads");
    let snapshots = |release: &str| BTreeSet::from([release.to_owned()]);
    assert_reported(
        "G10",
        &check_release(catalog, "0.12.0", &BTreeSet::new()),
        &format!("G10 release 0.12.0: {RELEASED_DIR}/0.12.0.json is missing"),
    );
    assert_none(
        "G10",
        check_release(catalog, "0.12.0", &snapshots("0.12.0")),
    );
    assert_reported(
        "G10",
        &check_release(catalog, "0.12.1", &snapshots("0.12.0")),
        &format!("G10 release 0.12.1: {RELEASED_DIR}/0.12.1.json is missing"),
    );

    let code =
        first_with_review(&catalog.codes, "pending").expect("the 0.12 catalog holds pending codes");
    let location = first_with_review(&catalog.locations, "pending")
        .expect("the 0.12 catalog holds pending locations");
    let violations = check_release(catalog, "0.13.0", &snapshots("0.13.0"));
    assert_reported(
        "G10",
        &violations,
        &format!("G10 release 0.13.0: code {code} is still pending"),
    );
    assert_reported(
        "G10",
        &violations,
        &format!("G10 release 0.13.0: location {location} is still pending"),
    );

    let mut settled = catalog.clone();
    for entry in settled.codes.values_mut() {
        entry.insert("review".to_owned(), "settled".into());
        for member in ["description", "common_causes", "handling"] {
            let unwritten = entry
                .get(member)
                .and_then(serde_json::Value::as_str)
                .is_none_or(|text| text.trim().is_empty() || text.trim() == "TBD");
            if unwritten {
                entry.insert(member.to_owned(), "Written for maintainers.".into());
            }
        }
    }
    for entry in settled.locations.values_mut() {
        entry.insert("review".to_owned(), "settled".into());
        let unwritten = entry
            .get("description")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|text| text.trim().is_empty() || text.trim() == "TBD");
        if unwritten {
            entry.insert("description".to_owned(), "Written for maintainers.".into());
        }
    }
    assert_none(
        "G10",
        check_release(&settled, "0.13.0", &snapshots("0.13.0")),
    );
}

/// #378 G10 (Alice 2026-10-19: the 0.12 series releases plain Z versions, no `-rc.N`; the
/// coordinator's decision of that day: no pending entry may reach v0.13.0, and a final of the
/// 0.12 series may carry pending entries): a final v0.12.1 with its snapshot passes the release
/// gate while codes and locations are still pending.
#[test]
fn gate_g10_a_final_v0121_release_passes_with_pending_entries() {
    let catalog = workspace().catalog.as_ref().expect("the catalog loads");
    assert!(
        first_with_review(&catalog.codes, "pending").is_some()
            && first_with_review(&catalog.locations, "pending").is_some(),
        "the 0.12 catalog holds pending codes and locations"
    );
    assert_none(
        "G10 release 0.12.1",
        check_release(catalog, "0.12.1", &BTreeSet::from(["0.12.1".to_owned()])),
    );
}

/// #378 A8b (model-378 §9 G10, review L14): the exact-SHA build runs `outcome-guard release` on
/// the commit it releases, in a step that runs exactly when it releases, fails the run on a
/// non-zero exit and comes before the step that creates the tag and the Release.
#[test]
fn gate_g10_the_release_build_runs_the_release_gate_before_it_publishes() {
    const BUILD_WORKFLOW: &str = ".github/workflows/windows-remote-build.yml";
    let root = workspace_root(Path::new(env!("CARGO_MANIFEST_DIR")));
    let workflow = fs::read_to_string(root.join(BUILD_WORKFLOW))
        .unwrap_or_else(|error| panic!("read {BUILD_WORKFLOW}: {error}"))
        .replace("\r\n", "\n");
    let steps = workflow
        .split("\n      - name: ")
        .skip(1)
        .collect::<Vec<_>>();
    let gate = steps
        .iter()
        .position(|step| step.contains("--bin outcome-guard -- release "))
        .unwrap_or_else(|| panic!("{BUILD_WORKFLOW}: no step runs outcome-guard release"));
    let publish = steps
        .iter()
        .position(|step| step.starts_with("Publish the release\n"))
        .unwrap_or_else(|| panic!("{BUILD_WORKFLOW}: no Publish the release step"));
    assert!(
        steps[gate].contains("\n        if: ${{ inputs.release_tag }}\n"),
        "the gate step runs exactly when the build releases"
    );
    assert!(
        steps[gate].contains("if ($LASTEXITCODE -ne 0) {") && steps[gate].contains("throw "),
        "the gate step fails the run when outcome-guard release fails"
    );
    assert!(
        gate < publish,
        "the gate runs before the release is published"
    );
}
