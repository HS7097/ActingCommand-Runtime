// SPDX-License-Identifier: AGPL-3.0-only

//! #463 SEL-F, Addendum 1 (`contracts/lab-recording.md`, "Converting a package from its own
//! files"): `package fix-select`, the package route. A package `record stop` generated with a
//! select whose wanted value is a package variable is converted into the fixed form from its own
//! files, without its recording, a device or a Runtime: the variable's declaration goes from
//! `control.json`, the select's policy document takes the fixed form's members, and the task's
//! provenance says the package was converted. The members it rewrites are written by the
//! generator's own functions (`variable_policy_members`, `fixed_policy_members`,
//! `variable_declaration`, `pretty`): what it compares is what `record stop` writes for a select
//! with a variable, and what it writes is what `record stop` writes for the same select fixed.
//! It refuses rather than guesses (`record_select_invalid` with its `select_problem`), and a refused
//! conversion changes nothing: this module only computes the files; the caller writes them.

use crate::LabOcr;
use crate::codes::{LabCode, SelectProblem};
use crate::recording::generate::{
    SELECT_FEATURE, WANTED_POLICY_MEMBERS, fixed_policy_members, pretty, variable_declaration,
    variable_policy_members,
};
use crate::recording::model::SelectNameSpec;
use crate::recording::store::{blocked, invalid, with_details};
use actingcommand_contract::outcome::{Detail, Outcome};
use actingcommand_contract::{
    LabError, LabResult, MAX_GOAL_ORDERED_COUNT, PACKAGE_CONTROL_FILE, VARIABLE_FACT_KEY_PREFIX,
};
use actingcommand_execution_kernel::PreparedContainedTask;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// The operation's words, as the converted package's provenance names it.
pub const SELECT_FIX_OPERATION: &str = "package fix-select";
/// The member of a task's `provenance` that lists the conversions made of it, one entry each.
pub const SELECT_FIX_PROVENANCE: &str = "select_fixes";
/// The instance the converted package is admitted for.
const ADMISSION_INSTANCE: &str = "lab.package.fix-select";
/// The time the converted package's admission may take, as `record stop`'s.
const ADMISSION_BUDGET: Duration = Duration::from_secs(120);

/// What `package fix-select` is asked: the variable's name and, optionally, the names to fix the
/// select to, in an order of preference (absent: the variable's default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectFixRequest {
    pub variable: String,
    pub fixed: Option<Vec<String>>,
}

/// One file of the package the conversion changes: its `/`-separated path in the package, the
/// members it rewrote (JSON pointers) and its new bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectFixFile {
    pub file: String,
    pub members: Vec<String>,
    pub bytes: Vec<u8>,
}

/// A conversion that holds: the package, the select step's operation, the variable it removes,
/// the default it had, the names it fixes the select to and the files it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectFix {
    pub package_id: String,
    pub operation: String,
    pub variable: String,
    pub default: Vec<String>,
    pub fixed: Vec<String>,
    pub files: Vec<SelectFixFile>,
}

/// `record_select_invalid` with `problem`, its details naming the variable and what `extra` adds;
/// nothing was written.
fn refused(problem: SelectProblem, message: String, variable: &str, extra: Value) -> LabError {
    let mut details = json!({
        "variable": variable,
        "select_problem": problem.as_str(),
        "outcome": Outcome::new(LabCode::RecordSelectInvalid)
            .select_problem(problem)
            .envelope(Detail::Full)
    });
    if let (Some(object), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    with_details(
        blocked(
            LabCode::RecordSelectInvalid.as_str(),
            format!("{message}; nothing was written"),
        ),
        details,
    )
}

/// The JSON document `path` of the package, read as UTF-8 JSON after an optional byte-order mark.
fn document(entries: &BTreeMap<String, Vec<u8>>, path: &str) -> LabResult<Value> {
    let bytes = entries
        .get(path)
        .ok_or_else(|| invalid("validation_failed", format!("the package holds no {path}")))?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|error| {
        invalid(
            "validation_failed",
            format!("{path} of the package is not a JSON document: {error}"),
        )
    })
}

/// Whether `value` holds the text `wanted` as a string anywhere.
fn names_text(value: &Value, wanted: &str) -> bool {
    match value {
        Value::String(text) => text == wanted,
        Value::Array(items) => items.iter().any(|item| names_text(item, wanted)),
        Value::Object(members) => members.values().any(|member| names_text(member, wanted)),
        _ => false,
    }
}

