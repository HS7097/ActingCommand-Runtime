// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-11 (register §1.240; design `rd-1029/429-A-runtime.md` A.7.3): a game root's
//! optional display metadata for clients, `ui.json` (`actingcommand.ui.v1`), beside
//! `bundle.json`: the groups a client sorts the game's packages into, each package's title,
//! summary, group, opaque risk words and parameter labels (each text per locale), and the display
//! the game supports (its aspect ratios and an opaque safe-area word).
//!
//! The Runtime checks the file's shape, its limits and its references only: every `package_id` is
//! a package of the game root's bundle, every `group` an id of `groups`, every parameter name one
//! the package declares. It never interprets a string: titles, summaries, labels, locales, group
//! ids, risk words, option names and the safe-area word are opaque, compared only for equality and
//! measured only in bytes; an aspect ratio is only checked to be of the form `<w>:<h>`. A file
//! that keeps every rule reads back exactly as written, its members in the file's order.
//!
//! Package-level parameter declarations (A.7.2) arrive with R-10. Until then no package declares
//! a parameter, so every name under a pack's `parameters` is an undeclared reference
//! (`ui_parameter_undeclared`).
//!
//! Only `actingctl packs` and `ac_packs` read the file (offline, [`read_standard_pack_root_with_ui`]
//! and [`parse_standard_pack_ui`]). The Runtime's root reader ([`read_standard_pack_root`]), its
//! run preparation, the policy crate and check-config never do. A refused `ui.json` never breaks
//! its game root: its packages are listed as without the file, each violation a warning at its
//! JSON pointer, at most [`MAX_STANDARD_PACK_UI_ISSUES`] of them in document order.

use crate::standard_pack::{
    STANDARD_PACK_BUNDLE_FILE, StandardPackProblem, StandardPackRegistry, StandardPackRoot,
    StandardPackRootBytes, StandardPackRootIssue, read_standard_pack_root,
};
use serde::Serialize;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::collections::BTreeSet;
use std::fmt;

/// The game root's display metadata for clients (R-11).
pub const STANDARD_PACK_UI_FILE: &str = "ui.json";
/// The one version of `ui.json` this build reads.
pub const STANDARD_PACK_UI_SCHEMA_VERSION: &str = "actingcommand.ui.v1";
/// The largest `ui.json`, in bytes.
pub const MAX_STANDARD_PACK_UI_BYTES: usize = 256 * 1024;
/// The most violations one refused `ui.json` reports; the first ones in document order.
pub const MAX_STANDARD_PACK_UI_ISSUES: usize = 64;
/// The most entries of `groups`.
pub const MAX_PACK_UI_GROUPS: usize = 64;
/// The most entries of `packs`.
pub const MAX_PACK_UI_PACKS: usize = 1024;
/// The most locales of one text (`title`, `summary`, `label`, an option's text): at least one.
pub const MAX_PACK_UI_LOCALES: usize = 16;
/// The longest locale name of a text, in bytes (at least one).
pub const MAX_PACK_UI_LOCALE_BYTES: usize = 35;
/// The longest word, in bytes (at least one): a group's `id`, a pack's `group`, a risk word, a
/// parameter name, an option name and `display.safe_area`.
pub const MAX_PACK_UI_WORD_BYTES: usize = 64;
/// The longest short text, in bytes (at least one): a `title`, a parameter's `label` and an
/// option's text, per locale.
pub const MAX_PACK_UI_TITLE_BYTES: usize = 128;
/// The longest `summary` text, in bytes (at least one), per locale.
pub const MAX_PACK_UI_SUMMARY_BYTES: usize = 1024;
/// The most risk words of one pack.
pub const MAX_PACK_UI_RISK_WORDS: usize = 8;
/// The most parameters of one pack.
pub const MAX_PACK_UI_PARAMETERS: usize = 16;
/// The most options of one parameter.
pub const MAX_PACK_UI_OPTIONS: usize = 32;
/// The most aspect ratios of `display` (at least one).
pub const MAX_PACK_UI_ASPECT_RATIOS: usize = 8;
/// The most digits of one side of an aspect ratio `<w>:<h>`.
pub const MAX_PACK_UI_ASPECT_DIGITS: usize = 5;

