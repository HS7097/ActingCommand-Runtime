// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #395 (LEDG-395, forced-compromise mapping G-02): the offline `actingledger
//! material` read and `actingledger facts --at` give each phase its own deadline, taken when
//! the phase starts, and a deadline that runs out names its phase as the failure's location.
//!
//! Field failure (F:\AC, 10-06): two sealed frames read with `actingledger material` reported
//! `ledger_read_budget_exceeded` during `read_only_snapshot` and no bytes; the two metadata
//! openings, the identity check and the verified read shared one 4000 ms deadline.
//! `actingledger facts --at` reported the same code from the read-only snapshot under its one
//! 30 s deadline.
//!
//! A phase given a budget of zero has run out at its first check; every other phase gets 60 s,
//! far more than the small formal root here needs. The root is the frame fixture of
//! `missing_material.rs`, with its writer closed, as an offline reader finds it.

use super::missing_material::{Capture, capture, formal_root, material_store};
use crate::{GlobalLedger, GlobalLedgerEvidenceConfig};
use actingcommand_artifact_store::ArtifactStore;
use actingcommand_contract::{
    EventQuery, LedgerEventPosition, MAX_RUNTIME_MATERIAL_REPLY_BYTES, ProjectionProfile,
    RuntimeEventQueryPageRequest, RuntimeMaterialReadLimit, RuntimeMaterialReadRequest,
    RuntimeMaterialReadResult, RuntimeMaterialReadState,
};
use actingcommand_ledger_forensics::{
    ForensicFactsPhase, ForensicMaterialPhase, ForensicMaterialRequest, ForensicRuntimeFactsResult,
    read_material_to_per_phase, runtime_facts_at_per_phase,
};
use actingcommand_runtime_database::MaintenanceLimits;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const BYTES: &[u8] = b"sealed frame read offline";
/// A phase's budget when that phase is not the one under test.
const ENOUGH: Duration = Duration::from_secs(60);
const MATERIAL_PHASES: [ForensicMaterialPhase; 3] = [
    ForensicMaterialPhase::Reference,
    ForensicMaterialPhase::CurrentReference,
    ForensicMaterialPhase::VerifiedRead,
];
const FACTS_PHASES: [ForensicFactsPhase; 3] = [
    ForensicFactsPhase::Position,
    ForensicFactsPhase::SnapshotSet,
    ForensicFactsPhase::Changes,
];
/// The deadline codes of the runtime fact read (`contracts/runtime-fact-store.md`, "Offline
/// read"): the ledger's own and the reader's.
const FACTS_DEADLINE_CODES: [&str; 2] = [
    "ledger_read_budget_exceeded",
    "runtime_facts_read_budget_exceeded",
];
/// The deadline codes of the offline material read (`contracts/material-read.md`, LEDG-395): a
/// phase whose deadline runs out keeps the code of the bound that ran out, the ledger's, the
/// artifact store's or the material read's.
const MATERIAL_DEADLINE_CODES: [&str; 3] = [
    "ledger_read_budget_exceeded",
    "artifact_read_budget_exceeded",
    "material_read_budget_exceeded",
];

/// One captured frame in a formal root whose writer is closed.
struct SealedRoot {
    frame: Capture,
    _artifacts: ArtifactStore,
    directory: TempDir,
}

