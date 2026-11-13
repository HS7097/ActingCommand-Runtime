// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #444 NOTICES-1 (since 0.12.58; register §1.334; `contracts/package-reference.md`,
//! "Notices"): the rules of a standard-package bundle's notices directory, `notices/`, which holds
//! the texts a package's author must ship with it. A bundle zip may carry the directory at its
//! root, beside `packs/`; the installer will place it in the game root as it is, and no reader of
//! the Runtime opens it.
//!
//! This module is the one statement of those rules, pure functions and constants with no file
//! system and no zip, so that every reader judges the directory alike:
//!
//! - the path rule, [`check_notice_path`]: one path below the directory (its depth, each
//!   segment's length, alphabet and first byte, a safe path segment, a file name's suffix);
//! - the size rule, [`check_notice_size`]: one file's size;
//! - the set rule, [`check_notice_listing`]: a whole listing of the directory's files, each path
//!   with its size (the count, each path and size, the total, no two paths equal under ASCII case
//!   folding, no file whose path is another path's directory, no two directories whose names are
//!   equal under ASCII case folding and not byte for byte);
//! - the pack rule, [`check_notice_pack_ids`]: a bundle that carries the directory lists no pack
//!   named like it, in any ASCII case;
//! - the text rule, [`check_notice_text`]: one file's bytes.
//!
//! Each rule answers the first thing it finds broken as a [`NoticeIssue`]: the word of the
//! vocabulary `standard_pack_problem`, what it concerns ([`NoticeSubject`]) and, where the rule has
//! a bound, the bound and the value found ([`NoticeBound`]), which `standard_pack_root_invalid`
//! carries as `limit_count` and `count` or `limit_bytes` and `actual_bytes`. A caller hands the
//! rules what it found, in its own terms: a zip's member table with the declared sizes
//! ([`notice_relative_path`] takes a member's name apart), a directory on disk, or a placed game
//! root; each turns an issue into its own outcome with [`NoticeIssue::member`] and
//! [`NoticeIssue::package_id`] and states no rule itself.
//!
//! The installer's `place` calls them from NOTICES-2 (with the `package` interface's revision
//! 10; register §1.335), on a zip's member table; NOTICES-3 (`package bundle --notices`,
//! `standard-pack check`, `resource verify`) is still to come.

use crate::package::safe_source_path;
use crate::standard_pack::{STANDARD_PACK_BUNDLE_FILE, StandardPackProblem};
use std::collections::{BTreeMap, BTreeSet};

/// The directory's name at the root of a bundle zip and of a game root, in exact lower case: a
/// member `Notices/…` is not of it.
pub const STANDARD_PACK_NOTICES_DIRECTORY: &str = "notices";
/// The most segments a path below the directory has: `<name>` or `<directory>/<name>`.
pub const MAX_NOTICE_DEPTH: usize = 2;
/// The longest segment of a path below the directory, in bytes; a segment has one byte at least.
pub const MAX_NOTICE_SEGMENT_BYTES: usize = 64;
/// The suffixes a file name may end in after its last dot, compared in any ASCII case. A name
/// without a dot has no suffix and is accepted too; a dot in a directory's name is no suffix.
pub const NOTICE_FILE_SUFFIXES: [&str; 4] = ["md", "txt", "tsv", "csv"];
/// The most files the directory holds.
pub const MAX_NOTICE_FILES: usize = 64;
/// The largest file of the directory, in bytes (2 MiB); a file has one byte at least.
pub const MAX_NOTICE_FILE_BYTES: u64 = 2_097_152;
/// The most bytes the directory's files hold together (8 MiB).
pub const MAX_NOTICE_TOTAL_BYTES: u64 = 8_388_608;

/// What a broken notices rule concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeSubject {
    /// The directory as a whole: the count of its files, or their total bytes.
    Directory,
    /// One path below the directory, `/`-separated, as the caller gave it.
    Path(String),
    /// A pack of the bundle, by its `package_id`, named like the directory.
    Package(String),
}

