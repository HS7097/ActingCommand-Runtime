// SPDX-License-Identifier: AGPL-3.0-only

//! The outcome-code drift guard (Workflow #378 model section 9).
//!
//! It reads the workspace as source and links no workspace crate. Its checks, and the slice
//! that turns each on:
//!
//! - G1 registries = catalog fragments, both ways (A1), the ladder class included (#387 D1a);
//! - G2 string channels, G3 `json!` keys, G4 construction, G5 stray literals: every workspace
//!   member starts on [`ALLOW_LIST`]; A2a-A2e remove members as they convert them and name the
//!   channels that stay strings as bridges;
//! - G6 persisted prose (A3): no text member in a contract type a stored record reaches
//!   (`persisted.rs`), outside the G6 bridges of [`ALLOW_LIST`];
//! - G10 review (A8) is not implemented yet;
//! - G7 key table and vocabularies = catalog (A1);
//! - G8 naming and prefixes (A1);
//! - G9 the merged file and the released contract (A1), the ladder class included (#387 D1a).
//!
//! Loading the catalog also checks each code's `ladder_class` (#387 D1a): required, a token of
//! the vocabulary `ladder_class`, and category warning or error for a `stability_warning` code.
//!
//! Crate roots come from each manifest (`[lib]`, `[[bin]]`) and Cargo's defaults. Code under
//! `#[cfg(test)]` or `#[cfg(all(test, ...))]`, `#[test]` functions and files reached only through
//! them are skipped. The guard cannot see source pulled in with `include!`, so a non-test
//! `include!` is itself a G2 finding.

pub mod catalog;
pub mod persisted;
pub mod source;

pub use catalog::Catalog;
pub use source::Check;

use source::{ChannelScope, Finding, KeyLine, ModuleDeclaration};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One allow-list entry for G2-G5. A member entry (`item: None`) covers a whole workspace
/// member; an item entry covers one function of one file (a method named `Type::method` or
/// `Trait::method`, a free function by its name), or one struct or variant field of one file,
/// named `Type.field` or `Enum::Variant.field` (a bridge that a later slice removes).
#[derive(Debug, Clone, Copy)]
pub struct AllowEntry {
    pub path: &'static str,
    pub item: Option<&'static str>,
    pub checks: &'static [Check],
    pub reason: &'static str,
}

const CHANNELS: &[Check] = &[Check::G2, Check::G3, Check::G4, Check::G5];

