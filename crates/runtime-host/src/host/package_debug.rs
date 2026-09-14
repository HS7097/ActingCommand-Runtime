// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

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
            CommandPayloadDraft::received(EventAction::RuntimeDebugPackage, AuditInput::new()),
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
                    debug_package_error("debug_package_name_invalid"),
                    RuntimeReceiptState::Failed,
                    None,
                )
            })?;
        let package = EvidencePackage::new(
            package_name,
            summary.verified_sha256(),
            PackageVerification::Passed,
        )
        .map_err(|error| {
            RequestFailure::request(
                debug_package_error(error.code()),
                RuntimeReceiptState::Failed,
                None,
            )
        })?;
        let mut debug_runs = lock(&self.debug_runs, "lock_runtime_debug_runs")?;
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
                        "runtime_debug_context_conflict",
                        "debug_package",
                        RuntimeErrorCode::PackageInvalid,
                    ),
                    RuntimeReceiptState::Denied,
                    Some(terminal(&event)),
                ));
            }
            None => {
                let run_id = self.events.issuer().mint_run_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "run_id_issue_failed",
                        "debug_package",
                        RuntimeErrorCode::RuntimeFatal,
                    ))
                })?;
                let task_id = self.events.issuer().mint_task_id().map_err(|_| {
                    RequestFailure::poison_without_terminal(RuntimeHostError::fatal(
                        "task_id_issue_failed",
                        "debug_package",
                        RuntimeErrorCode::RuntimeFatal,
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
        })
    }
}

fn inspect_debug_package(request: &PackageDebugRequest) -> RuntimeHostResult<PackageDebugSummary> {
    let path = Path::new(request.package_path());
    if !path.is_absolute() {
        return Err(debug_package_error("debug_package_path_not_absolute"));
    }
    let instance = ContainmentInstanceId::new("runtime-debug-package")
        .map_err(|_| debug_package_error("debug_package_instance_invalid"))?;
    let mut containment = Containment::new();
    let bundle = if let Some(hash) = request.expected_sha256().legacy_sha256() {
        let file =
            fs::File::open(path).map_err(|_| debug_package_error("debug_package_open_failed"))?;
        let metadata = file
            .metadata()
            .map_err(|_| debug_package_error("debug_package_metadata_failed"))?;
        if !metadata.is_file() || metadata.len() > DEFAULT_MAX_COMPRESSED_BYTES {
            return Err(debug_package_error("debug_package_file_invalid"));
        }
        let mut bytes = Vec::new();
        file.take(DEFAULT_MAX_COMPRESSED_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| debug_package_error("debug_package_read_failed"))?;
        if bytes.len() as u64 > DEFAULT_MAX_COMPRESSED_BYTES {
            return Err(debug_package_error("debug_package_file_too_large"));
        }
        let expected = Sha256Hash::parse_hex(hash)
            .map_err(|_| debug_package_error("debug_package_hash_invalid"))?;
        containment
            .load(&instance, &bytes, &expected)
            .map_err(|_| debug_package_error("debug_package_containment_failed"))?
    } else {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(
                ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS,
            ))
            .ok_or_else(|| debug_package_error("debug_package_deadline_overflow"))?;
        containment
            .load_path(&instance, path, request.expected_sha256(), false, deadline)
            .map_err(|_| debug_package_error("debug_package_containment_failed"))?
    };
    PackageDebugSummary::new(
        bundle.task_id().as_str(),
        bundle.package_ref().clone(),
        match bundle.layout() {
            PackageLayout::Lab => PackageDebugLayout::Lab,
            PackageLayout::Module => PackageDebugLayout::Module,
        },
        u32::try_from(bundle.entry_count())
            .map_err(|_| debug_package_error("debug_package_entry_count_overflow"))?,
        bundle.resident_bytes(),
        u32::try_from(bundle.task_count())
            .map_err(|_| debug_package_error("debug_package_task_count_overflow"))?,
        bundle.recognition_pack_path().is_some(),
        bundle.pages_path().is_some(),
        bundle.navigation_path().is_some(),
    )
    .map_err(|_| debug_package_error("debug_package_summary_invalid"))
}

fn debug_package_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, "debug_package", RuntimeErrorCode::PackageInvalid)
}
