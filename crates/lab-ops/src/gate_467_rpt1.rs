// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #467 RPT-1 (register §1.379; `contracts/problem-report.md`; the coordinator's work
//! order of 11-14 with its rulings, and the design's section G, row RPT-1): the slice's named
//! tests of `report export`. The reads go to a made-up Runtime ([`Fake`], a [`Source`] that
//! answers made-up runs and writes down every read it is asked for); the installation, the state
//! root and the output are temporary folders; no Runtime, no sleep, and no clock read by a test
//! (the private bindings' writer stamps its own binding time).
//!
//! - `rpt1_summary_names_the_failing_step_its_scores_and_the_display`: the summary page of two
//!   failed runs: each run's terminal code with the catalog's description, the step it ended in,
//!   the page that step and the run started from, the last page recognised; the failing step's
//!   template target below its threshold (last and best score, threshold, margin, evaluations)
//!   and its OCR target (the text read and its confidence); the display line of a scaled run and
//!   of a run of the package's size; the run that ended normally before the newest on its
//!   instance; the frames and the versions. The archive: its name, its first three entries, the
//!   manifest's digests, one file in the folder, and a second export that never replaces it.
//! - `rpt1_the_stream_is_read_for_the_failing_step_only`: the diagnostic stream's excerpt and the
//!   scores hold the failing step's records only; a stream above 8 MiB is not read, the scores
//!   come from the step's recognition record (each target matched or not, each page's best score
//!   and first failed target, no threshold), and the part is named missing.
//! - `rpt1_frames_are_the_failing_steps_decided_and_last_and_the_runs_last`: at most three frames
//!   a run, each once, bytes as read; the step's middle frames are never read; a frame the
//!   Runtime no longer has is named missing and the rest is written.
//! - `rpt1_an_output_inside_the_install_root_or_the_state_root_is_refused`: inside either root
//!   (in another letter case, through `..`, the root itself) the output is refused with that
//!   root; beside them it is taken; a file is no directory; through the row the refusal comes
//!   before the Runtime is reached and nothing is written.
//! - `rpt1_no_secret_and_no_unmasked_name_is_in_the_archive`: the configuration's salt, the key
//!   file's bytes (raw, hex, base64), a device serial and the user's and computer's names are in
//!   no entry's name or content; the answer line holds none of the secrets.
//! - `rpt1_the_report_only_reads_and_confirms_no_evidence`: every call the Runtime gets is one of
//!   the source's reads; the module's code names no confirmation, client action or capture, and
//!   hands the client only to the reads of `status`, `runs`, `events`, `run show`, `run evidence`
//!   and `material`.
//! - `rpt1_no_frames_leaves_out_every_frame_and_the_text_ocr_read`: no frame is read or written,
//!   no text OCR read is anywhere, the scores stay, and the summary and the manifest say so.
//! - `rpt1_the_size_bound_leaves_out_frames_then_excerpts_and_says_so`: frames of the older run
//!   first, then the newer run's, then the older run's excerpt, until the archive fits; the
//!   runs' statuses, steps and scores and the errors files stay; the warning, the manifest and
//!   the summary name every part left out.
//! - `rpt1_the_errors_view_of_each_instance_newest_rows_counted`: each instance's errors view is
//!   read with the window, the newest 200 rows kept and the newest ten on the summary, which says
//!   whether they are the newest of the window or every row of it.
//! - `rpt1_every_read_names_a_request_or_an_instance_and_the_window`: the reads in order, each
//!   naming an instance with the report's window, a request id with the failing step's range, or
//!   a material reference; with an instance named, no other instance is read.
//! - `rpt1_the_tables_row_report_export`: the row of the operation table.
//!
//! Fix round 1 (`rd-1115/brief-1725-fix1.md`), adapting the tests above where its items change
//! what they pin (the errors view's newest rows and no total, item 7; the reads' order, item 7;
//! the size bound in packed bytes, item 8; the parts missing with their part and reason; the
//! private bindings written by their one writer, item 2), and four new tests:
//! - `rpt1_fix1_a_listing_not_read_to_its_end_never_says_none`: a listing cut short is a part
//!   missing naming its instance, the summary says for each instance whether its listing was read
//!   to its end, and "none" is said only of listings that were (item 3).
//! - `rpt1_fix1_still_over_the_bound_is_said_in_the_manifest_the_summary_and_the_answer`: with
//!   every part that may be left out left out, in order, the archive still over its bound says
//!   so in `report_trimmed`, the manifest, the summary and the answer (item 4).
//! - `rpt1_fix1_no_frames_leaves_out_every_text_and_value_read_from_a_frame`: each member that
//!   holds what a frame shows, in the stream's records and the errors view's rows, is in the
//!   report with frames and in no entry without them; what measures a frame stays (item 5).
//! - `rpt1_fix1_the_masks_replace_names_inside_words_escaped_paths_and_dns_names`: the masks of
//!   item 11.
//!
//! Fix round 2 (`rd-1115/brief-1725-fix2.md`), adapting where its items change what they pin
//! (the fake's errors view says why it stopped, item 5; the masks of round 1's test, where the
//! coordinator's ruling replaces item 11; the reads' code, whose errors view now goes through
//! `errors_view`, item 4), and four new tests; the search's tests read the requests the search
//! builds on a made-up Runtime ([`Pages`]) whose time a count of its requests spends, no clock:
//! - `rpt1_fix2_the_masks_keep_member_names_hashes_and_ids_and_replace_a_short_name_alone`: with
//!   the short names `val`, `ada` and `deb`, the members' names, a hash, a commit, the ids, a
//!   code and an outcome's words stay whole, in a value and in a report, whose manifest checks
//!   its entries; the three path and host forms are replaced (item 1).
//! - `rpt1_fix2_an_alias_holding_the_name_names_its_entry_as_the_manifest_does`: the entry of an
//!   alias that holds the user's name is named from the masked alias, as the manifest, the
//!   summary's data and the page name it (item 2).
//! - `rpt1_fix2_every_read_of_the_errors_search_carries_the_instance`: the head's read selects
//!   no row; every other read of the search carries the instance (item 3).
//! - `rpt1_fix2_the_deadline_cuts_the_errors_search_and_the_part_says_so`: no read of the search
//!   starts once its time is spent, and the errors view is the part missing with the word for
//!   that (item 4).

use crate::code_lookup::{self, Answer, Query};
use crate::codes::{
    ActingctlCode, ReportFit, ReportOutputRoot, ReportPart, ReportPartReason, ReportWriteStep,
    RunState,
};
use crate::core_ops::{self, Caller, Place};
use crate::core_reads::READ_HERE;
use crate::devices::{self, AdbRun, Bind};
use crate::events::{CursorBinding, EventPages};
use crate::front::Connection;
use crate::material_export::MAX_EXPORT_BYTES;
use crate::report::{
    self, DisplayFacts, Errors, Exported, Geometry, Installed, Instance, Listed, Mask, Missed,
    PageVerdict, Recognition, Request, RunRow, Source, TargetVerdict, Which, Window,
};
use crate::run_evidence::{Evidence, StepDetail, StepEvidence};
use crate::table::{
    Entry, MAX_HELP_BYTES, Operation, OperationModule, Param, ParamForm, ParamType, RuntimeNeed,
    operation_table,
};
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_contract::{
    AuditInput, DeviceBoundVia, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, IdentifierIssuer, InstanceId,
    LedgerView, OriginModule, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeOperationKind, RuntimePayloadDraft,
    SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
};
use actingcommand_runtime_client::RuntimeClientError;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use tempfile::TempDir;
use zip::ZipArchive;

/// Now, as the report is asked for: 2026-11-15 00:00:00 UTC.
const NOW: u64 = 1_794_700_800_000;
const HOUR: u64 = 3_600_000;
/// The window's start: a day back.
const SINCE: u64 = NOW - 24 * HOUR;
/// The made-up user's and computer's names the masks know.
const USER: &str = "Zephyrine";
const COMPUTER: &str = "HOSTVERDANT";
/// The material references of run A (alpha, the newest failed run, scaled frames).
const A_STEP_ZERO: &str = "30:evt-30:art-30";
const A_DECIDED: &str = "41:evt-41:art-41";
const A_MIDDLE: &str = "43:evt-43:art-43";
const A_STEP_LAST: &str = "47:evt-47:art-47";
const A_RUN_LAST: &str = "52:evt-52:art-52";
const A_STREAM: &str = "60:evt-60:art-60";
/// And of run B (beta, an older failed run, frames of the package's size).
const B_DECIDED: &str = "110:evt-110:art-110";
const B_STEP_LAST: &str = "115:evt-115:art-115";
const B_STREAM: &str = "120:evt-120:art-120";

/// The code both failed runs ended with.
fn ending() -> CodeStr {
    Outcome::new(ActingctlCode::CallBudgetExhausted)
        .code()
        .clone()
}

/// The catalog's description of the code spelled `spelling`.
fn description_of(spelling: &str) -> String {
    match code_lookup::read_embedded(&Query::One(spelling.to_owned())) {
        Ok(Answer::Entry(entry)) => entry["description"].as_str().unwrap_or_default().to_owned(),
        _ => panic!("the embedded catalog lists {spelling}"),
    }
}

fn row(
    request: &str,
    run: &str,
    alias: &str,
    state: RunState,
    package: &str,
    first: u64,
    final_page: &str,
) -> RunRow {
    RunRow {
        request_id: request.to_owned(),
        run_id: Some(run.to_owned()),
        alias: alias.to_owned(),
        ended_with: matches!(state, RunState::Failed).then(ending),
        state,
        package: Some(package.to_owned()),
        final_page: Some(final_page.to_owned()),
        first_unix_ms: first,
        ended_unix_ms: Some(first + 60_000),
    }
}

/// The ledger sequence a material reference names first.
fn sequence_of(reference: &str) -> u64 {
    reference
        .split(':')
        .next()
        .and_then(|sequence| sequence.parse().ok())
        .unwrap_or_default()
}

/// One made-up step of a run's evidence.
#[derive(Clone)]
struct StepSpec {
    index: u32,
    page: &'static str,
    frames: Vec<&'static str>,
    first_sequence: u64,
    failed: bool,
}

impl StepSpec {
    fn evidence(&self) -> StepEvidence {
        StepEvidence {
            step_index: self.index,
            page: self.page.to_owned(),
            targets: Vec::new(),
            action: None,
            frames: self
                .frames
                .iter()
                .map(|reference| (sequence_of(reference), (*reference).to_owned()))
                .collect(),
            decided_frame: true,
            first_sequence: self.first_sequence,
            last_sequence: self.first_sequence + 9,
            started_unix_ms: NOW - HOUR,
            finished_unix_ms: (!self.failed).then_some(NOW - HOUR + 1_000),
            failure: self.failed.then(ending),
        }
    }
}

/// One made-up run the Runtime answers for its request id.
#[derive(Clone)]
struct FakeRun {
    row: RunRow,
    steps: Vec<StepSpec>,
    last_page: &'static str,
    last_frame: &'static str,
    stream: &'static str,
    geometry: Vec<Geometry>,
    recognitions: Vec<Recognition>,
}

impl FakeRun {
    fn evidence(&self) -> Evidence {
        let mut identity = Map::new();
        identity.insert("request_id".to_owned(), json!(self.row.request_id));
        identity.insert("run_id".to_owned(), json!(self.row.run_id));
        Evidence {
            identity,
            steps: self.steps.iter().map(StepSpec::evidence).collect(),
            final_: json!({"page": self.last_page, "frame_ref": self.last_frame}),
            overlay: Some(json!({"ref": self.stream})),
            from_step: 0,
            limit: usize::MAX,
            detail: StepDetail::Full,
        }
    }
}

/// The made-up Runtime: its instances, its failed and unfinished runs, the runs that ended
/// normally, each instance's errors view (its rows, oldest first), its materials (by reference;
/// those known only by a size larger than any bound the report reads with), the instance whose
/// run listing stops before its end, and every read it was asked for.
struct Fake {
    instances: Vec<&'static str>,
    runs: Vec<FakeRun>,
    settled: Vec<RunRow>,
    errors: BTreeMap<&'static str, Vec<Value>>,
    materials: BTreeMap<&'static str, Vec<u8>>,
    sizes: BTreeMap<&'static str, u64>,
    cut_listing: Option<&'static str>,
    reads: RefCell<Vec<String>>,
}

impl Fake {
    fn note(&self, read: String) {
        self.reads.borrow_mut().push(read);
    }

    fn run_of(&self, request_id: &str) -> Result<&FakeRun, Missed> {
        self.runs
            .iter()
            .find(|run| run.row.request_id == request_id)
            .ok_or_else(|| {
                Missed::new(
                    ReportPartReason::RuntimeRefused,
                    Outcome::new(ActingctlCode::RunUnknown).request_id(request_id),
                )
            })
    }

    fn reads(&self) -> Vec<String> {
        self.reads.borrow().clone()
    }
}

/// Whether `row` was first recorded in `window`.
fn within(row: &RunRow, window: Window) -> bool {
    row.first_unix_ms >= window.since_unix_ms
        && window
            .until_unix_ms
            .is_none_or(|until| row.first_unix_ms <= until)
}

