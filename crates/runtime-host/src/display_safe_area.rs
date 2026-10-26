// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #392 DA-4b-D (Alice's ruling 69 of 2026-10-26: 「只有安全区适配不做，但是需要留出来声明式的安全区
//! 接口，也就是说可以依照声明的文件直接切换安全区形态」, 「一份文件对应一个机型或多个机型，并且修改设置后实时生效」;
//! `contracts/display-adapter.md` section 9): the declared safe areas.
//!
//! A directory (`display_safe_area.directory`, relative to the state root) holds declaration
//! files, `*.json` of schema `actingcommand.display-safe-area.v1`: each declares one or more
//! device models and, for them, the insets of each display size and rotation it covers. An
//! instance declares its model by a setting (`instance.<id>.display_safe_area.model`); the
//! Runtime never reads a device's cutout and never guesses its model. This module reads the
//! directory and answers the insets of one frame; the host stamps them on the frame's geometry,
//! where the display adapter's map reads them.
//!
//! The directory is read again once `display_safe_area.check_interval_ms` has passed since its
//! last read (0: at every frame that asks). A read whose files have the same names and bytes as
//! the last one (their SHA-256) keeps its catalog; any change is parsed again and takes effect
//! from the frame that read it. A broken directory refuses every model until it is mended.

use actingcommand_contract::{CaptureExtent, CaptureInsets, CaptureRotation};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The schema version of a declaration file.
pub(crate) const DISPLAY_SAFE_AREA_SCHEMA: &str = "actingcommand.display-safe-area.v1";

/// The insets of an emulator, which needs no declaration file (ruling 69): none.
pub(crate) const EMULATOR_INSETS: CaptureInsets = CaptureInsets {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

/// Why the declared insets of a frame cannot be given; each is a `display_problem` of
/// `display_map_unresolved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SafeAreaRefusal {
    /// `safe_area_declaration_invalid`: the directory or a declaration file cannot be read, or
    /// the file breaks the schema at `json_pointer` (empty: the whole document).
    DeclarationInvalid { path: PathBuf, json_pointer: String },
    /// `safe_area_model_repeated`: the file `path` declares a model that `other_path`, before it
    /// in name order, declares too.
    ModelRepeated { path: PathBuf, other_path: PathBuf },
    /// `safe_area_model_undeclared`: no file of `directory` declares the instance's model.
    ModelUndeclared { directory: PathBuf },
    /// `safe_area_form_undeclared`: the file `path` declares the model, and no form of it is the
    /// display size and rotation the frame shows.
    FormUndeclared { path: PathBuf },
}

/// One form of a declaration: a display size at one rotation, and its insets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Form {
    display: CaptureExtent,
    rotation: CaptureRotation,
    insets: CaptureInsets,
}

/// One declaration file, parsed: the models it declares and their forms.
#[derive(Debug)]
struct Declaration {
    models: Vec<String>,
    forms: Vec<Form>,
}

/// Every model of the directory, with the file that declares it and its forms.
#[derive(Debug, Default)]
struct Catalog {
    models: BTreeMap<String, (PathBuf, Vec<Form>)>,
}

impl Catalog {
    /// The catalog of `files` (in name order): every file parsed, and no model in two files.
    fn build(files: &[(PathBuf, Vec<u8>)]) -> Result<Self, SafeAreaRefusal> {
        let mut models: BTreeMap<String, (PathBuf, Vec<Form>)> = BTreeMap::new();
        for (path, bytes) in files {
            let declaration = parse_declaration(bytes).map_err(|json_pointer| {
                SafeAreaRefusal::DeclarationInvalid {
                    path: path.clone(),
                    json_pointer,
                }
            })?;
            for model in declaration.models {
                if let Some((other_path, _)) = models.get(&model) {
                    return Err(SafeAreaRefusal::ModelRepeated {
                        path: path.clone(),
                        other_path: other_path.clone(),
                    });
                }
                models.insert(model, (path.clone(), declaration.forms.clone()));
            }
        }
        Ok(Self { models })
    }

    /// The insets `model` declares for `display` at `rotation`.
    fn insets(
        &self,
        directory: &Path,
        model: &str,
        display: CaptureExtent,
        rotation: Option<CaptureRotation>,
    ) -> Result<CaptureInsets, SafeAreaRefusal> {
        let (path, forms) =
            self.models
                .get(model)
                .ok_or_else(|| SafeAreaRefusal::ModelUndeclared {
                    directory: directory.to_path_buf(),
                })?;
        forms
            .iter()
            .find(|form| form.display == display && Some(form.rotation) == rotation)
            .map(|form| form.insets)
            .ok_or_else(|| SafeAreaRefusal::FormUndeclared { path: path.clone() })
    }
}

