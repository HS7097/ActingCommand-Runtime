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
//! - **The root**: the notices directory, 10 (Workflow #444); `goals.json`, 14 (Workflow #455
//!   G-R1 brought it at 12; Workflow #463 V1 fix round 3, the row 14, brought version 2, the one
//!   version the Tools read; 28 since Workflow #455 RR1, the row 28, which brought version 3):
//!   by its presence, as the installer's `place` reads it, whatever version it holds (a version 1
//!   or 2 file is the check's `goal_version_retired`, not the table's); `goal-tables.json`, 28
//!   (Workflow #455 RR1), by its presence the same way, and 32 (Workflow #455 RR4a, register
//!   §1.546) when its document writes the member `names`, read from its bytes by the contract
//!   crate's `goal_tables_package_revision`, as `pack-catalog.json`'s version is;
//!   `interrupts.json`'s `limit_cause` and `result`, 6 (Workflow #411 B43, UPD-1a), and an entry
//!   without handlers or with the `result` `environment_reset`, 19 (Workflow #463 INT-1);
//!   `pack-catalog.json`, 16 (Workflow #477 PC-1 fix round 1), by its presence, and 24 when it is
//!   of version 2 (Workflow #477 CV-1, register §1.507: the contract crate's
//!   `catalog_package_revision`). The game root's tables need none (row 6: no member of a pack
//!   reads them).
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
//!   Revisions 13 and 14 (Workflow #463 K1 and V1, after the train 0.12.61a): an operation's
//!   `select` in a `linear_steps` pack, 13, read where the kernel's admission reads it; a pack's
//!   `control.json` `variables`, 14, read as `place` reads it. Revision 17 (Workflow #463 KP-1
//!   fix round 1): an operation's `key` in a `linear_steps` pack, a key step, read where the
//!   kernel's admission reads it.
//! - **A `linear_steps` package's ending page**, 15 (Workflow #463 K2, after the train 0.12.61c;
//!   Workflow #464 P2b): a terminal page of the `scheduling_outcome` of the pack's entry task that
//!   is none of its target pages and none of its `on_empty` pages, read from the pack's
//!   `control.json` and its entry task file together, as the kernel's `declares_linear_endings`
//!   reads the admitted package ([`declares_linear_ending`]).
//! - **A select step's scan without an end**, 18 (Workflow #463 K-c, register §1.454; S2-9b,
//!   §1.458): a member left out, which no row of [`ROWS`] can find, so read on its own
//!   ([`scan_without_end`]).
//! - **A `repeated_anchor` layout's shared read**, 20 (Workflow #463 SEL-2L, register §1.480): a
//!   task file's candidate layout of the kind `repeated_anchor` that declares `shared_ocr`, a
//!   member a row finds only beside its layout's `kind`, so read on its own
//!   ([`anchored_shared_read`]).
//! - **The lines of a shared read's name boxes**, 21 (Workflow #463 SEL-GRID G2, register §1.491):
//!   a task file's `shared_ocr` that declares `lines`, of a layout of either kind, a row of
//!   [`ROWS`].
//! - **A `linear_steps` task's list reading**, 22 (Workflow #428 S168-LK, register §1.500): an
//!   operation's `read_list` in a `linear_steps` pack, read where the kernel's admission reads it,
//!   as revisions 13 and 17 read a select step and a key step; a page-graph list reading keeps
//!   revision 8.
//! - **Variables beside an irreversible capability**, 23 (Workflow #455 G-SLx, register §1.501):
//!   a pack's `control.json` that declares `variables` beside a `provides` entry marking its
//!   capability irreversible, two members of one file, so read on its own
//!   ([`variables_beside_irreversible`]).
//! - **A `linear_steps` press step's count**, 25 (Workflow #477 LR-1, register §1.513): an
//!   operation's `presses` in a `linear_steps` pack, read where the kernel's admission reads it,
//!   as revisions 13, 17 and 22 read a select step, a key step and a list reading, a row of
//!   [`ROWS`].
//! - **A variable a rule gives**, 26 (Workflow #455 TK-1, register §1.532): the member `given_by`
//!   of an entry of a pack's `control.json` `variables`, whatever it holds, a row of [`ROWS`].
//! - **A list reading of the wanted keys only**, 27 (Workflow #503 LIST-A, register §1.533): the
//!   member `others` of an operation's `read_list`, in a pack of either mode, a row of [`ROWS`].
//! - **A `repeated_anchor` layout's picture pool**, 29 (Workflow #503 LIST-B, register §1.536):
//!   the member `identity_templates` of a candidate layout and the member `region` of a layout's
//!   feature, whatever they hold, in a pack of either mode, two rows of [`ROWS`].
//! - **A scan's end that must also be stationary**, 30 (Workflow #503 LIST-C, register §1.541):
//!   the member `require_stationary` of a scan's `end`, of a select step or of a list reading,
//!   whatever it holds, in a pack of either mode, a row of [`ROWS`].
//! - **A layout's own budget of evaluations**, 31 (Workflow #503 LIST-D, register §1.544): the
//!   member `provider_evaluations` of a candidate layout of either kind, whatever it holds, in a
//!   pack of either mode, a row of [`ROWS`].
//!
//! Any other file needs nothing here, and so does a JSON file that does not read (the format step
//! refuses it). A feature that adds a `package` revision adds its row here, in its own slice.

