// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #450 FL-1a and FL-1b (register §1.340; `contracts/resource-readings.md`, "Lists from
// fields"): a field list groups the fields of one group of a task's `fields_v1` declaration, read
// one position per collected frame, into one list, whole or not at all.
//
// The builder's tests hand it frames made here. The run's tests play a scripted device: a frame's
// first pixel names its page (`item`, the detail page with a next arrow; `last`, the last detail
// page), the provider reads the name in column 1 (a code into the dictionary), the amount in
// column 2 (its red value), and their confidences from column 3 (red for the name, green for the
// amount, in hundredths); column 4 shows the previous arrow and column 5 a final mark. The task
// taps `next` from `item` to the next item or to `last`. Every name, key and target is made up
// (`entry_a`, `inventory.list.example_kind_a`, `mark/previous`). Nothing sleeps or reads a clock.

use super::field_list::{
    FieldListFrame, FieldListRead, FieldListReading, PreparedFieldList, condition_met,
    field_list_end_unseen, field_list_of, prepare_field_lists,
};
use super::tests::scripted::{ScriptedFrame, ScriptedRuntime};
use super::*;
use actingcommand_contract::{
    FactScalar, FieldListCondition, FieldListDeclaration, ocr_confidence_milli,
};
use actingcommand_recognition_pack::{
    FsAssetResolver, NnProviderRequest, NnProviderResult, OcrExecutionProviderKind,
    OcrProviderObservation, OcrProviderRequest, OcrProviderResult, OcrProviderTextBlock,
    VisionProviderError, VisionProviderErrorCode, load_pack_from_json_str,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const WIDTH: u32 = 8;
const ITEM: [u8; 3] = [255, 0, 0];
const LAST: [u8; 3] = [0, 0, 255];
const ARROW: [u8; 3] = [0, 255, 0];
const NAME_X: usize = 1;
const AMOUNT_X: usize = 2;
const CONFIDENCE_X: usize = 3;
const PREVIOUS_X: usize = 4;
const FINAL_X: usize = 5;
// The name each code reads; a code past them reads a name out of the dictionary.
const NAMES: [&str; 4] = ["", "entry_a", "entry_b", "entry_c"];
// The amount code that reads no number.
const UNPARSED: u8 = 250;
// The confidences every scripted frame shows, in hundredths: the name's and the amount's.
const SHOWN_CONFIDENCE: [u8; 2] = [96, 97];
const LIST_KEY: &str = "inventory.list.example_kind_a";

// ---- The builder (FL-1a): frames made here, the list `list_a` of `name` and `amount`.

fn field(id: &str, value: Value) -> Value {
    json!({"id": id, "group": "row", "target_id": format!("text/{id}"), "required": false,
           "privacy": "public", "trim": "whitespace_v1", "value": value})
}

fn fields_json(max_frames: u32) -> Value {
    json!({
        "mode": "fields_v1", "page_ids": ["item", "last"],
        "fields": [
            field("name", json!({"type": "dictionary_entry",
                                 "dictionary": {"path": "names.json"}})),
            field("amount", json!({"type": "unsigned_integer", "min": 0,
                                   "max": 9_007_199_254_740_991_u64}))
        ],
        "limits": {"max_frames": max_frames, "max_items": 64, "max_string_bytes": 64,
                   "max_total_bytes": 4096, "max_truth_entries": 8},
        "outcome_key": "fields_recorded"
    })
}

fn list_json() -> Value {
    json!({
        "id": "list_a", "group": "row", "key_field": "name", "key_column": "id",
        "columns": [{"column": "count", "field": "amount"}],
        "minimum_confidence_milli": 900,
        "fact_key": LIST_KEY, "valid_for_ms": 86_400_000,
        "start": {"forbidden": ["mark/previous"]},
        "end": {"page_ids": ["last"]}
    })
}

fn prepared(list: Value) -> PreparedFieldList {
    let declaration: FieldListDeclaration =
        serde_json::from_value(list).expect("field list declaration");
    let fields: OcrFieldsDeclaration =
        serde_json::from_value(fields_json(256)).expect("fields declaration");
    PreparedFieldList::of(&declaration, &fields)
}

fn resolved(value: OcrFieldValue, confidence_milli: u16) -> FieldListReading {
    FieldListReading {
        value: Some(value),
        reason: OcrFieldReason::Resolved,
        confidence_milli: Some(confidence_milli),
    }
}

fn unresolved(reason: OcrFieldReason, confidence_milli: Option<u16>) -> FieldListReading {
    FieldListReading {
        value: None,
        reason,
        confidence_milli,
    }
}

fn name(entry: &str) -> FieldListReading {
    resolved(OcrFieldValue::DictionaryEntry(entry.to_owned()), 950)
}

fn amount(value: u64) -> FieldListReading {
    resolved(OcrFieldValue::UnsignedInteger(value), 950)
}

fn frame(frame_index: u32, key: FieldListReading, count: FieldListReading) -> FieldListFrame {
    FieldListFrame {
        frame_index,
        key,
        columns: vec![count],
    }
}

fn row(key: &str, count: i64) -> BTreeMap<String, FactScalar> {
    BTreeMap::from([
        ("id".to_owned(), FactScalar::String(key.to_owned())),
        ("count".to_owned(), FactScalar::Integer(count)),
    ])
}

fn missed(read: &FieldListRead) -> (ResourceReadingProblem, u32, Vec<u32>) {
    match read {
        FieldListRead::Missed {
            miss,
            frame_indices,
        } => (miss.problem, miss.count, frame_indices.clone()),
        FieldListRead::Whole(rows) => panic!("a miss, got {rows:?}"),
    }
}

fn declaration_naming(frame_indices: Vec<u32>) -> ListReadingDeclaration {
    ListReadingDeclaration {
        operation_id: "list_a".to_owned(),
        fact_key: LIST_KEY.to_owned(),
        valid_for_ms: 86_400_000,
        frame_indices,
    }
}

// FL-1a: a list read whole holds one row per frame in ascending byte order of the key, its key
// under `key_column` and each column under its name (an integer column as an integer, a string
// column as its dictionary entry), with the lowest confidence of the fields the rows read; a list
// of one frame is whole; a 1025th position is `list_rows_exceeded` with the position past the
// bound and its frame.
#[test]
fn field_list_rows_whole() {
    let list = prepared(list_json());
    let frames = [
        frame(
            0,
            name("entry_c"),
            resolved(OcrFieldValue::UnsignedInteger(9), 990),
        ),
        frame(
            1,
            resolved(OcrFieldValue::DictionaryEntry("entry_a".into()), 931),
            amount(5),
        ),
        frame(2, name("entry_b"), amount(0)),
    ];
    assert_eq!(
        field_list_of(&list, &frames, true).expect("the list is decided"),
        FieldListRead::Whole(ListReadingRows {
            rows: vec![row("entry_a", 5), row("entry_b", 0), row("entry_c", 9)],
            confidence_milli: 931,
        })
    );
    assert_eq!(
        field_list_of(&list, &frames[..1], true).expect("one frame"),
        FieldListRead::Whole(ListReadingRows {
            rows: vec![row("entry_c", 9)],
            confidence_milli: 950,
        })
    );

    // A string column holds the field's dictionary entry; a list without columns holds the key.
    let mut labelled = list_json();
    let mut strings = fields_json(256);
    strings["fields"]
        .as_array_mut()
        .expect("fields")
        .push(field(
            "label",
            json!({"type": "dictionary_entry", "dictionary": {"path": "labels.json"}}),
        ));
    labelled["columns"] = json!([{"column": "label", "field": "label"}]);
    let declaration: FieldListDeclaration =
        serde_json::from_value(labelled).expect("labelled list");
    let fields: OcrFieldsDeclaration = serde_json::from_value(strings).expect("fields");
    let labelled = PreparedFieldList::of(&declaration, &fields);
    assert_eq!(
        field_list_of(
            &labelled,
            &[frame(4, name("entry_a"), name("label_x"))],
            true
        )
        .expect("labelled"),
        FieldListRead::Whole(ListReadingRows {
            rows: vec![BTreeMap::from([
                ("id".to_owned(), FactScalar::String("entry_a".to_owned())),
                ("label".to_owned(), FactScalar::String("label_x".to_owned())),
            ])],
            confidence_milli: 950,
        })
    );
    let mut bare = list_json();
    bare["columns"] = json!([]);
    let bare = prepared(bare);
    let keyed = FieldListFrame {
        frame_index: 0,
        key: name("entry_a"),
        columns: Vec::new(),
    };
    assert_eq!(
        field_list_of(&bare, &[keyed], true).expect("keys only"),
        FieldListRead::Whole(ListReadingRows {
            rows: vec![BTreeMap::from([(
                "id".to_owned(),
                FactScalar::String("entry_a".to_owned())
            )])],
            confidence_milli: 950,
        })
    );

    // The row bound of a record list stays: 1024 positions are whole, a 1025th is refused.
    let positions = (0..=1024_u32)
        .map(|index| frame(index, name(&format!("entry_{index:04}")), amount(1)))
        .collect::<Vec<_>>();
    match field_list_of(&list, &positions[..1024], true).expect("1024 rows") {
        FieldListRead::Whole(rows) => assert_eq!(rows.rows.len(), 1024),
        other => panic!("1024 rows are whole, got {other:?}"),
    }
    assert_eq!(
        missed(&field_list_of(&list, &positions, true).expect("1025 rows")),
        (ResourceReadingProblem::ListRowsExceeded, 1025, vec![1024])
    );
}

// FL-1a with the coordinator's decision 2 of 11-13 (the design note, section 7): every repeat
// fails the list. A key read on two frames, adjacent or not, with equal or different columns, is
// `list_duplicate_key`; `count` is the keys repeated and `frame_indices` names every frame that
// holds one, in ascending order. The core never folds, sums or picks one. The warning carries the
// frames.
#[test]
fn field_list_repeats() {
    let list = prepared(list_json());
    let cases = [
        // Adjacent, equal columns: a tap that did not advance, or two items that share a name.
        (
            vec![
                frame(0, name("entry_a"), amount(5)),
                frame(1, name("entry_a"), amount(5)),
                frame(2, name("entry_b"), amount(7)),
            ],
            1,
            vec![0, 1],
        ),
        // Adjacent, different columns.
        (
            vec![
                frame(0, name("entry_b"), amount(7)),
                frame(1, name("entry_a"), amount(5)),
                frame(2, name("entry_a"), amount(6)),
            ],
            1,
            vec![1, 2],
        ),
        // Separated by another key, equal columns.
        (
            vec![
                frame(0, name("entry_a"), amount(5)),
                frame(1, name("entry_b"), amount(7)),
                frame(2, name("entry_a"), amount(5)),
            ],
            1,
            vec![0, 2],
        ),
        // Three frames of one key.
        (
            vec![
                frame(0, name("entry_a"), amount(5)),
                frame(1, name("entry_a"), amount(5)),
                frame(2, name("entry_a"), amount(5)),
            ],
            1,
            vec![0, 1, 2],
        ),
        // Two keys repeated, interleaved.
        (
            vec![
                frame(0, name("entry_a"), amount(5)),
                frame(1, name("entry_b"), amount(7)),
                frame(2, name("entry_b"), amount(8)),
                frame(3, name("entry_c"), amount(9)),
                frame(4, name("entry_a"), amount(5)),
            ],
            2,
            vec![0, 1, 2, 4],
        ),
    ];
    for (frames, count, frame_indices) in cases {
        let read = field_list_of(&list, &frames, true).expect("the list is decided");
        assert_eq!(
            missed(&read),
            (
                ResourceReadingProblem::ListDuplicateKey,
                count,
                frame_indices.clone()
            ),
            "{frames:?}"
        );
        let FieldListRead::Missed { miss, .. } = read else {
            unreachable!("a miss")
        };
        let warning = miss.outcome(&declaration_naming(frame_indices.clone()));
        assert_eq!(
            warning.code().as_str(),
            ExecutionKernelCode::ContainedTaskResourceReadingUnresolved.as_str()
        );
        assert_eq!(warning.value("reading_id"), Some(&json!("list_a")));
        assert_eq!(warning.value("fact_key"), Some(&json!(LIST_KEY)));
        assert_eq!(
            warning.value("reading_problem"),
            Some(&json!("list_duplicate_key"))
        );
        assert_eq!(warning.value("count"), Some(&json!(count)));
        assert_eq!(warning.value("frame_indices"), Some(&json!(frame_indices)));
    }
    // A miss of a `read_list` step names no frame, as before.
    let warning = ListReadingMiss {
        problem: ResourceReadingProblem::ListDuplicateKey,
        count: 1,
    }
    .outcome(&declaration_naming(Vec::new()));
    assert_eq!(warning.value("frame_indices"), None);
}

// FL-1a: a frame whose key or a column is not resolved at the list's floor is unresolved, and the
// list is `list_position_unread` with `count` those frames and `frame_indices` naming them: a name
// out of the dictionary, a value below the list's floor (the target's own floor passed), a number
// that did not parse, a missing confidence, a field not read on the frame. A value at the floor is
// resolved. Unresolved frames come before repeated keys; past 64 frames the warning keeps 64 and
// the total.
#[test]
fn field_list_unresolved_rows() {
    let list = prepared(list_json());
    let frames = [
        frame(0, name("entry_a"), amount(5)),
        frame(
            1,
            unresolved(OcrFieldReason::UnknownEntry, Some(980)),
            amount(6),
        ),
        frame(
            2,
            resolved(OcrFieldValue::DictionaryEntry("entry_b".into()), 899),
            amount(7),
        ),
        frame(
            3,
            name("entry_c"),
            unresolved(OcrFieldReason::InvalidInteger, Some(990)),
        ),
        frame(
            4,
            FieldListReading {
                value: Some(OcrFieldValue::DictionaryEntry("entry_d".into())),
                reason: OcrFieldReason::Resolved,
                confidence_milli: None,
            },
            amount(8),
        ),
        frame(
            5,
            name("entry_e"),
            unresolved(OcrFieldReason::NotCollected, None),
        ),
        frame(
            6,
            resolved(OcrFieldValue::DictionaryEntry("entry_f".into()), 900),
            amount(9),
        ),
        // A repeat of `entry_a` as well: the unresolved frames are the list's miss.
        frame(7, name("entry_a"), amount(5)),
    ];
    assert_eq!(
        missed(&field_list_of(&list, &frames, true).expect("decided")),
        (
            ResourceReadingProblem::ListPositionUnread,
            5,
            vec![1, 2, 3, 4, 5]
        )
    );
    // At the floor exactly, the frame holds.
    assert_eq!(
        field_list_of(&list, &frames[6..7], true).expect("at the floor"),
        FieldListRead::Whole(ListReadingRows {
            rows: vec![row("entry_f", 9)],
            confidence_milli: 900,
        })
    );
    // Seventy unresolved frames: the warning keeps 64 of them and the total.
    let many = (0..70_u32)
        .map(|index| {
            frame(
                index,
                unresolved(OcrFieldReason::LowConfidence, Some(100)),
                amount(1),
            )
        })
        .collect::<Vec<_>>();
    let read = field_list_of(&list, &many, true).expect("decided");
    let (problem, count, frame_indices) = missed(&read);
    assert_eq!(
        (problem, count),
        (ResourceReadingProblem::ListPositionUnread, 70)
    );
    assert_eq!(frame_indices, (0..70).collect::<Vec<_>>());
    let FieldListRead::Missed { miss, .. } = read else {
        unreachable!("a miss")
    };
    let warning = miss.outcome(&declaration_naming(frame_indices));
    assert_eq!(
        warning.value("frame_indices"),
        Some(&json!((0..64).collect::<Vec<_>>()))
    );
    assert_eq!(warning.value("frame_indices_total"), Some(&json!(70)));
}

// FL-1a: `start` and `end`. A list whose first frame did not meet `start` is `list_start_unseen`
// with `count` the frames and `frame_indices` the first frame, before any other miss. A condition
// holds on one of its pages (with or without the game prefix) when it names any, with every
// required target passing and no forbidden one. A list that met no end is `list_end_unseen` with
// `count` the frames and `frame_indices` the last frame; a list without frames has none.
#[test]
fn field_list_start_end() {
    let list = prepared(list_json());
    let frames = [
        frame(3, name("entry_a"), amount(5)),
        frame(
            4,
            unresolved(OcrFieldReason::UnknownEntry, Some(980)),
            amount(5),
        ),
        frame(5, name("entry_a"), amount(5)),
    ];
    assert_eq!(
        missed(&field_list_of(&list, &frames, false).expect("decided")),
        (ResourceReadingProblem::ListStartUnseen, 3, vec![3])
    );
    assert_eq!(
        field_list_end_unseen(&frames).as_ref().map(missed),
        Some((ResourceReadingProblem::ListEndUnseen, 3, vec![5]))
    );
    assert_eq!(field_list_end_unseen(&[]), None);
    let warning = ListReadingMiss {
        problem: ResourceReadingProblem::ListEndUnseen,
        count: 3,
    }
    .outcome(&declaration_naming(vec![5]));
    assert_eq!(
        warning.value("reading_problem"),
        Some(&json!("list_end_unseen"))
    );
    assert_eq!(warning.value("frame_indices"), Some(&json!([5])));

    // The conditions: `mark/shown` passes on the frame, `mark/hidden` does not.
    let mut passes = |target: &str| -> Result<bool, ContainedTaskError> {
        match target {
            "mark/shown" => Ok(true),
            "mark/hidden" => Ok(false),
            other => panic!("no target {other}"),
        }
    };
    let condition =
        |value: Value| -> FieldListCondition { serde_json::from_value(value).expect("condition") };
    let cases = [
        (json!({"page_ids": ["last"]}), "neutral/last", true),
        (json!({"page_ids": ["neutral/last"]}), "neutral/last", true),
        (json!({"page_ids": ["item", "last"]}), "neutral/item", true),
        (json!({"page_ids": ["last"]}), "neutral/item", false),
        (json!({"forbidden": ["mark/hidden"]}), "neutral/item", true),
        (json!({"forbidden": ["mark/shown"]}), "neutral/item", false),
        (json!({"required": ["mark/shown"]}), "neutral/item", true),
        (json!({"required": ["mark/hidden"]}), "neutral/item", false),
        (
            json!({"page_ids": ["last"], "required": ["mark/shown"], "forbidden": ["mark/hidden"]}),
            "neutral/last",
            true,
        ),
        (
            json!({"page_ids": ["last"], "required": ["mark/shown", "mark/hidden"]}),
            "neutral/last",
            false,
        ),
        (
            json!({"page_ids": ["item"], "required": ["mark/shown"]}),
            "neutral/last",
            false,
        ),
    ];
    for (declared, page, holds) in cases {
        assert_eq!(
            condition_met("neutral", &condition(declared.clone()), page, &mut passes)
                .expect("judged"),
            holds,
            "{declared} on {page}"
        );
    }
}

// ---- The run (FL-1b): a scripted device and a fixture provider.

// Reads the name and the amount a frame shows, each read its own invocation of one CPU stream.
#[derive(Debug, Default)]
struct ShownText {
    reads: AtomicU64,
}

impl VisionProvider for ShownText {
    fn require_ocr_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Ok(())
    }

    fn require_nn_model(
        &self,
        _model_ref: &str,
        _model_sha256: &str,
    ) -> Result<(), VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }

    fn read_text(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderResult, VisionProviderError> {
        self.read_text_with_execution_evidence(request)
            .map(|observation| observation.result)
    }

    fn read_text_with_execution_evidence(
        &self,
        request: OcrProviderRequest<'_>,
    ) -> Result<OcrProviderObservation, VisionProviderError> {
        let pixel = |column: usize, channel: usize| request.frame.rgb8_pixels[column * 3 + channel];
        let column = usize::try_from(request.region.x).expect("a column of the frame");
        let (text, hundredths) = if column == NAME_X {
            let code = usize::from(pixel(NAME_X, 0));
            (
                NAMES.get(code).copied().unwrap_or("unlisted").to_owned(),
                pixel(CONFIDENCE_X, 0),
            )
        } else {
            let text = match pixel(AMOUNT_X, 0) {
                UNPARSED => "n/a".to_owned(),
                amount => amount.to_string(),
            };
            (text, pixel(CONFIDENCE_X, 1))
        };
        let confidence = Some(f32::from(hundredths) / 100.0);
        Ok(OcrProviderObservation {
            result: OcrProviderResult {
                ppocr_diagnostics: Vec::new(),
                text: text.clone(),
                blocks: vec![OcrProviderTextBlock {
                    text,
                    rect: request.region,
                    confidence,
                }],
                confidence,
                detection: None,
            },
            execution: Some(OcrProviderExecutionEvidence {
                invocation_id: format!("list-ocr-{}", self.reads.fetch_add(1, Ordering::Relaxed)),
                session_id: "list-session".to_owned(),
                session_generation: 1,
                requested_provider: OcrExecutionProviderKind::Cpu,
                resolved_provider: OcrExecutionProviderKind::Cpu,
                requested_cuda_ordinal: None,
                requested_cuda_identity: None,
                resolved_cuda_ordinal: None,
                resolved_cuda_identity: None,
                provider_implementation: "fixture-ocr".to_owned(),
                provider_binary_sha256: "b".repeat(64),
                runtime_version: "fixture-runtime".to_owned(),
                model_ref: request.model_ref.to_owned(),
                model_sha256: request.model_sha256.to_owned(),
                cpu_ep_registered: true,
                cpu_fallback_disabled: false,
                fallback_forbidden: true,
                fallback_observed: None,
                complete: true,
            }),
        })
    }

    fn classify(
        &self,
        _request: NnProviderRequest<'_>,
    ) -> Result<NnProviderResult, VisionProviderError> {
        Err(VisionProviderError::new(
            VisionProviderErrorCode::Unavailable,
            "no NN model in this fixture",
        ))
    }
}