impl Source for Fake {
    fn instances(&self, selector: Option<&str>) -> Result<Vec<Instance>, Outcome> {
        self.note(format!("instances {selector:?}"));
        Ok(self
            .instances
            .iter()
            .filter(|alias| selector.is_none_or(|wanted| wanted == **alias))
            .map(|alias| Instance {
                alias: (*alias).to_owned(),
                id: None,
            })
            .collect())
    }

    fn runs(
        &self,
        instance: &Instance,
        window: Window,
        which: Which,
        limit: usize,
    ) -> Result<Listed, Missed> {
        self.note(format!(
            "runs {} {} {:?} {which:?} {limit}",
            instance.alias, window.since_unix_ms, window.until_unix_ms
        ));
        // A listing that stops before its end holds no run (as `list_contained_runs` answers an
        // incomplete listing).
        if self.cut_listing == Some(instance.alias.as_str()) {
            return Ok(Listed {
                runs: Vec::new(),
                cut: Some(ReportPartReason::NotReadToEnd),
            });
        }
        let unfinished = self.runs.iter().map(|run| &run.row);
        let mut rows = match which {
            Which::Unfinished => unfinished.cloned().collect::<Vec<_>>(),
            Which::Any => unfinished.chain(&self.settled).cloned().collect(),
        };
        rows.retain(|row| row.alias == instance.alias && within(row, window));
        rows.sort_by_key(|row| Reverse(row.first_unix_ms));
        rows.truncate(limit);
        Ok(Listed {
            runs: rows,
            cut: None,
        })
    }

    fn errors(&self, instance: &Instance, window: Window, keep: usize) -> Result<Errors, Missed> {
        self.note(format!(
            "errors {} {} {:?} {keep}",
            instance.alias, window.since_unix_ms, window.until_unix_ms
        ));
        let all = self
            .errors
            .get(instance.alias.as_str())
            .cloned()
            .unwrap_or_default();
        let skipped = all.len().saturating_sub(keep);
        Ok(Errors {
            rows: all[skipped..].to_vec(),
            searched_from: None,
            stopped: None,
        })
    }

    fn run(&self, request_id: &str) -> Result<Value, Missed> {
        self.note(format!("run {request_id}"));
        let run = self.run_of(request_id)?;
        Ok(json!({
            "request_id": request_id,
            "state": run.row.state.as_str(),
            "package_label": run.row.package,
            "state_root": format!("C:\\Users\\{USER}\\AppData\\Local\\ActingCommand\\state"),
            "host": COMPUTER,
        }))
    }

    fn evidence(&self, request_id: &str) -> Result<Evidence, Missed> {
        self.note(format!("evidence {request_id}"));
        self.run_of(request_id).map(FakeRun::evidence)
    }

    fn geometry(&self, request_id: &str) -> Result<Vec<Geometry>, Missed> {
        self.note(format!("geometry {request_id}"));
        self.run_of(request_id).map(|run| run.geometry.clone())
    }

    fn recognitions(
        &self,
        request_id: &str,
        from: u64,
        through: Option<u64>,
    ) -> Result<Vec<Recognition>, Missed> {
        self.note(format!("recognitions {request_id} {from} {through:?}"));
        let run = self.run_of(request_id)?;
        Ok(run
            .recognitions
            .iter()
            .filter(|found| {
                found.sequence >= from && through.is_none_or(|last| found.sequence <= last)
            })
            .cloned()
            .collect())
    }

    fn material(&self, reference: &str, limit: u64) -> Result<Vec<u8>, Missed> {
        self.note(format!("material {reference} {limit}"));
        match (self.materials.get(reference), self.sizes.get(reference)) {
            (Some(bytes), _) if bytes.len() as u64 > limit => {
                Err(Missed::too_large(bytes.len() as u64, limit))
            }
            (Some(bytes), _) => Ok(bytes.clone()),
            (None, Some(size)) if *size > limit => Err(Missed::too_large(*size, limit)),
            // The Runtime no longer has it (the cleaner removed it).
            _ => Err(Missed::new(
                ReportPartReason::RuntimeRefused,
                Outcome::new(ActingctlCode::CallBudgetExhausted),
            )),
        }
    }
}

fn scaled() -> Geometry {
    Geometry {
        device: Some((1920, 1080)),
        map: Some(DisplayFacts {
            kind: "scale".to_owned(),
            scale_num: 3,
            scale_den: 2,
            scale_width: 1280,
            scale_height: 720,
            base_width: 1280,
            base_height: 720,
        }),
        conclusion: "pass".to_owned(),
        passed: true,
    }
}

fn identity() -> Geometry {
    Geometry {
        device: Some((1280, 720)),
        map: None,
        conclusion: "pass".to_owned(),
        passed: true,
    }
}

/// One diagnostic stream record as the stream writes it.
fn record(index: u64, parent: Option<u64>, kind: &str, data: Value) -> Value {
    json!({
        "index": index,
        "frame_id": null,
        "step_action_id": null,
        "physical_action_id": null,
        "parent_index": parent,
        "kind": kind,
        "data": data,
    })
}

/// A template target's row that did not pass.
fn template(id: &str, score: f64, threshold: f64) -> Value {
    json!({
        "id": id,
        "kind": "template",
        "passed": false,
        "template": {
            "x": 10, "y": 20, "width": 30, "height": 40,
            "raw_score": score, "score": score, "threshold": threshold,
        },
        "color": null,
        "source": {
            "role": "required", "group_index": null, "target_index": 0, "target_id": id,
            "passed": false,
        },
    })
}

/// `length` characters that deflate cannot shrink much (fix round 1, item 8: the size bound
/// counts packed bytes): a fixed pseudo-random sequence over 64 letters, digits, `-` and `_`.
fn noise(length: usize, seed: u64) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(ALPHABET[(state >> 58) as usize])
        })
        .collect()
}

/// The stream's framing: the header's line, one record a line (each after the first with its
/// comma), the footer.
fn framed(records: &[Value]) -> Vec<u8> {
    let mut text = String::from(
        "{\"schema_version\":\"actingcommand.runtime.task-diagnostic.v2\",\"records\":[\n",
    );
    for (place, record) in records.iter().enumerate() {
        if place > 0 {
            text.push(',');
        }
        text.push_str(&record.to_string());
        text.push('\n');
    }
    text.push_str("]}\n");
    text.into_bytes()
}

/// Run A's stream: step 0 (records 1 to 3) and the failing step 1 (records 4 to 12): a template
/// target evaluated twice below its threshold, an OCR target with its text, the page it failed
/// and a record naming a path; with `pad`, a record of that many bytes in step 1.
fn stream_a(pad: usize) -> Vec<u8> {
    let mut records = vec![
        record(
            1,
            None,
            "step_started",
            json!({"step_index": 0, "monotonic_ms": 10}),
        ),
        record(
            2,
            None,
            "page",
            json!({
                "phase": "page", "index": 0, "page_id": "start", "matched": true,
                "required_passed": 1, "required_total": 1, "any_of_passed": 0,
                "any_of_total": 0, "optional_passed": 0, "optional_total": 0,
                "forbidden_passed": 0, "forbidden_total": 0,
            }),
        ),
        record(3, None, "target", template("start_marker", 0.97, 0.8)),
        record(
            4,
            None,
            "step_started",
            json!({"step_index": 1, "monotonic_ms": 20}),
        ),
        record(5, None, "target", template("confirm_button", 0.74, 0.85)),
        record(6, None, "target", template("confirm_button", 0.62, 0.85)),
        record(
            7,
            None,
            "target",
            json!({
                "id": "title_text", "kind": "ocr", "passed": false, "template": null,
                "color": null, "source": {"phase": "guard"},
            }),
        ),
        record(
            8,
            Some(7),
            "ocr",
            json!({
                "region": {}, "raw_text": "Welcome back", "derived_text": "Welcome back",
                "confidence": 0.93, "matched_expected": null, "match_mode": "contains",
                "block_count": 1,
            }),
        ),
        record(
            9,
            Some(8),
            "ocr_block",
            json!({
                "source_index": 0, "derived_rank": 0,
                "raw": {"text": "Welcome back", "rect": {}, "confidence": 0.93},
            }),
        ),
        record(
            10,
            None,
            "page",
            json!({
                "phase": "page", "index": 1, "page_id": "confirm_dialog",
                "failed_target": {
                    "target_id": "confirm_button", "role": "required", "group_index": null,
                    "target_index": 0, "cause": {},
                },
            }),
        ),
        record(
            11,
            None,
            "unexecuted",
            json!({
                "phase": "guard", "target_id": "title_text",
                "note": format!("C:\\Users\\{USER}\\AppData\\Local\\Temp\\{COMPUTER}.log"),
            }),
        ),
    ];
    if pad > 0 {
        records.push(record(
            13,
            None,
            "error",
            json!({"phase": "page", "pad": noise(pad, 13)}),
        ));
    }
    records.push(record(
        12,
        None,
        "terminal",
        json!({"result": "failed", "executed_steps": 2}),
    ));
    framed(&records)
}

/// Run B's stream: its one step, a template target below its threshold.
fn stream_b(pad: usize) -> Vec<u8> {
    let mut records = vec![
        record(
            1,
            None,
            "step_started",
            json!({"step_index": 0, "monotonic_ms": 5}),
        ),
        record(2, None, "target", template("home_marker", 0.55, 0.8)),
    ];
    if pad > 0 {
        records.push(record(
            4,
            None,
            "error",
            json!({"phase": "page", "pad": noise(pad, 4)}),
        ));
    }
    records.push(record(
        3,
        None,
        "terminal",
        json!({"result": "failed", "executed_steps": 1}),
    ));
    framed(&records)
}

/// A made-up frame of `size` bytes (at least its signature and reference).
fn frame(reference: &str, size: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(reference.as_bytes());
    bytes.resize(size.max(bytes.len()), b'.');
    bytes
}

/// One row of an errors view, as `events --view errors --detail summary` answers it.
fn error_row(number: u64) -> Value {
    json!({
        "seq": 1_000 + number,
        "ts": SINCE + number * 1_000,
        "type": "task.failed",
        "severity": "error",
        "code": Outcome::new(ActingctlCode::CallBudgetExhausted).code().as_str(),
        "run_id": format!("run-e{number}"),
        "module": "runtime_host",
        "path": format!("C:\\Users\\{USER}\\Documents\\{COMPUTER}-notes"),
    })
}

/// The made-up Runtime: on `alpha` the newest failed run A (scaled frames, a template below its
/// threshold and an OCR target in its failing step) and before it a run that ended normally; on
/// `beta` the older failed run B (frames of the package's size); 250 error rows on `alpha`, 3 on
/// `beta`. Frames of `frame_size` bytes; with `pad`, a record of that many bytes in each failing
/// step's stream.
fn fake(frame_size: usize, pad: usize) -> Fake {
    let a = FakeRun {
        row: row(
            "req-a",
            "run-a",
            "alpha",
            RunState::Failed,
            "neutral.collect",
            NOW - HOUR,
            "menu",
        ),
        steps: vec![
            StepSpec {
                index: 0,
                page: "start",
                frames: vec![A_STEP_ZERO],
                first_sequence: 20,
                failed: false,
            },
            StepSpec {
                index: 1,
                page: "menu",
                frames: vec![A_DECIDED, A_MIDDLE, A_STEP_LAST],
                first_sequence: 40,
                failed: true,
            },
        ],
        last_page: "menu",
        last_frame: A_RUN_LAST,
        stream: A_STREAM,
        geometry: vec![scaled()],
        recognitions: vec![
            Recognition {
                sequence: 25,
                matched_page: Some("start".to_owned()),
                frame: (1920, 1080),
                targets: Vec::new(),
                pages: Vec::new(),
                omitted: 0,
            },
            Recognition {
                sequence: 45,
                matched_page: Some("menu".to_owned()),
                frame: (1920, 1080),
                targets: vec![TargetVerdict {
                    target: "confirm_button".to_owned(),
                    matched: false,
                }],
                pages: vec![
                    PageVerdict {
                        page: "menu".to_owned(),
                        matched: true,
                        best_milli: Some(990),
                        first_failed: None,
                    },
                    PageVerdict {
                        page: "confirm_dialog".to_owned(),
                        matched: false,
                        best_milli: Some(740),
                        first_failed: Some("confirm_button".to_owned()),
                    },
                ],
                omitted: 0,
            },
        ],
    };
    let b = FakeRun {
        row: row(
            "req-b",
            "run-b",
            "beta",
            RunState::Failed,
            "neutral.daily",
            NOW - 3 * HOUR,
            "home",
        ),
        steps: vec![StepSpec {
            index: 0,
            page: "home",
            frames: vec![B_DECIDED, B_STEP_LAST],
            first_sequence: 100,
            failed: true,
        }],
        last_page: "home",
        last_frame: B_STEP_LAST,
        stream: B_STREAM,
        geometry: vec![identity()],
        recognitions: Vec::new(),
    };
    let mut materials = BTreeMap::new();
    for reference in [
        A_STEP_ZERO,
        A_DECIDED,
        A_MIDDLE,
        A_STEP_LAST,
        A_RUN_LAST,
        B_DECIDED,
        B_STEP_LAST,
    ] {
        materials.insert(reference, frame(reference, frame_size));
    }
    materials.insert(A_STREAM, stream_a(pad));
    materials.insert(B_STREAM, stream_b(pad));
    let mut errors = BTreeMap::new();
    errors.insert("alpha", (1..=250).map(error_row).collect());
    errors.insert("beta", (1..=3).map(error_row).collect());
    Fake {
        instances: vec!["alpha", "beta"],
        runs: vec![a, b],
        settled: vec![row(
            "req-p",
            "run-p",
            "alpha",
            RunState::Succeeded,
            "neutral.collect",
            NOW - 2 * HOUR,
            "home",
        )],
        errors,
        materials,
        sizes: BTreeMap::new(),
        cut_listing: None,
        reads: RefCell::new(Vec::new()),
    }
}