/// The G2-G5 allow list. A1 seeds every workspace member; each A2 part removes the members the
/// Owners line of model section 11 assigns to it when it converts them. A channel of a converted
/// member that another owner's code fills, or that a later slice replaces, stays a string behind
/// a bridge entry (a function or a field) whose reason names the slice that removes it. The two
/// tools, MCP's JSON-RPC `error_response` and the configuration manifest's subsystem names
/// (`apps/actingd/src/config/manifest.rs` `build`, G5) are standing entries (kept after B6).
pub const ALLOW_LIST: &[AllowEntry] = &[
    member(
        "apps/actinglab",
        "A2d lists it until B4 converts the actinglab sites",
    ),
    member("crates/lab", "A2d lists it until B4 converts the lab sites"),
    member(
        "tools/actinglab-architecture",
        "standing: a test tool, not a result path",
    ),
    member(
        "tools/outcome-guard",
        "standing: the guard itself, not a result path",
    ),
    // A2c bridges: channels of the ledger, runtime-state, runtime-database, artifact-store and
    // the two ledger-forensics members that stay strings until the slice the reason names
    // removes the entry (brief-a2bcd.md; brief-a2a.md A2a-9).
    bridge(
        "crates/ledger/src/global.rs",
        "GlobalLedgerError.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "GlobalLedgerError.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "GlobalLedgerError::code",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "GlobalLedgerError::operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "GlobalLedgerError::relayed_fatal",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "TransactionWorkError.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "TransactionWorkError.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/global.rs",
        "is_scheduled_settlement_refusal_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: a ledger error carries the runtime host's transaction work codes (TransactionWorkError), which are the host error's codes and still relay ledger, store and kernel spellings (RuntimeHostError::relayed_*); the ledger and host channels take Code together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/ledger/src/critical.rs",
        "CriticalExecutionError::OutcomeUndurable.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries a ledger append error's code (GlobalLedgerError::code); it takes Code with the ledger error once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger/src/critical.rs",
        "persist_outcome",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries a ledger append error's code (GlobalLedgerError::code); it takes Code with the ledger error once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger/src/lib.rs",
        "low_margin_suspicion",
        &[Check::G3],
        "B3: a Lab suspicion record's JSON key (read by actinglab); B3 builds the record from a typed struct",
    ),
    bridge(
        "crates/ledger/src/lib.rs",
        "forbidden_target_suspicion",
        &[Check::G3],
        "B3: a Lab suspicion record's JSON key (read by actinglab); B3 builds the record from a typed struct",
    ),
    bridge(
        "crates/ledger/src/lib.rs",
        "guard_reject_suspicion",
        &[Check::G3],
        "B3: a Lab suspicion record's JSON key (read by actinglab); B3 builds the record from a typed struct",
    ),
    bridge(
        "crates/ledger/src/lib.rs",
        "stale_frame_suspicion",
        &[Check::G3],
        "B3: a Lab suspicion record's JSON key (read by actinglab); B3 builds the record from a typed struct",
    ),
    bridge(
        "crates/runtime-state/src/error.rs",
        "RuntimeStateError.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays a ledger error's code, which can be a runtime host transaction work code; it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/runtime-state/src/error.rs",
        "RuntimeStateError.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays a ledger error's code, which can be a runtime host transaction work code; it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/runtime-state/src/error.rs",
        "RuntimeStateError::code",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays a ledger error's code, which can be a runtime host transaction work code; it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/runtime-state/src/error.rs",
        "RuntimeStateError::operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays a ledger error's code, which can be a runtime host transaction work code; it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/runtime-state/src/error.rs",
        "RuntimeStateError::relayed_fatal",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays a ledger error's code, which can be a runtime host transaction work code; it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/runtime-database/src/maintenance.rs",
        "MaintenanceWarning.code",
        &[Check::G2],
        "A6a: a ledger-maintenance warning (backup manifest and report); A6a gives ledger-maintenance its outcome",
    ),
    bridge(
        "crates/runtime-database/src/maintenance.rs",
        "MaintenanceWarning.operation",
        &[Check::G2],
        "A6a: a ledger-maintenance warning (backup manifest and report); A6a gives ledger-maintenance its outcome",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "ArtifactStoreError.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host builds store errors with its own codes and relays ledger codes (ArtifactStoreError::fatal); it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "ArtifactStoreError.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host builds store errors with its own codes and relays ledger codes (ArtifactStoreError::fatal); it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "ArtifactStoreError::code",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host builds store errors with its own codes and relays ledger codes (ArtifactStoreError::fatal); it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "ArtifactStoreError::operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host builds store errors with its own codes and relays ledger codes (ArtifactStoreError::fatal); it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "ArtifactStoreError::fatal",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host builds store errors with its own codes and relays ledger codes (ArtifactStoreError::fatal); it takes Code once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "RetainedCause.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: a store failure's retained cause keeps the store error's code and operation as raised (the ArtifactStoreError.code and ArtifactStoreError.operation bridges); ledger revision 3 (A3) types them only where the failure is recorded (RetainedCause::recorded); they take Code and Location with the store error once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/error.rs",
        "RetainedCause.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: a store failure's retained cause keeps the store error's code and operation as raised (the ArtifactStoreError.code and ArtifactStoreError.operation bridges); ledger revision 3 (A3) types them only where the failure is recorded (RetainedCause::recorded); they take Code and Location with the store error once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/artifact-store/src/frame_store.rs",
        "RecognitionState::Failed.reason",
        &[Check::G2],
        "A2-P: carries the page detector's English error text (the Display of PageDetectorError or BatchLevelError, cut to 256 characters) as a frame's recognition failure reason; giving it a registered code changes the timeline's reason text",
    ),
    bridge(
        "crates/artifact-store/src/frame_store.rs",
        "RecognitionState::as_json",
        &[Check::G3],
        "B3: a frame store timeline JSON key (reason, pinned_reason); B3 builds the timeline from typed records",
    ),
    bridge(
        "crates/artifact-store/src/frame_store.rs",
        "FrameStore::timeline",
        &[Check::G3],
        "B3: a frame store timeline JSON key (reason, pinned_reason); B3 builds the timeline from typed records",
    ),
    bridge(
        "crates/ledger-forensics/src/lib.rs",
        "ForensicError.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger-forensics/src/lib.rs",
        "ForensicError.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger-forensics/src/lib.rs",
        "ForensicError::code",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger-forensics/src/lib.rs",
        "ForensicError::operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger-forensics/src/lib.rs",
        "ForensicError::relayed",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "crates/ledger-forensics/src/lib.rs",
        "task_evidence_report",
        &[Check::G5],
        "A2-P: the report gap token artifact_verification_failed shares its spelling with a contract code; A2-P renames one side (model section 2.2)",
    ),
    bridge(
        "apps/ledger-forensics/src/lib.rs",
        "CliError.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "apps/ledger-forensics/src/lib.rs",
        "CliError.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "apps/ledger-forensics/src/lib.rs",
        "CliError::code",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    bridge(
        "apps/ledger-forensics/src/lib.rs",
        "CliError::relayed",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays ledger and artifact store errors' codes; it takes Code with those errors once the ledger and host channels are retyped together after the A2 train",
    ),
    // A2a part 1 bridges: channels of the contract crate and runtime-client that stay strings
    // until the slice the reason names removes the entry (brief-a2a.md A2a-9).
    bridge(
        "crates/actingcommand-contract/src/emulator.rs",
        "EmulatorVersionEvidence::Unavailable.reason",
        &[Check::G2],
        "A2-P: the unavailable version's reason is English text (the device layer's offline fake, the host's backend registry) sent in Status capabilities; a typed reason changes that emitted value, so A2-P replaces it (A2e keeps every emitted string)",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticRecognitionError.code",
        &[Check::G2],
        "A2-P: a recognition pack error's PascalCase spelling (`InvalidPackage`, ...) in the task-diagnostic artifact; A2-P registers and renames these spellings, then the row carries an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft.operation",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft.stage",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord.operation",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord.stage",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "PolicyFailureRecord.error_code",
        &[Check::G2],
        "A2-P: the policy failure record's error_code carries the #336 failure identity (`<base>~v1~k..~f..`), a dynamic family A2-P splits into a fixed code and values; the execution record then carries an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "SchedulingOutcomeDeclaration.designated_operation",
        &[Check::G2],
        "B4: a package's operation id from its scheduling declaration (pack vocabulary, model section 8.1), not a Runtime location; B4 decides its typing with the package interface",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "Wire.error_code",
        &[Check::G2],
        "A2-P: the policy failure record's error_code carries the #336 failure identity (`<base>~v1~k..~f..`), a dynamic family A2-P splits into a fixed code and values; the execution record then carries an outcome",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft::new",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft::operation",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord::operation",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailDraft::stage",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "DiagnosticDetailRecord::stage",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "validate_diagnostic_detail",
        &[Check::G2],
        "A2-P/B5: a diagnostic detail's stage and operation are the producer's tokens (the dotted stage tokens A2-P renames, the device stages and operations B5 types); ledger revision 3 keeps them as typed context beside the record's outcome until they are registered locations",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/signature.rs",
        "SignatureLifecyclePredicate.code",
        &[Check::G2],
        "B3: a diagnostic signature's match pattern (a stored query over runtime.failed outcomes), not a result; B3 types its conditions",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/signature.rs",
        "SignatureLifecyclePredicate.operation",
        &[Check::G2],
        "B3: a diagnostic signature's match pattern (a stored query over runtime.failed outcomes), not a result; B3 types its conditions",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload/signature.rs",
        "SignatureLifecyclePredicate.stage",
        &[Check::G2],
        "B3: a diagnostic signature's match pattern (a stored query over runtime.failed outcomes), not a result; B3 types its conditions",
    ),
    bridge(
        "crates/actingcommand-contract/src/game_engine.rs",
        "RuntimeCommand.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/installation.rs",
        "InstallTransitionStatus.failure_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: filled with a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the field becomes a CodeStr",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "EnvelopeError.code",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError.code",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LedgerProjection.reason",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "NeedsDetection.reason",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError::new",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError::not_implemented",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LabError::safety_blocked",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "LedgerProjection::skipped",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/lab.rs",
        "NeedsDetection::unresolved_environment",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/page_projection.rs",
        "project_observation",
        &[Check::G3],
        "B4: the Lab page projection's json keys blocked_reason and reason hold registered PageProjectionCode spellings; B4 converts the Lab sites",
    ),
    bridge(
        "crates/actingcommand-contract/src/ppocr_diagnostic.rs",
        "PpocrCpuAssignedNodeDiagnostic.placement_reason",
        &[Check::G2],
        "A2-P: carries the vision provider's placement token unavailable, which breaks the form; A2-P renames it (or makes it a vocabulary), then the field is retyped",
    ),
    bridge(
        "crates/actingcommand-contract/src/primitive.rs",
        "CaptureRequest.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/primitive.rs",
        "SwipeRequest.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/primitive.rs",
        "TapRequest.reason",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/proposal.rs",
        "ProposalKind::LanguageExtension.extension_code",
        &[Check::G2],
        "B1: a free name or text field that is not an outcome code (legacy engine and primitive requests, extension ids); B1 settles it",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ResourceAuthoringEvent.failure_code",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection.host_code",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection.host_operation",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "SchedulingResumeSelfCheck.failure_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: filled with a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the field becomes a CodeStr",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ResourceAuthoringEvent::failure_code",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection::host_code",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "ResourceAuthoringEvent::new",
        &[Check::G2],
        "B4: carries a Lab error's code or reason, which crates/lab, apps/actinglab and resource tooling's Lab errors fill; the Lab sites stay on the allow list until B4, which registers their codes and retypes the channel",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "RuntimeErrorProjection::with_host_failure",
        &[Check::G2],
        "A4: receipt projection; receipt v2 carries the outcome (host_code becomes the top code)",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime/lab_operation.rs",
        "LabOperationFailure.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: filled with a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the field becomes a CodeStr",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime/material_read.rs",
        "RuntimeMaterialReadFailure.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: filled with a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the field becomes a CodeStr",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime/material_read.rs",
        "RuntimeMaterialReadFailure.operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: filled with a runtime host error's operation (RuntimeHostError::operation, which still relays ledger, state and artifact-store operations until the host and ledger channels take Location together after the A2 train); then the field takes a Location",
    ),
    // A2e bridges: channels of the device crate that stay strings until the slice the reason
    // names removes the entry (brief-a2e.md; A2e is mechanical and keeps every emitted string).
    bridge(
        "crates/device/src/adb.rs",
        "CommandProgram.timeout_stage",
        &[Check::G2],
        "A2-P: a child program's bound-expiry stage (mumu_manager.timeout) is dotted and breaks the form; A2-P renames it",
    ),
    bridge(
        "crates/device/src/adb_bounds_diagnostic.rs",
        "AdbBoundsAction::operation",
        &[Check::G2],
        "A2-P: the adb input bounds operations tap and swipe are one word and break the form; A2-P renames them",
    ),
    bridge(
        "crates/device/src/capture.rs",
        "NemuCaptureResolutionDetail::stage",
        &[Check::G2],
        "A2-P: the Nemu capture resolution stages (nemu.installation.resolve, nemu.capture.identity, nemu.target.resolve) are dotted and break the form; A2-P renames them",
    ),
    bridge(
        "crates/device/src/discovery.rs",
        "mumu_instance_id",
        &[Check::G2],
        "B5: a MuMu process command line without a usable instance id is refused with English text; B5 gives the refusal a device code and keeps the command-line text as raw_text",
    ),
    bridge(
        "crates/device/src/discovery.rs",
        "parse_dash_v_instance",
        &[Check::G2],
        "B5: a MuMu process command line without a usable instance id is refused with English text; B5 gives the refusal a device code and keeps the command-line text as raw_text",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceErrorDiagnostic.stage",
        &[Check::G2],
        "A2-P: a device error's diagnostic stage; the device stages are dotted (adb.*, maatouch.*, minitouch.*, mumu_manager.*, nemu.*, capture.*, input.*) and host and kernel stages share the channel; A2-P renames them, then the channel takes a typed stage",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceErrorDiagnostic::new",
        &[Check::G2],
        "A2-P: a device error's diagnostic stage; the device stages are dotted (adb.*, maatouch.*, minitouch.*, mumu_manager.*, nemu.*, capture.*, input.*) and host and kernel stages share the channel; A2-P renames them, then the channel takes a typed stage",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceErrorDiagnostic::stage",
        &[Check::G2],
        "A2-P: a device error's diagnostic stage; the device stages are dotted (adb.*, maatouch.*, minitouch.*, mumu_manager.*, nemu.*, capture.*, input.*) and host and kernel stages share the channel; A2-P renames them, then the channel takes a typed stage",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceError::with_diagnostic",
        &[Check::G2],
        "A2-P: a device error's diagnostic stage; the device stages are dotted (adb.*, maatouch.*, minitouch.*, mumu_manager.*, nemu.*, capture.*, input.*) and host and kernel stages share the channel; A2-P renames them, then the channel takes a typed stage",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceError::with_diagnostic_if_absent",
        &[Check::G2],
        "A2-P: a device error's diagnostic stage; the device stages are dotted (adb.*, maatouch.*, minitouch.*, mumu_manager.*, nemu.*, capture.*, input.*) and host and kernel stages share the channel; A2-P renames them, then the channel takes a typed stage",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceErrorContext.operation",
        &[Check::G2],
        "A2-P: a device error's diagnostic operation; the channel carries one-word input operations (tap, swipe, key, text, reset) from the device layer and the host, which break the form; A2-P renames them, then the channel takes a Location",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceErrorContext::operation",
        &[Check::G2],
        "A2-P: a device error's diagnostic operation; the channel carries one-word input operations (tap, swipe, key, text, reset) from the device layer and the host, which break the form; A2-P renames them, then the channel takes a Location",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceError::with_diagnostic_context",
        &[Check::G2],
        "A2-P: a device error's diagnostic operation; the channel carries one-word input operations (tap, swipe, key, text, reset) from the device layer and the host, which break the form; A2-P renames them, then the channel takes a Location",
    ),
    bridge(
        "crates/device/src/error.rs",
        "DeviceError::with_diagnostic_context_if_absent",
        &[Check::G2],
        "A2-P: a device error's diagnostic operation; the channel carries one-word input operations (tap, swipe, key, text, reset) from the device layer and the host, which break the form; A2-P renames them, then the channel takes a Location",
    ),
    bridge(
        "crates/device/src/mumu_manager.rs",
        "registry_error",
        &[Check::G2],
        "B5: the Windows registry step (open, enumerate, query) names a foreign failure inside developer text; B5 wraps the registry failure as a foreign_os_error link",
    ),
    bridge(
        "crates/device/src/touch.rs",
        "TouchBackendAttempt.error_reason",
        &[Check::G2],
        "B5: a touch backend attempt keeps the failed open's device error text (developer prose), which the backend-open observation records as native detail; B5 carries the device error's outcome instead",
    ),
    bridge(
        "crates/device/src/touch.rs",
        "TouchBackendDiagnostics::push_failure",
        &[Check::G2],
        "B5: a touch backend attempt keeps the failed open's device error text (developer prose), which the backend-open observation records as native detail; B5 carries the device error's outcome instead",
    ),
    bridge(
        "crates/device/src/touch.rs",
        "adb_shell_input_connect_error",
        &[Check::G2],
        "A2-P: the stage parameter carries a dotted device stage (adb.*) and the operation parameter feeds the device error's operation channel, which carries one-word operations; A2-P renames them",
    ),
    // A2a part 2 bridges: channels of actingd, actingctl and vision-provider-check that stay
    // strings until the slice the reason names removes the entry (brief-a2a.md A2a-9).
    bridge(
        "apps/actingd/src/main.rs",
        "ErrorCode::Host.host_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host's code inside an actingd error (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "apps/actingd/src/check_config.rs",
        "run",
        &[Check::G3],
        "A6a: check-config v1 JSON key; check-config v2 replaces it",
    ),
    bridge(
        "apps/actingd/src/check_config.rs",
        "mumu_root_report",
        &[Check::G3, Check::G5],
        "A6a: check-config v1 mumu_root_unresolved, with the provider native failure family <category>.<stage> and its fallback device_error (decision D6); A6a removes it",
    ),
    bridge(
        "apps/actingd/src/check_config.rs",
        "summarize",
        &[Check::G5],
        "A6a: check-config v1's instance key resource_package equals the check-config stage of that spelling (decision D12); A6a rewrites this v1 report builder for check-config v2 and removes the entry",
    ),
    bridge(
        "apps/actingd/src/config.rs",
        "ResourcePackageRejection::detail",
        &[Check::G3],
        "A6a: check-config v1 JSON key loader_code; check-config v2 makes the loader code a link",
    ),
    bridge(
        "apps/actingd/src/config/ac_adb.rs",
        "pinned_binaries",
        &[Check::G2],
        "A6a: prose about the embedded pin, reported only through check-config v1 and the FATAL line",
    ),
    bridge(
        "apps/actingd/src/owner_unlock.rs",
        "failed",
        &[Check::G3],
        "A6a: unlock-owner v1 JSON key; A6a's outcome replaces error {code, stage}",
    ),
    bridge(
        "apps/actingd/src/suspended.rs",
        "run",
        &[Check::G3],
        "A6a: suspended v1 JSON key; A6a's outcome replaces code and operation",
    ),
    bridge(
        "apps/actingctl/src/mcp/events.rs",
        "result_schema",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/lab.rs",
        "binding_draft_result",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/lab.rs",
        "check_config",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/lab.rs",
        "trimmed_lab_error",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "config",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "flag",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "parse_tiers",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "print_text",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "registered_program",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "serve",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/mod.rs",
        "value",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/observer.rs",
        "install_view",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/observer.rs",
        "instance_row",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/observer.rs",
        "overview",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runs.rs",
        "finish_suspension_report",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runs.rs",
        "start_suspension_report",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runtime.rs",
        "client_error",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/runtime.rs",
        "configured_state_root",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/runtime.rs",
        "read_bounded",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/server.rs",
        "parse_call",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/server.rs",
        "serve",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/server.rs",
        "spawn",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolCode::Relayed.relayed_code",
        &[Check::G2],
        "A5: a code relayed from actinglab, a job's stored error or the Runtime's material read failure, which A5 relays as outcome links; and actingctl's own dynamic family material_<state> (a code carrying the read state, not listed in decision D6), which A5 splits into one fixed code with a material_state value instead of relaying it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::host_code",
        &[Check::G2],
        "A5: reads the runtime host's code out of the MCP error details (a received spelling, which may be a relayed ledger, state, artifact-store or kernel code); A5's {class, outcome} error shape replaces the details",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::into_value",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::into_warning",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "ToolError::to_value",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "invalid_argument",
        &[Check::G2],
        "A5: the prose of an MCP argument refusal; A5 removes the message",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "output_schema",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/mcp/tools.rs",
        "pause_input",
        &[Check::G3],
        "A5: MCP JSON key; A5's {class, outcome} and typed results replace it",
    ),
    bridge(
        "apps/actingctl/src/process_probe.rs",
        "pid_alive",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/decide.rs",
        "Decision::key",
        &[Check::G2],
        "A5: the watchdog's decision key string; A5's last_decision {code, values} replaces it (decision D6)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/decide.rs",
        "fatal_stage",
        &[Check::G5],
        "A6a: the FATAL-line scan for the runtime host's restartable stop codes; A6a's terminal-outcome scan replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "detail",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "install",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "misconfigured_report",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "report",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "run_once",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "status",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/mod.rs",
        "uninstall",
        &[Check::G3],
        "A5: watchdog report key; report v2 replaces it",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "host_probe",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "live_owner",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "owner_blocks",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "parse_host_probe",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/observe.rs",
        "parse_processes",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/powershell.rs",
        "run",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/actingctl/src/watchdog/task.rs",
        "run",
        &[Check::G2],
        "A5: prose error text, surfaced only through outputs A5 rewrites (developer line, MCP error, watchdog report v2)",
    ),
    bridge(
        "apps/vision-provider-check/src/main.rs",
        "InvalidModelReport.reason",
        &[Check::G2],
        "A2-P: carries vision-ffi's model folder refusal, English text that the tool prints as the folder's reason; giving it a registered code changes that text",
    ),
    // A2b bridges: channels of runtime-host, the scheduler and host-metrics that stay strings
    // until the slice the reason names removes the entry (brief-a2bcd.md; the A2a pattern).
    // A2b/A2c merge follow-up: the runtime host error still relays the ledger, state,
    // artifact-store and kernel codes, which carry the host's codes back, so both sides take
    // Code together after the A2 train; A2-P: an emitted spelling that only a rename or a family
    // split can type without changing output.
    bridge(
        "crates/runtime-host/src/error.rs",
        "HostErrorCode::Relayed.relayed_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/error.rs",
        "HostErrorOperation::Relayed.relayed_operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/error.rs",
        "RuntimeHostError::code",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/error.rs",
        "RuntimeHostError::operation",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/error.rs",
        "RuntimeHostError::relayed_fatal",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/error.rs",
        "RuntimeHostError::relayed_request",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/error.rs",
        "RuntimeHostError::relayed_with_projection",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host error relays the codes and operations of the ledger, runtime-state and artifact-store errors and of the kernel's ContainedTaskCode as spellings (RuntimeHostError::relayed_*), and those carry the host's own codes back (A2c's TransactionWorkError and ArtifactStoreError::fatal, A2d's ContainedTaskCode::Host); both sides take Code and Location together in one follow-up after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/failure_identity.rs",
        "FailureIdentity::parse",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/failure_identity.rs",
        "encode_failure_base",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/failure_identity.rs",
        "failure_accumulates",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/failure_identity.rs",
        "failure_key",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/host.rs",
        "RuntimeLifecycleFailure::Process.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: actingd's own failure code may be a runtime host code that actingd holds as a spelling (ErrorCode::Host, filled from RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "EntryRecoveryRuntime::record_guard_evaluation",
        &[Check::G2],
        "A2-P: the kernel's guard-evaluation reason, a closed token set (candidate_sample, trusted_coordinate, page_mismatch and the single word evaluated) that cannot be registered as it is (the kernel's ContainedTaskRuntime::record_guard_evaluation bridge); A2-P renames it, then types the parameter",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "RuntimeContainedTask::record_guard_evaluation",
        &[Check::G2],
        "A2-P: the kernel's guard-evaluation reason, a closed token set (candidate_sample, trusted_coordinate, page_mismatch and the single word evaluated) that cannot be registered as it is (the kernel's ContainedTaskRuntime::record_guard_evaluation bridge); A2-P renames it, then types the parameter",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "RuntimeContainedTask::record_post_admission_ocr_failure",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the execution kernel's task failure code by its spelling (ContainedTaskCode::as_str), whose Host variant relays the runtime host's own code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the channel takes Code",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "RuntimeContainedTaskOcrFailureDiagnostic.failure_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the execution kernel's task failure code by its spelling (ContainedTaskCode::as_str), whose Host variant relays the runtime host's own code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the channel takes Code",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "bounded_post_admission_ocr_failure_detail",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the execution kernel's task failure code by its spelling (ContainedTaskCode::as_str), whose Host variant relays the runtime host's own code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the channel takes Code",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "fail_contained_task_entry",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the execution kernel's task failure code by its spelling (ContainedTaskCode::as_str), whose Host variant relays the runtime host's own code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the channel takes Code",
    ),
    bridge(
        "crates/runtime-host/src/host/contained_task.rs",
        "resource_reading_failure_detail",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the execution kernel's task failure code by its spelling (ContainedTaskCode::as_str), whose Host variant relays the runtime host's own code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the channel takes Code",
    ),
    bridge(
        "crates/runtime-host/src/host/evidence_export.rs",
        "relayed_evidence_request_error",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays the ledger's or the artifact store's code as a spelling (GlobalLedgerError::code, ArtifactStoreError::code), which carry the runtime host's own codes back; the relay goes when the host and ledger channels take Code together after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/host/failure_settlement.rs",
        "recorded_failure_input",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/host/frame_retention.rs",
        "FrameRetentionWarning::stage",
        &[Check::G2],
        "A2-P: dotted stage tokens (runtime.lifecycle.*, frame_retention.*) break the form; A2-P renames them to registered locations",
    ),
    bridge(
        "crates/runtime-host/src/host/frame_retention.rs",
        "pin_failure",
        &[Check::G2],
        "A2b/A2c merge follow-up: builds an artifact-store error, whose constructor takes spellings because the runtime host fills it with its own codes (A2c's ArtifactStoreError::fatal bridge); it takes Code with the store error when the host and ledger channels take Code together after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/host/policy_dispatch.rs",
        "policy_rejection_severity",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/policy_dispatch.rs",
        "scheduling_resume_selfcheck",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/policy_outcome.rs",
        "HostShared::validate_scheduled_policy_terminal",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/host/prerequisite.rs",
        "record_closed",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the execution kernel's task failure code by its spelling (ContainedTaskCode::as_str), whose Host variant relays the runtime host's own code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the channel takes Code",
    ),
    bridge(
        "crates/runtime-host/src/host/recovery_ladder.rs",
        "HostShared::finish_interrupted_ladder",
        &[Check::G2],
        "A2b/A2c merge follow-up: a recovery rung's failure reason can be a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/recovery_ladder.rs",
        "rung_failure_outcome",
        &[Check::G2],
        "A2b/A2c merge follow-up: a recovery rung's failure reason can be a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/recovery_ladder.rs",
        "rung_reason_outcome",
        &[Check::G2],
        "A2b/A2c merge follow-up: a recovery rung's failure reason can be a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/recovery_ladder.rs",
        "RungAttempt::Failed.reason",
        &[Check::G2],
        "A2b/A2c merge follow-up: a recovery rung's failure reason can be a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/recovery_ladder.rs",
        "is_hold_interruption",
        &[Check::G2],
        "A2b/A2c merge follow-up: a recovery rung's failure reason can be a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/recovery_ladder.rs",
        "recovery_entry_skip",
        &[Check::G2],
        "A2b/A2c merge follow-up: a recovery rung's failure reason can be a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/host/requests.rs",
        "protocol_error_at_code_spelling",
        &[Check::G2],
        "A2-P: an operation spelled like a shared code (material_read_deadline_overflow, material_read_length_overflow: one spelling for a code and a location); A2-P renames one side",
    ),
    bridge(
        "crates/runtime-host/src/host/requests.rs",
        "receipt_error",
        &[Check::G5],
        "A2-P: the host's receipt-build failure keeps the spelling runtime_receipt_invalid, which the runtime client registered with another meaning (M378:90); A2-P renames the host side",
    ),
    bridge(
        "crates/runtime-host/src/host/saved_artifact_ocr.rs",
        "relayed_source_error",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays the ledger's or the artifact store's code as a spelling (GlobalLedgerError::code, ArtifactStoreError::code), which carry the runtime host's own codes back; the relay goes when the host and ledger channels take Code together after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/host/signatures.rs",
        "relayed_signature_request_error",
        &[Check::G2],
        "A2b/A2c merge follow-up: relays the ledger's or the artifact store's code as a spelling (GlobalLedgerError::code, ArtifactStoreError::code), which carry the runtime host's own codes back; the relay goes when the host and ledger channels take Code together after the A2 train",
    ),
    bridge(
        "crates/runtime-host/src/host/task_diagnostic.rs",
        "RuntimeContainedTask::diagnostic_guard",
        &[Check::G2],
        "A2-P: the kernel's guard-evaluation reason, a closed token set (candidate_sample, trusted_coordinate, page_mismatch and the single word evaluated) that cannot be registered as it is (the kernel's ContainedTaskRuntime::record_guard_evaluation bridge); A2-P renames it, then types the parameter",
    ),
    bridge(
        "crates/runtime-host/src/host/task_diagnostic.rs",
        "RuntimeContainedTask::diagnostic_guard_inner",
        &[Check::G2],
        "A2-P: the kernel's guard-evaluation reason, a closed token set (candidate_sample, trusted_coordinate, page_mismatch and the single word evaluated) that cannot be registered as it is (the kernel's ContainedTaskRuntime::record_guard_evaluation bridge); A2-P renames it, then types the parameter",
    ),
    bridge(
        "crates/runtime-host/src/ledger_maintenance.rs",
        "LedgerMaintenanceFailure.code",
        &[Check::G2],
        "A6a: a ledger-maintenance failure in actingd's ledger-maintenance JSON report, beside the report's MaintenanceWarning; it carries the ledger's and the runtime database's codes and operations as strings, and A6a gives ledger-maintenance its outcome",
    ),
    bridge(
        "crates/runtime-host/src/ledger_maintenance.rs",
        "LedgerMaintenanceFailure.operation",
        &[Check::G2],
        "A6a: a ledger-maintenance failure in actingd's ledger-maintenance JSON report, beside the report's MaintenanceWarning; it carries the ledger's and the runtime database's codes and operations as strings, and A6a gives ledger-maintenance its outcome",
    ),
    bridge(
        "crates/runtime-host/src/owner_unlock.rs",
        "OwnerUnlockFailure.stage",
        &[Check::G2],
        "A2-P: the one-word unlock stages journal and ledger break the form; A2-P renames them (unlock-owner v2 is A6a's)",
    ),
    bridge(
        "crates/runtime-host/src/performance.rs",
        "PerformanceMonitor::ingest_monitor_failure",
        &[Check::G2],
        "A2-P: the performance monitor's failure code can be the dynamic family <code>.<event_type> (rejected_event_code); A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/performance.rs",
        "PerformanceMonitor::record_monitor_failure",
        &[Check::G2],
        "A2-P: the performance monitor's failure code can be the dynamic family <code>.<event_type> (rejected_event_code); A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/performance.rs",
        "rejected_event_code",
        &[Check::G2],
        "A2-P: the performance monitor's failure code can be the dynamic family <code>.<event_type> (rejected_event_code); A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/performance.rs",
        "valid_error_code",
        &[Check::G2],
        "A2-P: the performance monitor's failure code can be the dynamic family <code>.<event_type> (rejected_event_code); A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/performance/capacity.rs",
        "PerformanceMonitor::degrade_for_rejected_summary",
        &[Check::G2],
        "A2-P: the performance monitor's failure code can be the dynamic family <code>.<event_type> (rejected_event_code); A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/policy_control.rs",
        "FailureStreak.error_code",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/policy_control.rs",
        "PolicyExecutionInput::Failed.error_code",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/policy_control.rs",
        "interruption_lifted_by_restart",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/procedure_manifest.rs",
        "field_refusal",
        &[Check::G2],
        "A2-P: the operation names the refused manifest field (procedure_ref, package_digest, operation_id, yield_point), data carried as a location; A2-P moves the field into a value under a registered operation",
    ),
    bridge(
        "crates/runtime-host/src/procedure_manifest.rs",
        "validate_token",
        &[Check::G2],
        "A2-P: the operation names the refused manifest field (procedure_ref, package_digest, operation_id, yield_point), data carried as a location; A2-P moves the field into a value under a registered operation",
    ),
    bridge(
        "crates/runtime-host/src/provider.rs",
        "DeviceRegistryInputDiagnosticBackend::run",
        &[Check::G2],
        "A2-P: passes the device layer's one-word input operation names (tap, swipe, key, ...) to the device error's diagnostic context, whose operation channel stays a string (A2e bridge DeviceErrorContext.operation); A2-P renames them",
    ),
    bridge(
        "crates/runtime-host/src/provider_startup.rs",
        "ProviderStartup::failed",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries a runtime host error's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then it becomes a Code",
    ),
    bridge(
        "crates/runtime-host/src/suspension_report.rs",
        "Snapshot::frames",
        &[Check::G3],
        "A6a: actingd suspended v1 JSON keys (a pair's failure codes and a frame's unavailable reason); the report's outcome form replaces them",
    ),
    bridge(
        "crates/runtime-host/src/suspension_report.rs",
        "Snapshot::run_failure",
        &[Check::G2],
        "A2-P: the #336 failure identity, a dynamic code family (<code>~f...) persisted in policy.execution_recorded and recomputed at replay; A2-P splits it into a fixed code and values",
    ),
    bridge(
        "crates/runtime-host/src/suspension_report.rs",
        "suspension_report",
        &[Check::G3],
        "A6a: actingd suspended v1 JSON keys (a pair's failure codes and a frame's unavailable reason); the report's outcome form replaces them",
    ),
    bridge(
        "crates/runtime-host/src/host.rs",
        "artifact_store_error_at_code_spelling",
        &[Check::G2],
        "A2-P: an operation spelled like a shared code (effective_configuration_too_large: one spelling for a code and a location); A2-P renames one side",
    ),
    // End of the A2b bridges.
    AllowEntry {
        path: "apps/actingctl/src/mcp/protocol.rs",
        item: Some("error_response"),
        checks: &[Check::G3],
        reason: "standing: JSON-RPC requires error.code and error.message (model section 7.3)",
    },
    AllowEntry {
        path: "apps/actingd/src/config/manifest.rs",
        item: Some("build"),
        checks: &[Check::G5],
        reason: "standing: the configuration manifest's subsystem names are vocabulary tokens, not codes or locations (model section 2.1); the subsystem runtime_fact_snapshot equals the runtime client's location of that spelling",
    },
    // A2d bridges: channels of the execution and recognition group that stay strings until the
    // slice the reason names removes the entry (brief-a2bcd.md; A2-P changes emitted strings).
    bridge(
        "crates/execution-kernel/src/contained_task.rs",
        "ContainedTaskCode::Host.host_code",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the runtime host's code (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train); then the variant takes Code",
    ),
    bridge(
        "crates/execution-kernel/src/contained_task.rs",
        "ContainedTaskRunError::task",
        &[Check::G2],
        "A2b/A2c merge follow-up: the runtime host passes its own code, still a spelling (RuntimeHostError::code, which still relays ledger, state, artifact-store and kernel spellings until the host and ledger channels take Code together after the A2 train), into a contained task failure; then the constructor takes Code",
    ),
    bridge(
        "crates/execution-kernel/src/contained_task.rs",
        "ContainedTaskRuntime::record_guard_evaluation",
        &[Check::G2],
        "A2-P: the guard evaluation reason is a closed token set (candidate_sample, trusted_coordinate, page_mismatch and the single word evaluated) that cannot be registered as it is; A2-P renames it, then types the parameter",
    ),
    bridge(
        "crates/execution-kernel/src/contained_task/linear.rs",
        "optional_runs",
        &[Check::G2],
        "B4: not an outcome code: the token is a contained_task_linear_invalid reason the caller writes into the failure's detail as reason=<token>",
    ),
    bridge(
        "crates/execution-kernel/src/contained_task/selection.rs",
        "check_policy",
        &[Check::G2],
        "B4: the error is the English text of a selection policy mismatch, which the caller reports under contained_task_select_policy_mismatch; B4 types it",
    ),
    bridge(
        "crates/execution-kernel/src/contained_task/selection.rs",
        "read_policy",
        &[Check::G2],
        "B4: the error is the English text of a selection policy document refusal, which the caller reports under contained_task_select_policy_invalid; B4 types it",
    ),
    bridge(
        "crates/execution-kernel/src/environment.rs",
        "EnvironmentStateErrorKind::reason",
        &[Check::G2],
        "A2-P: the reason set includes the single word expired and the spelling low_confidence that other owners use as a vocabulary token, which the kernel cannot register as they are; A2-P renames them, then types the getter",
    ),
    bridge(
        "crates/execution-kernel/src/environment.rs",
        "env_rect_from_value",
        &[Check::G2],
        "B4: the error is the English text of an invalid environment region, which the caller reports as a deserialization error; B4 types it",
    ),
    bridge(
        "crates/execution-kernel/src/online_observation.rs",
        "PreparedPageObservation::evaluate",
        &[Check::G3],
        "B4: the observation evidence rows' json keys stage, reason and code (observation artifact facts, not ledger records; A3 left them) carry codes and locations; B4 settles the recognition records",
    ),
    bridge(
        "crates/execution-kernel/src/provider.rs",
        "InstanceDiscoveryFailure.code",
        &[Check::G2],
        "A2b/A2c merge follow-up: carries the runtime host's or the contract's discovery code (HostCode::MumuManagerVersionUnsupported, SharedCode::InstanceDiscoveryUnavailable), which actingd passes to ProviderStartup::failed beside the host's relayed codes; both take Code in the follow-up after the A2 train",
    ),
    bridge(
        "crates/execution-kernel/src/provider.rs",
        "VisionFfiProvider::retire",
        &[Check::G2],
        "B4: not an outcome code: the English text of why a model slot was retired, which later provider errors repeat as their message",
    ),
    bridge(
        "crates/execution-kernel/src/provider.rs",
        "invalid_folder",
        &[Check::G2],
        "B4: not an outcome code: vision-ffi's English model folder refusal, which the provider error's message embeds",
    ),
    bridge(
        "crates/execution-kernel/src/recovery.rs",
        "NodeOutcome::reason",
        &[Check::G2],
        "A2-P: the node outcome reason set includes the single word succeeded, which cannot be registered as it is; A2-P splits success from the registered failure codes, then types the channel",
    ),
    bridge(
        "crates/execution-kernel/src/recovery.rs",
        "RecoveryExecutionReport.terminal_reason",
        &[Check::G2],
        "B4: not an outcome code: English text saying why a recovery graph run ended",
    ),
    bridge(
        "crates/execution-kernel/src/recovery.rs",
        "RecoveryRuntime::save_on_error",
        &[Check::G2],
        "A2-P: the node outcome reason set includes the single word succeeded, which cannot be registered as it is; A2-P splits success from the registered failure codes, then types the channel",
    ),
    bridge(
        "crates/execution-kernel/src/run.rs",
        "RunStateConfig.designated_operation",
        &[Check::G2],
        "B4: not an outcome code or location: the id of a task operation that the package declares (pack data)",
    ),
    bridge(
        "crates/execution-kernel/src/run.rs",
        "RunStateConfig::with_phases",
        &[Check::G2],
        "B4: not an outcome code or location: the id of a task operation that the package declares (pack data)",
    ),
    bridge(
        "crates/execution-kernel/src/run.rs",
        "RunStateMachine.executing_operation",
        &[Check::G2],
        "B4: not an outcome code or location: the id of a task operation that the package declares (pack data)",
    ),
    bridge(
        "crates/onnx-provider-support/src/lib.rs",
        "OrtRuntimeInitializer::ensure",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "crates/onnx-provider-support/src/lib.rs",
        "OrtRuntimeInitializer::ensure_with",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "crates/onnx-provider-support/src/lib.rs",
        "ensure_same_runtime",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "crates/pack-containment/src/source/mod.rs",
        "ParseFiles::length",
        &[Check::G2],
        "B4: the error is the English text of a source snapshot entry that was not supplied or not read (with the OS error), which the caller reports under package_invalid; B4 types it",
    ),
    bridge(
        "crates/pack-containment/src/source/mod.rs",
        "ParseFiles::read",
        &[Check::G2],
        "B4: the error is the English text of a source snapshot entry that was not supplied or not read (with the OS error), which the caller reports under package_invalid; B4 types it",
    ),
    bridge(
        "crates/policy/src/canonical.rs",
        "canonical_json",
        &[Check::G2],
        "B4: the error is the English text of a canonical encoding failure, which its caller reports under a registered code; B4 types it",
    ),
    bridge(
        "crates/policy/src/canonical.rs",
        "canonical_serialized",
        &[Check::G2],
        "B4: the error is the English text of a canonical encoding failure, which its caller reports under a registered code; B4 types it",
    ),
    bridge(
        "crates/policy/src/canonical.rs",
        "catalog_hash",
        &[Check::G2],
        "B4: the error is the English text of a canonical encoding failure, which its caller reports under a registered code; B4 types it",
    ),
    bridge(
        "crates/policy/src/canonical.rs",
        "write_value",
        &[Check::G2],
        "B4: the error is the English text of a canonical encoding failure, which its caller reports under a registered code; B4 types it",
    ),
    bridge(
        "crates/policy/src/compiler.rs",
        "preflight_diagnostic",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/policy/src/evaluator.rs",
        "DetectionSuggestion.reason",
        &[Check::G2],
        "B4: carries detection-suggestion reasons, unknown_reason vocabulary tokens next to registered codes; B4 types the channel as a vocabulary or a code",
    ),
    bridge(
        "crates/policy/src/forward.rs",
        "ForwardEvidenceGap.code",
        &[Check::G2],
        "B4: carries detection-suggestion reasons, unknown_reason vocabulary tokens next to registered codes; B4 types the channel as a vocabulary or a code",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "add_score_fraction",
        &[Check::G2],
        "B4: the error is the English message its caller wraps in policy_evaluation_input_invalid, not a code; B4 types the resolution chain",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "check_target_v2",
        &[Check::G2],
        "B4: not an outcome code: the refusal's String is the JSON pointer of the refused field, beside the typed ResourceTargetsRejectionReason",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "resolution_why",
        &[Check::G2],
        "B4: the error is the English message its caller wraps in policy_evaluation_input_invalid, not a code; B4 types the resolution chain",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "resolve_instance_targets",
        &[Check::G2],
        "B4: the error is the English message its caller wraps in policy_evaluation_input_invalid, not a code; B4 types the resolution chain",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "resolve_target",
        &[Check::G2],
        "B4: not an outcome code: the refusal's String is the JSON pointer of the refused field, beside the typed ResourceTargetsRejectionReason",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "resolve_target_pool",
        &[Check::G2],
        "B4: not an outcome code: the refusal's String is the JSON pointer of the refused field, beside the typed ResourceTargetsRejectionReason",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "resolve_v2",
        &[Check::G2],
        "B4: the error is the English message its caller wraps in policy_evaluation_input_invalid, not a code; B4 types the resolution chain",
    ),
    bridge(
        "crates/policy/src/resource_targets.rs",
        "target_effects",
        &[Check::G2],
        "B4: the error is the English message its caller wraps in policy_evaluation_input_invalid, not a code; B4 types the resolution chain",
    ),
    bridge(
        "crates/policy/src/schema.rs",
        "CatalogDiagnostic.reason",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/policy/src/source.rs",
        "JsonScanner::invalid",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/policy/src/source.rs",
        "ScanError::Invalid.reason",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/policy/src/source.rs",
        "SourceMap::diagnostic",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/policy/src/source.rs",
        "basic_diagnostic",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/policy/src/source.rs",
        "bounded_reason",
        &[Check::G2],
        "B4: not an outcome code: the English explanation of a catalog diagnostic, whose code is the typed CatalogDiagnosticCode",
    ),
    bridge(
        "crates/recognition-pack/src/candidate_consensus.rs",
        "CandidateConsensus::validate",
        &[Check::G2],
        "B4: the error is the English message of a pack validation refusal, which the caller reports under its typed RecognitionPackErrorCode; B4 types it",
    ),
    bridge(
        "crates/recognition-pack/src/candidate_identity.rs",
        "CandidateIdentityDeclaration::validate",
        &[Check::G2],
        "B4: the error is the English message of a pack validation refusal, which the caller reports under its typed RecognitionPackErrorCode; B4 types it",
    ),
    bridge(
        "crates/recognition-pack/src/candidate_layout.rs",
        "CandidateIntegerDeclaration::validate",
        &[Check::G2],
        "B4: the error is the English message of a pack validation refusal, which the caller reports under its typed RecognitionPackErrorCode; B4 types it",
    ),
    bridge(
        "crates/recognition-pack/src/candidate_layout.rs",
        "CandidateProjectionFailure::unmeasurable",
        &[Check::G2],
        "B4: not an outcome code: English text saying why a target cannot be measured, written into the failure's detail",
    ),
    bridge(
        "crates/recognition-pack/src/lib.rs",
        "UnsupportedRecognitionTarget.reason",
        &[Check::G2],
        "A2-P: the English list of a target's unsupported features (method=..., mask) that resource tooling prints as the target's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/recognition-pack/src/lib.rs",
        "unsupported_template_reason",
        &[Check::G2],
        "A2-P: the English list of a target's unsupported features (method=..., mask) that resource tooling prints as the target's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/recognition-pack/src/target_consensus.rs",
        "TargetConsensus::validate",
        &[Check::G2],
        "B4: the error is the English message of a pack validation refusal, which the caller reports under its typed RecognitionPackErrorCode; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/api.rs",
        "UnsupportedRecognitionTargetResponse.reason",
        &[Check::G2],
        "A2-P: the English list of a target's unsupported features (method=..., mask) that resource tooling prints as the target's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/resource-tooling/src/authoring.rs",
        "AuthoringEvent.failure_code",
        &[Check::G2],
        "B4: carries a LabError's code, which stays a spelling until B4 converts the Lab sites",
    ),
    bridge(
        "crates/resource-tooling/src/business_catalog.rs",
        "CatalogUnknown.reason",
        &[Check::G2],
        "B4: not an outcome code: the business catalog's unknown-value reason token (not_confirmed), a vocabulary",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "PublicationEnvironment::current_process",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "PublicationEnvironment::inspect_process",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "PublicationEnvironment::now_unix_ms",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "PublicationEnvironment::random_seed",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "SystemPublicationEnvironment::current_process",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "SystemPublicationEnvironment::inspect_process",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "SystemPublicationEnvironment::now_unix_ms",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "SystemPublicationEnvironment::random_seed",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "inspect_system_process",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "parse_seed_hex",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "system_powershell_path",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/package_publish.rs",
        "system_random_seed",
        &[Check::G2],
        "B4: the error is the English text of a publication environment probe (process, clock, random seed, PowerShell path) with the OS's own text; B4 types it",
    ),
    bridge(
        "crates/resource-tooling/src/resource_restore.rs",
        "record_provenance",
        &[Check::G3],
        "B4: the restore provenance record's json keys stage and code hold a Lab operation failure's stage and code; B4 converts the Lab sites and builds the record from typed members",
    ),
    bridge(
        "crates/resource-tooling/src/resource_restore.rs",
        "restore_authoring_draft",
        &[Check::G5],
        "A2-P: the restore report's record_gaps token input_failed has the spelling of the contract's input_failed code with another meaning; resource tooling's side is the more specific one and A2-P renames it (CD-8), which changes the report",
    ),
    bridge(
        "crates/selection-policy/src/evaluator.rs",
        "Assessment::Aborted.reason",
        &[Check::G2],
        "B4: not an outcome code: the String is the detail text beside the typed UnknownReason",
    ),
    bridge(
        "crates/selection-policy/src/evaluator.rs",
        "SelectionOutcome::key",
        &[Check::G2],
        "B4: not an outcome code: the selection document's outcome key (pack vocabulary outcome_key)",
    ),
    bridge(
        "crates/vision-ffi/src/ffi.rs",
        "RuntimeLibraryClosureState::Poisoned.reason",
        &[Check::G2],
        "B4: not an outcome code: the native loader's failure text (foreign) kept for the poisoned closure's message",
    ),
    bridge(
        "crates/vision-ffi/src/ffi.rs",
        "cuda_success",
        &[Check::G2],
        "B4: not an outcome code: the name of the CUDA driver call in developer text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "InvalidModelFolder.reason",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "VisionModelDescription::validate",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "VisionModelDescription::validate_params",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "classify_folder",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "directory",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "read_description",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "crates/vision-ffi/src/models.rs",
        "regular_file",
        &[Check::G2],
        "A2-P: the model folder refusal is English text that vision-provider-check and check-config print as the folder's reason; giving it a registered code changes that text",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "NodePlacementLogCapture.failure",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "NodePlacementLogCapture::diagnostic",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "PpocrCtcModel::load_detector",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "ProviderSessionPlan::from_key",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "RectUsize::from_vision_rect",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "argmax_with_softmax_confidence",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "capture_cuda_node_placement",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "check_class_count",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "component_to_vision_rect",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "decode_ctc_output",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "detect_text_boxes_from_probability_map",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "detector_probability_map",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "dynamic_detection_shape_for_region",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "dynamic_width_for_region",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "frame_region_to_detection_tensor",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "frame_region_to_recognition_tensor",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "load_ort_session",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "node_placement_diagnostic_requested",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "output_layout",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "padded_rect",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "parse_dictionary",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "parse_node_placement",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "parse_provider_summary",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "positive_or_default",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "read_channels",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "read_rgb_pixel",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "remaining_inference_budget",
        &[Check::G2],
        "B4: not an outcome code: the name of the provider's inference step in developer text",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "require_recognizer_classes",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "select_detection_input_shape",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "select_recognition_input_shape",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/lib.rs",
        "union_rect",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/nn.rs",
        "frame_to_tensor",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/nn.rs",
        "labels_from_scores",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/nn.rs",
        "load_session",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    bridge(
        "providers/ppocr-onnx-json/src/nn.rs",
        "select_input_shape",
        &[Check::G2],
        "B4: the error is the vision provider's English text, which reaches the Runtime only as a vision failure message (foreign text); B4 types the provider's errors",
    ),
    // End of the A2d bridges.
    // A3 entries (ledger revision 3): G6 members that are data, not Runtime prose, the
    // policy's Severe code list (model section 8.1) and the policy failure code of a terminal
    // outcome.
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "LifecycleNativeDetail.text",
        &[Check::G6],
        "B5: foreign text (adb command output) the typed ADB target recovery evidence still holds; B5 moves device evidence onto foreign links' raw_text",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/artifact.rs",
        "TaskDiagnosticOcrBlock.text",
        &[Check::G6],
        "B4: text the OCR provider recognised (data the task-diagnostic artifact records), not Runtime prose; B4 settles the recognition records",
    ),
    bridge(
        "crates/actingcommand-contract/src/candidate_projection.rs",
        "CandidateRecognitionInput::Ocr.text",
        &[Check::G6],
        "B4: text the OCR provider recognised for a candidate (data a selection record keeps), not Runtime prose; B4 settles the recognition records",
    ),
    bridge(
        "crates/actingcommand-contract/src/runtime.rs",
        "InputAction::Text.text",
        &[Check::G6],
        "B4: the text an input action types (data; a task record persists it only as [redacted]), not Runtime prose; B4 settles the input records",
    ),
    bridge(
        "crates/actingcommand-contract/src/emulator.rs",
        "EmulatorVersionEvidence::Unavailable.reason",
        &[Check::G6],
        "A2-P: the unavailable version's reason is English text (the same channel as its G2 bridge); A2-P replaces it with a typed reason",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "severe_policy_failure_codes",
        &[Check::G5],
        "B4: the policy's Severe code list (model section 8.1) spells other owners' codes; B4 reviews the list with the policy",
    ),
    bridge(
        "crates/actingcommand-contract/src/event/payload.rs",
        "scheduled_failure_code",
        &[Check::G2],
        "A2-P: the policy failure code of a failed run's terminal outcome (an unregistered relayed spelling keeps its raw_code), the base of the #336 failure identity `error_code`, a dynamic family A2-P splits into a fixed code and values",
    ),
    // End of the A3 entries.
];

/// One bridge: a function (`Type::method`, `Trait::method` or a free function's name), or a
/// struct or variant field (`Type.field`, `Enum::Variant.field`), whose channel stays a string
/// until the slice its reason names converts it.
const fn bridge(
    path: &'static str,
    item: &'static str,
    checks: &'static [Check],
    reason: &'static str,
) -> AllowEntry {
    AllowEntry {
        path,
        item: Some(item),
        checks,
        reason,
    }
}

const fn member(path: &'static str, reason: &'static str) -> AllowEntry {
    AllowEntry {
        path,
        item: None,
        checks: CHANNELS,
        reason,
    }
}

/// The workspace root of a member at `tools/<name>`.
pub fn workspace_root(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest_dir.to_path_buf())
}

