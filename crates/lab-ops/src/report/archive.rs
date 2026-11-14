// SPDX-License-Identifier: AGPL-3.0-only

//! The archive of `report export` (Workflow #467 RPT-1; `contracts/problem-report.md`, "The
//! archive" and "What is never in it"): the masks every text part passes through, the zip, and
//! its one file written through a partial file moved into place, never over another.

use crate::codes::ActingctlCode;
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
/// A name shorter than this is not masked as a word (it would mask ordinary words).
const SHORTEST_NAME: usize = 2;

/// The masks of every text part of an archive: a path's `\Users\<name>\` segment becomes
/// `\Users\<user>\`, and the user's and the computer's names, as whole words in any ASCII case,
/// become `<user>` and `<computer>`. It counts what it masked.
#[derive(Debug)]
pub struct Mask {
    user: Option<String>,
    computer: Option<String>,
    applied: Cell<usize>,
}

impl Mask {
    /// The masks of `user` and `computer` (a name of fewer than two characters is left out).
    pub fn new(user: Option<&str>, computer: Option<&str>) -> Self {
        let name = |name: Option<&str>| {
            name.map(str::trim)
                .filter(|name| name.chars().count() >= SHORTEST_NAME)
                .map(str::to_owned)
        };
        Self {
            user: name(user),
            computer: name(computer),
            applied: Cell::new(0),
        }
    }

    /// The masks of this process's user and computer (`USERNAME` or `USER`, `COMPUTERNAME`).
    pub fn of_this_process() -> Self {
        let variable = |name: &str| std::env::var(name).ok();
        let user = variable("USERNAME").or_else(|| variable("USER"));
        let computer = variable("COMPUTERNAME");
        Self::new(user.as_deref(), computer.as_deref())
    }

    /// How many masks were applied so far.
    pub fn applied(&self) -> usize {
        self.applied.get()
    }

    fn count(&self) {
        self.applied.set(self.applied.get() + 1);
    }

    /// `text` masked.
    pub fn text(&self, text: &str) -> String {
        let mut masked = self.profile_segments(text);
        for (name, mark) in [(&self.user, USER_MARK), (&self.computer, COMPUTER_MARK)] {
            if let Some(name) = name {
                masked = self.words(&masked, name, mark);
            }
        }
        masked
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

    /// `text` with the segment after every `\Users\` or `/Users\` (any case, either separator)
    /// as [`USER_MARK`].
    fn profile_segments(&self, text: &str) -> String {
        const USERS: &[u8] = b"users";
        let bytes = text.as_bytes();
        let separator = |byte: u8| byte == b'\\' || byte == b'/';
        let ends =
            |byte: u8| separator(byte) || matches!(byte, b'"' | b'\'' | b'\n' | b'\r' | b'\t');
        let mut masked = String::with_capacity(text.len());
        let mut copied = 0;
        let mut index = 0;
        while index + USERS.len() + 2 <= bytes.len() {
            let after = index + USERS.len() + 1;
            if separator(bytes[index])
                && bytes[index + 1..after].eq_ignore_ascii_case(USERS)
                && separator(bytes[after])
            {
                let start = after + 1;
                let end = bytes[start..]
                    .iter()
                    .position(|byte| ends(*byte))
                    .map_or(bytes.len(), |offset| start + offset);
                if end > start && text.get(start..end) != Some(USER_MARK) {
                    masked.push_str(&text[copied..start]);
                    masked.push_str(USER_MARK);
                    copied = end;
                    index = end;
                    self.count();
                    continue;
                }
            }
            index += 1;
        }
        masked.push_str(&text[copied..]);
        masked
    }

    /// `text` with every whole-word occurrence of `name` (ASCII case aside) as `mark`.
    fn words(&self, text: &str, name: &str, mark: &str) -> String {
        let bytes = text.as_bytes();
        let needle = name.as_bytes();
        let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80;
        let mut masked = String::with_capacity(text.len());
        let mut copied = 0;
        let mut index = 0;
        while index + needle.len() <= bytes.len() {
            let end = index + needle.len();
            if bytes[index..end].eq_ignore_ascii_case(needle)
                && (index == 0 || !word(bytes[index - 1]))
                && (end == bytes.len() || !word(bytes[end]))
                && text.is_char_boundary(index)
                && text.is_char_boundary(end)
            {
                masked.push_str(&text[copied..index]);
                masked.push_str(mark);
                copied = end;
                index = end;
                self.count();
                continue;
            }
            index += 1;
        }
        masked.push_str(&text[copied..]);
        masked
    }
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

/// `report_write_failed` for the step `op` on `path`, caused by the OS failure.
fn write_failed(path: &Path, op: IoOp, error: &io::Error) -> Outcome {
    Outcome::new(ActingctlCode::ReportWriteFailed)
        .path(path)
        .caused_by(Outcome::from_io_error(error, op))
}

/// `bytes` written into `directory` (made when it is missing) as `name`, through
/// `<name>.partial` moved into place; a file of that name already there is never replaced.
pub(super) fn write(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, Outcome> {
    fs::create_dir_all(directory)
        .map_err(|error| write_failed(directory, IoOp::CreateDir, &error))?;
    let path = directory.join(name);
    let partial = directory.join(format!("{name}.partial"));
    if path.exists() {
        let error = io::Error::from(io::ErrorKind::AlreadyExists);
        return Err(write_failed(path.as_path(), IoOp::Create, &error));
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(partial.as_path())
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(|error| write_failed(partial.as_path(), IoOp::Write, &error))?;
    fs::rename(partial.as_path(), path.as_path()).map_err(|error| {
        write_failed(path.as_path(), IoOp::Rename, &error).from_path(partial.as_path())
    })?;
    Ok(path)
}
