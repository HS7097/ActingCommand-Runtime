// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #390 TOOL-1 (forced compromise P-03): the two authoring tools of a content
//! directory, which take the directory by its path. No digest is computed, compared or reported
//! anywhere (Workflow #336, option 丙).
//!
//! - `package fmt --package <directory> [--check]` writes every JSON file of the directory in
//!   the canonical serialization (`canonical_bytes`) and leaves every other file as it is.
//!   With `--check` it writes nothing and fails with `package_fmt_check_failed`, naming the
//!   files that are not canonical, so a CI job can require it.
//! - `package edit --package <directory> (--patch <file> | --file <path> (--add <pointer> |
//!   --replace <pointer>) --value <JSON>)` applies JSON-pointer edits (RFC 6901 pointers, the
//!   `add` and `replace` operations of RFC 6902, each naming its file) to the JSON files of the
//!   directory in memory, admits the edited package structurally as actingd admits a content
//!   directory, and only then writes the files the edits changed, in the canonical
//!   serialization. A refused admission writes nothing.
//!
//! The directory is read once with the loader's own snapshot, so these tools see exactly the
//! files admission reads, under the same rules (no links, no LFS pointers, the same limits).
//! A file is written to a `.part` file beside it first and then renamed over it, so a reader
//! sees the old file or the new one; a failed write names the file it stopped at, the files
//! already written and any `.part` file left behind, and removes nothing.

use super::codes::ActinglabCode;
use super::{CliError, CliOutcome, ErrorKind, FlagArgs, GlobalOptions};
use actingcommand_contract::ContainedTaskRequest;
use actingcommand_lab::{DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES, PreparedContainedTask};
use actingcommand_pack_containment::Containment;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The label of the in-memory admission of an edited package.
const ADMISSION_INSTANCE: &str = "lab.package.edit";
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";
const FMT_FLAGS: &[&str] = &["--package", "--check"];
const EDIT_FLAGS: &[&str] = &[
    "--package",
    "--patch",
    "--file",
    "--add",
    "--replace",
    "--value",
];

/// `package fmt --package <directory> [--check]`.
pub(super) fn run_fmt(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    only_flags(flags, "package fmt", FMT_FLAGS)?;
    let check = flags.bool("--check");
    if !check {
        crate::dry_run_gate::refuse(
            global,
            "package fmt",
            Some("package fmt --check"),
            Value::Null,
        )?;
    }
    let root = content_directory(flags, "package fmt")?;
    let entries = snapshot(&root)?;
    let mut json_files = 0_usize;
    let mut changes = Vec::new();
    for (relative, bytes) in &entries {
        if !is_json(relative) {
            continue;
        }
        json_files += 1;
        let canonical = canonical_bytes(&parse_document(relative, bytes)?)?;
        if canonical != *bytes {
            changes.push((relative.clone(), canonical));
        }
    }
    let changed = changes
        .iter()
        .map(|(relative, _)| relative.clone())
        .collect::<Vec<_>>();
    let package = root.display().to_string();
    if check {
        if !changed.is_empty() {
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                ActinglabCode::PackageFmtCheckFailed.as_str(),
                format!(
                    "{} of the {json_files} JSON files of {package} are not in the canonical \
                     serialization; run package fmt without --check",
                    changed.len()
                ),
                &[],
            )
            .with_details(json!({
                "package": package,
                "json_files": json_files,
                "unformatted": changed,
            })));
        }
        return Ok(json!({
            "mode": "package_fmt_check",
            "package": package,
            "json_files": json_files,
            "unformatted": changed,
        }));
    }
    write_files(&root, &changes, ActinglabCode::PackageFmtWriteFailed)?;
    Ok(json!({
        "mode": "package_fmt",
        "package": package,
        "json_files": json_files,
        "reformatted": changed,
    }))
}

/// One edit: an RFC 6902 `add` or `replace` operation on the JSON file `file` of the package,
/// a `/`-separated path in the directory.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    file: String,
    op: EditOp,
    path: String,
    value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EditOp {
    Add,
    Replace,
}

/// Why an edit does not apply to its document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditFailure {
    /// The pointer is not an RFC 6901 JSON pointer.
    PointerInvalid,
    /// The pointer does not name a place the operation can take: a missing member or element
    /// for `replace`, a missing parent or an array index past the end for `add`.
    PointerUnresolved,
}