/// One scanned source file.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub member: String,
    /// Relative to the workspace root, with `/`.
    pub path: String,
    pub scan: source::FileScan,
}

/// The workspace as the checks see it: members, catalog and scanned sources.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub members: Vec<String>,
    pub catalog: Result<Catalog, Vec<String>>,
    pub files: Vec<ScannedFile>,
    /// Files that cannot be read, parsed or found.
    pub errors: Vec<String>,
}

/// Reads the catalog and scans every member's non-test sources once.
pub fn load_workspace(root: &Path) -> Workspace {
    let mut errors = Vec::new();
    let members = workspace_members(root).unwrap_or_else(|error| {
        errors.push(error);
        Vec::new()
    });
    let catalog = catalog::load(root, &members);
    let registered = catalog
        .as_ref()
        .map(Catalog::registered_names)
        .unwrap_or_default();
    let mut files = Vec::new();
    for member in &members {
        let channels = !member_fully_allowed(member);
        scan_member(root, member, channels, &registered, &mut files, &mut errors);
    }
    Workspace {
        root: root.to_path_buf(),
        members,
        catalog,
        files,
        errors,
    }
}

/// The merged catalog text, as `outcome-guard merge` writes it.
pub fn merge(root: &Path) -> Result<String, Vec<String>> {
    let members = workspace_members(root).map_err(|error| vec![error])?;
    let catalog = catalog::load(root, &members)?;
    catalog::merged_text(&catalog).map_err(|error| vec![error])
}

