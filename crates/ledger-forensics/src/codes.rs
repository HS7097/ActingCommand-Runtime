// SPDX-License-Identifier: AGPL-3.0-only

//! Ledger forensics' outcome codes and locations (Workflow #378). Every entry is listed in the
//! fragment `contracts/outcome-codes/ledger-forensics.json`; the outcome guard (G1) checks both
//! directions. Spellings several owners share are the contract's (`SharedCode`, `ContractLocation`,
//! decision D3). Every entry added by A2c is pending (B3 settles them).

actingcommand_contract::outcome_codes! {
    /// Ledger forensics' codes: offline ledger queries, views and material reads.
    pub enum ForensicsCode {
        ArtifactRedactionPending => "artifact_redaction_pending": error,
        DiagnosticJsonInvalid => "diagnostic_json_invalid": error,
        DiagnosticSchemaUnavailable => "diagnostic_schema_unavailable": error,
        EffectiveConfigurationCaptureEvidenceMissing => "effective_configuration_capture_evidence_missing": error,
        EffectiveConfigurationFieldsInvalid => "effective_configuration_fields_invalid": error,
        EffectiveConfigurationSchemaUnsupported => "effective_configuration_schema_unsupported": error,
        EffectiveConfigurationSourceLinksMismatch => "effective_configuration_source_links_mismatch": error,
        EffectiveConfigurationSourceMissing => "effective_configuration_source_missing": error,
        InstanceBindingMalformed => "instance_binding_malformed": error,
        InstanceBindingsIncomplete => "instance_bindings_incomplete": error,
        InstancePortConflict => "instance_port_conflict": error,
        InstancePortUnknown => "instance_port_unknown": error,
        InvalidEventFilter => "invalid_event_filter": error,
        InvalidEventPage => "invalid_event_page": error,
        InvalidEvidenceArchivePath => "invalid_evidence_archive_path": error,
        InvalidRequestId => "invalid_request_id": error,
        InvalidSignatureReplay => "invalid_signature_replay": error,
        InvalidStateRoot => "invalid_state_root": error,
        InvalidTaskRecordPage => "invalid_task_record_page": error,
        InvalidViewOptions => "invalid_view_options": error,
        LegacyDiagnosticInvalid => "legacy_diagnostic_invalid": error,
        LegacyDiagnosticReadLimit => "legacy_diagnostic_read_limit": error,
        MaterialNotProvided => "material_not_provided": error,
        MaterialReadEncodeFailed => "material_read_encode_failed": error,
        MaterialReadOutputFailed => "material_read_output_failed": error,
        ReportSerializationFailed => "report_serialization_failed": error,
        RuntimeFactsNotAvailable => "runtime_facts_not_available": error,
        RuntimeFactsPositionMissing => "runtime_facts_position_missing": error,
        RuntimeFactsReadBudgetExceeded => "runtime_facts_read_budget_exceeded": error,
        RuntimeFactsSourceIncomplete => "runtime_facts_source_incomplete": error,
        StabilityArtifactTooLarge => "stability_artifact_too_large": error,
        StabilityFieldsInvalid => "stability_fields_invalid": error,
        StabilitySchemaUnsupported => "stability_schema_unsupported": error,
        StabilitySourceLinksMismatch => "stability_source_links_mismatch": error,
        TaskDiagnosticCursorInvalid => "task_diagnostic_cursor_invalid": error,
        TaskDiagnosticCursorMismatch => "task_diagnostic_cursor_mismatch": error,
        TaskDiagnosticCursorOutsideWindow => "task_diagnostic_cursor_outside_window": error,
        TaskDiagnosticFramingInvalid => "task_diagnostic_framing_invalid": error,
        TaskDiagnosticHeaderInvalid => "task_diagnostic_header_invalid": error,
        TaskDiagnosticIdentityMismatch => "task_diagnostic_identity_mismatch": error,
        TaskDiagnosticIntegrityFailed => "task_diagnostic_integrity_failed": error,
        TaskDiagnosticOpenFailed => "task_diagnostic_open_failed": error,
        TaskDiagnosticPageLimit => "task_diagnostic_page_limit": error,
        TaskDiagnosticReadFailed => "task_diagnostic_read_failed": error,
        TaskDiagnosticRecordInvalid => "task_diagnostic_record_invalid": error,
        TaskDiagnosticRecordLimit => "task_diagnostic_record_limit": error,
        TaskDiagnosticRecordOrderInvalid => "task_diagnostic_record_order_invalid": error,
        TaskDiagnosticTrailingData => "task_diagnostic_trailing_data": error,
        TaskDiagnosticUnsealed => "task_diagnostic_unsealed": error,
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
        // Workflow #395 (LEDG-395): the phases of the offline material and runtime fact reads;
        // a phase's deadline that runs out names the phase as the failure's location.
        ResolveMaterialReference => "resolve_material_reference",
        ResolveCurrentMaterialReference => "resolve_current_material_reference",
        ReadVerifiedMaterial => "read_verified_material",
        ReadRuntimeFactsPosition => "read_runtime_facts_position",
        ReadRuntimeFactSnapshotSet => "read_runtime_fact_snapshot_set",
        ReadRuntimeFactChanges => "read_runtime_fact_changes",
    }
}
