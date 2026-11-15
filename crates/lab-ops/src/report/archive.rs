// SPDX-License-Identifier: AGPL-3.0-only

//! The archive of `report export` (Workflow #467 RPT-1; `contracts/problem-report.md`, "The
//! archive" and "What is never in it"): the masks every text part passes through, the zip, and
//! its one file written through a partial file moved into place, never over another.

use crate::codes::{ActingctlCode, ReportWriteStep};
use actingcommand_contract::outcome::{IoOp, KeyKind, Outcome, key_kind};
use serde_json::Value;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use zip::result::ZipError;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipWriter};

/// What a masked user name becomes.
pub const USER_MARK: &str = "<user>";
/// What a masked computer name becomes.
pub const COMPUTER_MARK: &str = "<computer>";
/// Fix round 2, item 1 (the coordinator's ruling, which replaces item 11 of round 1): a name of
/// at least this many characters is replaced wherever it stands, inside a longer word too; a
/// shorter one only where it stands alone.
pub const SHORTEST_ANYWHERE: usize = 5;
/// The end of central directory record a zip closes with, no comment: counted once a whole
/// archive ([`packed_size`]).
pub(super) const END_BYTES: u64 = 22;
/// Fix round 2, item 1: the members whose string values are never replaced, in what the report
/// writes outside an outcome's `values`: the ids the Runtime gives and the references made of
/// them, the hashes, the codes and the words of closed vocabularies, and the report's own fixed
/// texts. An outcome's `values` are told by the outcome key table instead ([`kept_value`]).
const KEPT_MEMBERS: &[&str] = &[
    // The ids the Runtime gives, and the references made of them.
    "request_id",
    "run_id",
    "event_id",
    "correlation_id",
    "causation_id",
    "instance_id",
    "artifact_id",
    "frame_id",
    "action_id",
    "step_action_id",
    "physical_action_id",
    "recognition_id",
    "lease_id",
    "owner_epoch",
    "ref",
    "frame_ref",
    // The hashes.
    "sha256",
    "input_sha256",
    "digest",
    "commit",
    "commit_sha",
    "tree_sha",
    // The codes and the words of closed vocabularies.
    "code",
    "failure_code",
    "ended_with",
    "type",
    "event_type",
    "severity",
    "state",
    "kind",
    "category",
    "relation",
    "schema",
    "schema_version",
    "payload_schema",
    "view",
    "module",
    "media_type",
    "why",
    "from",
    "phase",
    "role",
    "result",
    "conclusion",
    "match_mode",
    "op",
    "stopped",
    // The report's own fixed texts.
    "program",
    "lab_ops",
    "display",
    "ended_with_description",
    "secrets",
    "names_not_looked_for",
    "part",
    "slice",
];

/// One name the masks replace, its characters lowercased, its mark, and whether it is replaced
/// inside a longer word too (at least [`SHORTEST_ANYWHERE`] characters).
#[derive(Debug)]
struct Name {
    folded: Vec<char>,
    mark: &'static str,
    anywhere: bool,
}

/// The masks of every text part of an archive (fix round 2, item 1, the coordinator's ruling;
/// `contracts/problem-report.md` section 7). The segment after a `Users` folder of a path, with
/// one or more separators of either kind on each side (a doubly escaped path too), becomes
/// [`USER_MARK`], whatever its length. The user's name becomes [`USER_MARK`] and each of the
/// computer's names (its NetBIOS name, its DNS host name, its full DNS name) [`COMPUTER_MARK`],
/// without regard to case: a name of at least [`SHORTEST_ANYWHERE`] characters wherever it
/// stands, a shorter one only where it stands alone (between characters that are no letter and
/// no digit, or at the text's ends). The longest name is tried first, and a mark already written
/// is never searched again. A value of JSON is masked in its free text only ([`Self::value`]).
/// It counts what it masked, and it names the names it could not learn (fix round 2, item 6).
#[derive(Debug)]
pub struct Mask {
    names: Vec<Name>,
    applied: Cell<usize>,
    not_looked_for: Vec<&'static str>,
}