/// G1: every registry entry is an active fragment entry of the registry's own crate, with the
/// same category and, for a code, the same ladder class; every active fragment entry is
/// registered; no spelling is registered twice.
pub fn check_registries(workspace: &Workspace) -> Vec<String> {
    let catalog = match &workspace.catalog {
        Ok(catalog) => catalog,
        Err(errors) => return prefixed("G1", errors),
    };
    let mut violations = Vec::new();
    for file in &workspace.files {
        for error in &file.scan.facts.errors {
            violations.push(format!("G1 {}: {error}", file.path));
        }
    }
    let mut places = BTreeMap::<String, String>::new();
    let mut codes = BTreeSet::new();
    let mut locations = BTreeSet::new();
    for file in &workspace.files {
        let owner = catalog.owner_of(&file.member);
        for registry in &file.scan.facts.registries {
            for entry in &registry.entries {
                let place = format!("{}:{}", file.path, entry.line);
                let spelling = &entry.spelling;
                if let Some(first) = places.insert(spelling.clone(), place.clone()) {
                    violations.push(format!(
                        "G1 {place}: {spelling} is registered twice (also at {first})"
                    ));
                }
                let (table, label) = if registry.locations {
                    locations.insert(spelling.clone());
                    (&catalog.locations, "location")
                } else {
                    codes.insert(spelling.clone());
                    (&catalog.codes, "code")
                };
                let Some(listed) = table.get(spelling) else {
                    violations.push(format!(
                        "G1 {place}: {label} {spelling} is in no fragment's {label}s"
                    ));
                    continue;
                };
                if catalog::text(listed, "status") != Some("active") {
                    violations.push(format!("G1 {place}: {label} {spelling} is retired"));
                }
                if catalog::text(listed, "owner") != owner {
                    violations.push(format!(
                        "G1 {place}: {label} {spelling} belongs to fragment owner {}, not the \
                         registry's crate {}",
                        catalog::text(listed, "owner").unwrap_or("?"),
                        file.member
                    ));
                }
                if !registry.locations
                    && catalog::text(listed, "category") != entry.category.as_deref()
                {
                    violations.push(format!(
                        "G1 {place}: code {spelling} has category {} in its registry and {} in \
                         its fragment",
                        entry.category.as_deref().unwrap_or("?"),
                        catalog::text(listed, "category").unwrap_or("?")
                    ));
                }
                if !registry.locations
                    && catalog::text(listed, "ladder_class") != entry.ladder_class.as_deref()
                {
                    violations.push(format!(
                        "G1 {place}: code {spelling} has ladder_class {} in its registry and {} \
                         in its fragment",
                        entry.ladder_class.as_deref().unwrap_or("?"),
                        catalog::text(listed, "ladder_class").unwrap_or("?")
                    ));
                }
            }
        }
    }
    for (table, registered, label) in [
        (&catalog.codes, &codes, "code"),
        (&catalog.locations, &locations, "location"),
    ] {
        for (name, entry) in table {
            if catalog::text(entry, "status") == Some("active") && !registered.contains(name) {
                violations.push(format!(
                    "G1 {label} {name} (owner {}) has no registry entry",
                    catalog::text(entry, "owner").unwrap_or("?")
                ));
            }
        }
    }
    violations
}