use super::bundle::PACKAGE_REVISION;
use actingcommand_contract::{
    GIVEN_BY_MEMBER, GIVEN_BY_PACKAGE_REVISION, GOALS_PACKAGE_REVISION, NOTICES_PACKAGE_REVISION,
    STANDARD_PACK_CATALOG_FILE, STANDARD_PACK_GOAL_TABLES_FILE, STANDARD_PACK_GOALS_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, TaskSchema, VARIABLES_PACKAGE_REVISION,
    catalog_package_revision, goal_tables_package_revision, notice_relative_path,
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
/// Revision 15: a `linear_steps` package's ending page (Workflow #463 K2).
const LINEAR_ENDINGS_REVISION: u64 = 15;
/// The members revision 15 reads: of `control.json`, the game and the entry task; of the entry
/// task file, its target page, its operations with the `from` page and the `select` with its
/// `on_empty`, and its scheduling outcome's mappings with their terminal pages.
const GAME: &str = "game";
const ENTRY_TASK_ID: &str = "entry_task_id";
const TARGET_PAGE: &str = "target_page";
const OPERATIONS: &str = "operations";
const FROM: &str = "from";
const SELECT: &str = "select";
const ON_EMPTY: &str = "on_empty";
const SCHEDULING_OUTCOME: &str = "scheduling_outcome";
const MAPPINGS: &str = "mappings";
const TERMINAL_PAGES: &str = "terminal_pages";
/// Revision 18: a select step's scan that declares no end (Workflow #463 K-c, S2-9b), and the
/// members it reads: an operation's `select`, its `scan`, the scan's `end`.
const SCAN_WITHOUT_END_REVISION: u64 = 18;
const SCAN: &str = "scan";
const END: &str = "end";
/// Revision 20: a `repeated_anchor` layout's shared read (Workflow #463 SEL-2L), and the members
/// it reads: a task file's `candidate_layouts`, a layout's `kind` with the value
/// `repeated_anchor`, and its `shared_ocr`; revision 7's masked anchor ([`layout_anchor_masked`])
/// reads `candidate_layouts`, `kind` and the value `repeated_anchor` by the same constants, and
/// not `shared_ocr`.
const ANCHORED_SHARED_READ_REVISION: u64 = 20;
/// Revision 21: the lines of a shared read's name boxes (Workflow #463 SEL-GRID G2), the member
/// `lines` of a `shared_ocr` (a row of [`ROWS`]).
const SHARED_LINES_REVISION: u64 = 21;
/// Revision 23: a pack's variables beside a capability its own `provides` marks irreversible
/// (Workflow #455 G-SLx), and the members it reads: `control.json`'s `variables` and `provides`,
/// and a `provides` entry's `irreversible`.
const IRREVERSIBLE_VARIABLES_REVISION: u64 = 23;
/// Revision 25: a `linear_steps` press step's count (Workflow #477 LR-1), the member `presses` of
/// an operation (a row of [`ROWS`]).
const PRESSES_REVISION: u64 = 25;
/// Revision 27: a list reading that wants only its declared keys (Workflow #503 LIST-A), the
/// member `others` of an operation's `read_list` (a row of [`ROWS`]).
const LIST_OTHERS_REVISION: u64 = 27;
/// Revision 29: a `repeated_anchor` layout's picture pool (Workflow #503 LIST-B), the members
/// `identity_templates` of a candidate layout and `region` of a layout's feature (two rows of
/// [`ROWS`]).
const LAYOUT_POOL_REVISION: u64 = 29;
/// Revision 30: a scan's end that must also be stationary (Workflow #503 LIST-C), the member
/// `require_stationary` of a scan's `end` (a row of [`ROWS`]).
const STATIONARY_END_REVISION: u64 = 30;
/// Revision 31: a layout's own budget of OCR and NN evaluations (Workflow #503 LIST-D), the member
/// `provider_evaluations` of a candidate layout (a row of [`ROWS`]).
const LAYOUT_EVALUATIONS_REVISION: u64 = 31;
const VARIABLES: &str = "variables";
const PROVIDES: &str = "provides";
const IRREVERSIBLE: &str = "irreversible";
const CANDIDATE_LAYOUTS: &str = "candidate_layouts";
const KIND: &str = "kind";
const REPEATED_ANCHOR: &str = "repeated_anchor";
const SHARED_OCR: &str = "shared_ocr";

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
    /// Workflow #463 INT-1: an array with no element.
    EmptyArray,
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

/// The members of the bundle's files that the `package` rows 4 to 11, 13, 14, 17, (Workflow #463
/// INT-1) 19, (Workflow #463 SEL-GRID G2) 21, (Workflow #428 S168-LK) 22, (Workflow #477 LR-1) 25,
/// (Workflow #455 TK-1) 26, (Workflow #503 LIST-A) 27, (Workflow #503 LIST-B) 29, (Workflow #503
/// LIST-C) 30 and (Workflow #503 LIST-D) 31 and the register sections they cite, and the B train's
/// sections, put into a revision: each with its file, its place, what it holds, its condition and
/// its revision (`contracts/resource-verify.md`, "The `package` revision", lists them with their
/// sources).
const ROWS: [Row; 57] = [
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
    // 13, Workflow #463 K1 (§1.374; contracts/linear-steps.md, "Select steps"): a select step of a
    // `linear_steps` task, an operation whose effect is `select`, found where the kernel's
    // admission finds it: in a pack whose control's `execution_mode` is `linear_steps`, the member
    // `select` of an element of the task's `operations`, holding the step's declaration, an object
    // (`TaskOperation.select`, an `Option` that an absent member or `null` leaves empty;
    // `TaskProgram::validate_linear` takes `click`, `application` and `select` of
    // `(None, None, Some(_))` as `LinearEffect::Select`). That reader is private to the kernel
    // crate, which this program does not depend on, so the row reads the same member.
    Row {
        files: TASK,
        name: "select",
        place: Place::Under("operations"),
        holds: Holds::Object,
        condition: Condition::LinearSteps,
        revision: 13,
    },
    // 14, Workflow #463 V1 (§1.378; contracts/package-variables.md): a pack's `control.json`
    // declares `variables`, the top-level member present, as the installer's `place` reads it
    // (`check_variables_declared`: `variable_undeclared` below 14); what they declare is the
    // check's.
    row(CONTROL, "variables", Place::Top, VARIABLES_PACKAGE_REVISION),
    // 17, Workflow #463 KP-1 (§1.448, fix round 1; contracts/linear-steps.md, "Key steps"): a key
    // step of a `linear_steps` task, an operation whose effect is `key`, found where the kernel's
    // admission finds it, as row 13 finds a select step: in a pack whose control's
    // `execution_mode` is `linear_steps`, the member `key` of an element of the task's
    // `operations`, holding the step's key, an object (`TaskOperation.key`, an `Option` that an
    // absent member or `null` leaves empty; `TaskProgram::validate_linear` takes `click`,
    // `application`, `select` and `key` of `(None, None, None, Some(_))` as `LinearEffect::Key`).
    // A key in a page-graph task is refused by every build, so it needs nothing here.
    Row {
        files: TASK,
        name: "key",
        place: Place::Under("operations"),
        holds: Holds::Object,
        condition: Condition::LinearSteps,
        revision: 17,
    },
    // 19, Workflow #463 INT-1 (§1.473; contracts/package-reference.md, "Standard package root"):
    // an `interrupts.json` entry that names no handler (`handlers` an empty array, which every
    // Runtime before it refuses, `packages_count`) and the word `environment_reset` of an entry's
    // `result` (which every Runtime before it refuses as `malformed`): either makes the whole game
    // root `standard_pack_root_invalid` there.
    holding(
        INTERRUPTS,
        "handlers",
        Place::Under("interrupts"),
        Holds::EmptyArray,
        19,
    ),
    holding(
        INTERRUPTS,
        "result",
        Place::Under("interrupts"),
        Holds::Text("environment_reset"),
        19,
    ),
    // 21, Workflow #463 SEL-GRID G2 (§1.491; contracts/selection-graph.md, "The lines of a name
    // box"): a layout's shared read that declares `lines`, of either kind, which every Runtime
    // before it refuses at its declaration gate (`UnknownField` at
    // `/candidate_layouts/<i>/shared_ocr/lines`), in a pack of any execution mode.
    row(
        TASK,
        "lines",
        Place::Under(SHARED_OCR),
        SHARED_LINES_REVISION,
    ),
    // 22, Workflow #428 S168-LK (§1.500; contracts/linear-steps.md, "List reading steps"): a list
    // reading of a `linear_steps` task, an operation whose effect is `read_list`, found where the
    // kernel's admission finds it, as rows 13 and 17 find a select step and a key step: in a pack
    // whose control's `execution_mode` is `linear_steps`, the member `read_list` of an element of
    // the task's `operations`, holding the reading's declaration, an object (`TaskOperation.
    // read_list`, an `Option` that an absent member or `null` leaves empty;
    // `TaskProgram::validate_linear` takes it as `LinearEffect::ReadList`). Every Runtime before
    // it refuses such a step at its admission (`contained_task_linear_invalid`, `reason=read_list`);
    // a page-graph list reading keeps row 8's revision.
    Row {
        files: TASK,
        name: "read_list",
        place: Place::Under("operations"),
        holds: Holds::Object,
        condition: Condition::LinearSteps,
        revision: 22,
    },
    // 25, Workflow #477 LR-1 (§1.513; contracts/linear-steps.md, "Pressing on one page"): a press
    // step's count, found where the kernel's admission finds it, as rows 13, 17 and 22 find a select
    // step, a key step and a list reading: in a pack whose control's `execution_mode` is
    // `linear_steps`, the member `presses` of an element of the task's `operations`
    // (`TaskOperation.presses`), whatever it holds, a whole number, `{fact_key, minus?}` or `null`:
    // every Runtime before it refuses the member's name at its declaration gate (`UnknownField` at
    // `/operations/<i>/presses`), whatever it holds, in every pack. A page-graph task's `presses`
    // is refused by this build too (`presses requires linear_steps`), so it needs nothing here.
    when(
        TASK,
        "presses",
        Place::Under("operations"),
        Condition::LinearSteps,
        PRESSES_REVISION,
    ),
    // 26, Workflow #455 TK-1 (§1.532; contracts/package-variables.md, "A variable a rule gives"):
    // a variable of a pack's `control.json` that says who gives its value (`given_by`), the member
    // of an entry of `variables`, whatever it holds: every Runtime before it refuses the member at
    // the declaration (`variables_invalid` at `/variables/<i>/given_by`), in every pack.
    row(
        CONTROL,
        GIVEN_BY_MEMBER,
        Place::Under(VARIABLES),
        GIVEN_BY_PACKAGE_REVISION,
    ),
    // 27, Workflow #503 LIST-A (§1.533; contracts/resource-readings.md, "List readings", "A list
    // of the wanted keys only"): a list reading's `others`, the member of the object an
    // operation's `read_list` holds, whatever it holds and in a pack of either mode: every Runtime
    // before it refuses the member's name at its declaration gate (`UnknownField` at
    // `/operations/<i>/read_list/others`), whose `read_list` object denies unknown members.
    row(
        TASK,
        "others",
        Place::Under("read_list"),
        LIST_OTHERS_REVISION,
    ),
    // 29, Workflow #503 LIST-B (§1.536; contracts/selection-graph.md, "Rules";
    // contracts/business-identity-consensus.md, "A layout's pool"): a candidate layout's
    // `identity_templates` (a slot's stands under `slots` and keeps its revision) and a layout
    // feature's `region`, whatever they hold and in a pack of either mode: every Runtime before it
    // refuses either member's name at its declaration gate (`UnknownField` at
    // `/candidate_layouts/<i>/identity_templates` and `/candidate_layouts/<i>/features/<j>/region`).
    row(
        TASK,
        "identity_templates",
        Place::Under(CANDIDATE_LAYOUTS),
        LAYOUT_POOL_REVISION,
    ),
    row(
        TASK,
        "region",
        Place::Under("features"),
        LAYOUT_POOL_REVISION,
    ),
    // 30, Workflow #503 LIST-C (§1.541; contracts/selection-graph.md, "An end that must also be
    // stationary"): a scan's `end` that writes `require_stationary`, the scan of a select step or
    // of a list reading, whatever it holds and in a pack of either mode: every Runtime before it
    // refuses the member at its admission, whose reader of `end` denies unknown members.
    row(
        TASK,
        "require_stationary",
        Place::Under(END),
        STATIONARY_END_REVISION,
    ),
    // 31, Workflow #503 LIST-D (§1.544; contracts/candidate-projection.md, "Budgets"): a candidate
    // layout's `provider_evaluations`, of either kind, whatever it holds and in a pack of either
    // mode: every Runtime before it refuses the member's name where it reads a layout, the
    // recognition pack's reader and the declaration gate (`UnknownField` at
    // `/candidate_layouts/<i>/provider_evaluations`).
    row(
        TASK,
        "provider_evaluations",
        Place::Under(CANDIDATE_LAYOUTS),
        LAYOUT_EVALUATIONS_REVISION,
    ),
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
                Holds::EmptyArray => member.as_array().is_some_and(Vec::is_empty),
            }
            && match self.condition {
                Condition::Always => true,
                Condition::Schema(schema) => TaskSchema::of(context.schema) == Some(schema),
                Condition::LinearSteps => context.linear,
            }
    }
}