impl Mask {
    /// The masks of the user's name `user` and the computer's names `computer` (an empty name is
    /// left out).
    pub fn new(user: Option<&str>, computer: &[&str]) -> Self {
        let mut names = Vec::new();
        let mut add = |name: &str, mark: &'static str| {
            let folded = name
                .trim()
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<Vec<_>>();
            if !folded.is_empty() {
                names.push(Name {
                    anywhere: name.trim().chars().count() >= SHORTEST_ANYWHERE,
                    folded,
                    mark,
                });
            }
        };
        if let Some(user) = user {
            add(user, USER_MARK);
        }
        for &name in computer {
            add(name, COMPUTER_MARK);
        }
        names.sort_by_key(|name| std::cmp::Reverse(name.folded.len()));
        Self {
            names,
            applied: Cell::new(0),
            not_looked_for: Vec::new(),
        }
    }

    /// The masks of this process's user (`USERNAME`, else `USER`) and of this computer: its
    /// NetBIOS name (`COMPUTERNAME`), and its DNS host name and its full DNS name as the
    /// operating system gives them (`GetComputerNameExW` on Windows; none elsewhere). Fix round
    /// 2, item 6: a name that is not set, or that the system cannot be asked for, is named
    /// (`user_name`, `netbios_name`, `dns_host_name`, `dns_full_name`): it was not looked for and
    /// may stand in the report, and the manifest says so ([`Self::not_looked_for`]).
    pub fn of_this_process() -> Self {
        let mut not_looked_for = Vec::new();
        let mut learned = |found: Option<String>, what: &'static str| {
            if found.is_none() {
                not_looked_for.push(what);
            }
            found
        };
        let variable = |name: &str| std::env::var(name).ok();
        let user = learned(
            variable("USERNAME").or_else(|| variable("USER")),
            "user_name",
        );
        let found = [
            learned(variable("COMPUTERNAME"), "netbios_name"),
            learned(
                actingcommand_host_metrics::dns_host_name().ok(),
                "dns_host_name",
            ),
            learned(
                actingcommand_host_metrics::dns_full_name().ok(),
                "dns_full_name",
            ),
        ];
        let computer = found
            .iter()
            .flatten()
            .map(String::as_str)
            .collect::<Vec<_>>();
        Self {
            not_looked_for,
            ..Self::new(user.as_deref(), &computer)
        }
    }

    /// How many masks were applied so far.
    pub fn applied(&self) -> usize {
        self.applied.get()
    }

    /// Fix round 2, item 6: the names this mask could not learn, which it never looked for.
    pub fn not_looked_for(&self) -> &[&'static str] {
        &self.not_looked_for
    }

    fn count(&self) {
        self.applied.set(self.applied.get() + 1);
    }

    /// `text` masked, in one pass from its start: at each character a `Users` folder's segment,
    /// else the longest name that stands there, else the character as it is.
    pub fn text(&self, text: &str) -> String {
        let mut masked = String::with_capacity(text.len());
        let mut index = 0;
        while let Some(character) = text[index..].chars().next() {
            if let Some((through, segment_end)) = profile_segment(text, index) {
                masked.push_str(&text[index..through]);
                masked.push_str(USER_MARK);
                self.count();
                index = segment_end;
                continue;
            }
            if let Some((end, mark)) = self.name_at(text, index) {
                masked.push_str(mark);
                self.count();
                index = end;
                continue;
            }
            masked.push(character);
            index += character.len_utf8();
        }
        masked
    }

    /// The end of the name that stands at `start` of `text` without regard to case, and its
    /// mark: the longest that does, a name shorter than [`SHORTEST_ANYWHERE`] only where it
    /// stands alone.
    fn name_at(&self, text: &str, start: usize) -> Option<(usize, &'static str)> {
        self.names.iter().find_map(|name| {
            let end = folded_end(&name.folded, text, start)?;
            (name.anywhere || stands_alone(text, start, end)).then_some((end, name.mark))
        })
    }

    /// The text `text` of the member `member` (a member of the report's own data, outside an
    /// outcome's `values`) masked as [`Self::value`] masks it: kept as it is when the member
    /// holds an id, a hash, a code or a word of a closed vocabulary, or the text is shaped as a
    /// hash or an id; masked otherwise. The summary page writes its values through it.
    pub fn member(&self, member: &str, text: &str) -> String {
        if kept_value(Some(member), false, text, &BTreeSet::new()) {
            text.to_owned()
        } else {
            self.text(text)
        }
    }

    /// Fix round 2, item 1: the free text of `value` masked; never a member's name, a number, a
    /// boolean, or a string that holds an id, a hash, a code or a word of a closed vocabulary
    /// (told by its member; in an outcome's `values`, by the outcome key table's type of its
    /// key; or by its shape).
    pub fn value(&self, value: &mut Value) {
        self.walk(value, None, false, &BTreeSet::new());
    }

    /// [`Self::value`], where a string that is the name of one of `entries` (the archive's
    /// entries, each named from text that went through this mask; fix round 2, item 2) is kept
    /// as it is, so that the manifest and the summary name each entry by the name it has in the
    /// archive.
    pub(super) fn value_naming(&self, value: &mut Value, entries: &BTreeSet<String>) {
        self.walk(value, None, false, entries);
    }

    /// The value `value` of an outcome's key `key` masked as [`Self::value_naming`] masks an
    /// outcome's `values` (the summary page writes an outcome's values through it).
    pub(super) fn outcome_value(&self, key: &str, value: &mut Value, entries: &BTreeSet<String>) {
        self.walk(value, Some(key), true, entries);
    }

    /// `value` masked ([`kept_value`]): `member` the member that holds it, `keyed` whether it
    /// lies in an outcome's `values` (an object that has a string `code` holds them in its
    /// member `values`; a record of a `records` value lies there too).
    fn walk(
        &self,
        value: &mut Value,
        member: Option<&str>,
        keyed: bool,
        entries: &BTreeSet<String>,
    ) {
        match value {
            Value::String(text) => {
                if !kept_value(member, keyed, text, entries) {
                    *text = self.text(text);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, member, keyed, entries);
                }
            }
            Value::Object(members) => {
                let outcome = members.get("code").is_some_and(Value::is_string);
                for (name, item) in members.iter_mut() {
                    let values = keyed || (outcome && name == "values");
                    self.walk(item, Some(name.as_str()), values, entries);
                }
            }
            _ => {}
        }
    }
}