/// G2-G5: findings outside the allow list.
pub fn check_channels(workspace: &Workspace) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &workspace.files {
        for finding in &file.scan.findings {
            if !finding_allowed(&file.member, &file.path, finding) {
                violations.push(format!(
                    "{:?} {}:{}: {}",
                    finding.check, file.path, finding.line, finding.detail
                ));
            }
        }
    }
    violations
}

/// G6: no persisted contract type holds a text member (Workflow #378 model section 9), outside
/// the G6 entries of the allow list.
pub fn check_persisted_prose(workspace: &Workspace) -> Vec<String> {
    let mut violations = Vec::new();
    let mut types = Vec::new();
    for file in workspace
        .files
        .iter()
        .filter(|file| file.member == CONTRACT_MEMBER)
    {
        let found = fs::read_to_string(workspace.root.join(&file.path))
            .map_err(|error| error.to_string())
            .and_then(|text| persisted::serde_types(&file.path, &text));
        match found {
            Ok(found) => types.extend(found),
            Err(error) => violations.push(format!("G6 {}: {error}", file.path)),
        }
    }
    for (path, finding) in persisted::persisted_prose(&types) {
        if !finding_allowed(CONTRACT_MEMBER, &path, &finding) {
            violations.push(format!(
                "{:?} {}:{}: {}",
                finding.check, path, finding.line, finding.detail
            ));
        }
    }
    violations
}

