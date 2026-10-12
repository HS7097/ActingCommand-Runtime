// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger forensics' outcome codes and locations (Workflow #378). Every entry is listed in the
//! fragment `contracts/outcome-codes/ledger-forensics.json`; the outcome guard (G1) checks both
//! directions. Spellings several owners share are the contract's (`SharedCode`, `ContractLocation`,
//! decision D3). Every entry added by A2c is pending (B3 settles them).

actingcommand_contract::outcome_codes! {
    /// Ledger forensics' codes: offline ledger queries, views and material reads.
    pub enum ForensicsCode {
        ArtifactRedactionPending => "artifact_redaction_pending": error / none,
        DiagnosticJsonInvalid => "diagnostic_json_invalid": error / none,
        DiagnosticSchemaUnavailable => "diagnostic_schema_unavailable": error / none,
        EffectiveConfigurationCaptureEvidenceMissing => "effective_configuration_capture_evidence_missing": error / none,
        EffectiveConfigurationFieldsInvalid => "effective_configuration_fields_invalid": error / none,
        EffectiveConfigurationSchemaUnsupported => "effective_configuration_schema_unsupported": error / none,
        EffectiveConfigurationSourceLinksMismatch => "effective_configuration_source_links_mismatch": error / none,
        EffectiveConfigurationSourceMissing => "effective_configuration_source_missing": error / none,
        InstanceBindingMalformed => "instance_binding_malformed": error / none,
        InstanceBindingsIncomplete => "instance_bindings_incomplete": error / none,
        InstancePortConflict => "instance_port_conflict": error / none,
        InstancePortUnknown => "instance_port_unknown": error / none,
        InvalidEventFilter => "invalid_event_filter": error / none,
        InvalidEventPage => "invalid_event_page": error / none,
        InvalidEvidenceArchivePath => "invalid_evidence_archive_path": error / none,
        InvalidRequestId => "invalid_request_id": error / none,
        InvalidSignatureReplay => "invalid_signature_replay": error / none,
        InvalidStateRoot => "invalid_state_root": error / none,
        InvalidTaskRecordPage => "invalid_task_record_page": error / none,
        InvalidViewOptions => "invalid_view_options": error / none,
        LegacyDiagnosticInvalid => "legacy_diagnostic_invalid": error / none,
        LegacyDiagnosticReadLimit => "legacy_diagnostic_read_limit": error / none,
        MaterialNotProvided => "material_not_provided": error / none,
        MaterialReadEncodeFailed => "material_read_encode_failed": error / none,
        MaterialReadOutputFailed => "material_read_output_failed": error / none,
        ReportSerializationFailed => "report_serialization_failed": error / none,
        RuntimeFactsNotAvailable => "runtime_facts_not_available": error / none,
        RuntimeFactsPositionMissing => "runtime_facts_position_missing": error / none,
        RuntimeFactsReadBudgetExceeded => "runtime_facts_read_budget_exceeded": error / none,
        RuntimeFactsSourceIncomplete => "runtime_facts_source_incomplete": error / none,
        StabilityArtifactTooLarge => "stability_artifact_too_large": error / none,
        StabilityFieldsInvalid => "stability_fields_invalid": error / none,
        StabilitySchemaUnsupported => "stability_schema_unsupported": error / none,
        StabilitySourceLinksMismatch => "stability_source_links_mismatch": error / none,
        TaskDiagnosticCursorInvalid => "task_diagnostic_cursor_invalid": error / none,
        TaskDiagnosticCursorMismatch => "task_diagnostic_cursor_mismatch": error / none,
        TaskDiagnosticCursorOutsideWindow => "task_diagnostic_cursor_outside_window": error / none,
        TaskDiagnosticFramingInvalid => "task_diagnostic_framing_invalid": error / none,
        TaskDiagnosticHeaderInvalid => "task_diagnostic_header_invalid": error / none,
        TaskDiagnosticIdentityMismatch => "task_diagnostic_identity_mismatch": error / none,
        TaskDiagnosticIntegrityFailed => "task_diagnostic_integrity_failed": error / none,
        TaskDiagnosticOpenFailed => "task_diagnostic_open_failed": error / none,
        TaskDiagnosticPageLimit => "task_diagnostic_page_limit": error / none,
        TaskDiagnosticReadFailed => "task_diagnostic_read_failed": error / none,
        TaskDiagnosticRecordInvalid => "task_diagnostic_record_invalid": error / none,
        TaskDiagnosticRecordLimit => "task_diagnostic_record_limit": error / none,
        TaskDiagnosticRecordOrderInvalid => "task_diagnostic_record_order_invalid": error / none,
        TaskDiagnosticTrailingData => "task_diagnostic_trailing_data": error / none,
        TaskDiagnosticUnsealed => "task_diagnostic_unsealed": error / none,
    }
}

actingcommand_contract::outcome_locations! {
    /// Ledger forensics' operations and stages (the `operation` of its errors).
    pub enum ForensicsLocation {
        BoundStabilityArtifact => "bound_stability_artifact",
        ParseEventQuery => "parse_event_query",
        ProjectEffectiveConfiguration => "project_effective_configuration",
        ProjectStabilityArtifact => "project_stability_artifact",
        ReadInstanceBindings => "read_instance_bindings",
        ReadLedgerMaterial => "read_ledger_material",
        ReadRuntimeFactsAt => "read_runtime_facts_at",
        ReadTaskDiagnostic => "read_task_diagnostic",
        RenderStorageSnapshot => "render_storage_snapshot",
        ResolveInstancePort => "resolve_instance_port",
        SerializeForensicExport => "serialize_forensic_export",
        TaskEvidence => "task_evidence",
        ValidateEventFilter => "validate_event_filter",
        ValidateEventPage => "validate_event_page",
        ValidateForensicReplay => "validate_forensic_replay",
        ValidateForensicRequest => "validate_forensic_request",
        ValidatePerformancePage => "validate_performance_page",
        ValidateSignatureReplay => "validate_signature_replay",
        ValidateStabilityPage => "validate_stability_page",
        ValidateViewRequest => "validate_view_request",
    }
}