/// `package edit --package <directory> (--patch <file> | --file <path> (--add <pointer> |
/// --replace <pointer>) --value <JSON>)`.
pub(super) fn run_edit(global: &GlobalOptions, flags: &FlagArgs) -> CliOutcome<Value> {
    only_flags(flags, "package edit", EDIT_FLAGS)?;
    let root = content_directory(flags, "package edit")?;
    let edits = edit_list(flags)?;
    let edit_count = edits.len();
    let mut entries = snapshot(&root)?;
    let mut documents = BTreeMap::<String, Value>::new();
    for (index, edit) in edits.into_iter().enumerate() {
        let file = edit.file.replace('\\', "/");
        let place = json!({"edit": index, "file": file, "path": edit.path});
        if !is_json(&file) {
            return Err(patch_invalid(
                format!("edit {index} names {file}, which is not a JSON file"),
                place,
            ));
        }
        let document = match documents.entry(file.clone()) {
            Entry::Occupied(document) => document.into_mut(),
            Entry::Vacant(slot) => {
                let bytes = entries.get(&file).ok_or_else(|| {
                    patch_invalid(
                        format!("edit {index} names {file}, which the package does not hold"),
                        place.clone(),
                    )
                })?;
                slot.insert(parse_document(&file, bytes)?)
            }
        };
        apply(document, edit.op, &edit.path, edit.value).map_err(|failure| match failure {
            EditFailure::PointerInvalid => patch_invalid(
                format!(
                    "edit {index}: {} is not a JSON pointer (RFC 6901)",
                    edit.path
                ),
                place.clone(),
            ),
            EditFailure::PointerUnresolved => CliError::new(
                ErrorKind::UsageValidation,
                ActinglabCode::PackageEditPointerUnresolved.as_str(),
                format!(
                    "edit {index}: {} does not resolve in {file} for {}",
                    edit.path,
                    op_name(edit.op)
                ),
                &[],
            )
            .with_details(place.clone()),
        })?;
    }
    let mut changes = Vec::new();
    for (relative, document) in &documents {
        let bytes = canonical_bytes(document)?;
        if entries.get(relative) != Some(&bytes) {
            entries.insert(relative.clone(), bytes.clone());
            changes.push((relative.clone(), bytes));
        }
    }
    let package = root.display().to_string();
    let prepared = PreparedContainedTask::load_content_entries(
        ADMISSION_INSTANCE,
        entries,
        super::offline_ocr::lab_ocr().provider(),
        deadline()?,
    )
    .map_err(|error| {
        CliError::new(
            ErrorKind::UsageValidation,
            ActinglabCode::PackageEditAdmissionRefused.as_str(),
            format!(
                "the edited package is not admitted, so nothing was written: {}: {}",
                error.code(),
                error.detail().unwrap_or("admission failed")
            ),
            &[],
        )
        .with_details(json!({
            "package": package,
            "code": error.code().as_str(),
            "detail": error.detail(),
            "declaration": error.declaration_issue(),
            "written": false,
        }))
    })?;
    let files = changes
        .iter()
        .map(|(relative, _)| relative.clone())
        .collect::<Vec<_>>();
    if !global.dry_run {
        write_files(&root, &changes, ActinglabCode::PackageEditWriteFailed)?;
    }
    let mut data = json!({
        "mode": "package_edit",
        "package": package,
        "package_id": prepared.package_label(),
        "task_id": prepared.task_label(),
        "edits": edit_count,
        "files": files,
        "admission": "passed",
        "written": !global.dry_run,
    });
    if global.dry_run {
        data["status"] = json!("validated");
        data["dry_run"] = json!(true);
        data["persisted"] = json!(false);
    }
    Ok(data)
}

/// The canonical serialization of one JSON document of a package (FCM P-03, with the
/// coordinator's wave-15 ruling of LF line endings): two-space indentation, one member or
/// element per line, `": "` after a member name, members in their document order, text as UTF-8
/// with only what JSON requires escaped (a line break inside a string stays `\n`), every line
/// ending in LF (the last one too), and no byte-order mark.
fn canonical_bytes(value: &Value) -> CliOutcome<Vec<u8>> {
    let mut canonical = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut canonical,
        serde_json::ser::PrettyFormatter::with_indent(b"  "),
    );
    value.serialize(&mut serializer).map_err(|error| {
        CliError::package_invalid(format!("a JSON document does not serialize: {error}"))
    })?;
    canonical.push(b'\n');
    Ok(canonical)
}

/// A JSON file of the package, read as UTF-8 JSON after an optional byte-order mark.
fn parse_document(relative: &str, bytes: &[u8]) -> CliOutcome<Value> {
    let bytes = bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|error| {
        CliError::package_invalid(format!("{relative} is not a JSON document: {error}"))
            .with_details(json!({"file": relative}))
    })
}