// One `ListReading` a run recorded.
#[derive(Debug, PartialEq)]
struct RecordedList {
    declaration: ListReadingDeclaration,
    captured_at: SystemTime,
    list: Result<ListReadingRows, ListReadingMiss>,
}

// The scripted device, which keeps the device capture time of every frame it shows.
struct ListDevice {
    script: ScriptedRuntime,
    captured_at: Vec<SystemTime>,
}

impl ListDevice {
    fn new(screens: Vec<ScriptedFrame>) -> Self {
        Self {
            script: ScriptedRuntime::new(screens.into_iter().map(|screen| vec![screen]).collect()),
            captured_at: Vec::new(),
        }
    }

    fn traces(&self) -> &[ContainedTaskTrace] {
        self.script.traces()
    }

    // The position of each trace `matches` selects, in order.
    fn positions(&self, matches: impl Fn(&ContainedTaskTrace) -> bool) -> Vec<usize> {
        self.traces()
            .iter()
            .enumerate()
            .filter(|(_, trace)| matches(trace))
            .map(|(index, _)| index)
            .collect()
    }

    // The traces of the list: each `ListReadingFrame`'s capture time and each `ListReading`.
    fn list_traces(&self) -> (Vec<SystemTime>, Vec<RecordedList>) {
        let mut frames = Vec::new();
        let mut lists = Vec::new();
        for trace in self.traces() {
            match trace {
                ContainedTaskTrace::ListReadingFrame { captured_at } => frames.push(*captured_at),
                ContainedTaskTrace::ListReading {
                    declaration,
                    captured_at,
                    list,
                } => lists.push(RecordedList {
                    declaration: declaration.clone(),
                    captured_at: *captured_at,
                    list: list.clone(),
                }),
                _ => {}
            }
        }
        (frames, lists)
    }

