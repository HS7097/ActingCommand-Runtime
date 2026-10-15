// SPDX-License-Identifier: AGPL-3.0-only

//! Package containment's outcome codes (Workflow #378 A2d). Every entry is listed in the fragment
//! `contracts/outcome-codes/pack-containment.json`; the outcome guard (G1) checks both
//! directions. A source or package admission refusal (`ContainmentError::SourceTree`) carries
//! one of these, or one of the contract's `SharedCode`s.

actingcommand_contract::outcome_codes! {
    /// Why a package, a content directory or container, or a source tree is not admitted.
    pub enum PackContainmentCode {
        // A package read by its path: a content directory, its content containers and an
        // assembled package ZIP (Workflow #288, #336).
        ContentDirectoryLocatorNotAbsolute => "content_directory_locator_not_absolute": error / declaration_error,
        ContentDirectoryMissing => "content_directory_missing": error / declaration_error,
        ContentDirectoryLinkOrType => "content_directory_link_or_type": error / declaration_error,
        ContentDirectoryNotDirectory => "content_directory_not_directory": error / declaration_error,
        ContentDirectoryReadFailed => "content_directory_read_failed": error / declaration_error,
        ContentDirectoryOpenFailed => "content_directory_open_failed": error / declaration_error,
        ContentDirectoryPathEncoding => "content_directory_path_encoding": error / declaration_error,
        ContentDirectoryDepthLimit => "content_directory_depth_limit": error / declaration_error,
        ContentDirectoryEntryLimit => "content_directory_entry_limit": error / declaration_error,
        ContentDirectoryPathInvalid => "content_directory_path_invalid": error / declaration_error,
        ContentDirectoryCaseCollision => "content_directory_case_collision": error / declaration_error,
        ContentDirectorySizeLimit => "content_directory_size_limit": error / declaration_error,
        ContentDirectoryLfsPointer => "content_directory_lfs_pointer": error / declaration_error,
        ContentDirectoryDeadline => "content_directory_deadline": error / budget_or_permission,
        // Workflow #336 PKG-S1 (D16): the directory was replaced while it was read.
        ContentDirectoryChangedDuringRead => "content_directory_changed_during_read": error / declaration_error,
        ContentContainerUnsupported => "content_container_unsupported": error / declaration_error,
        ContentContainerSizeLimit => "content_container_size_limit": error / declaration_error,
        ContentZipInvalid => "content_zip_invalid": error / declaration_error,
        ContentZipEntryInvalid => "content_zip_entry_invalid": error / declaration_error,
        ContentJsonInvalid => "content_json_invalid": error / declaration_error,
        ContentJsonDuplicatePath => "content_json_duplicate_path": error / declaration_error,
        ContentJsonFileNotString => "content_json_file_not_string": error / declaration_error,
        // A source tree snapshot.
        SourceTreeDeadline => "source_tree_deadline": error / budget_or_permission,
        SourceOperationPathInvalid => "source_operation_path_invalid": error / declaration_error,
        SourceOperationInvalid => "source_operation_invalid": error / declaration_error,
        SourceEntryOperationMissing => "source_entry_operation_missing": error / declaration_error,
        SourceConversionMetadataMissing => "source_conversion_metadata_missing": error / declaration_error,
        SourceConversionEncodeFailed => "source_conversion_encode_failed": error / none,
        SourceLocaleMissing => "source_locale_missing": error / declaration_error,
        SourceGameInvalid => "source_game_invalid": error / declaration_error,
        SourceServerInvalid => "source_server_invalid": error / declaration_error,
        SourceLocaleInvalid => "source_locale_invalid": error / declaration_error,
        SourceResourcesInvalid => "source_resources_invalid": error / declaration_error,
        SourceCompiledEntryLimit => "source_compiled_entry_limit": error / declaration_error,
        SourceCompiledSizeLimit => "source_compiled_size_limit": error / declaration_error,
        SourceContainsDerivedOutput => "source_contains_derived_output": error / declaration_error,
        // Post-admission OCR field declarations (the message of a `package_invalid` refusal).
        OcrFieldsTargetMissingOrDuplicate => "ocr_fields_target_missing_or_duplicate": error / declaration_error,
        OcrFieldsDictionaryHashMismatch => "ocr_fields_dictionary_hash_mismatch": error / declaration_error,
        OcrModeSchemaMismatch => "ocr_mode_schema_mismatch": error / declaration_error,
        OcrFieldsOutcomeMissing => "ocr_fields_outcome_missing": error / declaration_error,
    }
}