/// A `ui.json` that keeps every rule, as written: its members in the file's order. It serializes
/// as the document it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct StandardPackUi {
    document: Map<String, Value>,
}

impl StandardPackUi {
    /// The whole document, as written.
    pub const fn document(&self) -> &Map<String, Value> {
        &self.document
    }

    /// The entry of `packs` for `package_id`, as written; `None` when the file has none for it.
    pub fn pack(&self, package_id: &str) -> Option<&Value> {
        self.document
            .get("packs")?
            .as_array()?
            .iter()
            .find(|entry| entry.get("package_id").and_then(Value::as_str) == Some(package_id))
    }

    /// `display`, as written, when the file declares it.
    pub fn display(&self) -> Option<&Value> {
        self.document.get("display")
    }
}

/// R-11: [`read_standard_pack_root`] for a reader of `ui.json`. `ui` says whether the directory
/// holds `ui.json`, which makes a directory a game root as the other root files do: a directory
/// whose only root file is `ui.json` is `bundle_missing` (the bundle's file, the whole document).
/// The file itself is checked by [`parse_standard_pack_ui`] and never refuses the root; every
/// other answer is [`read_standard_pack_root`]'s.
pub fn read_standard_pack_root_with_ui(
    files: StandardPackRootBytes<'_>,
    ui: bool,
    game: &str,
) -> Result<Option<StandardPackRoot>, StandardPackRootIssue> {
    match read_standard_pack_root(files, game)? {
        None if ui => Err(StandardPackRootIssue::new(
            STANDARD_PACK_BUNDLE_FILE,
            StandardPackProblem::BundleMissing,
            "",
        )),
        root => Ok(root),
    }
}

/// `ui.json`'s bytes, checked for the game directory `game` against the game root's `registry`.
/// Every violation is one issue at its JSON pointer (empty for the whole document), in document
/// order, at most [`MAX_STANDARD_PACK_UI_ISSUES`]:
///
/// - `ui_out_of_limits`: the file is larger than [`MAX_STANDARD_PACK_UI_BYTES`] (the whole
///   document), or a list, a map, a word or a text is outside its limit (the `MAX_PACK_UI_*`
///   constants);
/// - `malformed`: no JSON or no object (the whole document), a member repeated in one object, a
///   member that is unknown, missing or not of its type, a `schema_version` other than
///   [`STANDARD_PACK_UI_SCHEMA_VERSION`], an aspect ratio not of the form `<w>:<h>`;
/// - `game_mismatch`: `game` is not the directory's;
/// - `package_not_listed`: a pack's `package_id` is not a package of the registry;
/// - `ui_duplicate`: a group id, a pack's `package_id`, a risk word of one pack or an aspect ratio
///   listed twice;
/// - `ui_group_unknown`: a pack's `group` is not an id of `groups`;
/// - `ui_parameter_undeclared`: a name under a pack's `parameters` that the package does not
///   declare; until R-10 brings package-level declarations (A.7.2), every name.
pub fn parse_standard_pack_ui(
    bytes: &[u8],
    game: &str,
    registry: &StandardPackRegistry,
) -> Result<StandardPackUi, Vec<StandardPackRootIssue>> {
    let mut check = Check {
        registry,
        issues: Vec::new(),
    };
    if bytes.len() > MAX_STANDARD_PACK_UI_BYTES {
        check.issue(StandardPackProblem::UiOutOfLimits, "");
        return Err(check.issues);
    }
    let mut repeated = Vec::new();
    let Some(document) = read_document(bytes, &mut repeated) else {
        check.issue(StandardPackProblem::Malformed, "");
        return Err(check.issues);
    };
    for pointer in repeated {
        check.issue(StandardPackProblem::Malformed, pointer);
    }
    check.document(&document, game);
    match document {
        Value::Object(document) if check.issues.is_empty() => Ok(StandardPackUi { document }),
        _ => Err(check.issues),
    }
}