/// The `package` revision the zip's `entries` (its `interfaces.json` aside) need: the highest any
/// entry needs (a task file with a scan without an end, 18, [`scan_without_end`]; with a
/// `repeated_anchor` layout's shared read, 20, [`anchored_shared_read`]),
/// [`LINEAR_ENDINGS_REVISION`] when a pack declares an ending page ([`declares_linear_ending`]),
/// [`PACKAGE_REVISION`] when nothing needs a later one.
pub(crate) fn content_revision(entries: &BTreeMap<String, Vec<u8>>) -> u64 {
    let linear = linear_packs(entries);
    let masked = masked_templates(entries);
    let revision = entries
        .iter()
        .map(|(name, bytes)| entry_revision(name, bytes, &linear, &masked))
        .fold(PACKAGE_REVISION, u64::max);
    if declares_linear_ending(entries) {
        revision.max(LINEAR_ENDINGS_REVISION)
    } else {
        revision
    }
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
    // `goals.json`, 14 since Workflow #463 V1 fix round 3, 28 since Workflow #455 RR1
    // (`GOALS_PACKAGE_REVISION`): its name at the zip's root, as the installer's `place` reads it
    // (`check_goals_declared`), whatever version it holds; a version 1 or 2 file is refused by the
    // check (`goal_version_retired`). Workflow #455 RR1: `goal-tables.json` 28 the same way
    // (`GOAL_TABLES_PACKAGE_REVISION`); Workflow #455 RR4a: 32 when its document writes `names`
    // (`GOAL_TABLE_NAMES_PACKAGE_REVISION`), a member every reader before it refuses as unknown,
    // by the contract crate's one reading of the file's bytes, as `pack-catalog.json`'s version
    // below.
    if name == STANDARD_PACK_GOALS_FILE {
        return GOALS_PACKAGE_REVISION;
    }
    if name == STANDARD_PACK_GOAL_TABLES_FILE {
        return goal_tables_package_revision(bytes);
    }
    // Workflow #477 PC-1 fix round 1: `pack-catalog.json`, 16 (`CATALOG_PACKAGE_REVISION`), by its
    // name at the zip's root, so that an installer that knows no such root file refuses the zip at
    // its `package` edge. Workflow #477 CV-1: 24 (`CATALOG_V2_PACKAGE_REVISION`) for a file of
    // version 2, which an installer of an earlier build reads as `malformed`.
    if name == STANDARD_PACK_CATALOG_FILE {
        return catalog_package_revision(bytes);
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
    if file == File::Task && scan_without_end(&document) {
        revision = revision.max(SCAN_WITHOUT_END_REVISION);
    }
    if file == File::Task && anchored_shared_read(&document) {
        revision = revision.max(ANCHORED_SHARED_READ_REVISION);
    }
    if file == File::Control && variables_beside_irreversible(&document) {
        revision = revision.max(IRREVERSIBLE_VARIABLES_REVISION);
    }
    revision
}

/// Revision 23 (Workflow #455 G-SLx, register §1.501; contracts/component-interfaces.md, the
/// `package` row 23): the control declaration `control` declares `variables` (the top-level member
/// present, as row 14 reads it) and its `provides` holds an entry whose `irreversible` is `true`,
/// a package every Runtime before 23 refuses at admission (`contained_task_control_invalid`,
/// `reason=variables_on_irreversible json_pointer=/variables`: its reader refused any `variables`
/// member, whatever it held, beside such a `provides`). A pack that declares no variable, or whose
/// `provides` marks nothing irreversible, needs nothing here.
fn variables_beside_irreversible(control: &Value) -> bool {
    control.get(VARIABLES).is_some()
        && control
            .get(PROVIDES)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|provided| provided.get(IRREVERSIBLE).and_then(Value::as_bool) == Some(true))
}

