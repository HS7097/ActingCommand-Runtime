// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #464 P2 (`contracts/resource-verify.md`, "`standard-pack build`", "The `package`
//! revision"): the `package` revision a standard-pack bundle declares, computed from its content.
//! `contracts/component-interfaces.md` says it: "A bundle writes `package`: its `max` is the
//! revision its packs need. Give `min` the same value." [`content_revision`] is the highest
//! revision any entry of the zip needs, and 3, the bundle formats of Runtime v0.12.0, when none
//! needs a later one.
//!
//! Fix round 1 (the independent review): the table holds every member, file and value that a row
//! of the `package` table of `component-interfaces.md` names, and every one the register sections
//! it cites, and the B train's sections, put into a `package` revision, each where it stands and
//! under the condition it needs it:
//!
//! - **The root**: the notices directory, 10 (Workflow #444); `goals.json`, 12 (Workflow #455
//!   G-R1); `interrupts.json`'s `limit_cause` and `result`, 6 (Workflow #411 B43, UPD-1a). The
//!   game root's tables need none (row 6: no member of a pack reads them).
//! - **A pack's loading copy** (`resources/loading.json`, `resources/loading/`), 6 (Workflow #407
//!   LOAD-1).
//! - **A member of a pack's file** ([`ROWS`]), read as JSON: of `control.json`, of a task file
//!   (`resources/operations/<task>/task.json`), of a selection-policy document
//!   (`resources/operations/<task>/policies/<name>.json`) or of the navigation projection
//!   (`resources/navigation/`), by its name, the member that holds it and, where the row says,
//!   the value it holds, the task schema of its file or the pack's execution mode; and a
//!   `repeated_anchor` layout whose anchor is a template target of its pack that declares a mask
//!   or scales, an `anchors[]` or `verify_templates[]` entry of any task file of the pack, as the
//!   parser resolves the anchor (fix round 2: [`masked_templates`], [`layout_anchor_masked`]).
//!
//! Any other file needs nothing here, and so does a JSON file that does not read (the format step
//! refuses it). A feature that adds a `package` revision adds its row here, in its own slice.

use super::bundle::PACKAGE_REVISION;
use actingcommand_contract::{
    GOALS_PACKAGE_REVISION, NOTICES_PACKAGE_REVISION, STANDARD_PACK_GOALS_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, TaskSchema, notice_relative_path,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The directory of a bundle's packs, `packs/<package_id>/`.
const PACKS_PREFIX: &str = "packs/";
/// A pack's control file.
const CONTROL_FILE: &str = "control.json";
/// Workflow #407 LOAD-1: a pack's loading copy, its file and the directory of its templates.
const LOADING_COPY_FILE: &str = "resources/loading.json";
const LOADING_COPY_PREFIX: &str = "resources/loading/";
/// A pack's task directories, `resources/operations/<task>/`, each with its `task.json` and its
/// task-local selection-policy documents `policies/<name>.json`.
const OPERATIONS_PREFIX: &str = "resources/operations/";
const TASK_FILE: &str = "task.json";
const POLICIES_PREFIX: &str = "policies/";
/// A pack's navigation projection, `resources/navigation/<game>.<server>.projection.json`.
const NAVIGATION_PREFIX: &str = "resources/navigation/";
const JSON_SUFFIX: &str = ".json";
/// The member of a task file that names its task schema.
const SCHEMA_VERSION: &str = "schema_version";
/// The member of a value that names its type.
const TYPE_MEMBER: &str = "type";
/// The member of `control.json` that names the pack's execution mode, and the mode of a
/// `linear_steps` pack.
const EXECUTION_MODE: &str = "execution_mode";
const LINEAR_STEPS: &str = "linear_steps";
/// Revision 6: a pack's loading copy (Workflow #407 LOAD-1).
const LOADING_COPY_REVISION: u64 = 6;
/// Revision 7: a `repeated_anchor` layout whose anchor declares a mask or scales (Workflow #404
/// FS-1).
const LAYOUT_ANCHOR_REVISION: u64 = 7;
/// The members of an `anchors[]` or `verify_templates[]` entry that declare a mask (`maskRange` is
/// the template's other spelling of `mask`) or scales.
const ANCHOR_MASK_MEMBERS: [&str; 3] = ["mask", "maskRange", "scales"];
/// Fix round 2: the lists of a task file whose entries derive the pack's template targets, each
/// with the prefix of the target id an entry derives from its `id`: an `anchors[]` entry derives
/// `page/<id>` (pack-containment's `anchor_target_id`), a `verify_templates[]` entry `<id>`.
const TEMPLATE_LISTS: [(&str, &str); 2] = [("anchors", "page/"), ("verify_templates", "")];
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// The file a row's member stands in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum File {
    /// A pack's `control.json`.
    Control,
    /// A pack's task file, `resources/operations/<task>/task.json`.
    Task,
    /// A selection-policy document, `resources/operations/<task>/policies/<name>.json`.
    Policy,
    /// A pack's navigation projection, below `resources/navigation/`.
    Navigation,
    /// The game root's `interrupts.json`.
    Interrupts,
}

