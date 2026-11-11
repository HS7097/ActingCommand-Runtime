// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #428 S168-LB1a (since 0.12.55; register §1.309 rows LB1-1, LB1-2 and LB1-6, the
// slice's section §1.311; `contracts/instance-fact-store.md`, "List reading facts", "Row bound",
// and "The frame bound with long lists"): a `record_list` fact holds up to 1024 rows, a bound of
// its own that every reader of a record checks, while `MAX_FACT_RECORDS` stays 256 for the records
// of an observation, a snapshot and a view; the kernel's list reading reads to the same bound; and
// the instance fact read's request member `key_prefix` is absent unless used, so a request without
// it serializes as before. Every expected value is the contract's. The records are made up; the
// key is `inventory.list.example_kind_a`.

use super::*;
use crate::{RuntimeOperation, RuntimeResult, RuntimeStateSource};
use serde_json::json;

const INSTANCE: &str = "instance-a";
const LIST_KEY: &str = "inventory.list.example_kind_a";

fn instance_scope() -> FactScope {
    FactScope::Instance {
        instance_id: INSTANCE.to_owned(),
    }
}

// `count` rows `{id, rank}`, the ids in ascending byte order.
fn rows(count: usize) -> Vec<BTreeMap<String, FactScalar>> {
    (0..count)
        .map(|index| {
            BTreeMap::from([
                (
                    "id".to_owned(),
                    FactScalar::String(format!("member.{index:04}")),
                ),
                (
                    "rank".to_owned(),
                    FactScalar::Integer(i64::try_from(index).expect("rank")),
                ),
            ])
        })
        .collect()
}

// One instance record of `key` holding `value`, valid for a day from the detector contract.
fn record(key: &str, value: FactValue) -> FactRecord {
    FactRecord {
        scope: instance_scope(),
        key: key.to_owned(),
        content: FactContent::Inline { value },
        observed_at_unix_ms: 1_000,
        expires_at_unix_ms: Some(1_000 + 86_400_000),
        ttl_policy: Some(FactTtlPolicy {
            minimum_ms: 86_400_000,
            maximum_ms: 86_400_000,
            source: FactTtlSource::DetectorContract,
        }),
        confidence_milli: 990,
        source_detector: "list_reading:task/read_members".to_owned(),
        source_snapshot_id: "run:fixture/frame:fixture/read_members".to_owned(),
        schema_version: "fact.v1".to_owned(),
        resource_bundle_hash: "a".repeat(64),
        invalidate_on: Vec::new(),
    }
}

fn list_record(count: usize) -> FactRecord {
    record(LIST_KEY, FactValue::RecordList(rows(count)))
}

fn snapshot_of(records: Vec<FactRecord>) -> InstanceFactSnapshot {
    InstanceFactSnapshot {
        snapshot_id: "snapshot:lb1a".to_owned(),
        ledger_position: 1,
        context: InstanceFactContext {
            instance_id: INSTANCE.to_owned(),
            server_id: "server-a".to_owned(),
            game_id: "game-a".to_owned(),
        },
        records,
    }
}

fn view_of(records: Vec<FactRecord>) -> InstanceFactView {
    InstanceFactView {
        instance_alias: INSTANCE.to_owned(),
        server_id: None,
        game_id: None,
        records,
    }
}

fn code_of(error: &SanitizationError) -> &'static str {
    error.code().as_str()
}

// LB1-1 and LB1-2: the bound is a constant of its own, 1024, equal to the kernel's list reading
// bound, and the records' bound stays 256. A record of 257 and one of 1024 rows, which a reader of
// revision 7 refused (`fact_record_limit_exceeded`), are accepted by each reader the contract's
// "Row bound" lists: the record's own check, an observation's write check (a publication and a
// client's publication), the structural check of a decoded `fact.published`, and a snapshot's and
// a view's check of each record (the client checks an `InstanceFactSnapshot` answer by the view's).
// A record of 1025 rows is refused by each with `fact_record_limit_exceeded`.
#[test]
fn lb1a_a_record_list_of_up_to_1024_rows_is_read_by_every_reader() {
    assert_eq!(MAX_FACT_RECORD_LIST_ROWS, 1_024);
    assert_eq!(crate::MAX_LIST_READING_ROWS, MAX_FACT_RECORD_LIST_ROWS);
    assert_eq!(MAX_FACT_RECORDS, 256);

    for count in [257, 1_024] {
        let record = list_record(count);
        record
            .validate()
            .unwrap_or_else(|error| panic!("{count} rows: the record's own check: {error}"));
        FactObservation {
            records: vec![record.clone()],
        }
        .validate()
        .unwrap_or_else(|error| panic!("{count} rows: the observation's write check: {error}"));
        validate_fact_observation(std::iter::once(&record))
            .unwrap_or_else(|error| panic!("{count} rows: a decoded fact.published: {error}"));
        snapshot_of(vec![record.clone()])
            .validate()
            .unwrap_or_else(|error| panic!("{count} rows: a snapshot's check: {error}"));
        view_of(vec![record])
            .validate()
            .unwrap_or_else(|error| panic!("{count} rows: a view's check: {error}"));
    }

    let refused = SanitizationCode::FactRecordLimitExceeded.as_str();
    let record = list_record(1_025);
    assert_eq!(code_of(&record.validate().expect_err("1025 rows")), refused);
    assert_eq!(
        code_of(
            &FactObservation {
                records: vec![record.clone()],
            }
            .validate()
            .expect_err("1025 rows")
        ),
        refused
    );
    assert_eq!(
        code_of(&validate_fact_observation(std::iter::once(&record)).expect_err("1025 rows")),
        refused
    );
    assert_eq!(
        code_of(
            &snapshot_of(vec![record.clone()])
                .validate()
                .expect_err("1025 rows")
        ),
        refused
    );
    assert_eq!(
        code_of(&view_of(vec![record]).validate().expect_err("1025 rows")),
        refused
    );
}