/// The bound a broken notices rule has and the value found: the bound broken, the least or the
/// most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeBound {
    /// A bound on a number of files or segments: the values `limit_count` and `count`.
    Count { limit: u64, found: u64 },
    /// A bound in bytes: the values `limit_bytes` and `actual_bytes`.
    Bytes { limit: u64, found: u64 },
}

/// One broken notices rule: its word of the vocabulary `standard_pack_problem` (one of the
/// `notice_*` words), what it concerns, and the bound with the value found where the rule has
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeIssue {
    pub problem: StandardPackProblem,
    pub subject: NoticeSubject,
    pub bound: Option<NoticeBound>,
}

impl NoticeIssue {
    fn new(
        problem: StandardPackProblem,
        subject: NoticeSubject,
        bound: Option<NoticeBound>,
    ) -> Self {
        Self {
            problem,
            subject,
            bound,
        }
    }

    fn at_path(problem: StandardPackProblem, path: &str, bound: Option<NoticeBound>) -> Self {
        Self::new(problem, NoticeSubject::Path(path.to_owned()), bound)
    }

    /// The path the issue names, `/`-separated and relative to the root that holds the directory
    /// (a bundle zip's root, a game root): `notices` for the directory as a whole,
    /// `notices/<path>` for a path below it, and `bundle.json` for a pack, whose id
    /// [`Self::package_id`] gives.
    pub fn member(&self) -> String {
        match &self.subject {
            NoticeSubject::Directory => STANDARD_PACK_NOTICES_DIRECTORY.to_owned(),
            NoticeSubject::Path(path) => format!("{STANDARD_PACK_NOTICES_DIRECTORY}/{path}"),
            NoticeSubject::Package(_) => STANDARD_PACK_BUNDLE_FILE.to_owned(),
        }
    }

    /// The `package_id` of the pack the issue names, for a pack named like the directory.
    pub fn package_id(&self) -> Option<&str> {
        if let NoticeSubject::Package(package_id) = &self.subject {
            Some(package_id.as_str())
        } else {
            None
        }
    }
}

impl std::fmt::Display for NoticeIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: standard_pack_problem={}",
            self.member(),
            self.problem
        )?;
        if let Some(package_id) = self.package_id() {
            write!(formatter, " package_id={package_id}")?;
        }
        match self.bound {
            Some(NoticeBound::Count { limit, found }) => {
                write!(formatter, " limit_count={limit} count={found}")
            }
            Some(NoticeBound::Bytes { limit, found }) => {
                write!(formatter, " limit_bytes={limit} actual_bytes={found}")
            }
            None => Ok(()),
        }
    }
}

/// One file of a listing of the directory: its path below the directory, `/`-separated, and its
/// size in bytes (a zip member's declared size, a file's length on disk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoticeEntry<'a> {
    pub path: &'a str,
    pub bytes: u64,
}

/// The path below the directory of `member`, a `/`-separated path relative to a bundle zip's root
/// or a game root, when `member` lies in the directory: what follows `notices/`, in exact lower
/// case, so a member `Notices/…` is not of the directory. A zip's directory entry (a name that ends
/// in `/`) is no file, and its caller skips it before it lists the files.
pub fn notice_relative_path(member: &str) -> Option<&str> {
    member
        .strip_prefix(STANDARD_PACK_NOTICES_DIRECTORY)
        .and_then(|rest| rest.strip_prefix('/'))
}