/// One read of the directory: when, the digest of its files, and their catalog or refusal.
#[derive(Debug)]
struct DirectoryRead {
    directory: PathBuf,
    read_at_ms: u64,
    /// SHA-256 of the files' names and bytes; empty for a read that failed.
    digest: Vec<u8>,
    catalog: Result<Catalog, SafeAreaRefusal>,
}

/// The host's reading of the declaration directory (see the module header). A leaf lock in
/// `HostShared`: nothing else is taken while it is held.
#[derive(Debug)]
pub(crate) struct SafeAreaDeclarations {
    /// The state root a relative directory resolves against.
    state_root: PathBuf,
    read: Option<DirectoryRead>,
}

impl SafeAreaDeclarations {
    pub(crate) fn new(state_root: PathBuf) -> Self {
        Self {
            state_root,
            read: None,
        }
    }

    /// The insets the instance's `model` declares for a frame of `display` at `rotation`
    /// (`None`: the source gives no rotation, which no form matches), from the declaration files
    /// of `directory` as read at most `check_interval_ms` before `now_ms` (the Runtime's
    /// monotonic clock).
    pub(crate) fn insets(
        &mut self,
        directory: &str,
        now_ms: u64,
        check_interval_ms: u64,
        model: &str,
        display: CaptureExtent,
        rotation: Option<CaptureRotation>,
    ) -> Result<CaptureInsets, SafeAreaRefusal> {
        let directory = self.state_root.join(directory);
        let read = match self.read.take() {
            Some(read)
                if read.directory == directory
                    && now_ms.saturating_sub(read.read_at_ms) < check_interval_ms =>
            {
                read
            }
            previous => Self::read_again(directory, now_ms, previous),
        };
        let insets = match &read.catalog {
            Ok(catalog) => catalog.insets(&read.directory, model, display, rotation),
            Err(refusal) => Err(refusal.clone()),
        };
        self.read = Some(read);
        insets
    }

    /// Reads `directory` again; files of the same digest as `previous`'s keep its catalog.
    fn read_again(
        directory: PathBuf,
        now_ms: u64,
        previous: Option<DirectoryRead>,
    ) -> DirectoryRead {
        let (digest, catalog) = match read_directory(&directory) {
            Ok(files) => {
                let digest = files_digest(&files);
                let catalog = match previous {
                    Some(previous)
                        if previous.directory == directory && previous.digest == digest =>
                    {
                        previous.catalog
                    }
                    _ => Catalog::build(&files),
                };
                (digest, catalog)
            }
            Err(refusal) => (Vec::new(), Err(refusal)),
        };
        DirectoryRead {
            directory,
            read_at_ms: now_ms,
            digest,
            catalog,
        }
    }
}

/// The declaration files of `directory`, in name order: every regular file whose extension is
/// `json` (any case); a directory that does not exist holds none.
fn read_directory(directory: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, SafeAreaRefusal> {
    let unreadable = |path: &Path| SafeAreaRefusal::DeclarationInvalid {
        path: path.to_path_buf(),
        json_pointer: String::new(),
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(unreadable(directory)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|_| unreadable(directory))?.path();
        let declaration = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
        if !declaration
            || !fs::metadata(&path)
                .map_err(|_| unreadable(path.as_path()))?
                .is_file()
        {
            continue;
        }
        let bytes = fs::read(&path).map_err(|_| unreadable(path.as_path()))?;
        files.push((path, bytes));
    }
    // The paths differ, so this is name order.
    files.sort();
    Ok(files)
}

/// SHA-256 of the files' names and bytes, each length-prefixed.
fn files_digest(files: &[(PathBuf, Vec<u8>)]) -> Vec<u8> {
    let length = |bytes: &[u8]| u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes();
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        hasher.update(length(name.as_bytes()));
        hasher.update(name.as_bytes());
        hasher.update(length(bytes.as_slice()));
        hasher.update(bytes);
    }
    hasher.finalize().to_vec()
}