/// The JSON files of the package that name `fact_key`, by their paths.
fn readers(entries: &BTreeMap<String, Vec<u8>>, fact_key: &str) -> LabResult<Vec<String>> {
    let mut found = Vec::new();
    for path in entries.keys() {
        let json_file = std::path::Path::new(path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if json_file && names_text(&document(entries, path)?, fact_key) {
            found.push(path.clone());
        }
    }
    Ok(found)
}

/// The names of the select whose operation is `operation` in `task`: the identity entries of the
/// feature `name` of the layout the operation names, as `record stop` writes them from the
/// select's names. `None` when the task holds no such layout or feature.
fn select_names(task: &Value, operation: &Value) -> Option<Vec<SelectNameSpec>> {
    let layout_id = operation["select"]["layout_id"].as_str()?;
    let layout = task["candidate_layouts"]
        .as_array()?
        .iter()
        .find(|layout| layout["id"] == layout_id)?;
    let feature = layout["features"]
        .as_array()?
        .iter()
        .find(|feature| feature["name"] == SELECT_FEATURE && feature["value"] == "identity")?;
    serde_json::from_value(feature["identity"]["entries"].clone()).ok()
}

/// The names `listed` as a list of names, each once, in their order; `None` when it is no array
/// of texts, is empty, or repeats a name.
fn name_list(listed: &Value) -> Option<Vec<String>> {
    let items = listed.as_array()?;
    let mut once = BTreeSet::new();
    let mut names = Vec::with_capacity(items.len());
    for item in items {
        let name = item.as_str()?;
        if !once.insert(name) {
            return None;
        }
        names.push(name.to_owned());
    }
    (!names.is_empty()).then_some(names)
}

/// #463 SEL-F: converts the select of the package `entries` (a content directory read as
/// admission reads it, by `/`-separated paths) whose wanted value is the variable
/// `request.variable` into the fixed form, fixed to `request.fixed`, else to the variable's
/// default. In this order it refuses, `record_select_invalid` with: `variable_missing` (no such
/// variable in `control.json`), `variable_read_elsewhere` (no select's policy document alone reads
/// it), `members_edited` (the variable's declaration but its default, the policy's `facts`, `gates`
/// and `scoring`, or the select's names are not as `record stop` writes them for a select with a
/// variable), `default_not_names` (its default is no list of names, each once),
/// `default_not_in_names` (a name of the default, or a name given, is none of the select's names),
/// `kernel_refused` (the converted package is not admitted); names given that are no list of 1 to
/// `min(32, names)` names, each once, are `validation_failed`. A refusal changes nothing. On
/// success it answers the files to write: `control.json` without the declaration (without
/// `variables` when it was the last), the policy document with the fixed form's members and the
/// task with one more entry in `provenance.select_fixes`.
pub fn fix_package_select(
    entries: &BTreeMap<String, Vec<u8>>,
    request: &SelectFixRequest,
    ocr: &LabOcr,
) -> LabResult<SelectFix> {
    let variable = request.variable.as_str();
    let mut control = document(entries, PACKAGE_CONTROL_FILE)?;
    let Some(at) = control["variables"].as_array().and_then(|declared| {
        declared
            .iter()
            .position(|declaration| declaration["name"] == variable)
    }) else {
        return Err(refused(
            SelectProblem::VariableMissing,
            format!("the package's {PACKAGE_CONTROL_FILE} declares no variable {variable}"),
            variable,
            json!({}),
        ));
    };
    let declaration = control["variables"][at].clone();
    let package_id = control["package_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let task_id = control["entry_task_id"].as_str().ok_or_else(|| {
        invalid(
            "validation_failed",
            format!("the package's {PACKAGE_CONTROL_FILE} names no entry_task_id"),
        )
    })?;
    let task_dir = format!("resources/operations/{task_id}");
    let task_path = format!("{task_dir}/task.json");
    let mut task = document(entries, &task_path)?;

    // The one select step that reads the variable, and nothing else that does.
    let fact_key = format!("{VARIABLE_FACT_KEY_PREFIX}{variable}");
    let found = readers(entries, &fact_key)?;
    let selects = task["operations"]
        .as_array()
        .map(|operations| {
            operations
                .iter()
                .filter(|operation| {
                    operation["select"]["policy"]["path"]
                        .as_str()
                        .is_some_and(|path| found.contains(&format!("{task_dir}/{path}")))
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let ([reader], [operation]) = (found.as_slice(), selects.as_slice()) else {
        return Err(refused(
            SelectProblem::VariableReadElsewhere,
            format!(
                "the variable {variable} is not read by one select step's policy document alone: \
                 {} file(s) of the package name {fact_key} and {} select step(s) read them",
                found.len(),
                selects.len()
            ),
            variable,
            json!({"readers": found}),
        ));
    };
    let policy_path = reader.clone();
    let operation_id = operation["id"].as_str().unwrap_or_default().to_owned();
    let edited = |what: &str, file: &str| {
        refused(
            SelectProblem::MembersEdited,
            format!(
                "{what} of {file} is not as record stop writes it for a select with the variable \
                 {variable}: the package was edited by hand and is not converted"
            ),
            variable,
            json!({"file": file, "member": what, "operation": operation_id}),
        )
    };

    // The members it rewrites, as the generator writes them for a select with a variable.
    let mut policy = document(entries, &policy_path)?;
    for (member, expected) in variable_policy_members(variable) {
        if policy.get(member) != Some(&expected) {
            return Err(edited(member, &policy_path));
        }
    }
    let names = select_names(&task, operation).ok_or_else(|| edited("the layout", &task_path))?;
    let locale = task["locale"]
        .as_str()
        .ok_or_else(|| edited("locale", &task_path))?;
    let mut expected = variable_declaration(locale, &names, variable, "");
    expected["default"] = declaration["default"].clone();
    if declaration != expected {
        return Err(edited(&format!("/variables/{at}"), PACKAGE_CONTROL_FILE));
    }

    // The default it had, and the names it fixes the select to.
    let known = names
        .iter()
        .map(|name| name.id.as_str())
        .collect::<BTreeSet<_>>();
    let default = name_list(&declaration["default"]).ok_or_else(|| {
        refused(
            SelectProblem::DefaultNotNames,
            format!("the default of the variable {variable} is no list of names, each once"),
            variable,
            json!({"default": declaration["default"]}),
        )
    })?;
    if let Some(absent) = default.iter().find(|name| !known.contains(name.as_str())) {
        return Err(refused(
            SelectProblem::DefaultNotInNames,
            format!("the default's name {absent} is none of the select's names"),
            variable,
            json!({"name": absent}),
        ));
    }
    let fixed = match &request.fixed {
        None => default.clone(),
        Some(given) => {
            let most = u64::try_from(names.len())
                .unwrap_or(MAX_GOAL_ORDERED_COUNT)
                .min(MAX_GOAL_ORDERED_COUNT);
            let count = u64::try_from(given.len()).unwrap_or(u64::MAX);
            let once = given.iter().collect::<BTreeSet<_>>().len() == given.len();
            if !(1..=most).contains(&count) || !once {
                return Err(invalid(
                    "validation_failed",
                    format!(
                        "the names to fix the select to are 1 to {most} names of the select, each \
                         once, in an order of preference; got {}",
                        given.join(",")
                    ),
                ));
            }
            if let Some(absent) = given.iter().find(|name| !known.contains(name.as_str())) {
                return Err(refused(
                    SelectProblem::DefaultNotInNames,
                    format!("the name {absent} given is none of the select's names"),
                    variable,
                    json!({"name": absent}),
                ));
            }
            given.clone()
        }
    };

    // The fixed form: the generator's members, the declaration gone, the conversion recorded.
    let fixed_names = fixed.iter().map(String::as_str).collect::<Vec<_>>();
    for (member, value) in fixed_policy_members(variable, &fixed_names) {
        policy[member] = value;
    }
    let mut control_members = vec![format!("/variables/{at}")];
    if let Some(declared) = control["variables"].as_array_mut() {
        declared.remove(at);
        if declared.is_empty()
            && let Some(object) = control.as_object_mut()
        {
            object.shift_remove("variables");
            control_members = vec!["/variables".to_owned()];
        }
    }
    let note = json!({
        "operation": SELECT_FIX_OPERATION,
        "step_operation": operation_id,
        "variable": variable,
        "default": default,
        "fixed": fixed
    });
    if !task["provenance"].is_object() {
        task["provenance"] = json!({});
    }
    match task["provenance"][SELECT_FIX_PROVENANCE].as_array_mut() {
        Some(listed) => listed.push(note),
        None => task["provenance"][SELECT_FIX_PROVENANCE] = json!([note]),
    }
    let files = vec![
        SelectFixFile {
            file: PACKAGE_CONTROL_FILE.to_owned(),
            members: control_members,
            bytes: pretty(&control)?,
        },
        SelectFixFile {
            file: policy_path,
            members: WANTED_POLICY_MEMBERS
                .iter()
                .map(|member| format!("/{member}"))
                .collect(),
            bytes: pretty(&policy)?,
        },
        SelectFixFile {
            file: task_path,
            members: vec![format!("/provenance/{SELECT_FIX_PROVENANCE}")],
            bytes: pretty(&task)?,
        },
    ];

    // The converted package is admitted as actingd admits a content directory.
    let mut converted = entries.clone();
    for file in &files {
        converted.insert(file.file.clone(), file.bytes.clone());
    }
    PreparedContainedTask::load_content_entries(
        ADMISSION_INSTANCE,
        converted,
        ocr.provider(),
        Instant::now() + ADMISSION_BUDGET,
    )
    .map_err(|error| {
        refused(
            SelectProblem::KernelRefused,
            format!("the converted package is not admitted: {error}"),
            variable,
            json!({"loader_code": error.code().as_str(), "detail": error.detail()}),
        )
    })?;
    Ok(SelectFix {
        package_id,
        operation: operation_id,
        variable: variable.to_owned(),
        default,
        fixed,
        files,
    })
}
