// SPDX-License-Identifier: AGPL-3.0-only

//! WD-MT (Workflow #421, INV-1119; `contracts/runtime-watchdog.md`, "Logs held open"): the first
//! thing the daemon does, before anything can start a child process, is to make its own stdout
//! and stderr handles not inheritable.
//!
//! On 2026-10-29 the adb server that an adb client of actingd's started had inherited actingd's
//! stdout and stderr, the files the starter redirected them to, and kept both open for its whole
//! life. Windows hands a new process every inheritable handle of its parent, not only the
//! standard handles it is given, so the fix that covers every child at once is the flag on
//! actingd's own handles: the adb clients and the adb server they start, MuMuManager,
//! PowerShell, and anything a vendor library starts with its own `CreateProcess`. Each child
//! actingd starts through Rust's `Command` is given explicit stdout and stderr (pipes or null)
//! already, so none receives the log as its own standard handle either.
//!
//! A failed call is one warning daemon line, `host_stdio_inheritance_failed` with each
//! failure as a `caused_by` link; the start goes on. Nothing is written when both handles were
//! cleared or the process has none.

use crate::codes::ActingdCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_host_metrics::standard_handles::{self as system, StandardStream};

/// The streams the start clears, in this order.
pub(crate) const STREAMS: [StandardStream; 2] = [StandardStream::Output, StandardStream::Error];

/// The operating system's call on this process's own standard handles, the seam the start
/// clears them through.
pub(crate) trait StandardHandleApi {
    /// Makes this process's `stream` handle not inheritable; `Ok(false)` when the process has
    /// no such handle. A failed call is its `foreign_os_error` with its operation.
    fn clear_inheritance(&self, stream: StandardStream) -> Result<bool, Outcome>;
}

/// The operating system itself, through `actingcommand_host_metrics::standard_handles`.
pub(crate) struct SystemStandardHandles;

impl StandardHandleApi for SystemStandardHandles {
    fn clear_inheritance(&self, stream: StandardStream) -> Result<bool, Outcome> {
        system::clear_own_inheritance(stream)
    }
}

/// Clears both handles through `api`, stdout first, each whatever the other answered: `None`
/// when no call failed, else `host_stdio_inheritance_failed` with each failure as a
/// `caused_by` link.
pub(crate) fn apply(api: &dyn StandardHandleApi) -> Option<Outcome> {
    let failures = STREAMS
        .iter()
        .filter_map(|stream| api.clear_inheritance(*stream).err())
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return None;
    }
    Some(failures.into_iter().fold(
        Outcome::new(ActingdCode::HostStdioInheritanceFailed),
        |outcome, failure| outcome.caused_by(failure),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::codes::ContractCode;
    use actingcommand_contract::outcome::{Category, CauseRelation, RawSource, Relation};
    use actingcommand_host_metrics::HostMetricsLocation;
    use std::cell::RefCell;

    /// A fake system that records each call and answers from its table: `Ok(true)` (cleared),
    /// `Ok(false)` (no such handle) or a refusal.
    struct RecordingHandles {
        calls: RefCell<Vec<StandardStream>>,
        refused: Vec<StandardStream>,
        absent: Vec<StandardStream>,
    }

    impl RecordingHandles {
        fn new(refused: &[StandardStream], absent: &[StandardStream]) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                refused: refused.to_vec(),
                absent: absent.to_vec(),
            }
        }
    }

    impl StandardHandleApi for RecordingHandles {
        fn clear_inheritance(&self, stream: StandardStream) -> Result<bool, Outcome> {
            self.calls.borrow_mut().push(stream);
            if self.refused.contains(&stream) {
                let operation = match stream {
                    StandardStream::Output => HostMetricsLocation::ProcessStdoutInheritanceClear,
                    StandardStream::Error => HostMetricsLocation::ProcessStderrInheritanceClear,
                };
                return Err(Outcome::new(ContractCode::ForeignOsError)
                    .raw_source(RawSource::Os)
                    .os_error(6u32)
                    .raw_text("The handle is invalid. (os error 6)")
                    .operation(operation));
            }
            Ok(!self.absent.contains(&stream))
        }
    }

    /// WD-MT: the start clears the inherit flag of its stdout, then its stderr, through the
    /// seam, and writes nothing when both were cleared or are absent. A refused call is one
    /// warning with the refusal as its `caused_by` link, and the other handle is still cleared.
    #[test]
    fn wd_mt_the_start_clears_the_inherit_flag_of_its_stdout_and_stderr() {
        let both = [StandardStream::Output, StandardStream::Error];

        let cleared = RecordingHandles::new(&[], &[]);
        assert!(apply(&cleared).is_none());
        assert_eq!(*cleared.calls.borrow(), both);

        let absent = RecordingHandles::new(&[], &both);
        assert!(
            apply(&absent).is_none(),
            "a process without the handles writes nothing"
        );
        assert_eq!(*absent.calls.borrow(), both);

        let refused = RecordingHandles::new(&[StandardStream::Output], &[]);
        let warning = apply(&refused).expect("a refused call is a warning");
        assert_eq!(*refused.calls.borrow(), both, "stderr is still cleared");
        assert!(warning.is(ActingdCode::HostStdioInheritanceFailed));
        assert_eq!(warning.category(), Category::Warning);
        assert_eq!(warning.causes().len(), 1);
        let cause = &warning.causes()[0];
        assert!(cause.relation() == &Relation::from(CauseRelation::CausedBy));
        assert!(cause.outcome().is(ContractCode::ForeignOsError));
        assert_eq!(
            cause
                .outcome()
                .value("operation")
                .and_then(|value| value.as_str()),
            Some("process_stdout_inheritance_clear")
        );

        let both_refused = RecordingHandles::new(&both, &[]);
        let warning = apply(&both_refused).expect("two refused calls are one warning");
        assert_eq!(warning.causes().len(), 2);
    }
}
