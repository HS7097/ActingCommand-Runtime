// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::codes::{HostCode, HostLocation};
use actingcommand_contract::codes::ContractLocation;
use actingcommand_contract::outcome::Code;

impl HostShared {
    pub(super) fn debug_package(
        &self,
        validated: &ValidatedRuntimeRequest<'_>,
        request: &PackageDebugRequest,
    ) -> Result<OperationSuccess, RequestFailure> {
        let links = validated.event_links(None, None, None);
        self.append_event(
            EventSeverity::Info,
            EventSource::Lab,
            OriginModule::Actinglab,
            EventActor::Lab,
            links.clone(),
            // Workflow #389 U-2: with the channel the door tagged the request with.
            CommandPayloadDraft::received_on_channel(
                EventAction::RuntimeDebugPackage,
                validated.channel(),
                AuditInput::new(),
            ),
        )?;

        let summary = match inspect_debug_package(request) {
            Ok(summary) => summary,
            Err(error) => {
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::rejected(
                        EventAction::RuntimeDebugPackage,
                        DiagnosticCode::CommandRejected,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    error,
                    RuntimeReceiptState::Failed,
                    Some(terminal(&event)),
                ));
            }
        };
        let package_name = Path::new(request.package_path())
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                RequestFailure::request(
                    debug_package_error(HostCode::DebugPackageNameInvalid),
                    RuntimeReceiptState::Failed,
                    None,
                )
            })?;
        // Workflow #336 PKG-S2: the evidence names the package by its file name only.
        let package =
            EvidencePackage::new(package_name, PackageVerification::Passed).map_err(|error| {
                RequestFailure::request(
                    RuntimeHostError::relayed_request(
                        error.code(),
                        ContractLocation::DebugPackage.as_str(),
                        RuntimeErrorCode::PackageInvalid,
                    ),
                    RuntimeReceiptState::Failed,
                    None,
                )
            })?;
        let mut debug_runs = lock(&self.debug_runs, HostLocation::LockRuntimeDebugRuns)?;
        let existing = debug_runs.get(&validated.correlation_id()).cloned();
        let context = match existing {
            Some(existing)
                if existing.package == package && existing.package_summary == summary =>
            {
                existing
            }
            Some(_) => {
                let event = self.append_event(
                    EventSeverity::Error,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    links,
                    CommandPayloadDraft::rejected(
                        EventAction::RuntimeDebugPackage,
                        DiagnosticCode::CommandRejected,
                        EffectDisposition::NotPerformed,
                        AuditInput::new(),
                    ),
                )?;
                return Err(RequestFailure::request(
                    RuntimeHostError::request(
                        HostCode::RuntimeDebugContextConflict,
                        ContractLocation::DebugPackage,
                        RuntimeErrorCode::PackageInvalid,
                    ),
                    RuntimeReceiptState::Denied,
                    Some(terminal(&event)),
                ));
            }
            None => {
                let run_id = self.events.issuer().mint_run_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        HostCode::RunIdIssueFailed,
                        ContractLocation::DebugPackage,
                        RuntimeErrorCode::RuntimeHalted,
                    ))
                })?;
                let task_id = self.events.issuer().mint_task_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        HostCode::TaskIdIssueFailed,
                        ContractLocation::DebugPackage,
                        RuntimeErrorCode::RuntimeHalted,
                    ))
                })?;
                let task_links = validated.task_event_links(task_id, run_id);
                self.append_event(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    task_links.clone(),
                    TaskPayloadDraft::requested(
                        EventAction::RuntimeDebugPackage,
                        AuditInput::new(),
                    ),
                )?;
                self.append_event(
                    EventSeverity::Info,
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                    task_links,
                    TaskPayloadDraft::started(EventAction::RuntimeDebugPackage, AuditInput::new()),
                )?;
                let context = DebugRunContext {
                    package,
                    package_summary: summary.clone(),
                    run_id,
                    task_id,
                    terminal_outcome: None,
                    completed_export: None,
                };
                debug_runs.insert(validated.correlation_id(), context.clone());
                context
            }
        };
        drop(debug_runs);
        let event = self.append_event(
            EventSeverity::Info,
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
            links,
            CommandPayloadDraft::validated(
                EventAction::RuntimeDebugPackage,
                EffectDisposition::NotPerformed,
                AuditInput::new(),
            ),
        )?;
        Ok(OperationSuccess {
            state: RuntimeReceiptState::Completed,
            terminal: Some(terminal(&event)),
            result: RuntimeResult::PackageDebugCompleted {
                summary: context.package_summary,
            },
            failure_outcome: None,
        })
    }
}

fn inspect_debug_package(request: &PackageDebugRequest) -> RuntimeHostResult<PackageDebugSummary> {
    let path = Path::new(request.package_path());
    if !path.is_absolute() {
        return Err(debug_package_error(HostCode::DebugPackagePathNotAbsolute));
    }
    let instance = ContainmentInstanceId::new("runtime-debug-package")
        .map_err(|_| debug_package_error(HostCode::DebugPackageInstanceInvalid))?;
    let mut containment = Containment::new();
    // Workflow #336 PKG-S1: the path chooses the loader; the request's reference is recorded
    // (the summary's `verified_sha256` until PKG-S4) and compared with nothing.
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(
            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
        ))
        .ok_or_else(|| debug_package_error(HostCode::DebugPackageDeadlineOverflow))?;
    let bundle = containment
        .load_path(&instance, path, request.expected_sha256(), false, deadline)
        .map_err(|_| debug_package_error(HostCode::DebugPackageContainmentFailed))?;
    PackageDebugSummary::new(
        bundle.task_id().as_str(),
        bundle.package_ref().clone(),
        match bundle.layout() {
            PackageLayout::Lab => PackageDebugLayout::Lab,
            PackageLayout::Module => PackageDebugLayout::Module,
        },
        u32::try_from(bundle.entry_count())
            .map_err(|_| debug_package_error(HostCode::DebugPackageEntryCountOverflow))?,
        bundle.resident_bytes(),
        u32::try_from(bundle.task_count())
            .map_err(|_| debug_package_error(HostCode::DebugPackageTaskCountOverflow))?,
        bundle.recognition_pack_path().is_some(),
        bundle.pages_path().is_some(),
        bundle.navigation_path().is_some(),
    )
    .map_err(|_| debug_package_error(HostCode::DebugPackageSummaryInvalid))
}

fn debug_package_error(code: impl Into<Code>) -> RuntimeHostError {
    RuntimeHostError::request(
        code,
        ContractLocation::DebugPackage,
        RuntimeErrorCode::PackageInvalid,
    )
}