    fn last_capture(&self) -> SystemTime {
        *self.captured_at.last().expect("a frame was captured")
    }

    // The same device, answering every step's gap with `gap` (ACT-2).
    fn with_action_gap(self, gap: Duration) -> Self {
        Self {
            script: self.script.with_action_gap(gap),
            ..self
        }
    }

    // The records of the run's fields report.
    fn fields_report(&self) -> Vec<OcrFieldRecord> {
        self.traces()
            .iter()
            .find_map(|trace| match trace {
                ContainedTaskTrace::PostAdmissionOcrFields { report } => {
                    Some(report.records.clone())
                }
                _ => None,
            })
            .expect("a fields report")
    }
}

impl ContainedTaskRuntime for ListDevice {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let observed = self.script.capture()?;
        self.captured_at.push(observed.frame.captured_at);
        Ok(observed)
    }

    fn input(
        &mut self,
        action: InputAction,
        frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.script.input(action, frame)
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.script.record(trace)
    }

    fn sampling_supported(&self) -> bool {
        self.script.sampling_supported()
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        self.script.step_pacing()
    }

    fn monotonic_now(&mut self) -> Instant {
        self.script.monotonic_now()
    }

    fn pause(&mut self, duration: Duration) {
        self.script.pause(duration);
    }

    fn step_action_gap(&mut self) -> Result<Duration, Self::Error> {
        self.script.step_action_gap()
    }
}

