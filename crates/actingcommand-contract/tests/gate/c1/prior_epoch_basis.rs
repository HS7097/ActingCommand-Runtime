// SPDX-License-Identifier: AGPL-3.0-only

//! G11, inventory key K9, the contract half: the basis of a prior-epoch close
//! (`contracts/ledger-store.md`, "Every import and scope close carries a `basis`"). The
//! classifier from a journal's shape to a reason belongs to the ledger.

use actingcommand_contract::{
    OwnerClosedRevision, OwnerEpoch, OwnerEpochCloseEvidence, OwnerResourceDisposition,
    PriorEpochCloseBasis, PriorEpochCloseFact, PriorEpochOwnerImport, UnprovenReason,
};
use serde_json::json;

#[test]
fn gate_prior_epoch_basis_values_round_trip_their_wire_form() {
    for (wire, expected) in [
        (json!({ "kind": "proven" }), PriorEpochCloseBasis::Proven),
        (
            json!({ "kind": "unproven", "reason": "legacy_journal" }),
            PriorEpochCloseBasis::Unproven {
                reason: UnprovenReason::LegacyJournal,
            },
        ),
        (
            json!({ "kind": "unproven", "reason": "process_exit_only" }),
            PriorEpochCloseBasis::Unproven {
                reason: UnprovenReason::ProcessExitOnly,
            },
        ),
        (
            json!({ "kind": "unproven", "reason": "proof_missing" }),
            PriorEpochCloseBasis::Unproven {
                reason: UnprovenReason::ProofMissing,
            },
        ),
    ] {
        let decoded: PriorEpochCloseBasis =
            serde_json::from_value(wire.clone()).expect("decode the basis");
        assert_eq!(decoded, expected, "{wire}");
        assert_eq!(
            serde_json::to_value(decoded).expect("encode the basis"),
            wire
        );
    }
}

const STARTED_AT_UNIX_MS: u64 = 1_760_000_000_000;

fn owner_epoch(index: u8) -> OwnerEpoch {
    serde_json::from_value(json!(format!("epoch_{index:032x}"))).expect("decode an owner epoch")
}

/// The native reader's evidence for the subject epoch: a journal block with the given positive
/// close revision and close suffix, or no journal block for the owner at all.
fn evidence(
    block: bool,
    confirmed_revision: u64,
    suffix: Vec<OwnerClosedRevision>,
) -> OwnerEpochCloseEvidence {
    OwnerEpochCloseEvidence {
        schema_version: "actingcommand.runtime-owner.v2".to_owned(),
        subject: owner_epoch(1),
        pid: if block { 4242 } else { 0 },
        started_at_unix_ms: if block { STARTED_AT_UNIX_MS } else { 0 },
        first_revision: u64::from(block),
        confirmed_revision,
        final_revision: if block { 4 } else { 0 },
        journal_through_revision: if block { 4 } else { 9 },
        journal_bytes: 2048,
        journal_sha256: "2991d12a23bce2d3dcc09d77fccc31a323b3cca97a8aa63019bf413682d5375b"
            .to_owned(),
        suffix,
    }
}

/// The replayable positive suffix of revision 3: ConfirmedClosed, then the epoch's inactive end.
fn close_suffix() -> Vec<OwnerClosedRevision> {
    vec![
        OwnerClosedRevision {
            revision: 3,
            active: true,
            closed_at_unix_ms: None,
            disposition: OwnerResourceDisposition::ConfirmedClosed,
        },
        OwnerClosedRevision {
            revision: 4,
            active: false,
            closed_at_unix_ms: Some(STARTED_AT_UNIX_MS + 100_000),
            disposition: OwnerResourceDisposition::None,
        },
    ]
}

fn positive_close() -> OwnerEpochCloseEvidence {
    evidence(true, 3, close_suffix())
}

fn block_without_close() -> OwnerEpochCloseEvidence {
    evidence(true, 0, Vec::new())
}

fn no_block() -> OwnerEpochCloseEvidence {
    evidence(false, 0, Vec::new())
}

/// An owner import written by another epoch, with `0 < scope_upper_sequence <= through_sequence`.
fn import(basis: PriorEpochCloseBasis, evidence: OwnerEpochCloseEvidence) -> PriorEpochCloseFact {
    PriorEpochCloseFact::OwnerImported(PriorEpochOwnerImport {
        evidence,
        basis,
        through_sequence: 10,
        scope_upper_sequence: 5,
    })
}

#[test]
fn gate_prior_epoch_import_validation_follows_its_basis() {
    let writer = owner_epoch(2);
    let unproven = |reason| PriorEpochCloseBasis::Unproven { reason };
    let mut rows = vec![
        // A proven import seals the replayable positive suffix.
        (
            "proven with a positive close",
            PriorEpochCloseBasis::Proven,
            positive_close(),
            true,
        ),
        (
            "proven without a positive close",
            PriorEpochCloseBasis::Proven,
            block_without_close(),
            false,
        ),
        // `proof_missing`: no journal epoch for the owner, or one with no positive close.
        (
            "proof_missing without a journal block",
            unproven(UnprovenReason::ProofMissing),
            no_block(),
            true,
        ),
        (
            "proof_missing with a journal block",
            unproven(UnprovenReason::ProofMissing),
            block_without_close(),
            true,
        ),
        // `legacy_journal` and `process_exit_only` describe an existing journal epoch.
        (
            "legacy_journal with a journal block",
            unproven(UnprovenReason::LegacyJournal),
            block_without_close(),
            true,
        ),
        (
            "process_exit_only with a journal block",
            unproven(UnprovenReason::ProcessExitOnly),
            block_without_close(),
            true,
        ),
        (
            "legacy_journal without a journal block",
            unproven(UnprovenReason::LegacyJournal),
            no_block(),
            false,
        ),
        (
            "process_exit_only without a journal block",
            unproven(UnprovenReason::ProcessExitOnly),
            no_block(),
            false,
        ),
    ];
    // An unproven import seals the observation, never a positive close.
    for reason in [
        UnprovenReason::LegacyJournal,
        UnprovenReason::ProcessExitOnly,
        UnprovenReason::ProofMissing,
    ] {
        rows.push((
            "unproven with a positive close",
            unproven(reason),
            positive_close(),
            false,
        ));
        rows.push((
            "unproven with a positive close revision",
            unproven(reason),
            evidence(true, 3, Vec::new()),
            false,
        ));
        rows.push((
            "unproven with a close suffix",
            unproven(reason),
            evidence(true, 0, close_suffix()),
            false,
        ));
    }

    for (row, basis, evidence, accepted) in rows {
        assert_eq!(
            import(basis, evidence).validate(writer).is_ok(),
            accepted,
            "{row} ({basis:?})"
        );
    }
}