/// Revision 20 (Workflow #463 SEL-2L, register §1.480; contracts/component-interfaces.md, the
/// `package` row 20): the task file `task` declares a candidate layout of the kind
/// `repeated_anchor` with a `shared_ocr`, which every Runtime before 20 refuses at its declaration
/// gate (`UnknownField` at `/candidate_layouts/<i>/shared_ocr`), in a pack of any execution mode
/// and a task of any schema. A `fixed_slots` layout's `shared_ocr` needs nothing here.
fn anchored_shared_read(task: &Value) -> bool {
    task.get(CANDIDATE_LAYOUTS)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|layout| layout.get(KIND).and_then(Value::as_str) == Some(REPEATED_ANCHOR))
        .any(|layout| layout.get(SHARED_OCR).is_some())
}

/// Revision 18 (Workflow #463 K-c, register §1.454; S2-9b, §1.458;
/// contracts/component-interfaces.md, the `package` row 18): the task file `task` has an operation
/// whose `select` holds a `scan` object that declares no `end` (absent or `null`, as the
/// declaration gate reads it: K-c's `declares`), the scan of a select that scrolls straight to the
/// item it wants. A Runtime before 18 requires `end` in its declaration gate in every pack,
/// `linear_steps` or not, so the row has no condition on the execution mode. Whether the scan also
/// declares `early_stop`, without which this build refuses it, is the declaration gate's.
fn scan_without_end(task: &Value) -> bool {
    task.get(OPERATIONS)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|operation| operation.get(SELECT))
        .filter_map(|select| select.get(SCAN))
        .filter_map(Value::as_object)
        .any(|scan| scan.get(END).is_none_or(Value::is_null))
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
    task.get(CANDIDATE_LAYOUTS)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|layout| layout.get(KIND).and_then(Value::as_str) == Some(REPEATED_ANCHOR))
        .filter_map(|layout| layout.get("anchor").and_then(Value::as_str))
        .any(|anchor| masked.contains(anchor))
}