const CONTROL: &[File] = &[File::Control];
const TASK: &[File] = &[File::Task];
const POLICY: &[File] = &[File::Policy];
const INTERRUPTS: &[File] = &[File::Interrupts];
/// A click: a task operation's or a page operation's, or a navigation edge's.
const CLICKS: &[File] = &[File::Task, File::Navigation];
/// A stage of a phased route: a task's `phases[]`, or a `Lab-1y.control.v2` control's.
const PHASES: &[File] = &[File::Task, File::Control];

/// Where a row's member stands in its file.
#[derive(Clone, Copy)]
enum Place {
    /// A member of the document itself.
    Top,
    /// A member of an object that the member of this name holds, itself or as an element of its
    /// array.
    Under(&'static str),
    /// As [`Place::Under`], in an object whose member `type` holds `of_type`.
    UnderTyped {
        holder: &'static str,
        of_type: &'static str,
    },
    /// Wherever it stands in its file.
    Anywhere,
}

/// What a row's member must hold.
#[derive(Clone, Copy)]
enum Holds {
    Anything,
    /// This text.
    Text(&'static str),
    /// An object.
    Object,
}

/// When a row's member needs its revision.
#[derive(Clone, Copy)]
enum Condition {
    Always,
    /// Only in a task file of this task schema.
    Schema(TaskSchema),
    /// Only in a pack whose `control.json` names the execution mode `linear_steps`.
    LinearSteps,
}

/// One member of a file of the bundle that needs a `package` revision above 3.
#[derive(Clone, Copy)]
struct Row {
    files: &'static [File],
    name: &'static str,
    place: Place,
    holds: Holds,
    condition: Condition,
    revision: u64,
}

const fn row(files: &'static [File], name: &'static str, place: Place, revision: u64) -> Row {
    Row {
        files,
        name,
        place,
        holds: Holds::Anything,
        condition: Condition::Always,
        revision,
    }
}

const fn holding(
    files: &'static [File],
    name: &'static str,
    place: Place,
    holds: Holds,
    revision: u64,
) -> Row {
    Row {
        files,
        name,
        place,
        holds,
        condition: Condition::Always,
        revision,
    }
}

const fn when(
    files: &'static [File],
    name: &'static str,
    place: Place,
    condition: Condition,
    revision: u64,
) -> Row {
    Row {
        files,
        name,
        place,
        holds: Holds::Anything,
        condition,
        revision,
    }
}

/// The members of the bundle's files that the `package` rows 4 to 11 and the register sections
/// they cite, and the B train's sections, put into a revision: each with its file, its place, what
/// it holds, its condition and its revision (`contracts/resource-verify.md`, "The `package`
/// revision", lists them with their sources).
const ROWS: [Row; 43] = [
    // 4, Workflow #411 B15 (register §1.110 B15-10, §1.111): a `scheduling_outcome` mapping's
    // `remedy`, `control.json`'s `provides`.
    row(TASK, "remedy", Place::Under("mappings"), 4),
    row(CONTROL, "provides", Place::Top, 4),
    // 5, Workflow #406 T3 (§1.137): the click `single_touch_path_drag_v1`.
    holding(
        CLICKS,
        "kind",
        Place::Anywhere,
        Holds::Text("single_touch_path_drag_v1"),
        5,
    ),
    // 6, Workflow #357 DB-2 (§1.65 TB6, §1.244): a selection document's `tables` and the value
    // source `table`.
    row(POLICY, "tables", Place::Top, 6),
    holding(POLICY, "source", Place::Anywhere, Holds::Text("table"), 6),
    // 6, Workflow #428 OCR-SCALE-1 (§1.251 OS-1): `post_admission_ocr` of a task schema `0.9`
    // task; a `0.7` and a `0.8` task must write it and keep their revision.
    when(
        TASK,
        "post_admission_ocr",
        Place::Top,
        Condition::Schema(TaskSchema::V0_9),
        6,
    ),
    // 6, ACT-4 (§1.254 AC4-1): a step's `expect_after.target_present` and `target_gone`.
    row(TASK, "target_present", Place::Under("expect_after"), 6),
    row(TASK, "target_gone", Place::Under("expect_after"), 6),
    // 6, Workflow #407 LOAD-2 (§1.258 LD2-1): a step's `expect_after.loading`.
    row(TASK, "loading", Place::Under("expect_after"), 6),
    // 6, Workflow #411 A35-S3 (§1.176 S3-2) and LOAD-2 (LD2-2): a step's `expect_after.wait_page`.
    row(TASK, "wait_page", Place::Under("expect_after"), 6),
    // 6, Workflow #411 BT-B2 (§1.146): the gate predicate `integer_compare`; `ocr_integer`'s
    // `integer.part`.
    holding(
        POLICY,
        "kind",
        Place::Anywhere,
        Holds::Text("integer_compare"),
        6,
    ),
    row(TASK, "part", Place::Under("integer"), 6),
    // 6, Workflow #411 BT-B3 (§1.149): `alternatives` of a color probe and of a `color_check`.
    row(TASK, "alternatives", Place::Under("color_probes"), 6),
    row(TASK, "alternatives", Place::Under("color_check"), 6),
    // 6, Workflow #411 A35-S1 (§1.152 B37-1): a mapping's `busy`.
    row(TASK, "busy", Place::Under("mappings"), 6),
    // 6, Workflow #411 BT-B5 (§1.154 B5-1, B14-1): a stage's `end_pages` and `entry_pages`.
    row(PHASES, "end_pages", Place::Under("phases"), 6),
    row(PHASES, "entry_pages", Place::Under("phases"), 6),
    // 6, Workflow #411 BT-B12 (§1.155 B12-1, B12-4): the input recheck `still_frames`, and an
    // input recheck of any kind in a `linear_steps` pack.
    holding(
        TASK,
        "kind",
        Place::Under("input_recheck"),
        Holds::Text("still_frames"),
        6,
    ),
    when(
        TASK,
        "input_recheck",
        Place::Under("operations"),
        Condition::LinearSteps,
        6,
    ),
    // 6, Workflow #411 BT-B1 (§1.163 B1-1, B1-2): an `unsigned_integer` value's `dictionary`,
    // and the `agreement` of a value-test side and of a reading mark.
    row(
        TASK,
        "dictionary",
        Place::UnderTyped {
            holder: "value",
            of_type: "unsigned_integer",
        },
        6,
    ),
    row(TASK, "agreement", Place::Under("left"), 6),
    row(TASK, "agreement", Place::Under("right"), 6),
    row(TASK, "agreement", Place::Under("resource_readings"), 6),
    // 6, Workflow #411 BT-KEY (§1.164 KEY-1, KEY-2): an operation's `max_runs` and `on_max_runs`,
    // and its keyed `on_guard_refused` (an object; the text `next_candidate` is older).
    row(TASK, "max_runs", Place::Under("operations"), 6),
    row(TASK, "on_max_runs", Place::Under("operations"), 6),
    holding(
        TASK,
        "on_guard_refused",
        Place::Under("operations"),
        Holds::Object,
        6,
    ),
    // 6, Workflow #411 BT-B13 (§1.165): a template target's `luma_floor`.
    row(TASK, "luma_floor", Place::Under("anchors"), 6),
    row(TASK, "luma_floor", Place::Under("verify_templates"), 6),
    // 6, Workflow #411 BT-B4 (§1.166 B4-1): a select step's `confirm`, whatever it holds.
    row(TASK, "confirm", Place::Under("select"), 6),
    // 6, Workflow #411 A35-S3 (§1.176 S3-1): an operation's `wait`.
    row(TASK, "wait", Place::Under("operations"), 6),
    // 6, SCH-M CF-2 (§1.181 CF2-1): a reading mark's value of type `timestamp_ms`.
    holding(
        TASK,
        "type",
        Place::Under("value"),
        Holds::Text("timestamp_ms"),
        6,
    ),
    // 6, Workflow #411 B43 (§1.203 B43-1): an `interrupts.json` entry's `limit_cause`.
    row(INTERRUPTS, "limit_cause", Place::Under("interrupts"), 6),
    // 6, STORE-FG-1 (§1.207 SF-1): `control.json`'s `store_update`.
    row(CONTROL, "store_update", Place::Top, 6),
    // 6, CFL-2 (§1.208 CFL2-1): `preprocess` of an OCR target and of a shared read.
    row(TASK, "preprocess", Place::Under("ocr_targets"), 6),
    row(TASK, "preprocess", Place::Under("shared_ocr"), 6),
    // 6, UPD-1a (§1.215 UPD-1): an `interrupts.json` entry's `result`.
    row(INTERRUPTS, "result", Place::Under("interrupts"), 6),
    // 7, Workflow #404 FS-2 (§1.283 FS2-1): the feature value `anchor_milli`; FS-3 (§1.284
    // FS3-1): a select step's `confirm: "track"` and its `track_px` (and FS-1, §1.282 FS1-3:
    // [`layout_anchor_masked`]).
    holding(
        TASK,
        "value",
        Place::Under("features"),
        Holds::Text("anchor_milli"),
        7,
    ),
    holding(
        TASK,
        "confirm",
        Place::Under("select"),
        Holds::Text("track"),
        7,
    ),
    row(TASK, "track_px", Place::Under("select"), 7),
    // 8, Workflow #428 S168-4 and S168-5a (§1.291 S168C-1, S168C-5): an operation's `read_list`
    // (the spelling of the kernel's location `read_list`, whose constant this program cannot
    // reach), the value source `table_max_over_list`.
    row(TASK, "read_list", Place::Under("operations"), 8),
    holding(
        POLICY,
        "source",
        Place::Anywhere,
        Holds::Text("table_max_over_list"),
        8,
    ),
    // 9, Workflow #428 S168-LB1b (§1.309 LB1-7, §1.312): `table_max_over_list`'s
    // `lower_bound_column`.
    row(POLICY, "lower_bound_column", Place::Anywhere, 9),
    // 11, Workflow #450 FL-1 (§1.343): a task's `field_lists`.
    row(TASK, "field_lists", Place::Top, 11),
];

/// What the rows read of the document they walk.
struct Context<'a> {
    file: File,
    /// The document's `schema_version`, a task file's task schema.
    schema: Option<&'a str>,
    /// The document's pack is a `linear_steps` pack.
    linear: bool,
}

impl Row {
    /// Whether the member `name` holding `member`, in the object `object` that the member
    /// `holder` holds, needs this row's revision.
    fn needs(
        &self,
        context: &Context<'_>,
        object: &Map<String, Value>,
        name: &str,
        member: &Value,
        holder: Option<&str>,
    ) -> bool {
        self.name == name
            && self.files.contains(&context.file)
            && match self.place {
                Place::Top => holder.is_none(),
                Place::Under(under) => holder == Some(under),
                Place::UnderTyped {
                    holder: under,
                    of_type,
                } => {
                    holder == Some(under)
                        && object.get(TYPE_MEMBER).and_then(Value::as_str) == Some(of_type)
                }
                Place::Anywhere => true,
            }
            && match self.holds {
                Holds::Anything => true,
                Holds::Text(text) => member.as_str() == Some(text),
                Holds::Object => member.is_object(),
            }
            && match self.condition {
                Condition::Always => true,
                Condition::Schema(schema) => TaskSchema::of(context.schema) == Some(schema),
                Condition::LinearSteps => context.linear,
            }
    }
}

/// The `package` revision the zip's `entries` (its `interfaces.json` aside) need: the highest any
/// entry needs, [`PACKAGE_REVISION`] when none needs a later one.
pub(crate) fn content_revision(entries: &BTreeMap<String, Vec<u8>>) -> u64 {
    let linear = linear_packs(entries);
    let masked = masked_templates(entries);
    entries
        .iter()
        .map(|(name, bytes)| entry_revision(name, bytes, &linear, &masked))
        .fold(PACKAGE_REVISION, u64::max)
}

/// Fix round 2: for each pack of `entries`, the ids of its template targets that declare a mask
/// or scales: every `anchors[]` and `verify_templates[]` entry of every task file of the pack
/// that holds a member of [`ANCHOR_MASK_MEMBERS`], by the target id it derives
/// ([`TEMPLATE_LISTS`]). The parser resolves a `repeated_anchor` layout's anchor among every
/// template target of the derived pack (pack-containment's `derive_candidate_layouts`), not among
/// its own task's anchors only.
fn masked_templates(entries: &BTreeMap<String, Vec<u8>>) -> BTreeMap<&str, BTreeSet<String>> {
    let mut masked = BTreeMap::<&str, BTreeSet<String>>::new();
    for (name, bytes) in entries {
        let Some((pack, within)) = pack_entry(name) else {
            continue;
        };
        if pack_file(within) != Some(File::Task) {
            continue;
        }
        let Some(task) = read_json(bytes) else {
            continue;
        };
        for (list, prefix) in TEMPLATE_LISTS {
            let targets = task.get(list).and_then(Value::as_array);
            for target in targets.into_iter().flatten() {
                let Some(id) = target.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if ANCHOR_MASK_MEMBERS
                    .iter()
                    .any(|member| target.get(*member).is_some())
                {
                    masked
                        .entry(pack)
                        .or_default()
                        .insert(format!("{prefix}{id}"));
                }
            }
        }
    }
    masked
}

/// The packs of `entries` whose `control.json` names the execution mode `linear_steps`.
fn linear_packs(entries: &BTreeMap<String, Vec<u8>>) -> BTreeSet<&str> {
    entries
        .iter()
        .filter_map(|(name, bytes)| {
            let (pack, _) = pack_entry(name).filter(|(_, within)| *within == CONTROL_FILE)?;
            let control = read_json(bytes)?;
            (control.get(EXECUTION_MODE).and_then(Value::as_str) == Some(LINEAR_STEPS))
                .then_some(pack)
        })
        .collect()
}

/// The revision the zip entry `name` with the bytes `bytes` needs; `linear` names the
/// `linear_steps` packs, `masked` each pack's template targets that declare a mask or scales.
fn entry_revision(
    name: &str,
    bytes: &[u8],
    linear: &BTreeSet<&str>,
    masked: &BTreeMap<&str, BTreeSet<String>>,
) -> u64 {
    if notice_relative_path(name).is_some() {
        return NOTICES_PACKAGE_REVISION;
    }
    if name == STANDARD_PACK_GOALS_FILE {
        return GOALS_PACKAGE_REVISION;
    }
    if name == STANDARD_PACK_INTERRUPTS_FILE {
        return document_revision(bytes, File::Interrupts, false, None);
    }
    let Some((pack, within)) = pack_entry(name) else {
        return PACKAGE_REVISION;
    };
    if within == LOADING_COPY_FILE || within.starts_with(LOADING_COPY_PREFIX) {
        return LOADING_COPY_REVISION;
    }
    match pack_file(within) {
        Some(file) => document_revision(bytes, file, linear.contains(pack), masked.get(pack)),
        None => PACKAGE_REVISION,
    }
}

/// A pack's file `packs/<package_id>/<path within the pack>`: the package id and the path.
fn pack_entry(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix(PACKS_PREFIX)?.split_once('/')
}

/// Which file of a pack the path `within` the pack is; `None` for a file no row reads.
fn pack_file(within: &str) -> Option<File> {
    if within == CONTROL_FILE {
        return Some(File::Control);
    }
    if let Some(projection) = within.strip_prefix(NAVIGATION_PREFIX) {
        return projection
            .ends_with(JSON_SUFFIX)
            .then_some(File::Navigation);
    }
    let (_, in_task) = within.strip_prefix(OPERATIONS_PREFIX)?.split_once('/')?;
    if in_task == TASK_FILE {
        return Some(File::Task);
    }
    in_task
        .strip_prefix(POLICIES_PREFIX)?
        .ends_with(JSON_SUFFIX)
        .then_some(File::Policy)
}

/// The JSON document of `bytes` (a byte order mark is accepted); `None` when it does not read.
fn read_json(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(bytes)).ok()
}