// A frame of `page` showing the name `name` (a code of `NAMES`) and the amount `amount`, at the
// shown confidences, with or without the previous arrow and the final mark.
fn screen(page: [u8; 3], name: u8, amount: u8, previous: bool, final_mark: bool) -> ScriptedFrame {
    let mut pixels = [[0_u8; 3]; WIDTH as usize];
    pixels[0] = page;
    pixels[NAME_X] = [name, 0, 0];
    pixels[AMOUNT_X] = [amount, 0, 0];
    pixels[CONFIDENCE_X] = [SHOWN_CONFIDENCE[0], SHOWN_CONFIDENCE[1], 0];
    if previous {
        pixels[PREVIOUS_X] = ARROW;
    }
    if final_mark {
        pixels[FINAL_X] = ARROW;
    }
    ScriptedFrame::rgb(WIDTH, 1, &pixels)
}

fn color(id: &str, x: usize, expected: [u8; 3]) -> Value {
    json!({"type": "color", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
           "expected": expected})
}

fn ocr_target(id: &str, x: usize) -> Value {
    json!({
        "type": "ocr", "id": id, "region": {"x": x, "y": 0, "width": 1, "height": 1},
        "languages": ["en"], "timeout_ms": 1000, "match_mode": "exact", "expected": ["unused"],
        "case_sensitive": false, "minimum_confidence": 0.1, "model_ref": "neutral-ocr",
        "model_sha256": "a".repeat(64)
    })
}

