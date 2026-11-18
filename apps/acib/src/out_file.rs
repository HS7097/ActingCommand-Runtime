// SPDX-License-Identifier: AGPL-3.0-only

//! The file `--out` names, the one file `acib` writes (board #479, ACIB-1): looked at before the
//! device is asked, made new, filled with the picture as the service gave it, and removed again
//! when the picture cannot be written whole into it. The file is this program's own to remove:
//! it is made only where nothing was, and a path that names anything already is refused.

use crate::answer::{Failure, Problem};
use std::io::{ErrorKind, Write};
use std::path::Path;

/// Why the picture is not in the file `--out` names.
#[derive(Debug)]
pub enum NotWritten {
    /// No file was made (`out_exists`, `out_unwritable`), so nothing was written.
    NotMade(Failure),
    /// The file was made but the picture could not be written whole into it
    /// (`out_write_failed`): the file was removed, unless `left` (its removal failed too).
    Cut { error: std::io::Error, left: bool },
}

/// Whether a file can be made at `out`, as far as can be told before the device is asked:
/// `out_exists` when `out` names anything already; `out_unwritable` when it cannot be looked
/// at, or when the directory the file is to be made in does not exist or is no directory.
pub fn check(out: &Path) -> Result<(), Failure> {
    match std::fs::symlink_metadata(out) {
        Ok(_) => return Err(Failure::own(Problem::OutExists)),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(Failure::library(Problem::OutUnwritable, &error)),
    }
    match std::fs::metadata(directory_of(out)) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(Failure::own(Problem::OutUnwritable)),
        Err(error) => Err(Failure::library(Problem::OutUnwritable, &error)),
    }
}

/// The directory a file at `out` is made in: its parent, or the current directory for a bare
/// file name (whose parent is the empty path).
fn directory_of(out: &Path) -> &Path {
    match out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Writes `picture` into a new file at `out`: `out_exists` when something is there by then (it
/// is not touched), `out_unwritable` when no file could be made (nothing was written). When the
/// picture cannot be written whole into the file just made, that file is removed again.
pub fn write_new(out: &Path, picture: &[u8]) -> Result<(), NotWritten> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|error| {
            let problem = if error.kind() == ErrorKind::AlreadyExists {
                Problem::OutExists
            } else {
                Problem::OutUnwritable
            };
            NotWritten::NotMade(Failure::library(problem, &error))
        })?;
    fill(file, picture, || std::fs::remove_file(out))
}

/// Writes `picture` whole into `file`, the file this program has just made, and closes it. When
/// the picture cannot be written whole, the file is removed by `remove` once it is closed (an
/// open file is not removed on every system), and the answer says whether a file is left.
fn fill<W: Write>(
    file: W,
    picture: &[u8],
    remove: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), NotWritten> {
    written_whole(file, picture).map_err(|error| NotWritten::Cut {
        left: remove().is_err(),
        error,
    })
}

/// Writes `picture` whole into `file` and flushes it; `file` is closed when this returns.
fn written_whole<W: Write>(mut file: W, picture: &[u8]) -> std::io::Result<()> {
    file.write_all(picture)?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const PICTURE: &[u8] = b"\x89PNG the picture's bytes";

    /// A file that takes `room` bytes and then refuses more, whose flush fails when
    /// `flush_fails`, and that marks when it is closed.
    struct FakeFile<'a> {
        room: usize,
        flush_fails: bool,
        closed: &'a Cell<bool>,
    }

    impl Write for FakeFile<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.room == 0 {
                return Err(std::io::Error::other("no room left"));
            }
            let taken = bytes.len().min(self.room);
            self.room -= taken;
            Ok(taken)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if self.flush_fails {
                Err(std::io::Error::other("flush failed"))
            } else {
                Ok(())
            }
        }
    }

    impl Drop for FakeFile<'_> {
        fn drop(&mut self) {
            self.closed.set(true);
        }
    }

    /// A picture written whole keeps its file and nothing is removed. A picture that cannot be
    /// written whole (the file takes a part of it, or none, or its flush fails) has the file
    /// this program made removed, once, after it is closed, and answers whether a file is left:
    /// only when the removal failed too.
    #[test]
    fn a_picture_not_written_whole_has_its_file_removed() {
        let whole = PICTURE.len();
        let cases = [
            (whole, false, false, None, "written whole"),
            (whole + 10, false, true, None, "whole, no removal asked"),
            (5, false, false, Some(false), "a part written"),
            (0, false, false, Some(false), "nothing written"),
            (whole, true, false, Some(false), "the flush failed"),
            (5, false, true, Some(true), "a part, removal failed"),
            (whole, true, true, Some(true), "flush and removal failed"),
        ];
        for (room, flush_fails, removal_fails, expected, case) in cases {
            let closed = Cell::new(false);
            let removals = Cell::new(0);
            let closed_when_removed = Cell::new(false);
            let file = FakeFile {
                room,
                flush_fails,
                closed: &closed,
            };
            let outcome = fill(file, PICTURE, || {
                removals.set(removals.get() + 1);
                closed_when_removed.set(closed.get());
                if removal_fails {
                    Err(std::io::Error::other("removal refused"))
                } else {
                    Ok(())
                }
            });
            assert!(closed.get(), "{case}: the file is closed");
            match (outcome, expected) {
                (Ok(()), None) => assert_eq!(removals.get(), 0, "{case}: the file is kept"),
                (Err(NotWritten::Cut { left, .. }), Some(left_expected)) => {
                    assert_eq!(removals.get(), 1, "{case}: removed once");
                    assert!(closed_when_removed.get(), "{case}: closed before removed");
                    assert_eq!(left, left_expected, "{case}: whether a file is left");
                }
                (outcome, _) => panic!("{case}: {outcome:?}"),
            }
        }
    }
}