/// The installation's versions, as `report::installed` reads them.
fn versions() -> Installed {
    Installed {
        versions: json!({
            "lab_ops": "0.1.0",
            "runtime": {
                "repository": "example/runtime",
                "commit_sha": "0123456789abcdef0123456789abcdef01234567",
            },
            "tools": {"commit_sha": "0123456789abcdef0123456789abcdef01234567"},
            "ui": null,
            "bundles": {
                "neutral": {
                    "repository": "example/neutral-pack",
                    "commit": "89abcdef0123456789abcdef0123456789abcdef",
                },
            },
        }),
        warnings: Vec::new(),
    }
}

fn asked(out: &Path, frames: bool, max_bytes: u64) -> Request {
    Request {
        instance: None,
        out: out.to_path_buf(),
        since_unix_ms: SINCE,
        until_unix_ms: None,
        now_unix_ms: NOW,
        max_runs: 5,
        max_bytes,
        frames,
    }
}

fn default_bound() -> u64 {
    report::DEFAULT_MIB * report::MIB
}

fn mask() -> Mask {
    Mask::new(Some(USER), &[COMPUTER])
}

fn export_from(source: &Fake, request: &Request, installed: &Installed) -> Exported {
    report::export(source, request, installed, &mask())
        .unwrap_or_else(|outcome| panic!("the report is written: {outcome}"))
}

/// The archive's entries in order: each name and its bytes.
fn archive_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let bytes = fs::read(path).expect("the archive is written");
    let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("the archive is a zip");
    (0..archive.len())
        .map(|index| {
            let mut file = archive.by_index(index).expect("an entry");
            let mut content = Vec::new();
            file.read_to_end(&mut content).expect("its bytes");
            (file.name().to_owned(), content)
        })
        .collect()
}

fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, bytes)| bytes.as_slice())
        .unwrap_or_else(|| panic!("the archive holds {name}"))
}

fn text(entries: &[(String, Vec<u8>)], name: &str) -> String {
    String::from_utf8(entry(entries, name).to_vec()).expect("text")
}

fn document(entries: &[(String, Vec<u8>)], name: &str) -> Value {
    serde_json::from_slice(entry(entries, name)).expect("JSON")
}

fn names(entries: &[(String, Vec<u8>)]) -> Vec<&str> {
    entries.iter().map(|(name, _)| name.as_str()).collect()
}

/// The files the warnings `report_part_missing` name.
fn missing_files(exported: &Exported) -> Vec<String> {
    exported
        .warnings
        .iter()
        .filter(|warning| warning.is(ActingctlCode::ReportPartMissing))
        .filter_map(|warning| warning.value("file")?.as_str().map(str::to_owned))
        .collect()
}

