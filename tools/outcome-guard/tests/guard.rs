// SPDX-License-Identifier: AGPL-3.0-only

//! The outcome guard on this workspace (G1, G2-G5 outside the allow list, G7, G8, G9), plus
//! counterexamples that show each string channel is caught.

use actingcommand_outcome_guard::source::{ChannelScope, Check, inspect_channels};
use actingcommand_outcome_guard::{
    ALLOW_LIST, Workspace, check_channels, check_contract, check_keys, check_naming,
    check_registries, load_workspace, workspace_members, workspace_root,
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