// LB1-1: `MAX_FACT_RECORDS` keeps bounding what it bounds besides: an observation of 257 records
// is refused (`inconsistent_fact_observation`), and so are a snapshot and a view of 257 records
// (`invalid_fact_snapshot`); 256 are accepted.
#[test]
fn lb1a_the_records_of_an_observation_a_snapshot_and_a_view_stay_256() {
    let records = |count: usize| {
        (0..count)
            .map(|index| record(&format!("inventory.example_{index}"), FactValue::Integer(1)))
            .collect::<Vec<_>>()
    };
    FactObservation {
        records: records(256),
    }
    .validate()
    .expect("256 records an observation");
    snapshot_of(records(256))
        .validate()
        .expect("256 records a snapshot");
    view_of(records(256))
        .validate()
        .expect("256 records a view");

    assert_eq!(
        code_of(
            &FactObservation {
                records: records(257),
            }
            .validate()
            .expect_err("257 records an observation")
        ),
        SanitizationCode::InconsistentFactObservation.as_str()
    );
    assert_eq!(
        code_of(
            &snapshot_of(records(257))
                .validate()
                .expect_err("257 records a snapshot")
        ),
        SanitizationCode::InvalidFactSnapshot.as_str()
    );
    assert_eq!(
        code_of(
            &view_of(records(257))
                .validate()
                .expect_err("257 records a view")
        ),
        SanitizationCode::InvalidFactSnapshot.as_str()
    );
}

// LB1-6: a request without `key_prefix` serializes byte for byte as before and an earlier
// request decodes without it; a request with it names it and round-trips; an empty prefix and
// one longer than a key (192 bytes) are refused `invalid_fact_key`. An answer without
// `records_total` serializes without the member, and one with it names it.
#[test]
fn lb1a_the_instance_fact_read_names_its_new_members_only_when_used() {
    let without = RuntimeOperation::InstanceFactSnapshot {
        instance: INSTANCE.to_owned(),
        key_prefix: None,
    };
    let before = json!({"operation": "instance_fact_snapshot", "instance": INSTANCE});
    assert_eq!(
        serde_json::to_vec(&without).expect("request JSON"),
        serde_json::to_vec(&before).expect("earlier request JSON"),
        "a request without key_prefix serializes as before"
    );
    let decoded: RuntimeOperation = serde_json::from_value(before).expect("an earlier request");
    assert!(
        decoded == without,
        "an earlier request decodes without key_prefix"
    );
    without.validate().expect("a request without key_prefix");

    let with = RuntimeOperation::InstanceFactSnapshot {
        instance: INSTANCE.to_owned(),
        key_prefix: Some("inventory.list.".to_owned()),
    };
    let wire = serde_json::to_value(&with).expect("request JSON");
    assert_eq!(wire["key_prefix"], json!("inventory.list."));
    let round: RuntimeOperation = serde_json::from_value(wire).expect("the request decodes");
    assert!(round == with, "the request round-trips");
    with.validate().expect("a key prefix");

    for (case, prefix) in [("empty", String::new()), ("long", "i".repeat(193))] {
        let refused = RuntimeOperation::InstanceFactSnapshot {
            instance: INSTANCE.to_owned(),
            key_prefix: Some(prefix),
        }
        .validate()
        .expect_err(case);
        assert_eq!(
            refused.code().as_str(),
            SanitizationCode::InvalidFactKey.as_str(),
            "{case}"
        );
    }

    let source = RuntimeStateSource {
        instance_facts: None,
        runtime_facts: None,
        ledger_committed_head: 0,
        sampled_started_at_unix_ms: 1,
        sampled_completed_at_unix_ms: 2,
    };
    let answer = |records_total| RuntimeResult::InstanceFactSnapshot {
        snapshot: Box::new(view_of(Vec::new())),
        source: source.clone(),
        records_total,
    };
    let plain = serde_json::to_value(answer(None)).expect("answer JSON");
    assert!(plain.get("records_total").is_none(), "{plain}");
    let counted = serde_json::to_value(answer(Some(3))).expect("answer JSON");
    assert_eq!(counted["records_total"], json!(3));
}
