// SPDX-License-Identifier: AGPL-3.0-only

//! The outcome guard on this workspace (G1, G2-G6 outside the allow list, G7, G8, G9), plus
//! counterexamples that show each check catches a break.

use actingcommand_outcome_guard::catalog::{RELEASE_SCHEMA_VERSION, check_snapshot};
use actingcommand_outcome_guard::source::{ChannelScope, Check, inspect_channels, scan_file};
use actingcommand_outcome_guard::{
    ALLOW_LIST, ScannedFile, Workspace, check_channels, check_contract, check_keys, check_naming,
    check_persisted_prose, check_registries, load_workspace, persisted, workspace_members,
    workspace_root,
};
use std::collections::BTreeSet;
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