/// The revision the document `bytes`, the file `file` of a pack that is `linear_steps` or not and
/// whose template targets `masked` declare a mask or scales, needs.
fn document_revision(
    bytes: &[u8],
    file: File,
    linear: bool,
    masked: Option<&BTreeSet<String>>,
) -> u64 {
    let Some(document) = read_json(bytes) else {
        return PACKAGE_REVISION;
    };
    let context = Context {
        file,
        schema: document.get(SCHEMA_VERSION).and_then(Value::as_str),
        linear,
    };
    let mut revision = PACKAGE_REVISION;
    walk(&context, &document, None, &mut revision);
    if file == File::Task
        && masked.is_some_and(|templates| layout_anchor_masked(&document, templates))
    {
        revision = revision.max(LAYOUT_ANCHOR_REVISION);
    }
    revision
}

/// Raises `revision` to what `value`, held by the member `holder`, and everything below it need.
fn walk(context: &Context<'_>, value: &Value, holder: Option<&str>, revision: &mut u64) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                for row in &ROWS {
                    if row.needs(context, members, name, member, holder) {
                        *revision = (*revision).max(row.revision);
                    }
                }
                walk(context, member, Some(name.as_str()), revision);
            }
        }
        Value::Array(elements) => {
            for element in elements {
                walk(context, element, holder, revision);
            }
        }
        _ => {}
    }
}

/// Revision 7 (Workflow #404 FS-1, register §1.282 FS1-3): the task file `task` declares a
/// `repeated_anchor` layout whose anchor names one of `masked`, the template targets of its pack
/// that declare a mask or scales ([`masked_templates`]), wherever in the pack they stand.
fn layout_anchor_masked(task: &Value, masked: &BTreeSet<String>) -> bool {
    task.get("candidate_layouts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|layout| layout.get("kind").and_then(Value::as_str) == Some("repeated_anchor"))
        .filter_map(|layout| layout.get("anchor").and_then(Value::as_str))
        .any(|anchor| masked.contains(anchor))
}