/// The end of `folded` (a name's characters, lowercased) where it stands at `start` of `text`,
/// without regard to case.
fn folded_end(folded: &[char], text: &str, start: usize) -> Option<usize> {
    let mut wanted = folded.iter();
    for (offset, character) in text[start..].char_indices() {
        for lowered in character.to_lowercase() {
            if wanted.next() != Some(&lowered) {
                return None;
            }
        }
        if wanted.as_slice().is_empty() {
            return Some(start + offset + character.len_utf8());
        }
    }
    None
}

/// Whether `text[start..end]` stands alone: the characters before and after it, where there are
/// any, are no letter and no digit.
fn stands_alone(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
}

/// Whether a value of the outcome key table's type `kind` is free text the masks search: a
/// name, a path, a URL, foreign text kept as evidence (or a list of them). Every other type is
/// an id, a hash, a commit, a version, a code, a location, a word of a vocabulary, a token, a
/// pointer, a number, a time or a boolean.
fn free_text(kind: KeyKind) -> bool {
    match kind {
        KeyKind::Name | KeyKind::Path | KeyKind::Url | KeyKind::Evidence => true,
        KeyKind::List(item) => free_text(*item),
        _ => false,
    }
}

/// Fix round 2, item 1: whether the string `text` of the member `member` is kept as it is. It
/// is when it names an entry of the archive (`entries`), when it is shaped as a hash or an id the
/// Runtime gives ([`hash_or_id`]), when it lies in an outcome's `values` (`keyed`) under a key
/// the outcome key table does not type as free text ([`free_text`]), and, elsewhere, when its
/// member is one of [`KEPT_MEMBERS`]. Every other string is free text.
fn kept_value(member: Option<&str>, keyed: bool, text: &str, entries: &BTreeSet<String>) -> bool {
    if entries.contains(text) || hash_or_id(text) {
        return true;
    }
    let Some(member) = member else {
        return false;
    };
    if keyed {
        return key_kind(member).is_some_and(|kind| !free_text(kind));
    }
    KEPT_MEMBERS.contains(&member)
}