/// The JSON document of `bytes` with its members in the file's order, and in `repeated` the
/// pointer of every member an object repeats (the last one is kept, as `serde_json` keeps it);
/// `None` when the bytes are no JSON document.
fn read_document(bytes: &[u8], repeated: &mut Vec<String>) -> Option<Value> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let document = Tracked {
        pointer: String::new(),
        repeated,
    }
    .deserialize(&mut deserializer)
    .ok()?;
    deserializer.end().ok()?;
    Some(document)
}

/// A JSON value at `pointer` whose repeated members are recorded.
struct Tracked<'r> {
    pointer: String,
    repeated: &'r mut Vec<String>,
}

impl<'de> DeserializeSeed<'de> for Tracked<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Tracked<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        // No member of the file is a number: any number is refused as not of its type.
        Ok(Number::from_f64(value).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let Tracked { pointer, repeated } = self;
        let mut items = Vec::new();
        while let Some(item) = access.next_element_seed(Tracked {
            pointer: format!("{pointer}/{}", items.len()),
            repeated: &mut *repeated,
        })? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let Tracked { pointer, repeated } = self;
        let mut members = Map::new();
        while let Some(name) = access.next_key::<String>()? {
            let at = at(&pointer, &name);
            let value = access.next_value_seed(Tracked {
                pointer: at.clone(),
                repeated: &mut *repeated,
            })?;
            if members.contains_key(&name) {
                repeated.push(at);
            }
            members.insert(name, value);
        }
        Ok(Value::Object(members))
    }
}

/// The checks of one document: the issues found so far.
struct Check<'a> {
    registry: &'a StandardPackRegistry,
    issues: Vec<StandardPackRootIssue>,
}

