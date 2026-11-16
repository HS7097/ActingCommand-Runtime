// SPDX-License-Identifier: AGPL-3.0-only

//! Lab recording core (Workflow #336): the recording state kept in the Lab state directory
//! next to the `record` command file, the six mark families with their mark-time
//! self-test, steps by serial index with transitions and remediation, the `--record`
//! frame and click hooks, the per-instance recording lock (R20), and the `linear_steps`
//! package `record stop` generates with its self-checks (L4). The ActingLab CLI only parses
//! arguments and prints these results; nothing here touches the ledger.

// Workflow #449 REC-ARCH: a recording ended as material only, reopened later.
pub(crate) mod archive;
// #463 S2-13: the stored frames just before and just after one step's press, read on request.
pub(crate) mod around;
// #463 S2-5: boxes drawn on a step's primary frame, a region to recognize by or a region to press.
pub(crate) mod boxes;
pub(crate) mod container;
pub(crate) mod crosscheck;
// #463 R2: a screen that ends the business another way after the last press, taken by a stopped
// recording from a frame.
pub(crate) mod ending;
pub(crate) mod follow;
pub(crate) mod frames;
pub(crate) mod generate;
// #463 S1-7: what a recording holds (the instance's scheduling pause), as its session header keeps
// it, and what `record status` shows of it.
pub(crate) mod hold;
// #463 R4: an optional step a stopped recording inserts after one of its steps, from a frame.
pub(crate) mod inserted;
pub(crate) mod lock;
pub(crate) mod marks;
pub(crate) mod model;
// #463 S2-2: a copy of a stored frame with a press and the declared boxes drawn on it.
pub(crate) mod preview;
// #463 S2-1: everything a recording still lacks before `record stop` can make its package.
pub(crate) mod ready;
// #463 S2-2: each step of a recording as the screen before, the press and the screen after.
pub(crate) mod review;
// #463 R1: a step that chooses in a list of fixed slots by the name read in each slot.
pub(crate) mod select;
pub(crate) mod steps;
pub(crate) mod store;

pub use archive::{
    ArchiveCopyFrames, record_archive, record_archive_copy, record_archive_warnings, record_reopen,
};
pub use around::{
    RecordFramesFrame, RecordFramesOutcome, RecordFramesPress, RecordFramesPressFrame,
    RecordFramesRequest, RecordFramesWarning, record_frames,
};
pub use hold::{
    LAB_RECORDING_HOLD_PREFIX, LAB_RECORDING_WARNING_HOLD_UNREAD,
    LAB_RECORDING_WARNING_INSTANCE_NOT_HELD, LAB_RECORDING_WARNING_NO_RUNTIME_LISTENING,
    LAB_RECORDING_WARNING_REPLACED_HEADER_UNREAD, RecordingHold, RecordingHoldFound,
    RecordingHoldState, RecordingHoldView, record_hold_status,
};
pub use lock::RecordingLock;
pub use model::*;
pub use ready::{
    PressPointOnly, ReadyDetails, ReadyItem, ReadyMark, ReadyOptionConflict, ReadySources,
    ReadyWarningItem, RecordReadyOutcome,
};
pub use review::{
    RecordReviewAfter, RecordReviewBetween, RecordReviewFrame, RecordReviewOutcome,
    RecordReviewPress, RecordReviewRequest, RecordReviewStep,
};
