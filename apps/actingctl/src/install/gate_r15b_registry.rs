// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-15b (frozen design `rd-1031/design-r15b-ab-v4.1.md` §7 R-15b, "码表和词表的一致性"):
// every code and every registered word the contracts list in their section "Registered codes
// and words (Workflow #429 R-15b)" (contracts/install-cli.md and contracts/stable-entry.md) is
// in `contracts/outcome-codes`: a token of an extended vocabulary in the catalog's vocabulary and
// in the enum of this build that builds it, a new vocabulary exactly the catalog's, each key in
// the key table, each location and code in its owner's fragment; every `entry_*` code
// stable-entry.md names is registered; R-7a's value keys are the catalog's own, not registered
// again. Only files of the repository are read.

use crate::codes::{InstallCaveat, InstallGuard, InstallLocation, InstallRefusal, ReleaseCheck};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const HEADING: &str = "### Registered codes and words (Workflow #429 R-15b)";

fn contracts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts")
}

fn read_json(path: &Path) -> Value {
    let text =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The lines of `text` outside its fenced code blocks.
fn prose(text: &str) -> String {
    let mut fenced = false;
    let mut kept = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

/// The words between backticks of `text`, in order.
fn backticked(text: &str) -> Vec<String> {
    text.split('`')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, word)| word.to_owned())
        .collect()
}

/// The items of the registry section of `document`: each `- ` line with its indented
/// continuation lines joined.
fn registry_items(document: &str) -> Vec<String> {
    let text = fs::read_to_string(contracts().join(document))
        .unwrap_or_else(|error| panic!("{document}: {error}"));
    let mut lines = text.lines();
    assert!(
        lines.any(|line| line == HEADING),
        "{document} has the section {HEADING}"
    );
    let mut items: Vec<String> = Vec::new();
    for line in lines {
        if line.starts_with('#') {
            break;
        }
        if let Some(item) = line.strip_prefix("- ") {
            items.push(item.to_owned());
        } else if let Some(more) = line.strip_prefix("  ")
            && let Some(last) = items.last_mut()
        {
            last.push(' ');
            last.push_str(more.trim());
        }
    }
    assert!(!items.is_empty(), "{document}: the registry lists nothing");
    items
}

/// A catalog object's member names.
fn names(object: &Value, member: &str) -> BTreeSet<String> {
    object[member]
        .as_object()
        .unwrap_or_else(|| panic!("{member} is an object"))
        .keys()
        .cloned()
        .collect()
}

/// The tokens a vocabulary enum of this build holds.
fn enum_tokens(vocabulary: &str) -> Option<BTreeSet<&'static str>> {
    let tokens: Vec<&'static str> = match vocabulary {
        "install_refusal" => InstallRefusal::ALL
            .iter()
            .map(|token| token.as_str())
            .collect(),
        "install_caveat" => InstallCaveat::ALL
            .iter()
            .map(|token| token.as_str())
            .collect(),
        "release_check" => ReleaseCheck::ALL
            .iter()
            .map(|token| token.as_str())
            .collect(),
        "install_guard" => InstallGuard::ALL
            .iter()
            .map(|token| token.as_str())
            .collect(),
        _ => return None,
    };
    Some(tokens.into_iter().collect())
}