impl Check<'_> {
    fn issue(&mut self, problem: StandardPackProblem, pointer: impl Into<String>) {
        if self.issues.len() < MAX_STANDARD_PACK_UI_ISSUES {
            self.issues.push(StandardPackRootIssue::new(
                STANDARD_PACK_UI_FILE,
                problem,
                pointer,
            ));
        }
    }

    /// An object of the members `required` and `optional`: an unknown member and a missing one
    /// are `malformed` at its pointer, a value that is no object at the value's.
    fn object<'v>(
        &mut self,
        value: &'v Value,
        pointer: &str,
        required: &[&str],
        optional: &[&str],
    ) -> Option<&'v Map<String, Value>> {
        let Some(object) = value.as_object() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return None;
        };
        for name in object.keys() {
            if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
                self.issue(StandardPackProblem::Malformed, at(pointer, name));
            }
        }
        for name in required {
            if !object.contains_key(*name) {
                self.issue(StandardPackProblem::Malformed, at(pointer, name));
            }
        }
        Some(object)
    }

    /// A list of `min..=max` items.
    fn list<'v>(&mut self, value: &'v Value, pointer: &str, min: usize, max: usize) -> &'v [Value] {
        let Some(items) = value.as_array() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return &[];
        };
        if !(min..=max).contains(&items.len()) {
            self.issue(StandardPackProblem::UiOutOfLimits, pointer);
        }
        items
    }

    /// An object of `min..=max` members of any name, each name 1 to `name_bytes` bytes.
    fn map<'v>(
        &mut self,
        value: &'v Value,
        pointer: &str,
        (min, max): (usize, usize),
        name_bytes: usize,
    ) -> Option<&'v Map<String, Value>> {
        let Some(members) = value.as_object() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return None;
        };
        if !(min..=max).contains(&members.len()) {
            self.issue(StandardPackProblem::UiOutOfLimits, pointer);
        }
        for name in members.keys() {
            if name.is_empty() || name.len() > name_bytes {
                self.issue(StandardPackProblem::UiOutOfLimits, at(pointer, name));
            }
        }
        Some(members)
    }

    /// A string of 1 to `max` bytes, never read further.
    fn string<'v>(&mut self, value: &'v Value, pointer: &str, max: usize) -> Option<&'v str> {
        let Some(text) = value.as_str() else {
            self.issue(StandardPackProblem::Malformed, pointer);
            return None;
        };
        if text.is_empty() || text.len() > max {
            self.issue(StandardPackProblem::UiOutOfLimits, pointer);
        }
        Some(text)
    }

    /// A text: 1 to [`MAX_PACK_UI_LOCALES`] locales, each a string of 1 to `max` bytes.
    fn text(&mut self, value: &Value, pointer: &str, max: usize) {
        if let Some(locales) = self.map(
            value,
            pointer,
            (1, MAX_PACK_UI_LOCALES),
            MAX_PACK_UI_LOCALE_BYTES,
        ) {
            for (locale, text) in locales {
                self.string(text, &at(pointer, locale), max);
            }
        }
    }

    fn document(&mut self, value: &Value, game: &str) {
        let Some(document) = self.object(
            value,
            "",
            &["schema_version", "game", "groups", "packs"],
            &["display"],
        ) else {
            return;
        };
        if let Some(version) = document.get("schema_version")
            && version.as_str() != Some(STANDARD_PACK_UI_SCHEMA_VERSION)
        {
            self.issue(StandardPackProblem::Malformed, "/schema_version");
        }
        match document.get("game").map(Value::as_str) {
            Some(None) => self.issue(StandardPackProblem::Malformed, "/game"),
            Some(Some(declared)) if declared != game => {
                self.issue(StandardPackProblem::GameMismatch, "/game");
            }
            _ => {}
        }
        let groups = document
            .get("groups")
            .map(|groups| self.groups(groups))
            .unwrap_or_default();
        if let Some(packs) = document.get("packs") {
            self.packs(packs, &groups);
        }
        if let Some(display) = document.get("display") {
            self.display(display);
        }
    }

    /// `groups`: each `{id, title}`, the ids unique. Answers every id written.
    fn groups<'v>(&mut self, value: &'v Value) -> BTreeSet<&'v str> {
        let mut ids = BTreeSet::new();
        for (index, group) in self
            .list(value, "/groups", 0, MAX_PACK_UI_GROUPS)
            .iter()
            .enumerate()
        {
            let pointer = format!("/groups/{index}");
            let Some(group) = self.object(group, &pointer, &["id", "title"], &[]) else {
                continue;
            };
            if let Some(id) = group.get("id") {
                let at = format!("{pointer}/id");
                if let Some(id) = self.string(id, &at, MAX_PACK_UI_WORD_BYTES)
                    && !ids.insert(id)
                {
                    self.issue(StandardPackProblem::UiDuplicate, at);
                }
            }
            if let Some(title) = group.get("title") {
                self.text(title, &format!("{pointer}/title"), MAX_PACK_UI_TITLE_BYTES);
            }
        }
        ids
    }

    /// `packs`: each `{package_id, title, summary, group?, risk?, parameters?}`, one entry per
    /// package of the registry.
    fn packs(&mut self, value: &Value, groups: &BTreeSet<&str>) {
        let mut listed = BTreeSet::new();
        for (index, pack) in self
            .list(value, "/packs", 0, MAX_PACK_UI_PACKS)
            .iter()
            .enumerate()
        {
            let pointer = format!("/packs/{index}");
            let Some(pack) = self.object(
                pack,
                &pointer,
                &["package_id", "title", "summary"],
                &["group", "risk", "parameters"],
            ) else {
                continue;
            };
            let member = |name: &str| format!("{pointer}/{name}");
            if let Some(package_id) = pack.get("package_id") {
                match package_id.as_str() {
                    None => self.issue(StandardPackProblem::Malformed, member("package_id")),
                    Some(package_id) => {
                        if self.registry.get(package_id).is_none() {
                            self.issue(StandardPackProblem::PackageNotListed, member("package_id"));
                        } else if !listed.insert(package_id) {
                            self.issue(StandardPackProblem::UiDuplicate, member("package_id"));
                        }
                    }
                }
            }
            if let Some(title) = pack.get("title") {
                self.text(title, &member("title"), MAX_PACK_UI_TITLE_BYTES);
            }
            if let Some(summary) = pack.get("summary") {
                self.text(summary, &member("summary"), MAX_PACK_UI_SUMMARY_BYTES);
            }
            if let Some(group) = pack.get("group")
                && let Some(group) = self.string(group, &member("group"), MAX_PACK_UI_WORD_BYTES)
                && !groups.contains(group)
            {
                self.issue(StandardPackProblem::UiGroupUnknown, member("group"));
            }
            if let Some(risk) = pack.get("risk") {
                self.words(risk, &member("risk"), MAX_PACK_UI_RISK_WORDS);
            }
            if let Some(parameters) = pack.get("parameters") {
                self.parameters(parameters, &member("parameters"));
            }
        }
    }

    /// A list of up to `max` distinct words.
    fn words(&mut self, value: &Value, pointer: &str, max: usize) {
        let mut seen = BTreeSet::new();
        for (index, word) in self.list(value, pointer, 0, max).iter().enumerate() {
            let at = format!("{pointer}/{index}");
            if let Some(word) = self.string(word, &at, MAX_PACK_UI_WORD_BYTES)
                && !seen.insert(word)
            {
                self.issue(StandardPackProblem::UiDuplicate, at);
            }
        }
    }

    /// A pack's `parameters`: each name a parameter the package declares, with `{label,
    /// options?}`; `options` names each option with its text.
    fn parameters(&mut self, value: &Value, pointer: &str) {
        let Some(parameters) = self.map(
            value,
            pointer,
            (0, MAX_PACK_UI_PARAMETERS),
            MAX_PACK_UI_WORD_BYTES,
        ) else {
            return;
        };
        for (name, parameter) in parameters {
            let pointer = at(pointer, name);
            // A.7.2: package-level parameter declarations arrive with R-10. Until then no package
            // declares a parameter, so every name is an undeclared reference.
            self.issue(StandardPackProblem::UiParameterUndeclared, pointer.as_str());
            let Some(parameter) = self.object(parameter, &pointer, &["label"], &["options"]) else {
                continue;
            };
            if let Some(label) = parameter.get("label") {
                self.text(label, &format!("{pointer}/label"), MAX_PACK_UI_TITLE_BYTES);
            }
            if let Some(options) = parameter.get("options") {
                let options_at = format!("{pointer}/options");
                if let Some(options) = self.map(
                    options,
                    &options_at,
                    (0, MAX_PACK_UI_OPTIONS),
                    MAX_PACK_UI_WORD_BYTES,
                ) {
                    for (option, text) in options {
                        self.text(text, &at(&options_at, option), MAX_PACK_UI_TITLE_BYTES);
                    }
                }
            }
        }
    }

    /// `display`: `{aspect_ratios, safe_area?}`.
    fn display(&mut self, value: &Value) {
        let Some(display) = self.object(value, "/display", &["aspect_ratios"], &["safe_area"])
        else {
            return;
        };
        if let Some(ratios) = display.get("aspect_ratios") {
            let mut seen = BTreeSet::new();
            for (index, ratio) in self
                .list(
                    ratios,
                    "/display/aspect_ratios",
                    1,
                    MAX_PACK_UI_ASPECT_RATIOS,
                )
                .iter()
                .enumerate()
            {
                let at = format!("/display/aspect_ratios/{index}");
                match ratio.as_str() {
                    Some(ratio) if aspect_ratio(ratio) => {
                        if !seen.insert(ratio) {
                            self.issue(StandardPackProblem::UiDuplicate, at);
                        }
                    }
                    _ => self.issue(StandardPackProblem::Malformed, at),
                }
            }
        }
        if let Some(safe_area) = display.get("safe_area") {
            self.string(safe_area, "/display/safe_area", MAX_PACK_UI_WORD_BYTES);
        }
    }
}

/// Whether `text` is of the form `<w>:<h>`: two positive decimal integers of 1 to
/// [`MAX_PACK_UI_ASPECT_DIGITS`] digits without a leading zero. Only the form is checked; the
/// Runtime uses neither number.
fn aspect_ratio(text: &str) -> bool {
    text.split_once(':').is_some_and(|(width, height)| {
        [width, height].iter().all(|side| {
            (1..=MAX_PACK_UI_ASPECT_DIGITS).contains(&side.len())
                && side.bytes().all(|byte| byte.is_ascii_digit())
                && !side.starts_with('0')
        })
    })
}

/// The pointer of the member `name` of the object at `pointer` (RFC 6901).
fn at(pointer: &str, name: &str) -> String {
    format!("{pointer}/{}", name.replace('~', "~0").replace('/', "~1"))
}

#[cfg(test)]
#[path = "standard_pack_ui_tests.rs"]
mod tests;