/// Revision 15 (Workflow #463 K2, register §1.380; contracts/linear-steps.md, "Endings"): whether
/// a pack of `entries` declares an ending page, a terminal page of its entry task's
/// `scheduling_outcome` that names neither a target page of that task nor the page of one of its
/// select steps that declares `on_empty`. This is the kernel's own test of an admitted package,
/// `PreparedContainedTask::declares_linear_endings` in
/// `crates/execution-kernel/src/contained_task/linear.rs`, whose reader is private to the kernel
/// crate, which actinglab does not depend on; so it reads the same members, member by member:
///
/// - `self.control.execution_mode`: the pack's `control.json` names the execution mode
///   `linear_steps` (as [`linear_packs`] reads it);
/// - `self.program`, the package's program: the task file of the control's `entry_task_id`,
///   `resources/operations/<entry_task_id>/task.json`, the one task pack-containment's source
///   assembly takes from the pack (`source_pack::compile`, `canonical_task`), never another task
///   file of the pack ([`pack_declares_ending`]);
/// - `self.scheduling_outcome`, `mappings()`, `terminal_pages()`: every page of every mapping's
///   `terminal_pages`; an absent or `null` `scheduling_outcome` declares none;
/// - `self.terminal_page_anchors()`: the task's `target_page`, one page or a set of pages;
/// - `operation.on_empty_key().is_some()`, `operation.from`: the `from` of every element of the
///   task's `operations` whose `select` holds an `on_empty` (an absent member or `null` declares
///   none, as for the kernel's `Option`s);
/// - `crate::canonical_page_anchor(game, page)`, `game` the control's `game`: each page compared
///   with the prefix `<game>/` taken off where it is written ([`page_anchor`]).
///
/// A control without `game` or `entry_task_id`, and an entry task file that is absent or does not
/// read, declare none: the kernel admits no such package.
fn declares_linear_ending(entries: &BTreeMap<String, Vec<u8>>) -> bool {
    entries.iter().any(|(name, bytes)| {
        pack_entry(name).is_some_and(|(pack, within)| {
            within == CONTROL_FILE && pack_declares_ending(entries, pack, bytes)
        })
    })
}