fn next_step() -> Value {
    json!({"id": "next", "from": "item", "to": ["item", "last"],
           "click": {"kind": "point", "x": 7, "y": 0}, "unguarded_trusted_coordinate": true})
}

// The task: `next` from `item` to the next item or `last`, the fields on both pages, the run's
// target and the fields' terminal `last`; with `list`, its field list.
fn task_json(list: Option<Value>, max_frames: u32) -> Value {
    let mut task = json!({
        "schema_version": "0.8", "task_id": "task", "game": "neutral", "server_scope": ["test"],
        "coordinate_space": {"width": WIDTH, "height": 1}, "target_page": "last",
        "timeout_ms": 60_000, "post_admission_ocr": fields_json(max_frames),
        "scheduling_outcome": {"mappings": [{"outcome_key": "fields_recorded",
            "effect": "no_designated_effect", "terminal_pages": ["last"]}]},
        "operations": [next_step()]
    });
    if let Some(list) = list {
        task["field_lists"] = json!([list]);
    }
    task
}

fn evaluator() -> RecognitionEvaluator {
    let pack = load_pack_from_json_str(
        &json!({
            "schema_version": "0.7", "game": "neutral", "server": "test",
            "coordinate_space": {"width": WIDTH, "height": 1},
            "defaults": {"color_max_distance": 0.0},
            "targets": [
                color("page/item", 0, ITEM),
                color("page/last", 0, LAST),
                color("mark/previous", PREVIOUS_X, ARROW),
                color("mark/final", FINAL_X, ARROW),
                ocr_target("text/name", NAME_X),
                ocr_target("text/amount", AMOUNT_X)
            ]
        })
        .to_string(),
    )
    .expect("recognition pack");
    RecognitionEvaluator::with_vision_provider(
        pack,
        Arc::new(FsAssetResolver::new(PathBuf::new())),
        Arc::new(ShownText::default()),
    )
    .expect("recognition evaluator")
}

fn prepared_fields(max_frames: u32) -> PreparedOcrFields {
    let fields = PreparedOcrFields {
        declaration: serde_json::from_value(fields_json(max_frames)).expect("fields"),
        dictionaries: BTreeMap::from([(
            "name".to_owned(),
            serde_json::from_value(json!({"schema_version": "actingcommand.ocr-truth-set.v2",
                                          "items": ["entry_a", "entry_b", "entry_c"]}))
            .expect("dictionary"),
        )]),
    };
    fields.declaration.validate().expect("fields declaration");
    fields
}

// The task `task`, its fields read up to `max_frames` frames, prepared as admission prepares it
// after the declaration gate: its field lists admitted by `prepare_field_lists`.
fn package(task: Value, max_frames: u32) -> PreparedContainedTask {
    let mut program: TaskProgram = serde_json::from_value(task).expect("task program");
    let evaluator = evaluator();
    let fields = prepared_fields(max_frames);
    prepare_field_lists(&mut program, "neutral", &evaluator, Some(&fields))
        .expect("the field lists are admitted");
    let pages: PageSet = serde_json::from_value(json!({"schema_version": "0.6", "pages": [
        {"id": "neutral/item", "required": ["page/item"]},
        {"id": "neutral/last", "required": ["page/last"]}
    ]}))
    .expect("page set");
    let control: TaskControl = serde_json::from_value(json!({
        "schema_version": CONTROL_SCHEMA, "package_id": "neutral.list.fields",
        "execution_mode": "navigable_route", "game": "neutral", "server": "test",
        "resolution": {"width": WIDTH, "height": 1}, "entry_task_id": "task",
        "capture_interval_ms": 1, "timeout_ms": 60_000
    }))
    .expect("task control");
    PreparedContainedTask {
        control,
        scheduling_outcome: program.scheduling_outcome.clone(),
        program,
        evaluator,
        detector: PageDetector::new(pages).expect("page detector"),
        entry_page: None,
        post_admission_ocr: None,
        post_admission_fields: Some(fields),
        entry_count: 1,
        task_count: 1,
    }
}