/// Whether a package path names a JSON file (its extension, ASCII case-insensitive).
fn is_json(relative: &str) -> bool {
    Path::new(relative)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// Applies one `add` or `replace` operation (RFC 6902 sections 4.1 and 4.3) at `pointer`.
fn apply(document: &mut Value, op: EditOp, pointer: &str, value: Value) -> Result<(), EditFailure> {
    let mut tokens = pointer_tokens(pointer).ok_or(EditFailure::PointerInvalid)?;
    let Some(last) = tokens.pop() else {
        // The whole document: both operations put the value in its place.
        *document = value;
        return Ok(());
    };
    let mut parent = document;
    for token in &tokens {
        parent = child_mut(parent, token).ok_or(EditFailure::PointerUnresolved)?;
    }
    match (op, parent) {
        // An existing member is replaced in its place; a new one goes last.
        (EditOp::Add, Value::Object(members)) => {
            members.insert(last, value);
        }
        (EditOp::Replace, Value::Object(members)) => {
            *members
                .get_mut(&last)
                .ok_or(EditFailure::PointerUnresolved)? = value;
        }
        (EditOp::Add, Value::Array(items)) => {
            let index = if last == "-" {
                items.len()
            } else {
                array_index(&last)
                    .filter(|index| *index <= items.len())
                    .ok_or(EditFailure::PointerUnresolved)?
            };
            items.insert(index, value);
        }
        (EditOp::Replace, Value::Array(items)) => {
            *array_index(&last)
                .and_then(|index| items.get_mut(index))
                .ok_or(EditFailure::PointerUnresolved)? = value;
        }
        _ => return Err(EditFailure::PointerUnresolved),
    }
    Ok(())
}

/// The reference tokens of an RFC 6901 JSON pointer: `""` is the whole document, every other
/// pointer starts with `/`, and `~0` (`~`) and `~1` (`/`) are the only escapes.
fn pointer_tokens(pointer: &str) -> Option<Vec<String>> {
    if pointer.is_empty() {
        return Some(Vec::new());
    }
    pointer
        .strip_prefix('/')?
        .split('/')
        .map(|token| {
            let mut unescaped = String::with_capacity(token.len());
            let mut characters = token.chars();
            while let Some(character) = characters.next() {
                if character != '~' {
                    unescaped.push(character);
                    continue;
                }
                match characters.next() {
                    Some('0') => unescaped.push('~'),
                    Some('1') => unescaped.push('/'),
                    _ => return None,
                }
            }
            Some(unescaped)
        })
        .collect()
}

fn child_mut<'v>(value: &'v mut Value, token: &str) -> Option<&'v mut Value> {
    match value {
        Value::Object(members) => members.get_mut(token),
        Value::Array(items) => array_index(token).and_then(move |index| items.get_mut(index)),
        _ => None,
    }
}

/// An RFC 6901 array index: `0`, or decimal digits without a leading zero.
fn array_index(token: &str) -> Option<usize> {
    let digits = !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_digit());
    if !digits || (token.len() > 1 && token.starts_with('0')) {
        return None;
    }
    token.parse().ok()
}

fn op_name(op: EditOp) -> &'static str {
    match op {
        EditOp::Add => "add",
        EditOp::Replace => "replace",
    }
}

/// The edits: the array of a `--patch` file, or the one edit of `--file` with `--add` or
/// `--replace` and `--value`.
fn edit_list(flags: &FlagArgs) -> CliOutcome<Vec<Edit>> {
    let inline = ["--file", "--add", "--replace", "--value"]
        .iter()
        .any(|name| flags.flags.contains_key(*name));
    if flags.flags.contains_key("--patch") {
        if inline {
            return Err(CliError::usage(
                "package edit takes --patch or one edit (--file, --add or --replace, --value), \
                 not both",
            ));
        }
        let patch = flags.required_path("--patch")?;
        let edits = read_patch(&patch)?;
        if edits.is_empty() {
            return Err(patch_invalid(
                format!("{} holds no edit", patch.display()),
                json!({"patch": patch.display().to_string()}),
            ));
        }
        return Ok(edits);
    }
    let file = flags.required("--file")?;
    let (op, path) = match (flags.optional("--add"), flags.optional("--replace")) {
        (Some(path), None) => (EditOp::Add, path),
        (None, Some(path)) => (EditOp::Replace, path),
        _ => {
            return Err(CliError::usage(
                "package edit takes --patch <file>, or --file with exactly one of --add <pointer> \
                 and --replace <pointer>",
            ));
        }
    };
    // A bare flag reads as `true`, which is also the JSON value true.
    let text = flags
        .optional("--value")
        .ok_or_else(|| CliError::usage("missing --value <JSON>"))?;
    let value = serde_json::from_str(&text).map_err(|error| {
        patch_invalid(
            format!("--value is not one JSON value: {error}"),
            json!({"edit": 0, "file": file, "path": path}),
        )
    })?;
    Ok(vec![Edit {
        file,
        op,
        path,
        value,
    }])
}

