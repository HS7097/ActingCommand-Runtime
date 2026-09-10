// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_contract::{SignaturePageRequest, SignatureReplayPage};
use actingcommand_ledger::signatures::{SignatureCatalog, SignaturePrefix, replay_signatures};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicSignatureRequest {
    pub input_state_root: PathBuf,
    pub catalog_state_root: PathBuf,
    pub input_through: u64,
    pub catalog_through: u64,
    pub page: SignaturePageRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignatureReplayReport {
    pub input_state_root: PathBuf,
    pub catalog_state_root: PathBuf,
    pub input_snapshot: OpenReport,
    pub catalog_snapshot: OpenReport,
    pub evidence_complete: bool,
    pub page: SignatureReplayPage,
}

pub fn replay_signatures_read_only(
    request: ForensicSignatureRequest,
) -> ForensicResult<ForensicOutput> {
    if request.input_state_root.as_os_str().is_empty()
        || request.catalog_state_root.as_os_str().is_empty()
        || request.input_through == 0
        || request.catalog_through == 0
        || request.page.validate().is_err()
    {
        return Err(ForensicError::new(
            "invalid_signature_replay",
            "validate_signature_replay",
            "both state roots, positive frozen bounds and a bounded page are required",
        ));
    }
    let open = |root: &Path| {
        GlobalLedger::open_evidence(GlobalLedgerEvidenceConfig::new(root), |reference| {
            verify_projected_read_only(root, reference).ok()
        })
        .map_err(map_ledger_error)
    };
    let input_snapshot = open(&request.input_state_root)?;
    let catalog_snapshot = open(&request.catalog_state_root)?;
    let input = SignaturePrefix::from_evidence(&input_snapshot, request.input_through)
        .map_err(map_ledger_error)?;
    let catalog_prefix = SignaturePrefix::from_evidence(&catalog_snapshot, request.catalog_through)
        .map_err(map_ledger_error)?;
    let catalog = SignatureCatalog::from_prefix(&catalog_prefix);
    let page = replay_signatures(&input, &catalog, &request.page).map_err(map_ledger_error)?;
    Ok(ForensicOutput::Machine(ForensicReport::Signatures(
        Box::new(SignatureReplayReport {
            input_state_root: request.input_state_root,
            catalog_state_root: request.catalog_state_root,
            input_snapshot: open_report(&input_snapshot),
            catalog_snapshot: open_report(&catalog_snapshot),
            evidence_complete: page.evidence_complete(),
            page,
        }),
    )))
}