// Three items, `entry_c`, `entry_a` and `entry_b`, the first without the previous arrow, the last
// on the last page.
fn three_items() -> Vec<ScriptedFrame> {
    vec![
        screen(ITEM, 3, 9, false, false),
        screen(ITEM, 1, 5, true, false),
        screen(LAST, 2, 7, true, false),
    ]
}

// The lowest confidence the fixture's reads give.
fn shown_confidence_milli() -> u16 {
    SHOWN_CONFIDENCE
        .iter()
        .map(|hundredths| {
            ocr_confidence_milli(Some(f32::from(*hundredths) / 100.0)).expect("a confidence")
        })
        .min()
        .expect("two confidences")
}

// FL-1b: a run over saved frames of three items collects one frame per item; its first frame
// meets `start` and its last, on `last`, meets `end`, so on that frame, the frame captured last,
// the run records `ListReadingFrame` and then `ListReading` with the list whole: one row per
// item in byte order of the name, the lowest confidence, the list's id, key and lifetime, and no
// frame named. Both come after that frame's fields observation and before `Finalizing`; the run
// succeeds on `last`.
#[test]
fn field_list_run_records_traces() {
    let task = package(task_json(Some(list_json()), 256), 256);
    assert!(task.has_list_readings());
    let mut device = ListDevice::new(three_items());
    let outcome = task.run(&mut device).expect("the run succeeds");
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    let (frames, lists) = device.list_traces();
    let end = device.last_capture();
    assert_eq!(frames, vec![end]);
    assert_eq!(
        lists,
        vec![RecordedList {
            declaration: declaration_naming(Vec::new()),
            captured_at: end,
            list: Ok(ListReadingRows {
                rows: vec![row("entry_a", 5), row("entry_b", 7), row("entry_c", 9)],
                confidence_milli: shown_confidence_milli(),
            }),
        }]
    );
    let observed = device.positions(|trace| {
        matches!(
            trace,
            ContainedTaskTrace::PostAdmissionOcrObservation { .. }
        )
    });
    let collected = observed
        .iter()
        .map(|position| match &device.traces()[*position] {
            ContainedTaskTrace::PostAdmissionOcrObservation { frame_index, .. } => *frame_index,
            _ => unreachable!("an observation"),
        })
        .collect::<Vec<_>>();
    assert_eq!(collected, vec![0, 1, 2]);
    let frame_trace =
        device.positions(|trace| matches!(trace, ContainedTaskTrace::ListReadingFrame { .. }));
    let list_trace =
        device.positions(|trace| matches!(trace, ContainedTaskTrace::ListReading { .. }));
    let finalizing =
        device.positions(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }));
    assert_eq!(frame_trace.len(), 1);
    assert_eq!(list_trace.len(), 1);
    assert_eq!(finalizing.len(), 1);
    assert!(observed[2] < frame_trace[0], "after the end frame's fields");
    assert!(frame_trace[0] < list_trace[0], "the frame is bound first");
    assert!(list_trace[0] < finalizing[0], "before Finalizing");
}

// FL-1b, fix 1 (the independent review of 11-13): with an action gap configured, the kernel
// captures the step's page again before each tap. No input came between that frame and the step's
// own, so it shows the same item: it is no list frame. Over the three items, the fields report holds
// the frames it held before field lists, five (each item's frame and the frame before each of the
// two taps), as a package without the member gets them on the same script; the list is whole with
// three rows, and its frame trace lands on the last frame, the end frame.
#[test]
fn field_list_under_an_action_gap() {
    const GAP: Duration = Duration::from_millis(500);
    let task = package(task_json(Some(list_json()), 256), 256);
    let mut device = ListDevice::new(three_items()).with_action_gap(GAP);
    let outcome = task.run(&mut device).expect("the run succeeds");
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    assert_eq!(device.script.action_gap_draws(), 2);
    assert_eq!(device.script.inputs().len(), 2);

    // The fields report: every frame, the two before an input included.
    let report = device.fields_report();
    let names = report
        .iter()
        .map(|record| {
            let name = record
                .fields
                .iter()
                .find(|field| field.field_id == "name")
                .and_then(|field| field.value.clone());
            (record.frame_index, name)
        })
        .collect::<Vec<_>>();
    let entry = |name: &str| Some(OcrFieldValue::DictionaryEntry(name.to_owned()));
    assert_eq!(
        names,
        vec![
            (0, entry("entry_c")),
            (1, entry("entry_c")),
            (2, entry("entry_a")),
            (3, entry("entry_a")),
            (4, entry("entry_b")),
        ]
    );
    let without = package(task_json(None, 256), 256);
    let mut plain = ListDevice::new(three_items()).with_action_gap(GAP);
    without.run(&mut plain).expect("the run succeeds");
    assert_eq!(plain.fields_report(), report);

    // The list: frames 0, 2 and 4, whole, bound to the frame captured last.
    let (frames, lists) = device.list_traces();
    let end = device.last_capture();
    assert_eq!(frames, vec![end]);
    assert_eq!(
        lists,
        vec![RecordedList {
            declaration: declaration_naming(Vec::new()),
            captured_at: end,
            list: Ok(ListReadingRows {
                rows: vec![row("entry_a", 5), row("entry_b", 7), row("entry_c", 9)],
                confidence_milli: shown_confidence_milli(),
            }),
        }]
    );
    let end_fields = device.positions(|trace| {
        matches!(
            trace,
            ContainedTaskTrace::PostAdmissionOcrObservation { frame_index: 4, .. }
        )
    });
    let frame_trace =
        device.positions(|trace| matches!(trace, ContainedTaskTrace::ListReadingFrame { .. }));
    assert_eq!(end_fields.len(), 1);
    assert_eq!(frame_trace.len(), 1);
    assert!(
        end_fields[0] < frame_trace[0],
        "after the end frame's fields"
    );
}