/// Whether `text` is lowercase hexadecimal of `length` digits.
fn hex_of(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `text` is an id the Runtime gives: lowercase words joined by `_`, then `_` and 32
/// lowercase hexadecimal digits (`request_…`, `run_…`, `evt_…`, `artifact_…`, `epoch_…`).
fn typed_id(text: &str) -> bool {
    text.rsplit_once('_').is_some_and(|(prefix, digits)| {
        hex_of(digits, 32)
            && !prefix.is_empty()
            && prefix
                .split('_')
                .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
    })
}

/// Whether `text` is shaped as a hash or an id the Runtime gives, wherever it stands: `sha256:`
/// and 64 hexadecimal digits; 40 or 64 hexadecimal digits (a commit, a tree, a digest); an id
/// ([`typed_id`]); a material reference, `<sequence>:<event id>:<artifact id>`.
fn hash_or_id(text: &str) -> bool {
    if let Some(digits) = text.strip_prefix("sha256:") {
        return hex_of(digits, 64);
    }
    if hex_of(text, 40) || hex_of(text, 64) || typed_id(text) {
        return true;
    }
    let parts = text.split(':').collect::<Vec<_>>();
    parts.len() == 3
        && !parts[0].is_empty()
        && parts[0].bytes().all(|byte| byte.is_ascii_digit())
        && typed_id(parts[1])
        && typed_id(parts[2])
}

/// A path separator, of either kind.
fn separator(byte: u8) -> bool {
    byte == b'\\' || byte == b'/'
}

/// The `Users` folder's segment of a path that starts at `start` of `text`: one or more
/// separators, `users` in any ASCII case, one or more separators (so a doubly escaped
/// `\\Users\\` too), then the segment up to the next separator, quote or line break. Its start
/// and its end, when the segment is not empty and not already [`USER_MARK`].
fn profile_segment(text: &str, start: usize) -> Option<(usize, usize)> {
    const USERS: &[u8] = b"users";
    let bytes = text.as_bytes();
    let run = |from: usize| {
        bytes[from..]
            .iter()
            .position(|byte| !separator(*byte))
            .map_or(bytes.len(), |offset| from + offset)
    };
    let word = run(start);
    if word == start || bytes.len() < word + USERS.len() {
        return None;
    }
    if !bytes[word..word + USERS.len()].eq_ignore_ascii_case(USERS) {
        return None;
    }
    let segment = run(word + USERS.len());
    if segment == word + USERS.len() {
        return None;
    }
    let ends = |byte: u8| separator(byte) || matches!(byte, b'"' | b'\'' | b'\n' | b'\r' | b'\t');
    let end = bytes[segment..]
        .iter()
        .position(|byte| ends(*byte))
        .map_or(bytes.len(), |offset| segment + offset);
    (end > segment && text.get(segment..end) != Some(USER_MARK)).then_some((segment, end))
}

/// One entry of the archive: its name, its bytes, and whether it is stored as it is (a frame,
/// already compressed) or deflated.
#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub(super) name: String,
    pub(super) bytes: Vec<u8>,
    pub(super) stored: bool,
}

fn zip_io(error: ZipError) -> io::Error {
    match error {
        ZipError::Io(error) => error,
        other => io::Error::other(other.to_string()),
    }
}

/// The zip of `entries`, in their order (no time is written: every entry carries the zip's
/// earliest date).
pub(super) fn zipped(entries: &[Entry]) -> io::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for entry in entries {
        let method = if entry.stored {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        writer
            .start_file(
                entry.name.as_str(),
                FileOptions::default().compression_method(method),
            )
            .map_err(zip_io)?;
        writer.write_all(&entry.bytes)?;
    }
    Ok(writer.finish().map_err(zip_io)?.into_inner())
}

/// Fix round 1, item 8: what `entry` adds to an archive, in the bytes the archive holds it in
/// (its local header, its packed bytes and its central directory record): the zip of it alone
/// less the end record, which an archive has once ([`END_BYTES`]). An archive of entries is the
/// sum of theirs and its end record: the zip writes each entry the same wherever it stands.
pub(super) fn packed_size(entry: &Entry) -> io::Result<u64> {
    let alone = zipped(std::slice::from_ref(entry))?;
    Ok((alone.len() as u64).saturating_sub(END_BYTES))
}

/// `report_write_failed` at the step `step` on `path`, caused by the OS failure of `op`.
pub(super) fn write_failed(
    step: ReportWriteStep,
    path: &Path,
    op: IoOp,
    error: &io::Error,
) -> Outcome {
    Outcome::new(ActingctlCode::ReportWriteFailed)
        .path(path)
        .report_write_step(step)
        .caused_by(Outcome::from_io_error(error, op))
}

/// `bytes` written into `directory` (made when it is missing) as `name`, through
/// `<name>.partial` moved into place; a file of that name already there is never replaced. A
/// partial file this call created and could not finish or move stays where it is, and the
/// failure names it (`retained`; fix round 1, item 9).
pub(super) fn write(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, Outcome> {
    fs::create_dir_all(directory).map_err(|error| {
        write_failed(
            ReportWriteStep::CreateDirectory,
            directory,
            IoOp::CreateDir,
            &error,
        )
    })?;
    let path = directory.join(name);
    let partial = directory.join(format!("{name}.partial"));
    if path.exists() {
        let error = io::Error::from(io::ErrorKind::AlreadyExists);
        return Err(write_failed(
            ReportWriteStep::NameExists,
            path.as_path(),
            IoOp::Create,
            &error,
        ));
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(partial.as_path())
        .map_err(|error| {
            write_failed(
                ReportWriteStep::WritePartial,
                partial.as_path(),
                IoOp::Create,
                &error,
            )
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            write_failed(
                ReportWriteStep::WritePartial,
                partial.as_path(),
                IoOp::Write,
                &error,
            )
            .retained([partial.as_path()])
        })?;
    drop(file);
    fs::rename(partial.as_path(), path.as_path()).map_err(|error| {
        write_failed(
            ReportWriteStep::Rename,
            path.as_path(),
            IoOp::Rename,
            &error,
        )
        .from_path(partial.as_path())
        .retained([partial.as_path()])
    })?;
    Ok(path)
}