/// G7: `outcome_keys!` equals the catalog key table (names, types, record fields), and every
/// vocabulary built from an enum equals its catalog vocabulary. A `setup_` or `console_` key is a
/// violation on either side: UI-only keys live only in the UI's own table (Workflow #378, board
/// comment 6108644961).
pub fn check_keys(workspace: &Workspace) -> Vec<String> {
    let catalog = match &workspace.catalog {
        Ok(catalog) => catalog,
        Err(errors) => return prefixed("G7", errors),
    };
    let mut violations = Vec::new();
    let mut tables = Vec::new();
    for file in &workspace.files {
        for table in &file.scan.facts.key_tables {
            if file.member == CONTRACT_MEMBER {
                tables.push(table);
            } else {
                violations.push(format!(
                    "G7 {}: only {CONTRACT_MEMBER} holds the key table",
                    file.path
                ));
            }
        }
    }
    match tables.as_slice() {
        [table] => compare_key_table(catalog, table, &mut violations),
        _ => violations.push(format!(
            "G7 {CONTRACT_MEMBER} must hold exactly one outcome_keys! table, found {}",
            tables.len()
        )),
    }

    let mut built = BTreeMap::<String, String>::new();
    for file in &workspace.files {
        for vocabulary in &file.scan.facts.vocabularies {
            let place = format!("{} enum {}", file.path, vocabulary.name);
            if let Some(first) = built.insert(vocabulary.vocabulary.clone(), place.clone()) {
                violations.push(format!(
                    "G7 {place}: vocabulary {} is already built by {first}",
                    vocabulary.vocabulary
                ));
            }
            compare_tokens(
                catalog,
                &vocabulary.vocabulary,
                &vocabulary.tokens,
                &place,
                &mut violations,
            );
        }
    }
    for (name, entry) in &catalog.vocabularies {
        let Some(origin) = entry.get("source").and_then(|origin| origin.as_object()) else {
            continue;
        };
        let (Some(file), Some(enum_name)) =
            (catalog::text(origin, "file"), catalog::text(origin, "enum"))
        else {
            continue;
        };
        let place = format!("vocabulary {name} source {file} enum {enum_name}");
        let tokens = fs::read_to_string(workspace.root.join(file))
            .map_err(|error| error.to_string())
            .and_then(|text| source::enum_tokens(&text, enum_name));
        match tokens {
            Ok(tokens) => compare_tokens(catalog, name, &tokens, &place, &mut violations),
            Err(error) => violations.push(format!("G7 {place}: {error}")),
        }
    }
    violations
}