/// The path rule: `path`, `/`-separated below the directory, has at most two segments
/// (`notice_depth`, with the bound and the depth found); each segment is 1 to 64 bytes
/// (`notice_name`, with the bound broken and the length found) of `[A-Za-z0-9._-]`, begins with a
/// letter or a digit and is a segment [`safe_source_path`] accepts, that is with no trailing dot
/// and no device name (`notice_name`); and the file name, its last segment, has no dot or ends
/// after its last dot in one of [`NOTICE_FILE_SUFFIXES`] in any ASCII case (`notice_name`). So
/// `a.tar.md` is a file of the suffix `md` and accepted, `a.md.html` one of the suffix `html` and
/// refused, and a trailing dot (`NOTICE.`) refused. The first rule broken, in this order, is the
/// answer.
pub fn check_notice_path(path: &str) -> Result<(), NoticeIssue> {
    let depth = path.split('/').count();
    if depth > MAX_NOTICE_DEPTH {
        return Err(NoticeIssue::at_path(
            StandardPackProblem::NoticeDepth,
            path,
            Some(NoticeBound::Count {
                limit: MAX_NOTICE_DEPTH as u64,
                found: depth as u64,
            }),
        ));
    }
    for segment in path.split('/') {
        let length_bound = if segment.is_empty() {
            Some(1)
        } else if segment.len() > MAX_NOTICE_SEGMENT_BYTES {
            Some(MAX_NOTICE_SEGMENT_BYTES as u64)
        } else {
            None
        };
        if let Some(limit) = length_bound {
            return Err(NoticeIssue::at_path(
                StandardPackProblem::NoticeName,
                path,
                Some(NoticeBound::Bytes {
                    limit,
                    found: segment.len() as u64,
                }),
            ));
        }
        if !notice_segment(segment) {
            return Err(NoticeIssue::at_path(
                StandardPackProblem::NoticeName,
                path,
                None,
            ));
        }
    }
    let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
    if !notice_file_suffix(name) {
        return Err(NoticeIssue::at_path(
            StandardPackProblem::NoticeName,
            path,
            None,
        ));
    }
    Ok(())
}

/// The size rule: a file of `bytes` bytes at `path` holds 1 to [`MAX_NOTICE_FILE_BYTES`]
/// (`notice_bytes`, with the bound broken, 1 or the most, and the size found). The set rule
/// applies it to each declared size; a reader that read a file through a limit applies it to the
/// bytes it read.
pub fn check_notice_size(path: &str, bytes: u64) -> Result<(), NoticeIssue> {
    let limit = match bytes {
        0 => 1,
        1..=MAX_NOTICE_FILE_BYTES => return Ok(()),
        _ => MAX_NOTICE_FILE_BYTES,
    };
    Err(NoticeIssue::at_path(
        StandardPackProblem::NoticeBytes,
        path,
        Some(NoticeBound::Bytes {
            limit,
            found: bytes,
        }),
    ))
}