/// Checks one document's registry against the catalog and the fragments; the violations.
fn check_registry(document: &str, catalog: &Value) -> Vec<String> {
    let codes = contracts().join("outcome-codes");
    let mut violations = Vec::new();
    for item in registry_items(document) {
        let (head, list) = item
            .split_once(':')
            .unwrap_or_else(|| panic!("{document}: an item without a colon: {item}"));
        let listed = backticked(list);
        let head_words = backticked(head);
        if head.starts_with("vocabulary ") {
            let vocabulary = head_words.first().expect("the vocabulary's name");
            let Some(entry) = catalog["vocabularies"].get(vocabulary.as_str()) else {
                violations.push(format!("{document}: no catalog vocabulary {vocabulary}"));
                continue;
            };
            let tokens = names(entry, "tokens");
            for token in &listed {
                if !tokens.contains(token) {
                    violations.push(format!(
                        "{document}: {token} is not in catalog vocabulary {vocabulary}"
                    ));
                }
            }
            if head.contains("(new)") && listed.iter().cloned().collect::<BTreeSet<_>>() != tokens {
                violations.push(format!(
                    "{document}: new vocabulary {vocabulary} lists other tokens than the catalog"
                ));
            }
            if let Some(built) = enum_tokens(vocabulary) {
                for token in &listed {
                    if !built.contains(token.as_str()) {
                        violations.push(format!(
                            "{document}: {token} of {vocabulary} is not in this build's enum"
                        ));
                    }
                }
            }
        } else if head == "keys" {
            let keys = names(catalog, "keys");
            for key in &listed {
                if !keys.contains(key) {
                    violations.push(format!("{document}: key {key} is not in the catalog"));
                }
            }
        } else if let Some(kind) = ["codes", "locations"]
            .into_iter()
            .find(|kind| head.starts_with(format!("{kind} of ").as_str()))
        {
            let owner = head_words.first().expect("the owner");
            let fragment = read_json(&codes.join(format!("{owner}.json")));
            let registered = names(&fragment, kind);
            for name in &listed {
                if !registered.contains(name) {
                    violations.push(format!("{document}: {name} is not in {owner}'s {kind}"));
                } else if fragment[kind][name.as_str()]["since"] != Value::from("0.12.35") {
                    violations.push(format!("{document}: {name} is not since 0.12.35"));
                }
            }
        } else {
            violations.push(format!("{document}: an item of no known kind: {item}"));
        }
    }
    violations
}

#[test]
fn r15b_every_code_and_word_of_the_contracts_is_registered() {
    let catalog = read_json(&contracts().join("outcome-codes").join("catalog.json"));
    let mut violations = check_registry("install-cli.md", &catalog);
    violations.extend(check_registry("stable-entry.md", &catalog));

    // Every `entry_*` code stable-entry.md names anywhere is a code of the forwarder's owner.
    let entry = read_json(
        &contracts()
            .join("outcome-codes")
            .join("actingcommand-entry.json"),
    );
    let entry_codes = names(&entry, "codes");
    let stable_entry =
        fs::read_to_string(contracts().join("stable-entry.md")).expect("stable-entry.md");
    // A vocabulary's or a key's name is no code.
    let other_names = names(&catalog, "vocabularies")
        .into_iter()
        .chain(names(&catalog, "keys"))
        .collect::<BTreeSet<_>>();
    for word in backticked(&prose(&stable_entry)) {
        if word.starts_with("entry_")
            && !word.contains(' ')
            && !entry_codes.contains(&word)
            && !other_names.contains(&word)
        {
            violations.push(format!(
                "stable-entry.md names {word}, which is no entry code"
            ));
        }
    }
    for (name, code) in entry["codes"].as_object().expect("codes") {
        if code["owner"] != Value::from("actingcommand-entry") {
            violations.push(format!("{name} is not owned by actingcommand-entry"));
        }
    }
    assert_eq!(
        catalog["fragments"]["actingcommand-entry"],
        Value::from("apps/actingcommand-entry"),
        "the forwarder's owner has its fragment"
    );

    // R-7a's value keys are referenced, not registered again: they are the catalog's already.
    let keys = names(&catalog, "keys");
    for key in [
        "hold_phase",
        "alias",
        "source_actor",
        "source_channel",
        "source_via",
        "ended_by_actor",
        "ended_by_channel",
        "ended_by_via",
    ] {
        if !keys.contains(key) {
            violations.push(format!("R-7a's key {key} is not in the catalog"));
        }
    }

    // The new commands' locations are this build's.
    for location in [
        "install_prepare",
        "install_convert_plan",
        "install_convert_apply",
        "install_clean",
        "install_restart",
    ] {
        if !InstallLocation::ALL
            .iter()
            .any(|known| known.as_str() == location)
        {
            violations.push(format!("{location} is not in InstallLocation"));
        }
    }
    assert!(
        violations.is_empty(),
        "the contracts and the outcome codes disagree:\n{}",
        violations.join("\n")
    );
}