/// Revision 15: whether the pack `pack` of `entries`, whose `control.json` holds `control`, is a
/// `linear_steps` pack whose entry task file declares an ending page ([`declares_linear_ending`]).
fn pack_declares_ending(entries: &BTreeMap<String, Vec<u8>>, pack: &str, control: &[u8]) -> bool {
    let Some(control) = read_json(control) else {
        return false;
    };
    if control.get(EXECUTION_MODE).and_then(Value::as_str) != Some(LINEAR_STEPS) {
        return false;
    }
    let (Some(game), Some(entry_task)) = (
        control.get(GAME).and_then(Value::as_str),
        control.get(ENTRY_TASK_ID).and_then(Value::as_str),
    ) else {
        return false;
    };
    let task_file = format!("{PACKS_PREFIX}{pack}/{OPERATIONS_PREFIX}{entry_task}/{TASK_FILE}");
    entries
        .get(&task_file)
        .map(Vec::as_slice)
        .and_then(read_json)
        .is_some_and(|task| task_declares_ending(&task, game))
}

/// Revision 15: whether the task document `task` of a pack of the game `game` maps a terminal page
/// that, compared by [`page_anchor`], is none of its target pages and none of its `on_empty` pages
/// ([`declares_linear_ending`]).
fn task_declares_ending(task: &Value, game: &str) -> bool {
    let Some(mappings) = task
        .get(SCHEDULING_OUTCOME)
        .and_then(|outcome| outcome.get(MAPPINGS))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let mut usual = match task.get(TARGET_PAGE) {
        Some(Value::String(page)) => vec![page_anchor(game, page)],
        Some(Value::Array(pages)) => pages
            .iter()
            .filter_map(Value::as_str)
            .map(|page| page_anchor(game, page))
            .collect(),
        _ => Vec::new(),
    };
    usual.extend(
        task.get(OPERATIONS)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|operation| {
                operation
                    .get(SELECT)
                    .and_then(|select| select.get(ON_EMPTY))
                    .is_some_and(|on_empty| !on_empty.is_null())
            })
            .filter_map(|operation| operation.get(FROM).and_then(Value::as_str))
            .map(|page| page_anchor(game, page)),
    );
    mappings
        .iter()
        .filter_map(|mapping| mapping.get(TERMINAL_PAGES).and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .any(|page| !usual.contains(&page_anchor(game, page)))
}

/// Revision 15: `page` with the prefix `<game>/` taken off where it is written, as the kernel's
/// `canonical_page_anchor` gives it.
fn page_anchor<'a>(game: &str, page: &'a str) -> &'a str {
    page.strip_prefix(game)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(page)
}