/// The set rule, over a whole listing of the directory's files, judged in the byte order of the
/// paths (then of the sizes), so the answer does not depend on the order a zip or a directory
/// lists them in. In this order: at most [`MAX_NOTICE_FILES`] files (`notice_count`, the
/// directory, with the bound and the count); each path by [`check_notice_path`] and its size by
/// [`check_notice_size`]; at most [`MAX_NOTICE_TOTAL_BYTES`] together (`notice_total_bytes`, the
/// directory, with the bound and the total); no two paths equal under ASCII case folding
/// (`notice_name` at the later path); no file whose path, in any ASCII case, is another path's
/// directory (`notice_name` at that file); and (Workflow #444 NOTICES-2) no two paths whose
/// directories' names are equal under ASCII case folding and not byte for byte, `A/x.md` beside
/// `a/y.md` (`notice_name` at the later path), since a file system that folds case would hold
/// them in one directory, which then no longer lists as the listing does. One directory named
/// alike by several paths is kept. An empty listing keeps every rule.
pub fn check_notice_listing(entries: &[NoticeEntry<'_>]) -> Result<(), NoticeIssue> {
    if entries.len() > MAX_NOTICE_FILES {
        return Err(NoticeIssue::new(
            StandardPackProblem::NoticeCount,
            NoticeSubject::Directory,
            Some(NoticeBound::Count {
                limit: MAX_NOTICE_FILES as u64,
                found: entries.len() as u64,
            }),
        ));
    }
    let mut sorted = entries.to_vec();
    sorted.sort_unstable_by_key(|entry| (entry.path, entry.bytes));
    for entry in &sorted {
        check_notice_path(entry.path)?;
        check_notice_size(entry.path, entry.bytes)?;
    }
    // Each size is within its bound and there are at most 64, so the sum cannot overflow.
    let total = sorted.iter().map(|entry| entry.bytes).sum::<u64>();
    if total > MAX_NOTICE_TOTAL_BYTES {
        return Err(NoticeIssue::new(
            StandardPackProblem::NoticeTotalBytes,
            NoticeSubject::Directory,
            Some(NoticeBound::Bytes {
                limit: MAX_NOTICE_TOTAL_BYTES,
                found: total,
            }),
        ));
    }
    let mut folded = BTreeSet::new();
    for entry in &sorted {
        if !folded.insert(entry.path.to_ascii_lowercase()) {
            return Err(NoticeIssue::at_path(
                StandardPackProblem::NoticeName,
                entry.path,
                None,
            ));
        }
    }
    // Every path has at most two segments here, so a directory is the first of two.
    let directories = sorted
        .iter()
        .filter_map(|entry| entry.path.split_once('/'))
        .map(|(directory, _)| directory.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    for entry in &sorted {
        if directories.contains(&entry.path.to_ascii_lowercase()) {
            return Err(NoticeIssue::at_path(
                StandardPackProblem::NoticeName,
                entry.path,
                None,
            ));
        }
    }
    // Each directory's name, folded, with the spelling of its first path in byte order: a later
    // path that spells it otherwise is the later of two.
    let mut spelled = BTreeMap::new();
    for entry in &sorted {
        if let Some((directory, _)) = entry.path.split_once('/')
            && *spelled
                .entry(directory.to_ascii_lowercase())
                .or_insert(directory)
                != directory
        {
            return Err(NoticeIssue::at_path(
                StandardPackProblem::NoticeName,
                entry.path,
                None,
            ));
        }
    }
    Ok(())
}

/// The pack rule: a bundle that carries the directory (`carries_notices`) lists no pack whose
/// `package_id` equals `notices` in any ASCII case (`notice_name`, naming the first such pack of
/// `package_ids`). A bundle without the directory may name a pack so: the name is not reserved by
/// the bundle index ([`crate::reserved_package_id`]), which would narrow what a reader of the
/// earlier `package` revisions accepts.
pub fn check_notice_pack_ids<'a>(
    carries_notices: bool,
    package_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), NoticeIssue> {
    if !carries_notices {
        return Ok(());
    }
    if let Some(package_id) = package_ids
        .into_iter()
        .find(|package_id| package_id.eq_ignore_ascii_case(STANDARD_PACK_NOTICES_DIRECTORY))
    {
        return Err(NoticeIssue::new(
            StandardPackProblem::NoticeName,
            NoticeSubject::Package(package_id.to_owned()),
            None,
        ));
    }
    Ok(())
}

/// The text rule: the bytes of the file at `path` are strict UTF-8 and hold none of U+0000 to
/// U+0008, U+000B, U+000E to U+001F and U+007F (`notice_not_text`). TAB, LF, FF and CR are
/// allowed, so any line ending is (LF, CRLF, CR, mixed); a byte-order mark at the start is
/// accepted, and nothing else is refused: a U+FEFF after the start is an ordinary character. The
/// rule judges the bytes and never changes one. It does not measure them: [`check_notice_size`]
/// does.
pub fn check_notice_text(path: &str, bytes: &[u8]) -> Result<(), NoticeIssue> {
    // Every refused character is ASCII, and in UTF-8 a byte below 0x80 is always an ASCII
    // character, so the bytes can be searched once the whole is known to be UTF-8.
    if std::str::from_utf8(bytes).is_ok() && !bytes.iter().copied().any(refused_control) {
        return Ok(());
    }
    Err(NoticeIssue::at_path(
        StandardPackProblem::NoticeNotText,
        path,
        None,
    ))
}

/// A segment's alphabet, first byte and safety (its length is checked before).
fn notice_segment(segment: &str) -> bool {
    segment
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && safe_source_path(segment)
}

/// Whether a file name has no dot, or one of [`NOTICE_FILE_SUFFIXES`] after its last dot.
fn notice_file_suffix(name: &str) -> bool {
    name.rsplit_once('.').is_none_or(|(_, suffix)| {
        NOTICE_FILE_SUFFIXES
            .iter()
            .any(|accepted| accepted.eq_ignore_ascii_case(suffix))
    })
}

/// The control characters a notice file may not hold: U+0000 to U+0008, U+000B, U+000E to U+001F
/// and U+007F.
const fn refused_control(byte: u8) -> bool {
    matches!(byte, 0x00..=0x08 | 0x0b | 0x0e..=0x1f | 0x7f)
}

#[cfg(test)]
mod tests {
    //! Workflow #444 NOTICES-1 (contract gate row "#444 NOTICES-1"): each rule of the notices
    //! directory at its edges, with placeholder names (a notice file is `NOTICE.md`).

    use super::*;

    fn at_path(
        problem: StandardPackProblem,
        path: &str,
        bound: Option<NoticeBound>,
    ) -> Result<(), NoticeIssue> {
        Err(NoticeIssue {
            problem,
            subject: NoticeSubject::Path(path.to_owned()),
            bound,
        })
    }

    fn name_refused(path: &str) -> Result<(), NoticeIssue> {
        at_path(StandardPackProblem::NoticeName, path, None)
    }

    fn entry(path: &str, bytes: u64) -> NoticeEntry<'_> {
        NoticeEntry { path, bytes }
    }

    #[test]
    fn notices_path_rule_holds_each_bound_at_its_edge() {
        use StandardPackProblem::{NoticeDepth, NoticeName};

        // The directory is `notices` at the root, in exact lower case.
        assert_eq!(notice_relative_path("notices/NOTICE.md"), Some("NOTICE.md"));
        assert_eq!(
            notice_relative_path("notices/texts/NOTICE.md"),
            Some("texts/NOTICE.md")
        );
        for member in [
            "Notices/NOTICE.md",
            "NOTICES/NOTICE.md",
            "notices",
            "noticesx/NOTICE.md",
            "packs/fixture.pack.a/notices/NOTICE.md",
        ] {
            assert_eq!(notice_relative_path(member), None, "{member}");
        }

        // Depth 2 is kept, depth 3 is not.
        assert_eq!(check_notice_path("NOTICE.md"), Ok(()));
        assert_eq!(check_notice_path("texts/NOTICE.md"), Ok(()));
        assert_eq!(
            check_notice_path("texts/more/NOTICE.md"),
            at_path(
                NoticeDepth,
                "texts/more/NOTICE.md",
                Some(NoticeBound::Count { limit: 2, found: 3 })
            )
        );

        // A segment of 64 bytes is kept, one of 65 is not, a file name or a directory's name.
        let name_64 = format!("{}.md", "n".repeat(61));
        let name_65 = format!("{}.md", "n".repeat(62));
        let directory_64 = format!("{}/NOTICE.md", "d".repeat(64));
        let directory_65 = format!("{}/NOTICE.md", "d".repeat(65));
        assert_eq!(name_64.len(), MAX_NOTICE_SEGMENT_BYTES);
        assert_eq!(check_notice_path(&name_64), Ok(()));
        assert_eq!(check_notice_path(&directory_64), Ok(()));
        for path in [&name_65, &directory_65] {
            assert_eq!(
                check_notice_path(path),
                at_path(
                    NoticeName,
                    path,
                    Some(NoticeBound::Bytes {
                        limit: 64,
                        found: 65
                    })
                ),
                "{path}"
            );
        }
        // An empty segment breaks the least length.
        for path in ["", "/NOTICE.md", "texts/"] {
            assert_eq!(
                check_notice_path(path),
                at_path(
                    NoticeName,
                    path,
                    Some(NoticeBound::Bytes { limit: 1, found: 0 })
                ),
                "{path:?}"
            );
        }

        // The first byte is a letter or a digit: not a dot, a dash or an underscore.
        for path in [
            ".NOTICE.md",
            "-NOTICE.md",
            "_NOTICE.md",
            ".texts/NOTICE.md",
            "-texts/NOTICE.md",
            "_texts/NOTICE.md",
            ".",
            "..",
        ] {
            assert_eq!(check_notice_path(path), name_refused(path), "{path}");
        }
        for path in ["0NOTICE.md", "9", "Texts/NOTICE.md", "texts/n_o-t.i.md"] {
            assert_eq!(check_notice_path(path), Ok(()), "{path}");
        }
        // The alphabet is `[A-Za-z0-9._-]`.
        for path in [
            "NOTICE file.md",
            "NOTICE+1.md",
            "texts\\NOTICE.md",
            "NOT\u{c9}.md",
            "NOTICE:1.md",
            "NOTICE~1.md",
        ] {
            assert_eq!(check_notice_path(path), name_refused(path), "{path}");
        }
        // A safe path segment: no device name, in any case, and no trailing dot.
        for path in [
            "NUL.md",
            "con",
            "Com1.txt",
            "lpt9",
            "aux/NOTICE.md",
            "NOTICE.",
            "NOTICE.md.",
            "texts./NOTICE.md",
        ] {
            assert_eq!(check_notice_path(path), name_refused(path), "{path}");
        }

        // Every accepted suffix, in lower and in upper case, and no suffix at all.
        for suffix in NOTICE_FILE_SUFFIXES {
            for name in [
                format!("NOTICE.{suffix}"),
                format!("NOTICE.{}", suffix.to_ascii_uppercase()),
                format!("texts/NOTICE.{suffix}"),
            ] {
                assert_eq!(check_notice_path(&name), Ok(()), "{name}");
            }
        }
        for path in ["NOTICE.Md", "NOTICE", "texts/NOTICE"] {
            assert_eq!(check_notice_path(path), Ok(()), "{path}");
        }
        // Any other suffix is refused.
        for path in [
            "NOTICE.html",
            "NOTICE.js",
            "NOTICE.ps1",
            "NOTICE.png",
            "NOTICE.mdx",
            "NOTICE.json",
            "texts/NOTICE.exe",
        ] {
            assert_eq!(check_notice_path(path), name_refused(path), "{path}");
        }
        // The last suffix decides; a dot in a directory's name is no suffix.
        assert_eq!(check_notice_path("a.tar.md"), Ok(()));
        assert_eq!(check_notice_path("a.md.html"), name_refused("a.md.html"));
        assert_eq!(check_notice_path("v1.2/NOTICE.md"), Ok(()));

        // An issue names its member below the directory.
        let issue = check_notice_path("a.md.html").unwrap_err();
        assert_eq!(issue.member(), "notices/a.md.html");
        assert_eq!(issue.package_id(), None);
    }

    #[test]
    fn notices_set_rule_holds_each_bound_at_its_edge() {
        use StandardPackProblem::{NoticeBytes, NoticeCount, NoticeDepth, NoticeTotalBytes};

        assert_eq!(check_notice_listing(&[]), Ok(()));

        // 64 files are kept, 65 are not.
        let names = (0..=MAX_NOTICE_FILES)
            .map(|index| format!("NOTICE-{index:02}.md"))
            .collect::<Vec<_>>();
        let listing = |count: usize| {
            names[..count]
                .iter()
                .map(|path| entry(path, 1))
                .collect::<Vec<_>>()
        };
        assert_eq!(check_notice_listing(&listing(64)), Ok(()));
        let issue = check_notice_listing(&listing(65)).unwrap_err();
        assert_eq!(
            issue,
            NoticeIssue {
                problem: NoticeCount,
                subject: NoticeSubject::Directory,
                bound: Some(NoticeBound::Count {
                    limit: 64,
                    found: 65
                }),
            }
        );
        assert_eq!(issue.member(), "notices");
        assert_eq!(
            issue.to_string(),
            "notices: standard_pack_problem=notice_count limit_count=64 count=65"
        );

        // A file of 0 bytes, of 1 byte, of the bound and of the bound plus one.
        for (bytes, expected) in [
            (
                0,
                at_path(
                    NoticeBytes,
                    "NOTICE.md",
                    Some(NoticeBound::Bytes { limit: 1, found: 0 }),
                ),
            ),
            (1, Ok(())),
            (MAX_NOTICE_FILE_BYTES, Ok(())),
            (
                MAX_NOTICE_FILE_BYTES + 1,
                at_path(
                    NoticeBytes,
                    "NOTICE.md",
                    Some(NoticeBound::Bytes {
                        limit: 2_097_152,
                        found: 2_097_153,
                    }),
                ),
            ),
        ] {
            assert_eq!(
                check_notice_listing(&[entry("NOTICE.md", bytes)]),
                expected,
                "{bytes}"
            );
            assert_eq!(check_notice_size("NOTICE.md", bytes), expected, "{bytes}");
        }

        // The total at the bound (four files of 2 MiB are 8 MiB) and one byte past it.
        let full = ["a.md", "b.md", "c.md", "d.md"].map(|path| entry(path, MAX_NOTICE_FILE_BYTES));
        assert_eq!(check_notice_listing(&full), Ok(()));
        let mut over = full.to_vec();
        over.push(entry("e.md", 1));
        let issue = check_notice_listing(&over).unwrap_err();
        assert_eq!(
            issue,
            NoticeIssue {
                problem: NoticeTotalBytes,
                subject: NoticeSubject::Directory,
                bound: Some(NoticeBound::Bytes {
                    limit: 8_388_608,
                    found: 8_388_609
                }),
            }
        );
        assert_eq!(
            issue.to_string(),
            "notices: standard_pack_problem=notice_total_bytes limit_bytes=8388608 \
             actual_bytes=8388609"
        );

        // Two paths equal under ASCII case folding: refused at the later one in byte order.
        for (listing, at) in [
            ([entry("notice.MD", 1), entry("NOTICE.md", 1)], "notice.MD"),
            (
                [entry("texts/NOTICE.md", 1), entry("Texts/NOTICE.md", 1)],
                "texts/NOTICE.md",
            ),
            ([entry("NOTICE.md", 1), entry("NOTICE.md", 1)], "NOTICE.md"),
        ] {
            assert_eq!(check_notice_listing(&listing), name_refused(at), "{at}");
        }
        // A file and a directory of one name, in any case: refused at the file.
        for (listing, at) in [
            ([entry("texts", 1), entry("texts/NOTICE.md", 1)], "texts"),
            ([entry("texts/NOTICE.md", 1), entry("TEXTS", 1)], "TEXTS"),
        ] {
            assert_eq!(check_notice_listing(&listing), name_refused(at), "{at}");
        }
        assert_eq!(
            check_notice_listing(&[entry("texts/NOTICE.md", 1), entry("NOTICE", 1)]),
            Ok(())
        );
        // Workflow #444 NOTICES-2: two directories whose names are equal under ASCII case folding
        // and not byte for byte, refused at the later path in byte order; one directory in one
        // case twice is kept; a file beside a directory of its name in another case stays the
        // earlier rule's, refused at the file.
        for (listing, at) in [
            (
                [entry("A/NOTICE.md", 1), entry("a/OTHER.md", 1)],
                "a/OTHER.md",
            ),
            (
                [entry("texts/OTHER.md", 1), entry("Texts/NOTICE.md", 1)],
                "texts/OTHER.md",
            ),
        ] {
            assert_eq!(check_notice_listing(&listing), name_refused(at), "{at}");
        }
        assert_eq!(
            check_notice_listing(&[entry("texts/NOTICE.md", 1), entry("texts/OTHER.md", 1)]),
            Ok(())
        );
        assert_eq!(
            check_notice_listing(&[entry("A", 1), entry("a/NOTICE.md", 1)]),
            name_refused("A")
        );

        // Each path is held to the path rule, and the first path in byte order decides, whatever
        // the listing's order.
        let mixed = [
            entry("z/y/NOTICE.md", 1),
            entry("NOTICE.html", 1),
            entry("A.md", 0),
        ];
        let mut reversed = mixed;
        reversed.reverse();
        for listing in [&mixed, &reversed] {
            assert_eq!(
                check_notice_listing(listing),
                at_path(
                    NoticeBytes,
                    "A.md",
                    Some(NoticeBound::Bytes { limit: 1, found: 0 })
                )
            );
        }
        assert_eq!(
            check_notice_listing(&mixed[..2]),
            name_refused("NOTICE.html")
        );
        assert_eq!(
            check_notice_listing(&mixed[..1]),
            at_path(
                NoticeDepth,
                "z/y/NOTICE.md",
                Some(NoticeBound::Count { limit: 2, found: 3 })
            )
        );
    }

    #[test]
    fn notices_pack_rule_refuses_a_pack_named_like_the_directory_only_beside_it() {
        let packs = |package_id: &'static str| ["fixture.pack.a", package_id, "fixture.pack.b"];
        for package_id in ["notices", "Notices", "NOTICES"] {
            let issue = check_notice_pack_ids(true, packs(package_id)).unwrap_err();
            assert_eq!(
                issue,
                NoticeIssue {
                    problem: StandardPackProblem::NoticeName,
                    subject: NoticeSubject::Package(package_id.to_owned()),
                    bound: None,
                }
            );
            assert_eq!(issue.member(), "bundle.json");
            assert_eq!(issue.package_id(), Some(package_id));
            assert_eq!(
                issue.to_string(),
                format!("bundle.json: standard_pack_problem=notice_name package_id={package_id}")
            );
            // Without the directory the name is free.
            assert_eq!(check_notice_pack_ids(false, packs(package_id)), Ok(()));
        }
        for package_id in ["notices.a", "notice", "fixture.notices"] {
            assert_eq!(
                check_notice_pack_ids(true, packs(package_id)),
                Ok(()),
                "{package_id}"
            );
        }
    }

    #[test]
    fn notices_text_rule_accepts_text_and_refuses_the_control_characters() {
        let not_text = |path: &str| at_path(StandardPackProblem::NoticeNotText, path, None);

        let accepted: [(&str, &[u8]); 9] = [
            ("plain ASCII", b"Placeholder notice.\nSecond line.\n"),
            (
                "UTF-8 without a byte-order mark",
                "Placeholder \u{e9} \u{3a9}\n".as_bytes(),
            ),
            (
                "UTF-8 with a byte-order mark",
                b"\xef\xbb\xbfPlaceholder \xc3\xa9\n",
            ),
            ("CRLF", b"one\r\ntwo\r\n"),
            ("CR", b"one\rtwo\r"),
            ("mixed line ends", b"one\r\ntwo\nthree\rfour"),
            ("TAB and FF", b"key\tvalue\x0cnext page\n"),
            ("no line end", b"x"),
            ("a byte-order mark after the start", b"one\xef\xbb\xbftwo"),
        ];
        for (case, bytes) in accepted {
            assert_eq!(check_notice_text("NOTICE.md", bytes), Ok(()), "{case}");
        }

        // Each refused control character at the start, the middle and the end; every other ASCII
        // byte, TAB, LF, FF and CR among them, is text.
        let refused = (0x00..=0x08)
            .chain([0x0b])
            .chain(0x0e..=0x1f)
            .chain([0x7f])
            .collect::<Vec<u8>>();
        assert_eq!(refused.len(), 29);
        for byte in 0x00..=0x7f_u8 {
            for bytes in [[byte, b'a', b'b'], [b'a', byte, b'b'], [b'a', b'b', byte]] {
                let expected = if refused.contains(&byte) {
                    not_text("NOTICE.md")
                } else {
                    Ok(())
                };
                assert_eq!(
                    check_notice_text("NOTICE.md", &bytes),
                    expected,
                    "{byte:#04x} in {bytes:?}"
                );
            }
        }

        // An invalid UTF-8 sequence anywhere.
        let invalid: [&[u8]; 6] = [
            b"\xff",
            b"one\xc3",
            b"\xc0\xaf",
            b"\xed\xa0\x80",
            b"\xef\xbb",
            b"one\x80two",
        ];
        for bytes in invalid {
            assert_eq!(
                check_notice_text("texts/NOTICE.md", bytes),
                not_text("texts/NOTICE.md"),
                "{bytes:?}"
            );
        }
    }
}
