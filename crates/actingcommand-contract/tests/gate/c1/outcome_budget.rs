// SPDX-License-Identifier: AGPL-3.0-only

//! G14, inventory key K12: the outcome budget (Workflow #378 model §3.2 and §4.2). A list keeps
//! at most 64 items and a cut adds `<key>_total`; an envelope is at most 32 KiB, and on overflow
//! the emitter cuts `raw_text` from the deepest link up, then drops the deepest links and sets
//! `causes_total`; links are at most 32 within depth 8.

use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::{Detail, Outcome, RawSource};
use serde_json::Value;
use std::collections::BTreeMap;

/// Model §3.2: "An envelope is at most 32 KiB."
const ENVELOPE_MAX_BYTES: usize = 32 * 1024;
/// Model §2.5: the `raw_text` bound is 4096 B.
const RAW_TEXT_MAX_BYTES: usize = 4096;

#[test]
fn gate_a_list_value_keeps_64_items_and_records_its_total() {
    let outcome = Outcome::new(ContractCode::ForeignOtherError)
        .aliases((1..=70).map(|index| format!("node-{index:02}")));

    assert_eq!(
        outcome
            .value("aliases")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(64)
    );
    assert_eq!(
        outcome.value("aliases_total").and_then(Value::as_u64),
        Some(70)
    );
}

/// Foreign text of exactly the `raw_text` bound, so setting it cuts nothing.
fn text_at_the_bound() -> String {
    let phrase = "the component reported a failure; ";
    let mut text = phrase.repeat(RAW_TEXT_MAX_BYTES / phrase.len() + 1);
    text.truncate(RAW_TEXT_MAX_BYTES);
    text
}

fn foreign_failure(raw_text: &str) -> Outcome {
    Outcome::new(ContractCode::ForeignOtherError)
        .raw_source(RawSource::Other)
        .raw_text(raw_text)
}

fn envelope_bytes(outcome: &Outcome) -> Vec<u8> {
    let bytes = serde_json::to_vec(&outcome.envelope(Detail::Full)).expect("encode the envelope");
    assert!(
        bytes.len() <= ENVELOPE_MAX_BYTES,
        "the envelope has {} bytes",
        bytes.len()
    );
    bytes
}

#[test]
fn gate_an_oversized_chain_loses_raw_text_from_the_deepest_link_first() {
    crate::diagnostic_sink();
    let text = text_at_the_bound();
    // One chain of depth 8 below the top: nine outcomes of 4096 B `raw_text` each, over 32 KiB,
    // and within the 32-link and depth-8 bound, so no link cut applies.
    let mut chain = foreign_failure(&text);
    for _ in 0..8 {
        chain = foreign_failure(&text).caused_by(chain);
    }

    let envelope: Value =
        serde_json::from_slice(&envelope_bytes(&chain)).expect("parse the envelope");
    assert!(
        envelope.get("causes_total").is_none(),
        "no link is dropped: {}",
        envelope["causes_total"]
    );
    let mut cut = Vec::new();
    let mut kept = Vec::new();
    let mut node = &envelope;
    let mut depth = 0_usize;
    loop {
        let values = &node["values"];
        let raw_text = values["raw_text"]
            .as_str()
            .expect("every outcome keeps its raw_text");
        if raw_text.len() < RAW_TEXT_MAX_BYTES {
            assert_eq!(
                values["raw_text_truncated"],
                Value::Bool(true),
                "the cut raw_text at depth {depth} is marked"
            );
            cut.push(depth);
        } else {
            kept.push(depth);
        }
        match node.get("causes").and_then(Value::as_array) {
            Some(causes) => {
                assert_eq!(causes.len(), 1, "depth {depth}");
                node = &causes[0];
                depth += 1;
            }
            None => break,
        }
    }

    assert_eq!(depth, 8, "no link is dropped");
    assert!(
        !cut.is_empty() && !kept.is_empty(),
        "cut {cut:?}, kept {kept:?}"
    );
    assert!(
        cut.iter().min() > kept.iter().max(),
        "every cut raw_text is deeper than every kept one: cut {cut:?}, kept {kept:?}"
    );
}

/// Records each link's tag with its depth and checks that its `raw_text` was cut.
fn present_links(node: &Value, depth: usize, present: &mut BTreeMap<String, usize>) {
    for link in node
        .get("causes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let values = &link["values"];
        let tag = values["raw_code"]
            .as_str()
            .expect("every link carries its tag");
        assert_eq!(
            values["raw_text_truncated"],
            Value::Bool(true),
            "the raw_text of {tag} is cut"
        );
        present.insert(tag.to_owned(), depth + 1);
        present_links(link, depth + 1, present);
    }
}

#[test]
fn gate_an_oversized_tree_drops_its_deepest_links_after_every_raw_text_is_cut() {
    crate::diagnostic_sink();
    // An ordinary long file path within the 1024-byte `path` bound.
    let path = format!("C:/state/{}frame.png", "retained/frames/".repeat(60));
    // Four branches of depth 8: 32 links, so no link cut applies; each link is tagged with its
    // branch and depth. With every `raw_text` cut, the paths alone exceed 32 KiB.
    let mut tags = BTreeMap::new();
    let mut top = foreign_failure("the operation failed");
    for branch in 1..=4 {
        let mut below: Option<Outcome> = None;
        for depth in (1..=8_usize).rev() {
            let tag = format!("b{branch}-d{depth}");
            let mut link = Outcome::new(ContractCode::ForeignOsError)
                .raw_source(RawSource::Os)
                .raw_code(tag.as_str())
                .path(path.as_str())
                .raw_text("access is denied");
            if let Some(child) = below.take() {
                link = link.caused_by(child);
            }
            tags.insert(tag, depth);
            below = Some(link);
        }
        top = top.caused_by(below.expect("every branch has links"));
    }

    let envelope: Value =
        serde_json::from_slice(&envelope_bytes(&top)).expect("parse the envelope");
    assert_eq!(
        envelope["causes_total"].as_u64(),
        Some(tags.len() as u64),
        "the top counts every link"
    );
    assert_eq!(
        envelope["values"]["raw_text_truncated"],
        Value::Bool(true),
        "the raw_text of the top is cut"
    );
    let mut present = BTreeMap::new();
    present_links(&envelope, 0, &mut present);
    for (tag, depth) in &present {
        assert_eq!(tags.get(tag), Some(depth), "{tag} keeps its place");
    }
    let dropped = tags
        .iter()
        .filter(|(tag, _)| !present.contains_key(*tag))
        .map(|(_, depth)| *depth)
        .collect::<Vec<_>>();
    assert!(!dropped.is_empty(), "the tree is cut to the budget");
    assert!(
        present.values().max() <= dropped.iter().min(),
        "the deepest links go first: present {present:?}, dropped depths {dropped:?}"
    );
}
