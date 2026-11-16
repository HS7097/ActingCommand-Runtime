// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-8 (register §1.444; contracts/runtime-interface.md, "The variable values of a
// contained run"; `runtime-client` 11; coordinator work order rd-1116/brief-463-s38-run-variables.md
// and the coordinator's ruling 1 of its take-over): the request member
// `ContainedTaskRequest.variables`.
// - `s38_a_request_without_values_is_todays_request_byte_for_byte`: a request built as a run start
//   without `--variable` builds it, and the same request given no values, encode byte for byte
//   alike and name no `variables`, alone and in the run's envelope; revision 10's encoding decodes
//   into that request, which validates.
// - `s38_a_request_with_values_round_trips`: values of the four kinds of a variable round-trip
//   under `variables` by name, alone and in the run's envelope, and validate; sixteen values and
//   names of 1 and 64 bytes pass; seventeen values are `invalid_contained_task_variable_count`, an
//   empty name and names outside a variable's form `invalid_contained_task_variable_name`, given
//   through `with_variables` and decoded from the wire alike.

use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PACKAGE: &str = "C:/ActingCommand/packs/neutral.trial";
const ALIAS: &str = "neutral.trial";

/// The request a run start without `--variable` builds: the package and a deadline.
fn todays() -> ContainedTaskRequest {
    ContainedTaskRequest::new(PACKAGE)
        .and_then(|request| request.with_response_deadline_ms(120_000))
        .expect("the request of today")
}

/// The run's envelope of `request`, as the Lab sends it.
fn run(ids: &IdentifierIssuer, request: ContainedTaskRequest) -> RuntimeRequest {
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Lab,
        EventSource::Lab,
        1,
        RuntimeOperation::run_contained_task(
            ALIAS,
            ids.mint_holder_id().expect("holder id"),
            request,
        ),
    )
    .expect("runtime request")
}

fn wire(value: &impl Serialize) -> Value {
    serde_json::to_value(value).expect("JSON")
}

/// `result` is the refusal `code`.
#[track_caller]
fn refused<T: fmt::Debug>(result: RuntimeContractResult<T>, code: RuntimeContractCode) {
    assert_eq!(result.expect_err("refused").code(), code.code());
}

#[test]
fn s38_a_request_without_values_is_todays_request_byte_for_byte() {
    let today = todays();
    let given_none = today
        .clone()
        .with_variables(BTreeMap::new())
        .expect("no values");
    let bytes = serde_json::to_vec(&today).expect("JSON");
    assert_eq!(
        String::from_utf8(bytes.clone()).expect("UTF-8"),
        r#"{"package_path":"C:/ActingCommand/packs/neutral.trial","response_deadline_ms":120000}"#
    );
    assert_eq!(serde_json::to_vec(&given_none).expect("JSON"), bytes);
    assert_eq!(given_none, today);
    assert!(today.variables().is_empty());

    // In the run's envelope: its request names no `variables`, and it round-trips.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let sent = run(&ids, given_none);
    let encoded = wire(&sent);
    assert_eq!(
        encoded["operation"]["request"],
        json!({"package_path": PACKAGE, "response_deadline_ms": 120_000}),
        "{encoded}"
    );
    let decoded: RuntimeRequest = serde_json::from_value(encoded).expect("the request decodes");
    assert_eq!(decoded, sent);
    decoded.validate().expect("the request validates");

    // Revision 10's encoding of the same request decodes into it.
    let revision_ten: ContainedTaskRequest = serde_json::from_slice(
        br#"{"package_path":"C:/ActingCommand/packs/neutral.trial","response_deadline_ms":120000}"#,
    )
    .expect("revision 10's request decodes");
    revision_ten.validate().expect("it validates");
    assert_eq!(revision_ten, today);
}

#[test]
fn s38_a_request_with_values_round_trips() {
    let values = BTreeMap::from([
        ("floor".to_owned(), json!(20)),
        ("pick".to_owned(), json!("item_a")),
        ("order".to_owned(), json!(["key_b", "key_a"])),
        ("skip".to_owned(), json!(true)),
    ]);
    let given = todays()
        .with_variables(values.clone())
        .expect("four values");
    assert_eq!(given.variables(), &values);
    let encoded = wire(&given);
    assert_eq!(
        encoded,
        json!({
            "package_path": PACKAGE,
            "response_deadline_ms": 120_000,
            "variables": {"floor": 20, "pick": "item_a", "order": ["key_b", "key_a"], "skip": true}
        })
    );
    let decoded: ContainedTaskRequest =
        serde_json::from_value(encoded).expect("the request decodes");
    decoded.validate().expect("the request validates");
    assert_eq!(decoded, given);

    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let sent = run(&ids, given);
    let encoded = wire(&sent);
    assert_eq!(
        encoded["operation"]["request"]["variables"],
        json!({"floor": 20, "pick": "item_a", "order": ["key_b", "key_a"], "skip": true})
    );
    let decoded: RuntimeRequest = serde_json::from_value(encoded).expect("the envelope decodes");
    decoded.validate().expect("the envelope validates");
    assert_eq!(decoded, sent);

    // As many values as a package declares variables, and names of 1 and 64 bytes, pass.
    let numbered = |count: usize| {
        (0..count)
            .map(|index| (format!("v{index:02}"), json!(index)))
            .collect::<BTreeMap<_, _>>()
    };
    let most = todays()
        .with_variables(numbered(crate::MAX_PACKAGE_VARIABLES))
        .expect("sixteen values");
    assert_eq!(most.variables().len(), crate::MAX_PACKAGE_VARIABLES);
    let long_name = format!("v{}", "x".repeat(63));
    todays()
        .with_variables(BTreeMap::from([
            ("v".to_owned(), json!(1)),
            (long_name.clone(), json!(2)),
        ]))
        .expect("names of 1 and 64 bytes");

    // One more value than a package can declare variables, and a name outside a variable's form,
    // each given and decoded.
    let count = RuntimeContractCode::InvalidContainedTaskVariableCount;
    let name_code = RuntimeContractCode::InvalidContainedTaskVariableName;
    let too_many = numbered(crate::MAX_PACKAGE_VARIABLES + 1);
    refused(todays().with_variables(too_many.clone()), count);
    let mut decoded_too_many = wire(&todays());
    decoded_too_many["variables"] = json!(too_many);
    let decoded: ContainedTaskRequest =
        serde_json::from_value(decoded_too_many).expect("the shape decodes");
    refused(decoded.validate(), count);
    for name in ["", "Floor", "1floor", "floor-a", "floor a"]
        .into_iter()
        .map(str::to_owned)
        .chain([format!("{long_name}x")])
    {
        refused(
            todays().with_variables(BTreeMap::from([(name.clone(), json!(1))])),
            name_code,
        );
        let mut encoded = wire(&todays());
        encoded["variables"] = json!({name.as_str(): 1});
        let decoded: ContainedTaskRequest =
            serde_json::from_value(encoded).expect("the shape decodes");
        refused(decoded.validate(), name_code);
        refused(
            RuntimeOperation::run_contained_task(
                ALIAS,
                ids.mint_holder_id().expect("holder id"),
                decoded,
            )
            .validate(),
            name_code,
        );
    }
}