/// G8: names, prefixes and reserved prefixes.
pub fn check_naming(workspace: &Workspace) -> Vec<String> {
    match &workspace.catalog {
        Ok(catalog) => catalog::check_naming(catalog),
        Err(errors) => prefixed("G8", errors),
    }
}

/// G9: the committed merged file is the merge, and released settled entries hold.
pub fn check_contract(workspace: &Workspace) -> Vec<String> {
    match &workspace.catalog {
        Ok(catalog) => catalog::check_contract(&workspace.root, catalog),
        Err(errors) => prefixed("G9", errors),
    }
}

const CONTRACT_MEMBER: &str = "crates/actingcommand-contract";

fn compare_key_table(catalog: &Catalog, table: &[KeyLine], violations: &mut Vec<String>) {
    let lines = table
        .iter()
        .map(|line| (line.name.as_str(), line))
        .collect::<BTreeMap<_, _>>();
    if lines.len() != table.len() {
        violations.push("G7 outcome_keys! lists a key twice".to_owned());
    }
    for (name, entry) in &catalog.keys {
        if is_ui_key(name) {
            violations.push(format!(
                "G7 catalog key {name}: the setup_ and console_ prefixes are the UI's; its keys \
                 belong to the UI's own table"
            ));
            continue;
        }
        let Some(line) = lines.get(name.as_str()) else {
            violations.push(format!(
                "G7 catalog key {name} is missing from outcome_keys!"
            ));
            continue;
        };
        let kind = entry
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");
        if kind != line.kind {
            violations.push(format!(
                "G7 key {name} is {kind} in the catalog and {} in outcome_keys!",
                line.kind
            ));
        }
        let fields = entry
            .get("fields")
            .and_then(|fields| fields.as_object())
            .map(|fields| {
                fields
                    .iter()
                    .map(|(field, presence)| (field.clone(), presence.as_str() == Some("required")))
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        if fields != line.fields.iter().cloned().collect::<BTreeSet<_>>() {
            violations.push(format!(
                "G7 key {name} has other record fields in the catalog than in outcome_keys!"
            ));
        }
    }
    for name in lines.keys() {
        if is_ui_key(name) {
            violations.push(format!(
                "G7 outcome_keys! key {name}: the setup_ and console_ prefixes are the UI's; its \
                 keys belong to the UI's own table"
            ));
        } else if !catalog.keys.contains_key(*name) {
            violations.push(format!("G7 outcome_keys! key {name} is not in the catalog"));
        }
    }
}

fn compare_tokens(
    catalog: &Catalog,
    vocabulary: &str,
    tokens: &[String],
    place: &str,
    violations: &mut Vec<String>,
) {
    let Some(listed) = catalog
        .vocabularies
        .get(vocabulary)
        .and_then(|entry| entry.get("tokens"))
        .and_then(|tokens| tokens.as_object())
    else {
        violations.push(format!("G7 {place}: no catalog vocabulary {vocabulary}"));
        return;
    };
    let built = tokens.iter().cloned().collect::<BTreeSet<_>>();
    let listed = listed.keys().cloned().collect::<BTreeSet<_>>();
    if built.len() != tokens.len() {
        violations.push(format!("G7 {place}: a token is listed twice"));
    }
    for token in built.difference(&listed) {
        violations.push(format!(
            "G7 {place}: token {token} is not in catalog vocabulary {vocabulary}"
        ));
    }
    for token in listed.difference(&built) {
        violations.push(format!(
            "G7 {place}: catalog vocabulary {vocabulary} token {token} is not in the enum"
        ));
    }
}

fn is_ui_key(name: &str) -> bool {
    catalog::UI_PREFIXES
        .into_iter()
        .any(|prefix| name.starts_with(prefix))
}

fn prefixed(check: &str, errors: &[String]) -> Vec<String> {
    errors
        .iter()
        .map(|error| format!("{check} catalog: {error}"))
        .collect()
}

fn member_fully_allowed(member: &str) -> bool {
    ALLOW_LIST.iter().any(|entry| {
        entry.item.is_none()
            && entry.path == member
            && CHANNELS.iter().all(|check| entry.checks.contains(check))
    })
}

fn finding_allowed(member: &str, path: &str, finding: &Finding) -> bool {
    ALLOW_LIST.iter().any(|entry| {
        entry.checks.contains(&finding.check)
            && match entry.item {
                None => entry.path == member,
                Some(item) => entry.path == path && finding.item.as_deref() == Some(item),
            }
    })
}

/// The `members` of the root `Cargo.toml`.
pub fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("Cargo.toml: cannot read: {error}"))?;
    let start = manifest
        .find("members = [")
        .ok_or_else(|| "Cargo.toml: no workspace members list".to_owned())?;
    let rest = &manifest[start + "members = [".len()..];
    let end = rest
        .find(']')
        .ok_or_else(|| "Cargo.toml: the members list does not end".to_owned())?;
    Ok(rest[..end]
        .split(',')
        .map(|member| member.trim().trim_matches('"').to_owned())
        .filter(|member| !member.is_empty())
        .collect())
}