impl SealedRoot {
    fn create() -> Self {
        let directory = tempfile::tempdir().expect("root");
        let (database, ledger) = formal_root(directory.path());
        let artifacts = material_store(directory.path());
        let frame = capture(&ledger, &artifacts, BYTES);
        ledger.close().expect("close the writer");
        drop(database);
        Self {
            frame,
            _artifacts: artifacts,
            directory,
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    /// The frame's `actingledger material` request: the whole frame in one range.
    fn material_request(&self) -> ForensicMaterialRequest {
        ForensicMaterialRequest::new(
            self.path(),
            RuntimeMaterialReadRequest {
                event: LedgerEventPosition {
                    event_id: self.frame.verified.event_id,
                    sequence: self.frame.verified.sequence,
                },
                artifact_id: self.frame.reference.artifact_id,
                snapshot_position: self.frame.verified.sequence,
                byte_count: self.frame.reference.byte_count,
                sha256: self.frame.reference.sha256.clone(),
                expected_run_id: None,
                expected_frame_id: None,
                expected_request_id: None,
                expected_correlation_id: None,
                offset: 0,
                requested_length: u32::try_from(BYTES.len()).expect("length"),
                max_reply_bytes: MAX_RUNTIME_MATERIAL_REPLY_BYTES,
            },
        )
        .expect("material request")
    }
}

/// One offline material read.
struct MaterialRead {
    /// The phases that took a deadline, in order.
    asked: Vec<ForensicMaterialPhase>,
    /// The command's own answer: `<code> during <location>` when it fails.
    answer: Result<(), String>,
    /// The printed report.
    result: RuntimeMaterialReadResult,
}

/// One offline material read in which only `ran_out` (if any) has no time.
fn read_material(root: &SealedRoot, ran_out: Option<ForensicMaterialPhase>) -> MaterialRead {
    let mut asked = Vec::new();
    let mut output = Vec::new();
    let answer = read_material_to_per_phase(root.material_request(), &mut output, |phase| {
        asked.push(phase);
        if Some(phase) == ran_out {
            Duration::ZERO
        } else {
            ENOUGH
        }
    })
    .map_err(|error| format!("{} during {}", error.code(), error.operation()));
    let report: serde_json::Value = serde_json::from_slice(&output).expect("one JSON report");
    assert_eq!(report["command"], "material_read", "{report}");
    let result = serde_json::from_value(report["data"].clone()).expect("material read result");
    MaterialRead {
        asked,
        answer,
        result,
    }
}

/// One offline runtime fact read in which only `ran_out` (if any) has no time: the phases that
/// took a deadline, in order, and the result.
fn read_facts(
    root: &SealedRoot,
    position: u64,
    ran_out: Option<ForensicFactsPhase>,
) -> (Vec<ForensicFactsPhase>, ForensicRuntimeFactsResult) {
    let mut asked = Vec::new();
    let result = runtime_facts_at_per_phase(root.path(), position, |phase| {
        asked.push(phase);
        if Some(phase) == ran_out {
            Duration::ZERO
        } else {
            ENOUGH
        }
    });
    (asked, result)
}

/// A deadline `ENOUGH` from now. A gate file reads no clock itself (test plan v2.2 §2.1 guard
/// 3); as in the crate's gate fixture, the deadline is the one `MaintenanceLimits` computes.
fn deadline_enough_ahead() -> Instant {
    MaintenanceLimits {
        timeout_seconds: ENOUGH.as_secs(),
        ..MaintenanceLimits::default()
    }
    .deadline()
    .expect("a deadline ENOUGH ahead")
}

/// A deadline that has run out: the moment a deadline `ENOUGH` ahead was taken. The ledger's
/// check (`now >= deadline`) finds it reached at once, like a phase budget of zero.
fn deadline_run_out() -> Instant {
    deadline_enough_ahead()
        .checked_sub(ENOUGH)
        .expect("the moment the deadline was taken")
}

/// `actingledger material`: the first lookup, the second lookup under the reader's use guard and
/// the verified read each take their own deadline when they start. A phase whose deadline runs
/// out leaves the report `not_provided` with `budget_exceeded`, no bytes, and that phase as the
/// failure's location, in the report and in the returned error.
#[test]
fn gate_each_material_phase_has_its_own_deadline_and_a_run_out_names_its_phase() {
    let root = SealedRoot::create();

    let MaterialRead {
        asked,
        answer,
        result,
    } = read_material(&root, None);
    assert_eq!(answer, Ok(()));
    assert_eq!(
        result.state,
        RuntimeMaterialReadState::Verified,
        "{result:?}"
    );
    assert_eq!(
        result.chunk.as_ref().map(|chunk| chunk.bytes.as_slice()),
        Some(BYTES)
    );
    assert_eq!(asked, MATERIAL_PHASES, "one deadline per phase, in order");

    for (index, phase) in MATERIAL_PHASES.into_iter().enumerate() {
        let MaterialRead {
            asked,
            answer,
            result,
        } = read_material(&root, Some(phase));
        let location = phase.location().as_str();
        assert_eq!(
            asked,
            MATERIAL_PHASES[..=index],
            "{phase:?}: the phases before it finished under their own deadlines"
        );
        assert_eq!(
            result.state,
            RuntimeMaterialReadState::NotProvided,
            "{result:?}"
        );
        assert_eq!(result.limit, Some(RuntimeMaterialReadLimit::BudgetExceeded));
        assert!(result.chunk.is_none(), "{phase:?}: no bytes");
        let failure = result.failure.as_ref().expect("the failure");
        assert!(
            MATERIAL_DEADLINE_CODES.contains(&failure.code.as_str()),
            "{phase:?}: a deadline code: {failure:?}"
        );
        assert_eq!(failure.operation, location, "{phase:?}: {failure:?}");
        let answer = answer.expect_err("a deadline that ran out fails the command");
        assert_eq!(
            answer,
            format!("{} during {location}", failure.code),
            "{phase:?}"
        );
    }
}

/// `actingledger facts --at`: opening the snapshot and reading the position, finding the newest
/// complete snapshot set, and applying the changes after it each take their own deadline when
/// they start. A phase whose deadline runs out fails with a deadline code and that phase as the
/// failure's location.
#[test]
fn gate_each_runtime_facts_phase_has_its_own_deadline_and_a_run_out_names_its_phase() {
    let root = SealedRoot::create();
    let position = root.frame.verified.sequence;

    let (asked, result) = read_facts(&root, position, None);
    assert!(
        matches!(&result, ForensicRuntimeFactsResult::Available { position: read, .. } if *read == position),
        "{result:?}"
    );
    assert_eq!(asked, FACTS_PHASES, "one deadline per phase, in order");

    for (index, phase) in FACTS_PHASES.into_iter().enumerate() {
        let (asked, result) = read_facts(&root, position, Some(phase));
        assert_eq!(
            asked,
            FACTS_PHASES[..=index],
            "{phase:?}: the phases before it finished under their own deadlines"
        );
        let ForensicRuntimeFactsResult::Failed {
            code, operation, ..
        } = &result
        else {
            panic!("{phase:?}: a deadline that ran out fails the read: {result:?}");
        };
        assert!(FACTS_DEADLINE_CODES.contains(code), "{phase:?}: {result:?}");
        assert_eq!(
            *operation,
            phase.location().as_str(),
            "{phase:?}: {result:?}"
        );
    }
}

/// The read-only snapshot that the runtime fact read opens in its first phase serves the later
/// phases under each phase's own deadline: a deadline that has run out refuses the next page, and
/// the next phase's deadline lets the same snapshot read again.
#[test]
fn gate_a_metadata_snapshot_reads_under_the_deadline_of_the_current_phase() {
    let root = SealedRoot::create();
    let mut snapshot = GlobalLedger::open_metadata(
        GlobalLedgerEvidenceConfig::new(root.path()).with_deadline(deadline_enough_ahead()),
    )
    .expect("metadata snapshot");
    let request = RuntimeEventQueryPageRequest::default()
        .at_snapshot(snapshot.latest_sequence())
        .expect("page request");
    let query = EventQuery::default();

    snapshot.set_read_deadline(deadline_run_out());
    let refused = snapshot
        .project_view_page(&query, ProjectionProfile::Cli, &request)
        .expect_err("a phase whose deadline ran out reads no page");
    assert_eq!(refused.code(), "ledger_read_budget_exceeded");

    snapshot.set_read_deadline(deadline_enough_ahead());
    let page = snapshot
        .project_view_page(&query, ProjectionProfile::Cli, &request)
        .expect("the next phase reads under its own deadline");
    assert!(!page.events().is_empty());
}