/// The edit list of a `--patch` file: a JSON array of `{file, op, path, value}`, `op` `add` or
/// `replace`, with no other member.
fn read_patch(patch: &Path) -> CliOutcome<Vec<Edit>> {
    let mut bytes = Vec::new();
    fs::File::open(patch)
        .and_then(|file| {
            file.take(DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| {
            patch_invalid(
                format!("{} is unreadable: {error}", patch.display()),
                json!({"patch": patch.display().to_string()}),
            )
        })?;
    if bytes.len() > DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES {
        return Err(patch_invalid(
            format!(
                "{} exceeds {DEFAULT_MAX_BUFFERED_PAYLOAD_BYTES} bytes",
                patch.display()
            ),
            json!({"patch": patch.display().to_string()}),
        ));
    }
    let bytes = bytes.strip_prefix(BYTE_ORDER_MARK).unwrap_or(&bytes);
    serde_json::from_slice(bytes).map_err(|error| {
        patch_invalid(
            format!(
                "{} is not a list of edits {{file, op: add|replace, path, value}}: {error}",
                patch.display()
            ),
            json!({"patch": patch.display().to_string()}),
        )
    })
}

fn patch_invalid(message: String, details: Value) -> CliError {
    CliError::new(
        ErrorKind::UsageValidation,
        ActinglabCode::PackageEditPatchInvalid.as_str(),
        message,
        &[],
    )
    .with_details(details)
}

/// Refuses a flag the command does not take (FlagArgs accepts any) and every positional.
fn only_flags(flags: &FlagArgs, command: &str, accepted: &[&str]) -> CliOutcome<()> {
    if let Some(name) = flags
        .flags
        .keys()
        .find(|name| !accepted.contains(&name.as_str()))
    {
        return Err(CliError::usage(format!("{command} does not accept {name}")));
    }
    if let Some(positional) = flags.positionals.first() {
        return Err(CliError::usage(format!(
            "{command} takes no positional argument: {positional}"
        )));
    }
    Ok(())
}

/// The absolute path of `--package`, which must not name a file: these tools work on a
/// content directory only.
fn content_directory(flags: &FlagArgs, command: &str) -> CliOutcome<PathBuf> {
    let given = flags.required_path("--package")?;
    let path = if given.is_absolute() {
        given
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliError::usage(format!("the working directory is unreadable: {error}"))
            })?
            .join(given)
    };
    if fs::metadata(&path).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err(CliError::usage(format!(
            "{command} takes a content directory; {} is a file",
            path.display()
        )));
    }
    Ok(path)
}

/// The directory read once, as admission reads it: every regular file by its `/`-separated
/// relative path.
fn snapshot(root: &Path) -> CliOutcome<BTreeMap<String, Vec<u8>>> {
    Containment::new()
        .snapshot_content_directory(root, deadline()?)
        .map_err(|error| {
            CliError::package_invalid(format!("{}: {error}", root.display()))
                .with_details(json!({"package": root.display().to_string()}))
        })
}

fn deadline() -> CliOutcome<Instant> {
    Instant::now()
        .checked_add(Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| CliError::package_invalid("package read deadline overflow"))
}

/// Writes each file over its old one: the bytes go to `<name>.<process id>.part` beside it,
/// which is then renamed over it. A failure stops at that file and names it, the files already
/// written and the `.part` file it left; nothing is removed.
fn write_files(root: &Path, files: &[(String, Vec<u8>)], failed: ActinglabCode) -> CliOutcome<()> {
    let mut written = Vec::new();
    for (relative, bytes) in files {
        let target = relative
            .split('/')
            .fold(root.to_path_buf(), |path, segment| path.join(segment));
        let mut part_name = target.file_name().unwrap_or_default().to_os_string();
        part_name.push(format!(".{}.part", std::process::id()));
        let part = target.with_file_name(part_name);
        let result = fs::write(&part, bytes).and_then(|()| fs::rename(&part, &target));
        if let Err(error) = result {
            let left = part.exists().then(|| part.display().to_string());
            return Err(CliError::new(
                ErrorKind::UsageValidation,
                failed.as_str(),
                format!("{} could not be written: {error}", target.display()),
                &[],
            )
            .with_details(json!({
                "package": root.display().to_string(),
                "file": relative,
                "written": written,
                "part_left": left,
            })));
        }
        written.push(relative.clone());
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/gate_390_tool1.rs"]
pub(crate) mod gate_390_tool1;