fn is_outcome_module(path: &str) -> bool {
    path == "crates/actingcommand-contract/src/outcome.rs"
        || path.starts_with("crates/actingcommand-contract/src/outcome/")
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The crate roots of a member: the `[lib]` and `[[bin]]` paths its manifest names (else
/// `src/lib.rs`), plus `src/main.rs` and the `src/bin` targets Cargo discovers.
fn crate_roots(root: &Path, member: &str, errors: &mut Vec<String>) -> Vec<PathBuf> {
    let directory = root.join(member);
    let manifest = match fs::read_to_string(directory.join("Cargo.toml")) {
        Ok(manifest) => manifest,
        Err(error) => {
            errors.push(format!("{member}/Cargo.toml: cannot read: {error}"));
            return Vec::new();
        }
    };
    let (library, binaries) = manifest_target_paths(&manifest);
    let source = directory.join("src");
    let mut declared = binaries
        .iter()
        .map(|path| directory.join(path))
        .collect::<Vec<_>>();
    if let Some(path) = &library {
        declared.push(directory.join(path));
    }
    for path in &declared {
        if !path.is_file() {
            errors.push(format!(
                "{member}/Cargo.toml: target path {} is not a file",
                relative_path(root, path)
            ));
        }
    }
    let mut roots = declared
        .into_iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    let defaults = if library.is_some() {
        vec![source.join("main.rs")]
    } else {
        vec![source.join("lib.rs"), source.join("main.rs")]
    };
    roots.extend(defaults.into_iter().filter(|path| path.is_file()));
    if let Ok(entries) = fs::read_dir(source.join("bin")) {
        let mut bins = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter_map(|path| {
                if path.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
                    Some(path)
                } else {
                    Some(path.join("main.rs")).filter(|main| main.is_file())
                }
            })
            .collect::<Vec<_>>();
        bins.sort();
        roots.extend(bins);
    }
    roots
}

/// The `path` of the `[lib]` section and of each `[[bin]]` section of a manifest.
fn manifest_target_paths(manifest: &str) -> (Option<String>, Vec<String>) {
    let mut section = "";
    let mut library = None;
    let mut binaries = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let Some(value) = line
            .strip_prefix("path")
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('='))
        else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_owned();
        match section {
            "[lib]" => library = Some(value),
            "[[bin]]" => binaries.push(value),
            _ => {}
        }
    }
    (library, binaries)
}

/// Walks one member's module tree from its crate roots, skipping test modules.
fn scan_member(
    root: &Path,
    member: &str,
    channels: bool,
    registered: &BTreeSet<String>,
    files: &mut Vec<ScannedFile>,
    errors: &mut Vec<String>,
) {
    let roots = crate_roots(root, member, errors);
    if roots.is_empty() {
        errors.push(format!("{member}: no crate root under src"));
        return;
    }
    let mut seen = BTreeSet::new();
    let mut queue = roots
        .into_iter()
        .map(|path| (path, true))
        .collect::<Vec<_>>();
    while let Some((path, crate_root)) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let relative = relative_path(root, &path);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{relative}: cannot read: {error}"));
                continue;
            }
        };
        let scope = channels.then_some(ChannelScope {
            registered,
            outcome_module: is_outcome_module(&relative),
        });
        let scan = match source::scan_file(&text, scope) {
            Ok(scan) => scan,
            Err(error) => {
                errors.push(format!("{relative}: does not parse: {error}"));
                continue;
            }
        };
        for declaration in scan.modules.iter().filter(|declaration| !declaration.test) {
            match module_file(&path, crate_root, declaration) {
                Some(child) => queue.push((child, false)),
                None => errors.push(format!(
                    "{relative}: no file for module {}",
                    declaration.names.join("::")
                )),
            }
        }
        files.push(ScannedFile {
            member: member.to_owned(),
            path: relative,
            scan,
        });
    }
}

/// The file of an out-of-line module declared in `file`.
fn module_file(file: &Path, crate_root: bool, declaration: &ModuleDeclaration) -> Option<PathBuf> {
    let directory = file.parent()?;
    let (name, inline) = declaration.names.split_last()?;
    let mod_rs = file
        .file_name()
        .is_some_and(|file_name| file_name == "mod.rs");
    if let Some(path) = &declaration.path {
        let mut base = directory.to_path_buf();
        if !inline.is_empty() {
            if !crate_root && !mod_rs {
                base.push(file.file_stem()?);
            }
            base.extend(inline);
        }
        let candidate = base.join(path);
        return candidate.is_file().then_some(candidate);
    }
    let mut base = directory.to_path_buf();
    if !crate_root && !mod_rs {
        base.push(file.file_stem()?);
    }
    base.extend(inline);
    [
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}