/// The warning `report_part_missing` of `part` for `why` whose `key` is `value`.
fn missing_part<'a>(
    exported: &'a Exported,
    part: ReportPart,
    why: ReportPartReason,
    key: &str,
    value: &str,
) -> &'a Outcome {
    exported
        .warnings
        .iter()
        .find(|warning| {
            warning.is(ActingctlCode::ReportPartMissing)
                && warning.value("report_part") == Some(&json!(part.as_str()))
                && warning.value("report_part_reason") == Some(&json!(why.as_str()))
                && warning.value(key) == Some(&json!(value))
        })
        .unwrap_or_else(|| {
            panic!(
                "report_part_missing {} {} with {key} {value}: {:?}",
                part.as_str(),
                why.as_str(),
                exported.warnings
            )
        })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `bytes` in standard base64 with its padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let triple = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |value, (place, byte)| {
                value | (u32::from(*byte) << (16 - 8 * place))
            });
        for place in 0..4 {
            if place <= chunk.len() {
                let sextet = (triple >> (18 - 6 * place)) & 63;
                text.push(char::from(ALPHABET[sextet as usize]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

fn param<'a>(row: &'a Operation, name: &str) -> &'a Param {
    row.params
        .iter()
        .find(|param| param.name == name)
        .unwrap_or_else(|| panic!("the row declares {name}"))
}

#[test]
fn rpt1_summary_names_the_failing_step_its_scores_and_the_display() {
    let folder = TempDir::new().expect("a temporary folder");
    let out = folder.path().join("reports");
    let source = fake(64, 0);
    let request = asked(&out, true, default_bound());
    let exported = export_from(&source, &request, &versions());
    let entries = archive_entries(&exported.path);
    let summary = text(&entries, "report/SUMMARY.md");
    let ending_code = ending();
    let code = ending_code.as_str();

    // The first lines: the view, the instances, the window, the frames, the versions.
    assert!(
        summary.starts_with(
            "# Problem report / 问题报告\n\nView / 视图: errors · instances / 实例: alpha, beta \
             · window / 时间窗: 2026-11-14 00:00:00Z to 2026-11-15 00:00:00Z · Runtime online \
             / Runtime 在运行 · frames: included / 带截图\n\nVersions / 版本: runtime \
             0123456789ab · tools 0123456789ab · ui none · lab-ops 0.1.0 · neutral \
             example/neutral-pack@89abcdef0123\n"
        ),
        "{summary}"
    );
    // Fix round 1: what the screenshots can show and how to leave them out (item 10); the size
    // within its bound (item 4); each instance's listing read to its end (item 3).
    for line in [
        "Screenshots / 截图: the screenshots in this report can show a player's name and \
         numbers. To send a report without them, export again with `--no-frames`",
        "Size / 大小: within the bound of 24 MiB / 在 24 MiB 上限内\n",
        "- Listing of alpha / alpha 的运行列表: read to its end / 读完\n",
        "- Listing of beta / beta 的运行列表: read to its end / 读完\n",
    ] {
        assert!(summary.contains(line), "{line}\n{summary}");
    }
    assert!(exported.within_bound);
    // The runs, newest first, and the run that ended normally before the newest on its
    // instance, which shows where the game was left.
    for line in [
        format!("| 1 | alpha | failed | neutral.collect | {code} | menu | 2026-11-14 23:00:00Z |"),
        format!("| 2 | beta | failed | neutral.daily | {code} | home | 2026-11-14 21:00:00Z |"),
        "| before 1 / 1 之前 | alpha | succeeded | neutral.collect |  | home | 2026-11-14 \
         22:00:00Z |"
            .to_owned(),
    ] {
        assert!(summary.contains(&line), "{line}\n{summary}");
    }
    // Run 1: its terminal with the catalog's description, the step it ended in, the page it
    // and the run started from, the last page recognised, the display line of its scaled
    // frames.
    let described = format!("- Ended / 结束: failed, `{code}`: {}", description_of(code));
    for line in [
        described.as_str(),
        "## Run 1 / 运行 1: run-a (req-a)",
        "- Package / 包: neutral.collect",
        "- Step / 步骤: 1 (started on page `menu`; the run started on page `start`)",
        "- Last page recognised / 最后识别的页: `menu`",
        "- Display / 显示: device 1920×1080, package 1280×720, scale 3/2, recognition on the \
         reduced frame",
        // The template target below its threshold: matched, last, best, threshold, margin,
        // the frames it was evaluated on.
        "| confirm_button | template | no | 0.620 | 0.740 | 0.850 | -0.230 | 2 |  |",
        // The OCR target: the text it read and its confidence.
        "| title_text | ocr | no |  |  |  |  | 1 | `Welcome back` (0.93) |",
        // The page the step evaluated and the target that failed it.
        "| confirm_dialog | no | 1 |  | confirm_button |",
        "- Frames / 截图: runs/run-a/frames/41-decided.png, runs/run-a/frames/47-step-last.png, \
         runs/run-a/frames/52-run-last.png",
        // Run 2: frames of the package's size.
        "- Display / 显示: device 1280×720, package 1280×720, not scaled",
        "| home_marker | template | no | 0.550 | 0.550 | 0.800 | -0.250 | 1 |  |",
        "## Not in this report yet / 这份报告还没有",
    ] {
        assert!(summary.contains(line), "{line}\n{summary}");
    }
    assert!(
        summary.contains("- Targets / 目标 (from the diagnostic stream / 来自诊断流)"),
        "{summary}"
    );

    // The summary's data says the same.
    let data = document(&entries, "report/summary.json");
    assert_eq!(data["schema_version"], report::SCHEMA);
    assert_eq!(data["view"], report::VIEW_ERRORS);
    assert_eq!(data["runs"][0]["step"], 1);
    assert_eq!(data["runs"][0]["run_started_on_page"], "start");
    assert_eq!(data["runs"][0]["last_page"], "menu");
    assert_eq!(
        data["runs"][1]["display"],
        "device 1280×720, package 1280×720, not scaled"
    );
    assert_eq!(data["succeeded_before_newest"]["request_id"], "req-p");

    // The archive: its name, the summary first, then the manifest and the data; the manifest
    // names every other entry with its size and digest; the answer's digest is the file's.
    let name = exported
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let digest = name
        .strip_prefix("problem-report-20261115-000000Z-")
        .and_then(|rest| rest.strip_suffix(".zip"))
        .unwrap_or_else(|| panic!("the archive's name: {name}"));
    assert!(
        digest.len() == 8
            && digest
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
        "{name}"
    );
    assert_eq!(
        names(&entries)[..3],
        [
            "report/SUMMARY.md",
            "report/manifest.json",
            "report/summary.json"
        ]
    );
    let archive = fs::read(exported.path.as_path()).expect("the archive");
    assert_eq!(
        exported.sha256,
        format!("sha256:{}", hex(&Sha256::digest(&archive)))
    );
    assert_eq!(exported.size, archive.len() as u64);
    let manifest = document(&entries, "report/manifest.json");
    assert_eq!(manifest["view"], report::VIEW_ERRORS);
    assert_eq!(manifest["frames"], true);
    assert_eq!(manifest["within_bound"], true);
    // Fix round 1, item 6: the manifest says the configuration is read and nothing of it written.
    assert!(
        manifest["secrets"]
            .as_str()
            .is_some_and(|text| text.starts_with(
                "the configuration is read for its \
                 standard_packs.root alone and nothing of it is written"
            )),
        "{manifest}"
    );
    let files = manifest["files"].as_array().expect("the manifest's files");
    assert_eq!(files.len() + 1, entries.len());
    for file in files {
        let path = file["path"].as_str().unwrap_or_default();
        let bytes = entry(&entries, path);
        assert_eq!(file["size"], bytes.len(), "{path}");
        assert_eq!(file["sha256"], hex(&Sha256::digest(bytes)), "{path}");
    }
    assert_eq!(
        manifest["not_in_this_report_yet"].as_array().map(Vec::len),
        Some(7)
    );

    // One file in the folder, no partial file left; the same report again never replaces it.
    let written = fs::read_dir(out.as_path())
        .expect("the output folder")
        .filter_map(Result::ok)
        .map(|item| item.file_name())
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 1, "{written:?}");
    let again = report::export(&fake(64, 0), &request, &versions(), &mask())
        .expect_err("the same archive is never replaced");
    assert!(again.is(ActingctlCode::ReportWriteFailed), "{again}");
    assert_eq!(
        again.value("report_write_step"),
        Some(&json!(ReportWriteStep::NameExists.as_str())),
        "{again}"
    );
    assert_eq!(
        fs::read(exported.path.as_path()).expect("the archive"),
        archive
    );
}

#[test]
fn rpt1_the_stream_is_read_for_the_failing_step_only() {
    let folder = TempDir::new().expect("a temporary folder");
    let out = folder.path().join("reports");
    let source = fake(64, 0);
    let exported = export_from(&source, &asked(&out, true, default_bound()), &versions());
    let entries = archive_entries(&exported.path);

    // The excerpt: the failing step's records, from its `step_started` on; none of step 0.
    let excerpt = text(&entries, "report/runs/run-a/overlay-excerpt.jsonl");
    let indices = excerpt
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).expect("a record")["index"]
                .as_u64()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    assert_eq!(indices, [4, 5, 6, 7, 8, 9, 10, 11, 12]);
    // The scores: the step's targets and the page it evaluated, not step 0's.
    let scores = document(&entries, "report/runs/run-a/scores.json");
    assert_eq!(scores["step"], 1);
    assert_eq!(scores["scores"]["from"], "stream");
    let targets = scores["scores"]["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .map(|target| target["target"].clone())
        .collect::<Vec<_>>();
    assert_eq!(targets, [json!("confirm_button"), json!("title_text")]);
    let button = &scores["scores"]["targets"][0];
    assert_eq!(button["frames"], 2);
    assert_eq!(button["last"], 0.62);
    assert_eq!(button["best"], 0.74);
    assert_eq!(button["threshold"], 0.85);
    assert_eq!(scores["scores"]["pages"][0]["page"], "confirm_dialog");
    assert_eq!(scores["scores"]["pages"].as_array().map(Vec::len), Some(1));
    // The step's recognition record, read by its request id from the step's first record on.
    assert_eq!(scores["recognition"]["sequence"], 45);
    assert!(
        source
            .reads()
            .contains(&"recognitions req-a 40 None".to_owned())
    );

    // A stream above 8 MiB is not read: the scores are what the step's recognition record
    // holds (each target matched or not, each candidate page's best score and the target that
    // failed it, no threshold), and the part is named missing.
    let mut large = fake(64, 0);
    large.materials.remove(A_STREAM);
    large.sizes.insert(A_STREAM, report::MAX_STREAM_BYTES + 1);
    let folder = TempDir::new().expect("a temporary folder");
    let exported = export_from(
        &large,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let entries = archive_entries(&exported.path);
    assert!(!names(&entries).contains(&"report/runs/run-a/overlay-excerpt.jsonl"));
    let scores = document(&entries, "report/runs/run-a/scores.json");
    assert_eq!(scores["scores"]["from"], "recognition");
    let button = &scores["scores"]["targets"][0];
    assert_eq!(button["target"], "confirm_button");
    assert_eq!(button["matched"], false);
    assert_eq!(button["threshold"], Value::Null);
    let dialog = &scores["scores"]["pages"][1];
    assert_eq!(dialog["page"], "confirm_dialog");
    assert_eq!(dialog["best_milli"], 740);
    assert_eq!(dialog["first_failed_target"], "confirm_button");
    // Fix round 1: the part named is the run's excerpt, for its size, with the run.
    let missing = missing_part(
        &exported,
        ReportPart::RunExcerpt,
        ReportPartReason::TooLarge,
        "file",
        "runs/run-a/overlay-excerpt.jsonl",
    );
    assert_eq!(missing.value("request_id"), Some(&json!("req-a")));
    assert!(
        missing
            .causes()
            .iter()
            .any(|link| link.outcome().is(ActingctlCode::MaterialTooLarge))
    );
    let summary = text(&entries, "report/SUMMARY.md");
    assert!(
        summary.contains("- Targets / 目标 (from the recognition record:"),
        "{summary}"
    );
}

#[test]
fn rpt1_frames_are_the_failing_steps_decided_and_last_and_the_runs_last() {
    let folder = TempDir::new().expect("a temporary folder");
    let mut source = fake(64, 0);
    // The cleaner already removed run A's last frame.
    source.materials.remove(A_RUN_LAST);
    let exported = export_from(
        &source,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let entries = archive_entries(&exported.path);
    let frames = names(&entries)
        .into_iter()
        .filter(|name| name.ends_with(".png"))
        .collect::<Vec<_>>();
    // The failing step's decided frame and its last frame, then the run's last frame (B's is
    // its step's last: once).
    assert_eq!(
        frames,
        [
            "report/runs/run-a/frames/41-decided.png",
            "report/runs/run-a/frames/47-step-last.png",
            "report/runs/run-b/frames/110-decided.png",
            "report/runs/run-b/frames/115-step-last.png",
        ]
    );
    assert_eq!(exported.frames, 4);
    for (name, reference) in [
        ("report/runs/run-a/frames/41-decided.png", A_DECIDED),
        ("report/runs/run-a/frames/47-step-last.png", A_STEP_LAST),
        ("report/runs/run-b/frames/110-decided.png", B_DECIDED),
        ("report/runs/run-b/frames/115-step-last.png", B_STEP_LAST),
    ] {
        assert_eq!(entry(&entries, name), frame(reference, 64).as_slice());
    }
    // The step's middle frame and the earlier step's frame are never read; each chosen frame
    // once.
    let reads = source.reads();
    for reference in [A_MIDDLE, A_STEP_ZERO] {
        assert!(
            !reads.iter().any(|read| read.contains(reference)),
            "{reads:?}"
        );
    }
    for reference in [A_DECIDED, A_STEP_LAST, A_RUN_LAST, B_DECIDED, B_STEP_LAST] {
        assert_eq!(
            reads.iter().filter(|read| read.contains(reference)).count(),
            1,
            "{reference}: {reads:?}"
        );
    }
    // The frame that is gone is named; the rest of the report is written.
    assert!(
        missing_files(&exported).contains(&"runs/run-a/frames/52-run-last.png".to_owned()),
        "{:?}",
        missing_files(&exported)
    );
    missing_part(
        &exported,
        ReportPart::Frame,
        ReportPartReason::RuntimeRefused,
        "file",
        "runs/run-a/frames/52-run-last.png",
    );
    let summary = text(&entries, "report/SUMMARY.md");
    assert!(
        summary.contains(
            "- Frames / 截图: runs/run-a/frames/41-decided.png, runs/run-a/frames/47-step-last.png\n"
        ),
        "{summary}"
    );
}

#[test]
fn rpt1_an_output_inside_the_install_root_or_the_state_root_is_refused() {
    let folder = TempDir::new().expect("a temporary folder");
    let root = folder.path().join("ActingCommand");
    let state_root = folder.path().join("state");
    fs::create_dir_all(root.join("tools")).expect("the installation root");
    fs::create_dir_all(&state_root).expect("the state root");
    let refused = |out: &Path| {
        report::refuse_output(out, Some(root.as_path()), &state_root)
            .expect_err("an output inside a root is refused")
    };

    // Inside the installation root: below it, the root itself, in another letter case, through
    // `..`.
    for out in [
        root.join("tools").join("reports"),
        root.clone(),
        folder.path().join("ACTINGCOMMAND").join("reports"),
        folder
            .path()
            .join("elsewhere")
            .join("..")
            .join("ActingCommand")
            .join("reports"),
    ] {
        let outcome = refused(out.as_path());
        assert!(
            outcome.is(ActingctlCode::ReportOutputRefused),
            "{}: {outcome}",
            out.display()
        );
        assert!(outcome.value("install_root").is_some(), "{outcome}");
        assert!(outcome.value("state_root").is_none(), "{outcome}");
        assert!(outcome.value("path").is_some(), "{outcome}");
        assert_eq!(
            outcome.value("report_output_root"),
            Some(&json!(ReportOutputRoot::InstallRoot.as_str())),
            "{outcome}"
        );
    }
    // Inside the state root.
    for out in [
        state_root.join("artifacts"),
        state_root.clone(),
        folder.path().join("STATE").join("kept"),
    ] {
        let outcome = refused(out.as_path());
        assert!(
            outcome.is(ActingctlCode::ReportOutputRefused),
            "{}: {outcome}",
            out.display()
        );
        assert!(outcome.value("state_root").is_some(), "{outcome}");
        assert!(outcome.value("install_root").is_none(), "{outcome}");
        assert_eq!(
            outcome.value("report_output_root"),
            Some(&json!(ReportOutputRoot::StateRoot.as_str())),
            "{outcome}"
        );
    }
    // Beside them, a name that begins like a root's included: taken.
    for out in [
        folder.path().join("ActingCommand-reports"),
        folder.path().join("statement"),
        folder.path().join("reports"),
    ] {
        assert!(
            report::refuse_output(&out, Some(root.as_path()), &state_root).is_ok(),
            "{}",
            out.display()
        );
    }
    // A file is no directory.
    let file = folder.path().join("a-file");
    fs::write(&file, b"made up").expect("a file");
    let outcome = refused(file.as_path());
    assert!(
        outcome.is(ActingctlCode::ActingctlUsageInvalid),
        "{outcome}"
    );

    // Through the row: refused before anything is read and before the Runtime is reached (no
    // Runtime runs on this state root), with exit 1; nothing is written.
    let table = operation_table().unwrap_or_else(|outcome| panic!("the table reads: {outcome}"));
    let operation = table
        .operation(report::ROW)
        .unwrap_or_else(|| panic!("the table has the row {}", report::ROW));
    let place = || Place {
        state_root: Ok(state_root.clone()),
        root: Some(root.clone()),
    };
    let caller = Caller {
        place: &place,
        unlocated: "the state root is not located",
        connection: Connection::per_call(),
        budget: None,
        cursor: CursorBinding::fresh(),
    };
    let inside = root.join("tools").join("reports");
    let mut params = Map::new();
    params.insert("out".to_owned(), json!(inside.display().to_string()));
    let exit = core_ops::answer(operation, &params, &caller);
    let line = exit.line.expect("a line");
    assert_eq!(exit.code, 1, "{line}");
    assert_eq!(
        line["outcome"]["code"],
        Outcome::new(ActingctlCode::ReportOutputRefused)
            .code()
            .as_str(),
        "{line}"
    );
    assert!(!inside.exists());
}

#[test]
fn rpt1_no_secret_and_no_unmasked_name_is_in_the_archive() {
    let folder = TempDir::new().expect("a temporary folder");
    // The installation: its configuration with the salt, a game root's bundle and a build
    // manifest, each naming the user or the computer.
    let root = folder.path().join("install");
    let salt = "salt-5b7e0c19d2a64f38";
    fs::create_dir_all(root.join("runtime")).expect("the runtime folder");
    fs::create_dir_all(root.join("games").join("neutral")).expect("a game root");
    fs::write(
        root.join("actingd.config.json"),
        json!({
            "secret_fingerprint_salt": salt,
            "standard_packs": {"root": "games"},
            "instances": [{"alias": "alpha", "device": {"kind": "handset", "transport": "usb"}}],
        })
        .to_string(),
    )
    .expect("the configuration");
    fs::write(
        root.join("games").join("neutral").join("bundle.json"),
        json!({
            "source": {
                "repository": format!("C:\\Users\\{USER}\\packs\\neutral"),
                "commit": "89abcdef0123456789abcdef0123456789abcdef",
            },
        })
        .to_string(),
    )
    .expect("the bundle");
    fs::write(
        root.join("runtime").join("BUILD-MANIFEST.json"),
        json!({
            "repository": "example/runtime",
            "commit_sha": "0123456789abcdef0123456789abcdef01234567",
            "source_artifact_name": format!("build-{COMPUTER}-{USER}"),
            "build_folder": format!("C:\\Users\\{USER}\\build"),
        })
        .to_string(),
    )
    .expect("the build manifest");
    // The state root: its key file, and the private device bindings with a serial, written by
    // their one writer (`devices bind`, from a made-up adb listing; fix round 1, item 2: the
    // architecture guard T10 lets only the binding reader and writer name the file).
    let state_root = folder.path().join("state");
    fs::create_dir_all(&state_root).expect("the state root");
    let key = (0_u8..32)
        .map(|place| 0xA5 ^ place.wrapping_mul(29))
        .collect::<Vec<_>>();
    fs::write(state_root.join("runtime-state.key"), &key).expect("the key file");
    let serial = "TESTSERIAL0467";
    let listing = |_: &Path| {
        AdbRun::Printed(format!(
            "List of devices attached\n{serial}         device usb:1-1 product:neutral \
             model:Neutral_Phone device:neutral transport_id:7\n\n"
        ))
    };
    let bound = Bind {
        alias: "alpha",
        handle: "usb:7",
        via: DeviceBoundVia::Cli,
    };
    devices::bind(&state_root, Some(root.as_path()), &bound, &listing)
        .unwrap_or_else(|outcome| panic!("the device is bound: {outcome}"));
    assert!(state_root.join("private").is_dir());

    let out = folder.path().join("reports");
    report::refuse_output(&out, Some(root.as_path()), &state_root)
        .unwrap_or_else(|outcome| panic!("an output beside both roots: {outcome}"));
    let installed = report::installed(Some(root.as_path()));
    let exported = export_from(
        &fake(64, 0),
        &asked(&out, true, default_bound()),
        &installed,
    );
    let entries = archive_entries(&exported.path);
    let words = [
        salt.to_owned(),
        serial.to_ascii_lowercase(),
        hex(&key),
        USER.to_ascii_lowercase(),
        COMPUTER.to_ascii_lowercase(),
    ];
    let encoded = base64(&key);
    for (name, bytes) in &entries {
        let content = String::from_utf8_lossy(bytes);
        let lowered = format!("{name}\n{content}").to_ascii_lowercase();
        for word in &words {
            assert!(!lowered.contains(word.as_str()), "{word} in {name}");
        }
        assert!(!content.contains(&encoded), "the key in base64 in {name}");
        assert!(
            !bytes
                .windows(key.len())
                .any(|window| window == key.as_slice()),
            "the key's bytes in {name}"
        );
    }
    // The masks were applied where the names stood; the versions are there.
    let manifest = document(&entries, "report/manifest.json");
    assert!(
        manifest["masks_applied"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "{manifest}"
    );
    let versions = document(&entries, "report/versions.json");
    assert_eq!(
        versions["runtime"]["commit_sha"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(
        versions["runtime"]["source_artifact_name"],
        "build-<computer>-<user>"
    );
    assert_eq!(versions["runtime"].get("build_folder"), None);
    assert_eq!(
        versions["bundles"]["neutral"]["repository"],
        "C:\\Users\\<user>\\packs\\neutral"
    );
    let run = document(&entries, "report/runs/run-a/run.json");
    assert_eq!(
        run["status"]["state_root"],
        "C:\\Users\\<user>\\AppData\\Local\\ActingCommand\\state"
    );
    assert_eq!(run["status"]["host"], "<computer>");
    // The answer line holds none of the secrets either.
    let line = report::line(&exported);
    let answered = match &line.line {
        Ok(value) => value.to_string(),
        Err(text) => text.clone(),
    };
    let key_hex = hex(&key);
    for word in [salt, serial, key_hex.as_str(), encoded.as_str()] {
        assert!(!answered.contains(word), "{word} in the answer");
    }
}

#[test]
fn rpt1_the_report_only_reads_and_confirms_no_evidence() {
    let folder = TempDir::new().expect("a temporary folder");
    let source = fake(64, 0);
    export_from(
        &source,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    // Every call the Runtime got is one of the source's reads.
    let reads = source.reads();
    assert!(!reads.is_empty());
    for read in &reads {
        let kind = read.split(' ').next().unwrap_or_default();
        assert!(
            [
                "instances",
                "runs",
                "errors",
                "run",
                "evidence",
                "geometry",
                "recognitions",
                "material"
            ]
            .contains(&kind),
            "{read}"
        );
    }
    // The module's code names no confirmation that evidence was read, no client action and no
    // capture; the Runtime source hands its client only to the reads of `status`, the run
    // listing, event pages, `run show`, `run evidence` and `material`.
    let sources = [
        ("report.rs", include_str!("report.rs")),
        ("report/source.rs", include_str!("report/source.rs")),
        ("report/scores.rs", include_str!("report/scores.rs")),
        ("report/summary.rs", include_str!("report/summary.rs")),
        ("report/archive.rs", include_str!("report/archive.rs")),
    ];
    for (file, body) in sources {
        let code = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for word in [
            "evidence_read",
            "client_action",
            "ClientAction",
            "confirm",
            "observe_export",
            "ObserveReadonly",
            "CaptureSequence",
            "frames::",
        ] {
            assert!(!code.contains(word), "{file} names {word}");
        }
    }
    let compact = include_str!("report/source.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::split_whitespace)
        .collect::<String>();
    // Fix round 1, item 7: the errors view through the search for the newest records `events`
    // has; fix round 2, item 4: through `errors_view`, which holds that search to its time.
    let reaching = [
        "self.client.status(",
        "self.client.list_contained_runs(",
        "self.client.visit_events(",
        "errors_view(self.client,",
        "run_show::read(self.client,",
        "run_evidence::read(self.client,",
        "material::resolve(self.client,",
        "material::read_whole(self.client,",
    ]
    .iter()
    .map(|pattern| compact.matches(pattern).count())
    .sum::<usize>();
    assert!(reaching > 0);
    assert_eq!(compact.matches("self.client").count(), reaching);
}

#[test]
fn rpt1_no_frames_leaves_out_every_frame_and_the_text_ocr_read() {
    let folder = TempDir::new().expect("a temporary folder");
    let source = fake(64, 0);
    let exported = export_from(
        &source,
        &asked(folder.path(), false, default_bound()),
        &versions(),
    );
    let entries = archive_entries(&exported.path);
    // No frame is read or written: the only materials read are the two streams.
    assert_eq!(exported.frames, 0);
    assert!(!names(&entries).iter().any(|name| name.ends_with(".png")));
    let materials = source
        .reads()
        .into_iter()
        .filter(|read| read.starts_with("material "))
        .collect::<Vec<_>>();
    assert_eq!(
        materials,
        [
            format!("material {A_STREAM} {}", report::MAX_STREAM_BYTES),
            format!("material {B_STREAM} {}", report::MAX_STREAM_BYTES),
        ]
    );
    // The text OCR read is nowhere; the scores stay.
    for (name, bytes) in &entries {
        assert!(
            !String::from_utf8_lossy(bytes).contains("Welcome back"),
            "{name}"
        );
    }
    let summary = text(&entries, "report/SUMMARY.md");
    for line in [
        "- Frames / 截图: none: what the screen showed is missing, and the text and values read \
         from it are removed / 未带截图：画面内容缺失，从截图读出的文字和数值已去掉",
        "Screenshots / 截图: none; the text and values read from them are left out too",
        "| title_text | ocr | no |  |  |  |  | 1 | (removed with the frames) |",
        "| confirm_button | template | no | 0.620 | 0.740 | 0.850 | -0.230 | 2 |  |",
        "frames: left out / 不带截图",
    ] {
        assert!(summary.contains(line), "{line}\n{summary}");
    }
    let scores = document(&entries, "report/runs/run-a/scores.json");
    assert_eq!(scores["scores"]["text_removed"], true);
    let excerpt = text(&entries, "report/runs/run-a/overlay-excerpt.jsonl");
    let ocr = excerpt
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a record"))
        .find(|record| record["index"] == 8)
        .expect("the OCR record");
    assert_eq!(ocr["text_removed"], true);
    assert_eq!(ocr["data"].get("raw_text"), None);
    assert_eq!(ocr["data"].get("derived_text"), None);
    // The manifest says what is missing and why.
    let manifest = document(&entries, "report/manifest.json");
    assert_eq!(manifest["frames"], false);
    assert!(
        manifest["left_out"]
            .as_array()
            .expect("left_out")
            .contains(&json!({"file": "runs/*/frames/*.png", "why": "no_frames"})),
        "{manifest}"
    );
}

#[test]
fn rpt1_the_size_bound_leaves_out_frames_then_excerpts_and_says_so() {
    let folder = TempDir::new().expect("a temporary folder");
    // Fix round 1, item 8: the bound counts the archive's packed bytes. Frames of 100 KiB
    // (stored as they are) and a record of 900 KiB that deflate cannot shrink much (some 680 KiB
    // packed) in each failing step's stream, within 1 MiB: with every frame left out the two
    // excerpts are still over it, with one of them left out the rest fits.
    let source = fake(100 * 1024, 900 * 1024);
    let limit = report::MIB;
    let exported = export_from(&source, &asked(folder.path(), true, limit), &versions());
    let entries = archive_entries(&exported.path);
    // The older run's frames first (its last first), then the newer run's, then the older
    // run's excerpt; then the archive fits and the rest stays.
    let expected = [
        "runs/run-b/frames/115-step-last.png",
        "runs/run-b/frames/110-decided.png",
        "runs/run-a/frames/52-run-last.png",
        "runs/run-a/frames/47-step-last.png",
        "runs/run-a/frames/41-decided.png",
        "runs/run-b/overlay-excerpt.jsonl",
    ];
    let left = exported
        .left_out
        .iter()
        .filter(|left| left["why"] == "size_bound")
        .map(|left| left["file"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(left, expected);
    let trimmed = exported
        .warnings
        .iter()
        .find(|warning| warning.is(ActingctlCode::ReportTrimmed))
        .expect("report_trimmed");
    assert_eq!(trimmed.value("count"), Some(&json!(expected.len())));
    assert_eq!(trimmed.value("limit_bytes"), Some(&json!(limit)));
    assert_eq!(
        trimmed.value("report_fit"),
        Some(&json!(ReportFit::Fits.as_str()))
    );
    assert!(exported.size <= limit, "{}", exported.size);
    assert!(exported.within_bound);
    let kept = names(&entries);
    for name in [
        "report/runs/run-a/overlay-excerpt.jsonl",
        "report/runs/run-a/run.json",
        "report/runs/run-a/steps.json",
        "report/runs/run-a/scores.json",
        "report/runs/run-b/run.json",
        "report/runs/run-b/steps.json",
        "report/runs/run-b/scores.json",
        "report/instances/alpha/errors.jsonl",
        "report/instances/beta/errors.jsonl",
        "report/versions.json",
    ] {
        assert!(kept.contains(&name), "{name}: {kept:?}");
    }
    for name in expected {
        assert!(!kept.contains(&format!("report/{name}").as_str()), "{name}");
    }
    // The manifest and the summary name every part left out.
    let manifest = document(&entries, "report/manifest.json");
    assert_eq!(manifest["left_out"], json!(exported.left_out));
    let summary = text(&entries, "report/SUMMARY.md");
    assert!(summary.contains("## Left out / 未放入\n"), "{summary}");
    for name in expected {
        assert!(
            summary.contains(&format!("- {name}: size_bound\n")),
            "{name}\n{summary}"
        );
    }
    assert!(
        summary.contains("runs/run-a/frames/41-decided.png (left out: size bound)"),
        "{summary}"
    );
}

#[test]
fn rpt1_the_errors_view_of_each_instance_newest_rows_counted() {
    let folder = TempDir::new().expect("a temporary folder");
    let source = fake(64, 0);
    let exported = export_from(
        &source,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let entries = archive_entries(&exported.path);
    // Each instance's errors view, read with the report's window and the bound of its file.
    let reads = source.reads();
    for alias in ["alpha", "beta"] {
        let read = format!(
            "errors {alias} {SINCE} None {}",
            report::ERRORS_PER_INSTANCE
        );
        assert!(reads.contains(&read), "{read}: {reads:?}");
    }
    // The file keeps the newest rows, oldest first.
    let alpha = text(&entries, "report/instances/alpha/errors.jsonl");
    let rows = alpha
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a row"))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), report::ERRORS_PER_INSTANCE);
    assert_eq!(rows[0]["run_id"], "run-e51");
    assert_eq!(rows[rows.len() - 1]["run_id"], "run-e250");
    // The summary says how many rows the file keeps, whether they are the newest of the window
    // or every row of it (fix round 1, item 7: the newest rows are searched for, the window is
    // not read from its start, so no total is counted), and shows the newest ten.
    let summary = text(&entries, "report/SUMMARY.md");
    for line in [
        "## Errors view / 错误视图: alpha\n",
        "200 rows in instances/alpha/errors.jsonl, the newest of the window; older rows were not \
         read / 时间窗里最新的 200 行，更早的没有读.\n",
        "3 rows in instances/beta/errors.jsonl, every row of the window / 时间窗里的全部记录.\n",
        "| time | severity | code | module | run |\n",
        "| runtime_host | run-e250 |\n",
        "| runtime_host | run-e241 |\n",
    ] {
        assert!(summary.contains(line), "{line}\n{summary}");
    }
    assert!(!summary.contains("run-e240"), "{summary}");
    let data = document(&entries, "report/summary.json");
    assert_eq!(data["errors"][0]["alias"], "alpha");
    assert_eq!(data["errors"][0]["kept"], report::ERRORS_PER_INSTANCE);
    assert_eq!(data["errors"][0]["newest_only"], true);
    assert_eq!(data["errors"][0]["searched_to_start"], true);
    assert_eq!(data["errors"][0]["in_archive"], true);
    assert_eq!(data["errors"][1]["newest_only"], false);
}

#[test]
fn rpt1_every_read_names_a_request_or_an_instance_and_the_window() {
    let folder = TempDir::new().expect("a temporary folder");
    let source = fake(64, 0);
    export_from(
        &source,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let stream = report::MAX_STREAM_BYTES;
    let frame_bound = MAX_EXPORT_BYTES as u64;
    let errors = report::ERRORS_PER_INSTANCE;
    let listed = report::CANDIDATES_PER_INSTANCE;
    let before = NOW - HOUR - 1;
    assert_eq!(
        source.reads(),
        [
            "instances None".to_owned(),
            // Each instance's failed and unfinished runs, in the window.
            format!("runs alpha {SINCE} None Unfinished {listed}"),
            format!("runs beta {SINCE} None Unfinished {listed}"),
            // The run first recorded before the newest on its instance, in the window.
            format!("runs alpha {SINCE} Some({before}) Any 1"),
            // Each run by its request id; its recognition records from its failing step's
            // first record; its stream and frames by their references.
            "run req-a".to_owned(),
            "evidence req-a".to_owned(),
            "geometry req-a".to_owned(),
            "recognitions req-a 40 None".to_owned(),
            format!("material {A_STREAM} {stream}"),
            format!("material {A_DECIDED} {frame_bound}"),
            format!("material {A_STEP_LAST} {frame_bound}"),
            format!("material {A_RUN_LAST} {frame_bound}"),
            "run req-b".to_owned(),
            "evidence req-b".to_owned(),
            "geometry req-b".to_owned(),
            "recognitions req-b 100 None".to_owned(),
            format!("material {B_STREAM} {stream}"),
            format!("material {B_DECIDED} {frame_bound}"),
            format!("material {B_STEP_LAST} {frame_bound}"),
            // Fix round 1, item 7: then each instance's errors view, its newest rows in the
            // window.
            format!("errors alpha {SINCE} None {errors}"),
            format!("errors beta {SINCE} None {errors}"),
        ]
    );

    // With an instance named, no other instance is read.
    let folder = TempDir::new().expect("a temporary folder");
    let source = fake(64, 0);
    let named = Request {
        instance: Some("beta".to_owned()),
        ..asked(folder.path(), true, default_bound())
    };
    let exported = export_from(&source, &named, &versions());
    assert_eq!(exported.instances, ["beta"]);
    let reads = source.reads();
    assert_eq!(reads[0], "instances Some(\"beta\")");
    assert!(
        !reads
            .iter()
            .any(|read| read.contains("alpha") || read.contains("req-a")),
        "{reads:?}"
    );
}

#[test]
fn rpt1_the_tables_row_report_export() {
    let table = operation_table().unwrap_or_else(|outcome| panic!("the table reads: {outcome}"));
    let row = table
        .operation(report::ROW)
        .unwrap_or_else(|| panic!("the table has the row {}", report::ROW));
    assert_eq!(row.module, OperationModule::Core);
    assert_eq!(row.slice, "RPT-1");
    assert_eq!(row.runtime, RuntimeNeed::Required);
    assert!(!row.writes);
    assert!(!row.irreversible);
    assert!(!row.disruptive);
    assert_eq!(row.entries, [Entry::Lab, Entry::Ui]);
    assert_eq!(
        row.requests,
        [
            RuntimeOperationKind::Status,
            RuntimeOperationKind::QueryEvents,
            RuntimeOperationKind::ReadMaterial,
        ]
    );
    // Every kind it sends is on the UI list (the console will run the Lab's program for it).
    for kind in &row.requests {
        assert!(table.ui_kinds.iter().any(|ui| ui.kind == *kind), "{kind:?}");
    }
    // Its parameters are those its code reads, in order.
    let reads = READ_HERE
        .iter()
        .find(|reads| reads.row == report::ROW)
        .unwrap_or_else(|| panic!("core_reads holds {}", report::ROW));
    let declared = row
        .params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(declared, reads.names);
    assert_eq!(param(row, "instance").value_type, ParamType::Instance);
    assert_eq!(
        param(row, "view").value_type,
        ParamType::Enum(vec![report::VIEW_ERRORS.to_owned()])
    );
    assert_eq!(param(row, "out").value_type, ParamType::Dir);
    assert_eq!(
        param(row, "out").required_in,
        Some(vec!["cli".to_owned(), "serve".to_owned()])
    );
    assert_eq!(param(row, "no_frames").form, ParamForm::Switch);
    // One sentence of its own; the core module serves it; its tool; the placeholder it replaces.
    let help = row.help.as_deref().unwrap_or_default();
    assert!(
        !help.is_empty() && help.len() <= MAX_HELP_BYTES && help.is_ascii(),
        "{help}"
    );
    assert!(!help.contains("ac_"), "{help}");
    assert!(core_ops::serves(row));
    assert_eq!(row.mcp_tool(), "ac_report_export");
    assert_eq!(row.source, ["actinglab report export --last-error"]);
}

#[test]
fn rpt1_fix1_a_listing_not_read_to_its_end_never_says_none() {
    // alpha's listing stops before its end: beta's run is the only one, the summary says for
    // each instance whether its listing was read to its end, and the part missing names alpha.
    let folder = TempDir::new().expect("a temporary folder");
    let mut source = fake(64, 0);
    source.cut_listing = Some("alpha");
    let exported = export_from(
        &source,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let warning = missing_part(
        &exported,
        ReportPart::RunListing,
        ReportPartReason::NotReadToEnd,
        "alias",
        "alpha",
    );
    assert_eq!(warning.value("file"), None, "{warning}");
    assert_eq!(exported.runs.len(), 1);
    assert_eq!(exported.runs[0]["alias"], "beta");
    let entries = archive_entries(&exported.path);
    let summary = text(&entries, "report/SUMMARY.md");
    for line in [
        "- Listing of alpha / alpha 的运行列表: not read to its end (`not_read_to_end`): the \
         window may hold failed or unfinished runs it does not show",
        "- Listing of beta / beta 的运行列表: read to its end / 读完\n",
    ] {
        assert!(summary.contains(line), "{line}\n{summary}");
    }
    let data = document(&entries, "report/summary.json");
    assert_eq!(
        data["listings"][0],
        json!({"alias": "alpha", "read_to_end": false, "why": "not_read_to_end"})
    );
    assert_eq!(data["listings"][1]["read_to_end"], true);

    // Only alpha, its listing cut: no run, and the summary never says the window holds none.
    let folder = TempDir::new().expect("a temporary folder");
    let mut alone = fake(64, 0);
    alone.instances = vec!["alpha"];
    alone.cut_listing = Some("alpha");
    let exported = export_from(
        &alone,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    assert!(exported.runs.is_empty());
    let summary = text(&archive_entries(&exported.path), "report/SUMMARY.md");
    assert!(
        !summary.contains("No failed or unfinished run in the window"),
        "{summary}"
    );
    assert!(
        summary.contains(
            "No failed or unfinished run was found, but the listing of alpha was not read to \
             its end: the window may hold some"
        ),
        "{summary}"
    );

    // An instance without a run whose listing was read to its end: none, and it says so.
    let folder = TempDir::new().expect("a temporary folder");
    let mut quiet = fake(64, 0);
    quiet.instances = vec!["gamma"];
    let exported = export_from(
        &quiet,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    assert!(exported.runs.is_empty());
    let summary = text(&archive_entries(&exported.path), "report/SUMMARY.md");
    assert!(
        summary.contains(
            "No failed or unfinished run in the window / 时间窗里没有失败或未结束的运行。"
        ),
        "{summary}"
    );
}

#[test]
fn rpt1_fix1_still_over_the_bound_is_said_in_the_manifest_the_summary_and_the_answer() {
    // A bound smaller than the parts that are never left out: every part that may be left out
    // is, in its order, and the archive is still over the bound, which the warning, the
    // manifest, the summary and the answer say.
    let folder = TempDir::new().expect("a temporary folder");
    let limit = 1024;
    let exported = export_from(
        &fake(64, 0),
        &asked(folder.path(), true, limit),
        &versions(),
    );
    let expected = [
        "runs/run-b/frames/115-step-last.png",
        "runs/run-b/frames/110-decided.png",
        "runs/run-a/frames/52-run-last.png",
        "runs/run-a/frames/47-step-last.png",
        "runs/run-a/frames/41-decided.png",
        "runs/run-b/overlay-excerpt.jsonl",
        "runs/run-a/overlay-excerpt.jsonl",
        "instances/beta/errors.jsonl",
        "instances/alpha/errors.jsonl",
    ];
    let left = exported
        .left_out
        .iter()
        .map(|left| left["file"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(left, expected);
    assert!(exported.size > limit, "{}", exported.size);
    assert!(!exported.within_bound);
    let trimmed = exported
        .warnings
        .iter()
        .find(|warning| warning.is(ActingctlCode::ReportTrimmed))
        .expect("report_trimmed");
    assert_eq!(
        trimmed.value("report_fit"),
        Some(&json!(ReportFit::StillOver.as_str()))
    );
    assert_eq!(trimmed.value("count"), Some(&json!(expected.len())));
    let entries = archive_entries(&exported.path);
    let manifest = document(&entries, "report/manifest.json");
    assert_eq!(manifest["within_bound"], false);
    let summary = text(&entries, "report/SUMMARY.md");
    assert!(
        summary.contains(
            "Size / 大小: over the bound of 1024 bytes, with every part that may be left out \
             left out"
        ),
        "{summary}"
    );
    let answer = report::line(&exported)
        .line
        .unwrap_or_else(|text| panic!("the answer is a line: {text}"));
    assert_eq!(answer["within_bound"], false, "{answer}");
}

/// Run A's failing step with one record of every kind that holds what a frame shows, each value
/// a sentinel, and an errors view row whose outcome keeps a frame's text as evidence.
fn sentinel_fake() -> Fake {
    let records = [
        record(
            1,
            None,
            "step_started",
            json!({"step_index": 0, "monotonic_ms": 10}),
        ),
        record(
            2,
            None,
            "step_started",
            json!({"step_index": 1, "monotonic_ms": 20}),
        ),
        record(
            3,
            None,
            "target",
            json!({
                "id": "count_check", "kind": "value_test", "passed": false, "template": null,
                "color": null, "source": {"phase": "guard"},
                "failure": {"code": "text_locate_not_found", "values": {
                    "raw_text": "SENTINEL-FAILURE-TEXT",
                }},
                "value_test": {
                    "left": {
                        "target_id": "count_text", "raw_text": "SENTINEL-LEFT-TEXT",
                        "value": 7_340_123,
                    },
                    "op": "ge",
                    "right": {
                        "kind": "operand",
                        "operand": {
                            "target_id": "goal_text", "raw_text": "SENTINEL-RIGHT-TEXT",
                            "value": 8_120_456,
                        },
                        "mul": 1, "add": 0,
                    },
                    "passed": false,
                },
            }),
        ),
        record(
            4,
            Some(3),
            "ocr",
            json!({
                "region": {}, "raw_text": "SENTINEL-OCR-RAW", "derived_text": "SENTINEL-OCR-DERIVED",
                "confidence": 0.91, "matched_expected": "SENTINEL-OCR-EXPECTED",
                "match_mode": "contains", "block_count": 1,
            }),
        ),
        record(
            5,
            Some(4),
            "ocr_block",
            json!({
                "source_index": 0, "derived_rank": 0,
                "raw": {"text": "SENTINEL-BLOCK-TEXT", "rect": {}, "confidence": 0.91},
            }),
        ),
        record(
            6,
            None,
            "nn",
            json!({
                "requested_region": {}, "selected_label": "SENTINEL-NN-SELECTED",
                "selected_score": 0.88, "selection": "best", "label_count": 1,
            }),
        ),
        record(
            7,
            Some(6),
            "nn_label",
            json!({
                "source_index": 0, "raw": {"label": "SENTINEL-NN-LABEL", "score": 0.88},
                "derived": {"candidate": true, "rank": 0},
            }),
        ),
        record(
            8,
            Some(3),
            "color_digest",
            json!({
                "algorithm": "color_digest.v1", "columns": 2, "rows": 1, "active_cells": 2,
                "mean_milli": 10, "max_cell": 3, "worst_cell": 1, "max_mean_milli": 50,
                "max_cell_threshold": null, "observed_cells": "0badc0ffee0badc0ffee",
            }),
        ),
        record(
            9,
            None,
            "error",
            json!({
                "phase": "page",
                "outcome": {"code": "text_locate_not_found", "values": {
                    "raw_text": "SENTINEL-ERROR-TEXT",
                }},
            }),
        ),
        record(
            10,
            None,
            "terminal",
            json!({"result": "failed", "executed_steps": 2}),
        ),
    ];
    let mut source = fake(64, 0);
    source.materials.insert(A_STREAM, framed(&records));
    source.errors.entry("alpha").or_default().push(json!({
        "seq": 2_000, "ts": SINCE + 300_000, "type": "task.failed", "severity": "error",
        "code": "text_locate_not_found", "run_id": "run-e300", "module": "runtime_host",
        "outcome": {"code": "text_locate_not_found", "values": {"raw_text": "SENTINEL-ROW-TEXT"}},
    }));
    source
}

/// The member at `path` of `value`, when it is there.
fn member<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |inner, name| inner.get(*name))
}

#[test]
fn rpt1_fix1_no_frames_leaves_out_every_text_and_value_read_from_a_frame() {
    // What a frame shows, as the failing step's stream and the errors view hold it: an OCR
    // read's text, derived text and matched expected text, an OCR block's text, the neural
    // recognition's selected label and each label, a colour digest's observed cells, both
    // sides of a value test (the text read and the value taken from it), and the text an
    // outcome keeps as evidence (a target's failure, an error record, an errors view row).
    let texts = [
        "SENTINEL-FAILURE-TEXT",
        "SENTINEL-LEFT-TEXT",
        "SENTINEL-RIGHT-TEXT",
        "SENTINEL-OCR-RAW",
        "SENTINEL-OCR-DERIVED",
        "SENTINEL-OCR-EXPECTED",
        "SENTINEL-BLOCK-TEXT",
        "SENTINEL-NN-SELECTED",
        "SENTINEL-NN-LABEL",
        "0badc0ffee0badc0ffee",
        "SENTINEL-ERROR-TEXT",
        "SENTINEL-ROW-TEXT",
        "7340123",
        "8120456",
    ];
    let contents = |exported: &Exported| {
        archive_entries(&exported.path)
            .iter()
            .map(|(name, bytes)| format!("{name}\n{}", String::from_utf8_lossy(bytes)))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // With frames every one of them is in the archive: the fixture reaches it.
    let folder = TempDir::new().expect("a temporary folder");
    let with = export_from(
        &sentinel_fake(),
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let written = contents(&with);
    for text in texts {
        assert!(
            written.contains(text),
            "{text} is in the report with frames"
        );
    }

    // Without frames none of them is in any entry.
    let folder = TempDir::new().expect("a temporary folder");
    let without = export_from(
        &sentinel_fake(),
        &asked(folder.path(), false, default_bound()),
        &versions(),
    );
    let written = contents(&without);
    for text in texts {
        assert!(
            !written.contains(text),
            "{text} is in the report without frames"
        );
    }
    // Each record and row lost exactly those members and says so; what measures a frame
    // without showing it stays (a confidence, a score, the comparison).
    let entries = archive_entries(&without.path);
    let excerpt = text(&entries, "report/runs/run-a/overlay-excerpt.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a record"))
        .collect::<Vec<_>>();
    let gone: [(u64, &[&str]); 13] = [
        (3, &["data", "failure", "values", "raw_text"]),
        (3, &["data", "value_test", "left", "raw_text"]),
        (3, &["data", "value_test", "left", "value"]),
        (3, &["data", "value_test", "right", "operand", "raw_text"]),
        (3, &["data", "value_test", "right", "operand", "value"]),
        (4, &["data", "raw_text"]),
        (4, &["data", "derived_text"]),
        (4, &["data", "matched_expected"]),
        (5, &["data", "raw", "text"]),
        (6, &["data", "selected_label"]),
        (7, &["data", "raw", "label"]),
        (8, &["data", "observed_cells"]),
        (9, &["data", "outcome", "values", "raw_text"]),
    ];
    for (index, path) in gone {
        let record = excerpt
            .iter()
            .find(|record| record["index"] == index)
            .unwrap_or_else(|| panic!("record {index} is in the excerpt"));
        assert_eq!(record["text_removed"], true, "{record}");
        assert_eq!(member(record, path), None, "{index} {path:?}: {record}");
    }
    let stays: [(u64, &[&str], Value); 5] = [
        (3, &["data", "value_test", "op"], json!("ge")),
        (4, &["data", "confidence"], json!(0.91)),
        (6, &["data", "selected_score"], json!(0.88)),
        (8, &["data", "max_cell"], json!(3)),
        (9, &["data", "phase"], json!("page")),
    ];
    for (index, path, kept) in stays {
        let record = excerpt
            .iter()
            .find(|record| record["index"] == index)
            .unwrap_or_else(|| panic!("record {index} is in the excerpt"));
        assert_eq!(member(record, path), Some(&kept), "{record}");
    }
    let rows = text(&entries, "report/instances/alpha/errors.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a row"))
        .collect::<Vec<_>>();
    let row = rows
        .iter()
        .find(|row| row["run_id"] == "run-e300")
        .expect("the row with evidence");
    assert_eq!(row["text_removed"], true, "{row}");
    assert_eq!(
        member(row, &["outcome", "values", "raw_text"]),
        None,
        "{row}"
    );
    assert_eq!(row["outcome"]["code"], "text_locate_not_found", "{row}");
    let scores = document(&entries, "report/runs/run-a/scores.json");
    assert_eq!(scores["scores"]["text_removed"], true);
    assert_eq!(scores["scores"]["targets"][0]["confidence"], 0.91);
}

#[test]
fn rpt1_fix1_the_masks_replace_names_inside_words_escaped_paths_and_dns_names() {
    let mask = Mask::new(
        Some("Zephyrine"),
        &[
            "HOSTVERDANT",
            "hostverdant-tower",
            "hostverdant-tower.corp.example",
        ],
    );
    for (given, masked) in [
        // A name inside a longer word, in another case.
        (
            "D:\\packs\\zephyrine_packs\\neutral",
            "D:\\packs\\<user>_packs\\neutral",
        ),
        // A path's Users folder segment, once and doubly escaped.
        ("C:\\Users\\Someone\\AppData", "C:\\Users\\<user>\\AppData"),
        (
            "C:\\\\Users\\\\Someone\\\\AppData",
            "C:\\\\Users\\\\<user>\\\\AppData",
        ),
        ("/users//someone/x", "/users//<user>/x"),
        // The full DNS name, the longer DNS host name and the NetBIOS name, the longest first.
        (
            "from hostverdant-tower.corp.example:5555",
            "from <computer>:5555",
        ),
        ("HostVerdant-Tower is up", "<computer> is up"),
        ("build-HOSTVERDANT-01", "build-<computer>-01"),
        ("ZEPHYRINEhostverdant", "<user><computer>"),
    ] {
        assert_eq!(mask.text(given), masked, "{given}");
    }
    assert!(mask.applied() > 0);
    // A mark already written is never searched again, even for a name that is in it. Fix round
    // 2 (the coordinator's ruling, which replaces round 1's item 11): a name of fewer than five
    // characters is replaced only where it stands alone, so `User` in `username` stays.
    let user = Mask::new(Some("User"), &[]);
    assert_eq!(
        user.text("the username in C:\\Users\\Bob\\x and User"),
        "the username in C:\\Users\\<user>\\x and <user>"
    );
    let short = Mask::new(Some("Al"), &["PC"]);
    assert_eq!(short.text("Alpha on PC-2"), "Alpha on <computer>-2");
    assert_eq!(report::SHORTEST_ANYWHERE, 5);
}

// ---- Fix round 2 (`rd-1115/brief-1725-fix2.md`)

/// A made-up instance id: the requests of the errors view's search carry it.
const INSTANCE_ID: &str = "instance_0123456789abcdef0123456789abcdef";
/// The made-up Runtime's ledger head for the search, and the sequences of the records it holds.
const SEARCH_HEAD: u64 = 1_000;
const SEARCH_RECORDS: [u64; 3] = [900, 950, 980];
/// An alias that holds the made-up user's name.
const NAMED_ALIAS: &str = "zephyrine-pc";

fn instance_id() -> InstanceId {
    serde_json::from_value(json!(INSTANCE_ID))
        .unwrap_or_else(|_| panic!("{INSTANCE_ID} is an instance id"))
}

/// The instance `alpha` with its id, and the report's window.
fn searched_instance() -> Instance {
    Instance {
        alias: "alpha".to_owned(),
        id: Some(instance_id()),
    }
}

fn day() -> Window {
    Window {
        since_unix_ms: SINCE,
        until_unix_ms: None,
    }
}

/// A failure of the made-up Runtime, which never fails.
fn read_failed(error: &RuntimeClientError) -> Missed {
    Missed::new(ReportPartReason::ReadFailed, error.outcome())
}

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the made-up records hold no secret")
    }
}

/// One made-up record at `sequence`, recorded `sequence` seconds after the window's start, as a
/// forensic read projects it (the issuer mints its event id).
fn search_record(issuer: &IdentifierIssuer, sequence: u64) -> ProjectedEvent {
    let payload: EventPayloadDraft =
        RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into();
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        SINCE + sequence * 1_000,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        EventLinksDraft::default(),
        payload,
    )
    .sanitize(&NoSecrets)
    .expect("sanitize a made-up record");
    ProjectedEvent {
        views: Vec::new(),
        artifact_evictions: Vec::new(),
        schema_version: sanitized.schema_version().to_owned(),
        sequence,
        event_id: *sanitized.event_id(),
        timestamp_unix_ms: sanitized.timestamp_unix_ms(),
        event_type: sanitized.event_type(),
        severity: sanitized.severity(),
        sensitivity: sanitized.sensitivity(),
        origin: sanitized.origin().clone(),
        links: sanitized.links().clone(),
        payload_schema: sanitized.payload_schema().to_owned(),
        payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
        artifacts: Vec::new(),
    }
}

/// One QueryEvents page request the search sent.
struct Asked {
    query: EventQuery,
    page: RuntimeEventQueryPageRequest,
}

/// The made-up Runtime the errors view's search reads (fix round 2): its ledger at the snapshot
/// [`SEARCH_HEAD`] holds a record at each of [`SEARCH_RECORDS`]; a page answers the records of
/// its sequence range recorded from its time bound's start on, at most its limit, with nothing
/// more to come; every request is kept as the search built it.
struct Pages {
    records: Vec<ProjectedEvent>,
    asked: RefCell<Vec<Asked>>,
}

impl Pages {
    fn new() -> Self {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        Self {
            records: SEARCH_RECORDS
                .iter()
                .map(|sequence| search_record(&issuer, *sequence))
                .collect(),
            asked: RefCell::new(Vec::new()),
        }
    }

    /// How many pages were asked for.
    fn count(&self) -> usize {
        self.asked.borrow().len()
    }
}

impl EventPages for Pages {
    fn event_page(
        &self,
        query: EventQuery,
        _profile: ProjectionProfile,
        page: RuntimeEventQueryPageRequest,
    ) -> Result<RuntimeEventQueryPage, RuntimeClientError> {
        let from = query.from_sequence.unwrap_or(1);
        let through = query.to_sequence.unwrap_or(SEARCH_HEAD);
        let since = query.from_timestamp_unix_ms.unwrap_or(0);
        let limit = page.limit();
        let events = self
            .records
            .iter()
            .filter(|record| {
                (from..=through).contains(&record.sequence) && record.timestamp_unix_ms >= since
            })
            .take(usize::from(limit))
            .cloned()
            .collect::<Vec<_>>();
        self.asked.borrow_mut().push(Asked { query, page });
        Ok(
            RuntimeEventQueryPage::new(events, SEARCH_HEAD, limit, false, None)
                .unwrap_or_else(|_| panic!("a made-up page")),
        )
    }
}

/// The made-up Runtime of [`Fake`] whose errors views are read by the search itself, through
/// [`report::errors_view`] on `pages`, held to the time `late` says is spent (fix round 2,
/// item 4); its instances carry an id.
struct Searched<'a> {
    fake: &'a Fake,
    pages: &'a Pages,
    late: &'a dyn Fn() -> Option<ReportPartReason>,
}

impl Source for Searched<'_> {
    fn instances(&self, selector: Option<&str>) -> Result<Vec<Instance>, Outcome> {
        Ok(self
            .fake
            .instances(selector)?
            .into_iter()
            .map(|instance| Instance {
                id: Some(instance_id()),
                ..instance
            })
            .collect())
    }

    fn runs(
        &self,
        instance: &Instance,
        window: Window,
        which: Which,
        limit: usize,
    ) -> Result<Listed, Missed> {
        self.fake.runs(instance, window, which, limit)
    }

    fn errors(&self, instance: &Instance, window: Window, keep: usize) -> Result<Errors, Missed> {
        report::errors_view(self.pages, instance, window, keep, self.late, &read_failed)
    }

    fn run(&self, request_id: &str) -> Result<Value, Missed> {
        self.fake.run(request_id)
    }

    fn evidence(&self, request_id: &str) -> Result<Evidence, Missed> {
        self.fake.evidence(request_id)
    }

    fn geometry(&self, request_id: &str) -> Result<Vec<Geometry>, Missed> {
        self.fake.geometry(request_id)
    }

    fn recognitions(
        &self,
        request_id: &str,
        from: u64,
        through: Option<u64>,
    ) -> Result<Vec<Recognition>, Missed> {
        self.fake.recognitions(request_id, from, through)
    }

    fn material(&self, reference: &str, limit: u64) -> Result<Vec<u8>, Missed> {
        self.fake.material(reference, limit)
    }
}

/// Every member name of `value`, at any depth.
fn member_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                names.push(name.clone());
                member_names(member, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                member_names(item, names);
            }
        }
        _ => {}
    }
}

#[test]
fn rpt1_fix2_the_masks_keep_member_names_hashes_and_ids_and_replace_a_short_name_alone() {
    // The short names of the review: `val` would cut `values` and `interval`; `ada` and `deb`,
    // made of the letters a to f, would cut a hash and the ids.
    let mask = Mask::new(Some("val"), &["ada", "deb"]);
    for (given, masked) in [
        // The three path and host forms: a Users folder's segment (once and doubly escaped), a
        // host name, a folder whose name begins with the name.
        ("C:\\Users\\val\\x", "C:\\Users\\<user>\\x"),
        ("C:\\\\Users\\\\val\\\\x", "C:\\\\Users\\\\<user>\\\\x"),
        ("VAL-PC", "<user>-PC"),
        ("D:\\packs\\val_packs", "D:\\packs\\<user>_packs"),
        ("on ada.deb.example", "on <computer>.<computer>.example"),
        // Inside a longer word a short name stays.
        ("values in an interval", "values in an interval"),
        ("cadabra", "cadabra"),
    ] {
        assert_eq!(mask.text(given), masked, "{given}");
    }

    // In a value: the members' names, the numbers, a hash, a commit, an id, a code and the
    // words of an outcome's vocabularies stay whole; the free text is masked.
    let digest = format!("sha256:{}adad", "adadeb".repeat(10));
    let commit = format!("{}adad", "adadeb".repeat(6));
    let request = format!("request_{}", "ada0".repeat(8));
    let mut value = json!({
        "values": "val",
        "interval": {"max_bytes": 24, "runtime_online": true},
        "sha256": digest,
        "commit_sha": commit,
        "request_id": request,
        "path": "C:\\Users\\val\\x",
        "row": {
            "code": "val_missing",
            "outcome": {
                "code": "report_part_missing",
                "values": {
                    "report_part": "run_listing",
                    "report_part_reason": "not_read_to_end",
                    "alias": "val",
                },
            },
        },
    });
    mask.value(&mut value);
    assert_eq!(
        value,
        json!({
            "values": "<user>",
            "interval": {"max_bytes": 24, "runtime_online": true},
            "sha256": digest,
            "commit_sha": commit,
            "request_id": request,
            "path": "C:\\Users\\<user>\\x",
            "row": {
                "code": "val_missing",
                "outcome": {
                    "code": "report_part_missing",
                    "values": {
                        "report_part": "run_listing",
                        "report_part_reason": "not_read_to_end",
                        "alias": "<user>",
                    },
                },
            },
        })
    );
    // A short name that stands alone in a code or a vocabulary's word: the word stays where it
    // is a code or a word, the same text is replaced where it is free text.
    let run = Mask::new(Some("run"), &[]);
    let mut outcome =
        json!({"code": "run_unknown", "values": {"report_part": "run_listing", "alias": "run"}});
    run.value(&mut outcome);
    assert_eq!(
        outcome,
        json!({"code": "run_unknown", "values": {"report_part": "run_listing", "alias": "<user>"}})
    );
    assert_eq!(run.text("run_listing"), "<user>_listing");

    // A report masked by them: its run's ids hold `ada` and `deb`, its errors view a row with
    // the three forms, its versions a commit of `ada` and `deb`.
    let folder = TempDir::new().expect("a temporary folder");
    let mut source = fake(64, 0);
    let run_id = format!("run_{}", "deb0".repeat(8));
    source.runs[0].row.request_id = request.clone();
    source.runs[0].row.run_id = Some(run_id.clone());
    source.errors.entry("alpha").or_default().push(json!({
        "seq": 2_000, "ts": SINCE + 400_000, "type": "task.failed", "severity": "error",
        "code": "val_missing", "run_id": run_id, "module": "runtime_host", "interval_ms": 5,
        "path": "C:\\Users\\val\\x", "host": "VAL-PC", "folder": "D:\\packs\\val_packs",
        "outcome": {"code": "report_part_missing", "values": {
            "report_part": "run_listing", "alias": "val",
        }},
    }));
    let installed = Installed {
        versions: json!({
            "lab_ops": "0.1.0",
            "runtime": {"repository": "example/runtime", "commit_sha": commit},
            "tools": null,
            "ui": null,
            "bundles": {},
        }),
        warnings: Vec::new(),
    };
    let exported = report::export(
        &source,
        &asked(folder.path(), true, default_bound()),
        &installed,
        &mask,
    )
    .unwrap_or_else(|outcome| panic!("the report is written: {outcome}"));
    let entries = archive_entries(&exported.path);
    // Every member name of every JSON entry is whole.
    for (name, bytes) in &entries {
        let documents = if name.ends_with(".jsonl") {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
                .collect::<Vec<_>>()
        } else if name.ends_with(".json") {
            vec![serde_json::from_slice::<Value>(bytes).expect("JSON")]
        } else {
            continue;
        };
        for document in documents {
            let mut found = Vec::new();
            member_names(&document, &mut found);
            assert!(
                found
                    .iter()
                    .all(|member| !member.contains(report::USER_MARK)
                        && !member.contains(report::COMPUTER_MARK)),
                "{name}: {found:?}"
            );
        }
    }
    let manifest = document(&entries, "report/manifest.json");
    for member in [
        "max_bytes",
        "runtime_online",
        "within_bound",
        "masks_applied",
    ] {
        assert!(manifest.get(member).is_some(), "{member}: {manifest}");
    }
    // The manifest checks its entries: each file it names is an entry of its size and digest,
    // and it names every entry but itself.
    let files = manifest["files"].as_array().expect("the manifest's files");
    assert_eq!(files.len() + 1, entries.len());
    for file in files {
        let path = file["path"].as_str().unwrap_or_default();
        let bytes = entry(&entries, path);
        assert_eq!(file["size"], bytes.len(), "{path}");
        assert_eq!(file["sha256"], hex(&Sha256::digest(bytes)), "{path}");
    }
    // The ids and the commit stay whole, in the run's folder too.
    let run_folder = format!("report/runs/{run_id}");
    let run_json = document(&entries, &format!("{run_folder}/run.json"));
    assert_eq!(run_json["listing"]["request_id"], json!(request));
    assert_eq!(run_json["listing"]["run_id"], json!(run_id));
    assert_eq!(
        document(&entries, &format!("{run_folder}/steps.json"))["request_id"],
        json!(request)
    );
    assert_eq!(
        document(&entries, "report/versions.json")["runtime"]["commit_sha"],
        json!(commit)
    );
    let summary = text(&entries, "report/SUMMARY.md");
    assert!(
        summary.contains(&format!("## Run 1 / 运行 1: {run_id} ({request})")),
        "{summary}"
    );
    // The short names are replaced where they stand alone: the three forms of the errors row;
    // its code, its member names and the word of its outcome stay.
    let row = text(&entries, "report/instances/alpha/errors.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a row"))
        .find(|row| row["seq"] == 2_000)
        .expect("the row with the three forms");
    assert_eq!(row["path"], "C:\\Users\\<user>\\x", "{row}");
    assert_eq!(row["host"], "<user>-PC", "{row}");
    assert_eq!(row["folder"], "D:\\packs\\<user>_packs", "{row}");
    assert_eq!(row["code"], "val_missing", "{row}");
    assert_eq!(row["interval_ms"], 5, "{row}");
    assert_eq!(
        row["outcome"]["values"],
        json!({"report_part": "run_listing", "alias": "<user>"}),
        "{row}"
    );
}

#[test]
fn rpt1_fix2_an_alias_holding_the_name_names_its_entry_as_the_manifest_does() {
    // A third instance whose alias holds the user's name, with three rows in its errors view.
    let folder = TempDir::new().expect("a temporary folder");
    let mut source = fake(64, 0);
    source.instances.push(NAMED_ALIAS);
    source
        .errors
        .insert(NAMED_ALIAS, (1..=3).map(error_row).collect());
    let exported = export_from(
        &source,
        &asked(folder.path(), true, default_bound()),
        &versions(),
    );
    let entries = archive_entries(&exported.path);
    // The entry is named from the masked alias, and the manifest names it as the archive does.
    let file = "instances/_user_-pc/errors.jsonl";
    let named = format!("report/{file}");
    let bytes = entry(&entries, &named);
    let manifest = document(&entries, "report/manifest.json");
    let files = manifest["files"].as_array().expect("the manifest's files");
    let row = files
        .iter()
        .find(|row| row["path"] == named.as_str())
        .unwrap_or_else(|| panic!("the manifest names {named}: {manifest}"));
    assert_eq!(row["size"], bytes.len());
    assert_eq!(row["sha256"], hex(&Sha256::digest(bytes)));
    // Every name the manifest gives is an entry's; no entry's name holds the user's name.
    let entry_names = names(&entries);
    for row in files {
        let path = row["path"].as_str().unwrap_or_default();
        assert!(entry_names.contains(&path), "{path}: {entry_names:?}");
    }
    for name in &entry_names {
        assert!(!name.to_ascii_lowercase().contains("zephyrine"), "{name}");
    }
    // The same masked alias in the manifest, the summary's data and the page.
    assert_eq!(manifest["instances"], json!(["alpha", "beta", "<user>-pc"]));
    let data = document(&entries, "report/summary.json");
    assert_eq!(data["errors"][2]["alias"], "<user>-pc");
    assert_eq!(data["errors"][2]["file"], file);
    assert_eq!(data["errors"][2]["in_archive"], true);
    let summary = text(&entries, "report/SUMMARY.md");
    for line in [
        "## Errors view / 错误视图: <user>-pc\n".to_owned(),
        format!("3 rows in {file}, every row of the window"),
    ] {
        assert!(summary.contains(&line), "{line}\n{summary}");
    }
}

#[test]
fn rpt1_fix2_every_read_of_the_errors_search_carries_the_instance() {
    let pages = Pages::new();
    let errors = report::errors_view(
        &pages,
        &searched_instance(),
        day(),
        report::ERRORS_PER_INSTANCE,
        &|| None,
        &read_failed,
    )
    .unwrap_or_else(|miss| panic!("the errors view is read: {miss:?}"));
    assert_eq!(
        errors
            .rows
            .iter()
            .map(|row| row["seq"].as_u64())
            .collect::<Vec<_>>(),
        SEARCH_RECORDS.map(Some)
    );
    assert_eq!(errors.stopped, None);
    // The requests the search built: the head's, the window's start, one window, the rows.
    let asked = pages.asked.borrow();
    assert_eq!(asked.len(), 4);
    // The head's selects no row: its range starts past the head and it has no other condition,
    // the instance included.
    let (head, rest) = asked.split_first().expect("the head's read");
    assert!(
        head.query
            .from_sequence
            .is_some_and(|from| from > SEARCH_HEAD),
        "{:?}",
        head.query
    );
    assert_eq!(
        EventQuery {
            from_sequence: None,
            ..head.query.clone()
        },
        EventQuery::default()
    );
    assert_eq!(head.page.limit(), 1);
    // Every other read carries the instance and the window's time bound, at the head's
    // snapshot, without run recovery.
    for read in rest {
        assert_eq!(
            read.query.instance_id,
            Some(instance_id()),
            "{:?}",
            read.query
        );
        assert_eq!(
            (
                read.query.from_timestamp_unix_ms,
                read.query.to_timestamp_unix_ms
            ),
            (Some(SINCE), None),
            "{:?}",
            read.query
        );
        assert_eq!(read.page.snapshot_position(), Some(SEARCH_HEAD));
        assert!(!read.page.derives_run_recovery(), "{:?}", read.page);
    }
    // The window's start: the instance and the time bound alone, through the head, one row.
    assert_eq!(
        rest[0].query,
        EventQuery {
            from_timestamp_unix_ms: Some(SINCE),
            to_sequence: Some(SEARCH_HEAD),
            instance_id: Some(instance_id()),
            ..EventQuery::default()
        }
    );
    assert_eq!(rest[0].page.limit(), 1);
    // The window and the rows: the errors view, between the instance's first record of the
    // window and the head.
    for read in &rest[1..] {
        assert_eq!(
            read.query.view,
            Some(LedgerView::Errors),
            "{:?}",
            read.query
        );
        let (Some(from), Some(through)) = (read.query.from_sequence, read.query.to_sequence) else {
            panic!("a read without its range: {:?}", read.query);
        };
        assert!(
            SEARCH_RECORDS[0] <= from && from <= through && through <= SEARCH_HEAD,
            "[{from}, {through}]"
        );
    }
}

#[test]
fn rpt1_fix2_the_deadline_cuts_the_errors_search_and_the_part_says_so() {
    // The time spent once the window's start was read (before the search's first window), or
    // once its window was read (before the rows): no read starts after it, and the errors view
    // is missing with the word the time gives.
    for (reads, word) in [
        (2, ReportPartReason::BudgetSpent),
        (3, ReportPartReason::NotReadToEnd),
    ] {
        let pages = Pages::new();
        let late = || (pages.count() >= reads).then_some(word);
        let miss = report::errors_view(
            &pages,
            &searched_instance(),
            day(),
            report::ERRORS_PER_INSTANCE,
            &late,
            &read_failed,
        )
        .expect_err("the search is cut");
        assert_eq!(miss.why, word);
        assert!(miss.cause.is_none(), "{miss:?}");
        assert_eq!(pages.count(), reads, "a read after the time was spent");
    }

    // Through the report: its errors views read by that search, the time spent once alpha's
    // window start was read; both instances' errors views are missing with the word, nothing
    // is read after it, and the rest of the report is written.
    let folder = TempDir::new().expect("a temporary folder");
    let made_up = fake(64, 0);
    let pages = Pages::new();
    let late = || (pages.count() >= 2).then_some(ReportPartReason::BudgetSpent);
    let source = Searched {
        fake: &made_up,
        pages: &pages,
        late: &late,
    };
    let exported = export_from_source(&source, &asked(folder.path(), true, default_bound()));
    assert_eq!(pages.count(), 2);
    for alias in ["alpha", "beta"] {
        let warning = missing_part(
            &exported,
            ReportPart::ErrorsView,
            ReportPartReason::BudgetSpent,
            "alias",
            alias,
        );
        assert_eq!(
            warning.value("file"),
            Some(&json!(format!("instances/{alias}/errors.jsonl"))),
            "{warning}"
        );
    }
    let entries = archive_entries(&exported.path);
    assert!(
        !names(&entries)
            .iter()
            .any(|name| name.ends_with("errors.jsonl")),
        "{:?}",
        names(&entries)
    );
    assert!(names(&entries).contains(&"report/runs/run-a/run.json"));
    let data = document(&entries, "report/summary.json");
    for place in 0..2 {
        assert_eq!(data["errors"][place]["read"], false, "{}", data["errors"]);
        assert_eq!(
            data["errors"][place]["why"], "budget_spent",
            "{}",
            data["errors"]
        );
    }
    let summary = text(&entries, "report/SUMMARY.md");
    assert!(
        summary.contains(
            "## Errors view / 错误视图: alpha\n\nNot read (`budget_spent`); see the warnings"
        ),
        "{summary}"
    );
}

/// The report of `source` with the made-up versions and masks.
fn export_from_source(source: &dyn Source, request: &Request) -> Exported {
    report::export(source, request, &versions(), &mask())
        .unwrap_or_else(|outcome| panic!("the report is written: {outcome}"))
}