// FL-1b: a successful run whose fields stop at `max_frames` (2) before the last page is
// collected meets no `end`: at its end, before `Finalizing`, it records `ListReading` with the
// miss `list_end_unseen`, `count` the two frames collected and `frame_indices` the last of them,
// on the frame captured last; no `ListReadingFrame`. A run whose first frame shows the previous
// arrow decides its list on `last` as `list_start_unseen`, naming its first frame.
#[test]
fn field_list_end_unseen() {
    let task = package(task_json(Some(list_json()), 2), 2);
    let mut device = ListDevice::new(three_items());
    let outcome = task.run(&mut device).expect("the run succeeds");
    assert_eq!(outcome.outcome, TaskOutcome::Success);
    let (frames, lists) = device.list_traces();
    assert!(frames.is_empty());
    let miss = ListReadingMiss {
        problem: ResourceReadingProblem::ListEndUnseen,
        count: 2,
    };
    assert_eq!(
        lists,
        vec![RecordedList {
            declaration: declaration_naming(vec![1]),
            captured_at: device.last_capture(),
            list: Err(miss),
        }]
    );
    assert_eq!(
        miss.outcome(&declaration_naming(vec![1]))
            .value("frame_indices"),
        Some(&json!([1]))
    );
    let list_trace =
        device.positions(|trace| matches!(trace, ContainedTaskTrace::ListReading { .. }));
    let finalizing =
        device.positions(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }));
    assert!(list_trace[0] < finalizing[0]);

    // Started inside the list: the first frame shows the previous arrow.
    let task = package(task_json(Some(list_json()), 256), 256);
    let mut device = ListDevice::new(vec![
        screen(ITEM, 3, 9, true, false),
        screen(LAST, 2, 7, true, false),
    ]);
    task.run(&mut device).expect("the run succeeds");
    let (frames, lists) = device.list_traces();
    assert!(frames.is_empty());
    assert_eq!(
        lists,
        vec![RecordedList {
            declaration: declaration_naming(vec![0]),
            captured_at: device.last_capture(),
            list: Err(ListReadingMiss {
                problem: ResourceReadingProblem::ListStartUnseen,
                count: 2,
            }),
        }]
    );
}

// FL-1b: a frame of the list's group collected after the list was decided fails the run with
// `contained_task_state_invalid` (`runtime_invariant`): here the end is `last` with the final
// mark, and the task's `back` returns from `last` to an item, which the fields collect. The list
// decided on `last` stays recorded once; the run does not reach `Finalizing`, so nothing of it
// is published.
#[test]
fn field_list_frame_after_end_fails_run() {
    let mut list = list_json();
    list["end"] = json!({"page_ids": ["last"], "required": ["mark/final"]});
    let mut task = task_json(Some(list), 256);
    task["target_page"] = json!("done");
    task["operations"] = json!([
        next_step(),
        {"id": "back", "from": "last", "to": "item",
         "click": {"kind": "point", "x": 6, "y": 0}, "unguarded_trusted_coordinate": true}
    ]);
    let task = package(task, 256);
    let mut device = ListDevice::new(vec![
        screen(ITEM, 3, 9, false, false),
        screen(LAST, 1, 5, true, true),
        screen(ITEM, 2, 7, true, false),
    ]);
    let error = match task.run(&mut device) {
        Err(ContainedTaskRunError::Task(error)) => error,
        other => panic!("the run fails, got {other:?}"),
    };
    assert_eq!(
        error.code().as_str(),
        SharedCode::ContainedTaskStateInvalid.as_str(),
        "{error}"
    );
    assert_eq!(
        error.outcome().value("state_problem"),
        Some(&json!("runtime_invariant"))
    );
    let (frames, lists) = device.list_traces();
    assert_eq!(frames.len(), 1);
    assert_eq!(lists.len(), 1);
    assert!(matches!(&lists[0].list, Ok(rows) if rows.rows.len() == 2));
    assert!(
        device
            .positions(|trace| matches!(trace, ContainedTaskTrace::Finalizing { .. }))
            .is_empty()
    );
}

// The admission refusal of `task`'s field lists, as `prepare_bundle` runs it after the gate.
fn admission_refusal(task: Value, fields: &PreparedOcrFields) -> ContainedTaskError {
    let mut program: TaskProgram = serde_json::from_value(task).expect("task program");
    prepare_field_lists(&mut program, "neutral", &evaluator(), Some(fields))
        .expect_err("admission refuses the list")
}

