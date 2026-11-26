// SPDX-License-Identifier: AGPL-3.0-only

// #503 CYC-3 (0.12.86; register §1.529, the coordinator's rulings 1, 3 and 4; ledger revision 10),
// t4, the wire: the admission record that `policy.dispatch_admitted` and
// `policy.dispatch_completed` carry gains `window_until_unix_ms`, the end of the activity window
// the run was admitted in, beside `window_id`. A record with it and one without it read back
// unchanged, and the one without names no such member at all; a window end that is not later than
// the admission is refused where the record is written and where it is read; the public
// projection carries the admission without it, as the projection of a record without it.

use super::sh1::{AT_MS, admission, dispatch_data, holder, read_back, refused, written};
use super::*;
use serde_json::{Map, Value, json};

/// The end of the fixture admission's window: ten hours after it.
const UNTIL_MS: u64 = AT_MS + 36_000_000;
const MEMBER: &str = "window_until_unix_ms";

/// The fixture admission with the window end `until`.
fn admission_until(until: Option<u64>) -> PolicyAdmissionRecord {
    let mut record = admission();
    record.activity.window_until_unix_ms = until;
    record
}

/// The two dispatch records that carry `record`: the admitted one and the completed one.
fn dispatch_records(record: &PolicyAdmissionRecord) -> [EventPayloadDraft; 2] {
    [
        PolicyPayloadDraft::dispatch_admitted(dispatch_data(), record.clone(), AuditInput::new())
            .into(),
        PolicyPayloadDraft::dispatch_completed(dispatch_data(), record.clone(), AuditInput::new())
            .into(),
    ]
}

/// The stored record's activity sample, the object that holds `window_id`.
fn stored_activity(stored: &mut Value) -> &mut Map<String, Value> {
    holder(stored, "window_id").expect("the record's activity sample")
}

#[test]
fn cyc3_t4_an_admission_reads_back_with_its_window_end_and_without_it() {
    for until in [Some(UNTIL_MS), None] {
        let record = admission_until(until);
        for draft in dispatch_records(&record) {
            let stored = written(draft);
            let mut shown = stored.clone();
            assert_eq!(
                stored_activity(&mut shown).get(MEMBER),
                until.map(|until| json!(until)).as_ref(),
                "the member is written when present and as nothing at all when absent: {stored}"
            );
            let payload = read_back(&stored);
            let EventPayload::Policy(
                PolicyPayload::DispatchAdmitted(dispatch)
                | PolicyPayload::DispatchCompleted(dispatch),
            ) = &payload
            else {
                panic!("a dispatch record: {payload:?}");
            };
            assert_eq!(dispatch.admission(), Some(&record));
        }
    }
}

#[test]
fn cyc3_t4_a_window_end_not_later_than_the_admission_is_refused_where_written_and_read() {
    for until in [AT_MS, AT_MS - 1] {
        for draft in dispatch_records(&admission_until(Some(until))) {
            assert_eq!(
                sanitize_error(draft).code(),
                SanitizationCode::InvalidPolicyActivitySample.code(),
                "the writer refuses a window end of {until}"
            );
        }
        for draft in dispatch_records(&admission_until(Some(UNTIL_MS))) {
            let mut stored = written(draft);
            stored_activity(&mut stored).insert(MEMBER.to_owned(), json!(until));
            assert_eq!(
                refused(stored).code(),
                SanitizationCode::InvalidPolicyActivitySample.code(),
                "the reader refuses a window end of {until}"
            );
        }
    }
}

#[test]
fn cyc3_t4_the_public_projection_carries_the_admission_without_its_window_end() {
    let record = admission_until(Some(UNTIL_MS));
    for (index, draft) in (1_u64..).zip(dispatch_records(&record)) {
        let event = sanitize(draft, index);
        let PublicEventPayload::Policy(public) = event.payload().public_projection() else {
            panic!("a policy projection");
        };
        assert_eq!(
            public.policy_admission(),
            Some(&admission_until(None)),
            "the projection of the record is the one of the record without the member"
        );
    }
}