/// One declaration file's bytes, decoded and checked (`contracts/display-adapter.md` section
/// 9.1); otherwise the JSON pointer of the first member that breaks the schema (an unknown,
/// missing or misfit member, in document order; empty for the whole document).
fn parse_declaration(bytes: &[u8]) -> Result<Declaration, String> {
    let document: Value = serde_json::from_slice(bytes).map_err(|_| String::new())?;
    let object = members(&document, "", &["schema_version", "models", "forms"])?;
    if field(object, "schema_version").as_str() != Some(DISPLAY_SAFE_AREA_SCHEMA) {
        return Err("/schema_version".to_owned());
    }
    let mut models: Vec<String> = Vec::new();
    for (index, model) in non_empty_array(object, "models")?.iter().enumerate() {
        match model.as_str() {
            Some(name) if !name.is_empty() && !models.iter().any(|seen| seen == name) => {
                models.push(name.to_owned());
            }
            _ => return Err(format!("/models/{index}")),
        }
    }
    let mut forms: Vec<Form> = Vec::new();
    for (index, form) in non_empty_array(object, "forms")?.iter().enumerate() {
        let at = format!("/forms/{index}");
        let form = parse_form(form, &at)?;
        if forms
            .iter()
            .any(|seen| seen.display == form.display && seen.rotation == form.rotation)
        {
            return Err(at);
        }
        forms.push(form);
    }
    Ok(Declaration { models, forms })
}

/// One form at `pointer`: `display {width, height}` (each at least 1), `rotation` (`r0`, `r90`,
/// `r180` or `r270`) and `insets {left, top, right, bottom}`, which leave the safe area a width
/// and a height.
fn parse_form(value: &Value, pointer: &str) -> Result<Form, String> {
    let object = members(value, pointer, &["display", "rotation", "insets"])?;
    let display_at = format!("{pointer}/display");
    let display = members(field(object, "display"), &display_at, &["width", "height"])?;
    let side = |name: &str| {
        pixels(display, name)
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("{display_at}/{name}"))
    };
    let (width, height) = (side("width")?, side("height")?);
    let extent = CaptureExtent::new(width, height).ok_or_else(|| display_at.clone())?;
    let rotation = match field(object, "rotation").as_str() {
        Some("r0") => CaptureRotation::R0,
        Some("r90") => CaptureRotation::R90,
        Some("r180") => CaptureRotation::R180,
        Some("r270") => CaptureRotation::R270,
        _ => return Err(format!("{pointer}/rotation")),
    };
    let insets_at = format!("{pointer}/insets");
    let sides = members(
        field(object, "insets"),
        &insets_at,
        &["left", "top", "right", "bottom"],
    )?;
    let inset = |name: &str| pixels(sides, name).ok_or_else(|| format!("{insets_at}/{name}"));
    let insets = CaptureInsets {
        left: inset("left")?,
        top: inset("top")?,
        right: inset("right")?,
        bottom: inset("bottom")?,
    };
    // The safe area keeps a width and a height inside the display, as a recorded geometry's
    // insets must (`CaptureGeometry::safe_insets_fit_display`).
    if u64::from(insets.left) + u64::from(insets.right) >= u64::from(width) {
        return Err(format!("{insets_at}/right"));
    }
    if u64::from(insets.top) + u64::from(insets.bottom) >= u64::from(height) {
        return Err(format!("{insets_at}/bottom"));
    }
    Ok(Form {
        display: extent,
        rotation,
        insets,
    })
}

/// The object `value` at `pointer` with every member of `names` and no other.
fn members<'a>(
    value: &'a Value,
    pointer: &str,
    names: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value.as_object().ok_or_else(|| pointer.to_owned())?;
    if let Some(unknown) = object.keys().find(|key| !names.contains(&key.as_str())) {
        return Err(format!("{pointer}/{}", segment(unknown)));
    }
    if let Some(missing) = names.iter().find(|name| !object.contains_key(**name)) {
        return Err(format!("{pointer}/{missing}"));
    }
    Ok(object)
}

/// The member `name` of `object`, `null` when it has none.
fn field<'a>(object: &'a Map<String, Value>, name: &str) -> &'a Value {
    object.get(name).unwrap_or(&Value::Null)
}

/// The non-empty array `name` of the document.
fn non_empty_array<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Vec<Value>, String> {
    field(object, name)
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| format!("/{name}"))
}

/// The pixel count `name` of `object`.
fn pixels(object: &Map<String, Value>, name: &str) -> Option<u32> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

/// One JSON pointer segment (RFC 6901).
fn segment(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}
