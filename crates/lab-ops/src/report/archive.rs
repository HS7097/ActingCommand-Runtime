// SPDX-License-Identifier: AGPL-3.0-only

//! The archive of `report export` (Workflow #467 RPT-1; `contracts/problem-report.md`, "The
//! archive" and "What is never in it"): the masks every text part passes through, the zip, and
//! its one file written through a partial file moved into place, never over another.

use crate::codes::{ActingctlCode, ReportWriteStep};
use actingcommand_contract::outcome::{IoOp, Outcome};
use serde_json::Value;
use std::cell::Cell;
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
/// Fix round 1, item 11: a name shorter than this is not masked (it would mask parts of ordinary
/// words).
pub const SHORTEST_NAME: usize = 3;
/// The end of central directory record a zip closes with, no comment: counted once a whole
/// archive ([`packed_size`]).
pub(super) const END_BYTES: u64 = 22;

/// One name the masks replace, its characters lowercased, and its mark.
#[derive(Debug)]
struct Name {
    folded: Vec<char>,
    mark: &'static str,
}

/// The masks of every text part of an archive (fix round 1, item 11; `contracts/problem-report.md`
/// section 7). The segment after a `Users` folder of a path, with one or more separators of
/// either kind on each side (a doubly escaped path too), becomes [`USER_MARK`]. The user's name
/// becomes [`USER_MARK`] and each of the computer's names (its NetBIOS name, its DNS host name,
/// its full DNS name) [`COMPUTER_MARK`] wherever they stand, inside a longer word too, without
/// regard to case, when the name has at least [`SHORTEST_NAME`] characters; the longest name is
/// tried first, and a mark already written is never searched again. It counts what it masked.
#[derive(Debug)]
pub struct Mask {
    names: Vec<Name>,
    applied: Cell<usize>,
}

impl Mask {
    /// The masks of the user's name `user` and the computer's names `computer` (a name of fewer
    /// than [`SHORTEST_NAME`] characters is left out).
    pub fn new(user: Option<&str>, computer: &[&str]) -> Self {
        let mut names = Vec::new();
        let mut add = |name: &str, mark: &'static str| {
            let trimmed = name.trim();
            if trimmed.chars().count() >= SHORTEST_NAME {
                names.push(Name {
                    folded: trimmed.chars().flat_map(char::to_lowercase).collect(),
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
        }
    }

    /// The masks of this process's user (`USERNAME`, else `USER`) and of this computer: its
    /// NetBIOS name (`COMPUTERNAME`), and its DNS host name and its full DNS name as the
    /// operating system gives them (`GetComputerNameExW` on Windows; none elsewhere).
    pub fn of_this_process() -> Self {
        let variable = |name: &str| std::env::var(name).ok();
        let user = variable("USERNAME").or_else(|| variable("USER"));
        let found = [
            variable("COMPUTERNAME"),
            actingcommand_host_metrics::dns_host_name().ok(),
            actingcommand_host_metrics::dns_full_name().ok(),
        ];
        let computer = found
            .iter()
            .flatten()
            .map(String::as_str)
            .collect::<Vec<_>>();
        Self::new(user.as_deref(), &computer)
    }

    /// How many masks were applied so far.
    pub fn applied(&self) -> usize {
        self.applied.get()
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
    /// mark: the longest that does.
    fn name_at(&self, text: &str, start: usize) -> Option<(usize, &'static str)> {
        self.names.iter().find_map(|name| {
            let mut wanted = name.folded.iter();
            for (offset, character) in text[start..].char_indices() {
                for folded in character.to_lowercase() {
                    if wanted.next() != Some(&folded) {
                        return None;
                    }
                }
                if wanted.as_slice().is_empty() {
                    return Some((start + offset + character.len_utf8(), name.mark));
                }
            }
            None
        })
    }

    /// Every string of `value` masked, its members' names too.
    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => {
                for item in items {
                    self.value(item);
                }
            }
            Value::Object(members) => {
                let taken = std::mem::take(members);
                for (name, mut member) in taken {
                    self.value(&mut member);
                    members.insert(self.text(&name), member);
                }
            }
            _ => {}
        }
    }
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
