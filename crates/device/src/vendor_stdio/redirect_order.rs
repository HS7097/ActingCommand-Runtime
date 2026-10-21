// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A36a: the order in which the vendor stdio redirect points the process standard
//! targets at its capture files and back, and where its restore takes each Win32 standard handle
//! from. The native session (`imp::RedirectGuard`) runs exactly these orders.
//!
//! A standard descriptor's install (`_dup2(capture, FD)`) closes the handle the descriptor owned.
//! At process start that handle is also the value of the Win32 standard-handle slot, so from the
//! install on that number is free and any file opened anywhere in the process may get it. The
//! restore therefore never writes back a saved number that a standard descriptor owned: it
//! restores both descriptors first, then points each Win32 slot at the handle its restored
//! descriptor owns now, a new handle to the same original object.

/// One of the two redirected standard streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdStream {
    Stdout,
    Stderr,
}

impl StdStream {
    /// The stream's position in the session's per-stream arrays.
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
        }
    }

    /// The stream's CRT standard descriptor.
    pub(crate) const fn fd(self) -> i32 {
        match self {
            Self::Stdout => 1,
            Self::Stderr => 2,
        }
    }
}

/// What one redirect step changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StdTarget {
    /// The CRT standard descriptor. Install: `_dup2(capture, FD)`. Restore: retire the capture
    /// duplicate, then `_dup2(saved, FD)`.
    Descriptor(StdStream),
    /// The Win32 standard-handle slot. Install: `SetStdHandle` to the capture descriptor's own
    /// handle. Restore: `SetStdHandle` from the slot's [`TableSource`].
    Table(StdStream),
}

/// The install order: both descriptors, then both Win32 slots.
pub(crate) const INSTALL_ORDER: [StdTarget; 4] = [
    StdTarget::Descriptor(StdStream::Stdout),
    StdTarget::Descriptor(StdStream::Stderr),
    StdTarget::Table(StdStream::Stdout),
    StdTarget::Table(StdStream::Stderr),
];

/// The restore order: both descriptors first, so that each Win32 slot can then take the handle
/// its restored descriptor owns.
// A36a red proof 1: today's order, the Win32 slots before the descriptors.
pub(crate) const RESTORE_ORDER: [StdTarget; 4] = [
    StdTarget::Table(StdStream::Stdout),
    StdTarget::Table(StdStream::Stderr),
    StdTarget::Descriptor(StdStream::Stdout),
    StdTarget::Descriptor(StdStream::Stderr),
];

/// Where the restore takes a Win32 standard handle from; decided once at acquisition.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableSource {
    /// The slot held the handle this standard descriptor owned. The install closes that number,
    /// so the restore takes the handle the descriptor owns after its own restore.
    Descriptor(StdStream),
    /// The slot held a value no standard descriptor owned. The redirect never closes it, and the
    /// restore writes it back.
    Unowned(isize),
}

/// The restore source of `stream`'s Win32 slot, from the values read at acquisition: the slot's
/// value and the handles FD1 and FD2 owned then. The stream's own descriptor wins when both
/// descriptors owned the slot's handle.
pub(crate) fn table_source(
    stream: StdStream,
    slot: isize,
    stdout_owned: isize,
    stderr_owned: isize,
) -> TableSource {
    // A36a red proof 1: today's restore writes the slot's saved value back.
    let _ = (stream, stdout_owned, stderr_owned);
    TableSource::Unowned(slot)
}