// FL-1b, admission's half (the declaration gate's half is the pack-containment test of the same
// name; `contracts/resource-readings.md`, "Lists from fields", "Declaration"): the base list is
// admitted and refused as a startup package and as a layer; each rule of the declaration table is
// refused by admission's own check with `contained_task_resource_reading_invalid` and
// `<id>:<member>`, or the task's member alone; a condition's target that is no template or colour
// target of the recognition pack, and a field the projection marks personal, are refused by the
// checks only admission makes.
#[test]
fn field_list_gate_and_admission_refusals() {
    let base = package(task_json(Some(list_json()), 256), 256);
    assert!(base.has_list_readings());
    assert_eq!(base.startup_incompatibility(), Some("field_lists"));
    assert_eq!(
        base.scheduling_outcome_incompatibility(),
        Some("field_lists")
    );
    assert!(base.prerequisite_incompatibility().is_some());
    assert!(!base.is_entry_recovery_compatible());

    let fields = prepared_fields(256);
    let with_list = |patch: &dyn Fn(&mut Value)| {
        let mut list = list_json();
        patch(&mut list);
        task_json(Some(list), 256)
    };
    let with_task = |patch: &dyn Fn(&mut Value)| {
        let mut task = task_json(Some(list_json()), 256);
        patch(&mut task);
        task
    };
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "an older schema",
            with_task(&|task| task["schema_version"] = json!("0.7")),
            "field_lists",
        ),
        (
            "two lists",
            with_task(&|task| task["field_lists"] = json!([list_json(), list_json()])),
            "field_lists",
        ),
        (
            "no list",
            with_task(&|task| task["field_lists"] = json!([])),
            "field_lists",
        ),
        (
            "not a declaration",
            with_task(&|task| task["field_lists"] = json!({"id": "list_a"})),
            "field_lists",
        ),
        (
            "no fields",
            with_task(&|task| {
                task["schema_version"] = json!("0.9");
                task.as_object_mut()
                    .expect("task")
                    .remove("post_admission_ocr");
            }),
            "post_admission_ocr",
        ),
        (
            "no outcome",
            with_task(&|task| {
                task.as_object_mut()
                    .expect("task")
                    .remove("scheduling_outcome");
            }),
            "scheduling_outcome",
        ),
        (
            "an operation's id",
            with_list(&|list| list["id"] = json!("next")),
            "next:id",
        ),
        (
            "an integer key",
            with_list(&|list| list["key_field"] = json!("amount")),
            "list_a:key_field",
        ),
        (
            "a column of the key",
            with_list(&|list| list["columns"][0]["field"] = json!("name")),
            "list_a:columns/0/field",
        ),
        (
            "no floor",
            with_list(&|list| list["minimum_confidence_milli"] = json!(0)),
            "list_a:minimum_confidence_milli",
        ),
        (
            "a key outside the prefix",
            with_list(&|list| list["fact_key"] = json!("inventory.example_list")),
            "list_a:fact_key",
        ),
        (
            "no start",
            with_list(&|list| list["start"] = json!({})),
            "list_a:start",
        ),
        (
            "an end page that is no terminal",
            with_list(&|list| list["end"] = json!({"page_ids": ["item"]})),
            "list_a:end/page_ids/0",
        ),
        (
            "an OCR target as a condition",
            with_list(&|list| list["start"] = json!({"required": ["text/name"]})),
            "list_a:start/required/0",
        ),
        (
            "a target the pack lacks",
            with_list(&|list| list["start"] = json!({"forbidden": ["mark/absent"]})),
            "list_a:start/forbidden/0",
        ),
        (
            "an OCR target ending the list",
            with_list(&|list| {
                list["end"] = json!({"page_ids": ["last"], "forbidden": ["mark/final"],
                                     "required": ["text/amount"]});
            }),
            "list_a:end/required/0",
        ),
    ];
    for (case, task, detail) in cases {
        let error = admission_refusal(task, &fields);
        assert_eq!(
            error.code().as_str(),
            ExecutionKernelCode::ContainedTaskResourceReadingInvalid.as_str(),
            "{case}: {error}"
        );
        assert_eq!(error.detail(), Some(detail), "{case}");
    }
    // The projection metadata marks the name's target personal: admission's own reason.
    let mut personal = prepared_fields(256);
    personal.declaration.fields[0].privacy = actingcommand_contract::OcrFieldPrivacy::Personal;
    let error = admission_refusal(task_json(Some(list_json()), 256), &personal);
    assert_eq!(error.detail(), Some("list_a:target_personal"));
}

// A package without the member passes admission and runs exactly as before: the same inputs, the
// same traces but the list's, the same fields report; it reads no list, and a startup package or
// a layer is judged without it.
#[test]
fn field_list_absent_package_runs_as_before() {
    let without = package(task_json(None, 256), 256);
    assert!(!without.has_list_readings());
    assert_eq!(without.startup_incompatibility(), None);
    assert_eq!(without.scheduling_outcome_incompatibility(), None);
    let with = package(task_json(Some(list_json()), 256), 256);
    let mut plain = ListDevice::new(three_items());
    let mut listed = ListDevice::new(three_items());
    let plain_outcome = without.run(&mut plain).expect("the run succeeds");
    let listed_outcome = with.run(&mut listed).expect("the run succeeds");
    assert_eq!(plain_outcome.outcome, listed_outcome.outcome);
    assert_eq!(plain_outcome.final_page, listed_outcome.final_page);
    assert_eq!(plain.script.inputs(), listed.script.inputs());
    assert_eq!(plain.list_traces(), (Vec::new(), Vec::new()));
    let kinds = |device: &ListDevice| {
        device
            .traces()
            .iter()
            .filter(|trace| {
                !matches!(
                    trace,
                    ContainedTaskTrace::ListReadingFrame { .. }
                        | ContainedTaskTrace::ListReading { .. }
                )
            })
            .map(std::mem::discriminant)
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&plain), kinds(&listed));
    let report = |device: &ListDevice| {
        device
            .traces()
            .iter()
            .find_map(|trace| match trace {
                ContainedTaskTrace::PostAdmissionOcrFields { report } => {
                    Some(report.records.clone())
                }
                _ => None,
            })
            .expect("a fields report")
    };
    assert_eq!(report(&plain), report(&listed));
}

// A package that declares the member under a task schema before `0.8` is refused at its member,
// and one run as a startup package or as a layer is refused by its run kind's check.
#[test]
fn field_list_refused_under_an_older_schema_or_as_a_layer() {
    let fields = prepared_fields(256);
    for schema in ["0.7", "0.6"] {
        let mut task = task_json(Some(list_json()), 256);
        task["schema_version"] = json!(schema);
        assert_eq!(
            admission_refusal(task, &fields).detail(),
            Some("field_lists"),
            "{schema}"
        );
    }
    let task = package(task_json(Some(list_json()), 256), 256);
    assert_eq!(task.startup_incompatibility(), Some("field_lists"));
    assert_eq!(
        task.prerequisite_incompatibility(),
        Some("post_admission_ocr")
    );
    assert!(!task.is_entry_recovery_compatible());
}
