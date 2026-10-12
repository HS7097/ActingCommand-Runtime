// SPDX-License-Identifier: AGPL-3.0-only

//! Package containment's outcome codes (Workflow #378 A2d). Every entry is listed in the fragment
//! `contracts/outcome-codes/pack-containment.json`; the outcome guard (G1) checks both
//! directions. A source or package admission refusal (`ContainmentError::SourceTree`) carries
//! one of these, or one of the contract's `SharedCode`s.

actingcommand_contract::outcome_codes! {
    /// Why a package, a content directory or container, or a source tree is not admitted.
    pub enum PackContainmentCode {
        // A legacy package file.
        PackageOpenFailed => "package_open_failed": error,
        PackageMetadataFailed => "package_metadata_failed": error,
        PackageSizeInvalid => "package_size_invalid": error,
        PackageDeadline => "package_deadline": error,
        // A content directory and its content containers (Workflow #288, #336).
        ContentDirectoryNameMismatch => "content_directory_name_mismatch": error,
        ContentDirectoryLocatorNotAbsolute => "content_directory_locator_not_absolute": error,
        ContentDirectoryMissing => "content_directory_missing": error,
        ContentDirectoryLinkOrType => "content_directory_link_or_type": error,
        ContentDirectoryNotDirectory => "content_directory_not_directory": error,
        ContentDirectoryReadFailed => "content_directory_read_failed": error,
        ContentDirectoryOpenFailed => "content_directory_open_failed": error,
        ContentDirectoryPathEncoding => "content_directory_path_encoding": error,
        ContentDirectoryDepthLimit => "content_directory_depth_limit": error,
        ContentDirectoryEntryLimit => "content_directory_entry_limit": error,
        ContentDirectoryPathInvalid => "content_directory_path_invalid": error,
        ContentDirectoryCaseCollision => "content_directory_case_collision": error,
        ContentDirectorySizeLimit => "content_directory_size_limit": error,
        ContentDirectoryLfsPointer => "content_directory_lfs_pointer": error,
        ContentDirectoryDeadline => "content_directory_deadline": error,
        ContentContainerUnsupported => "content_container_unsupported": error,
        ContentContainerSizeLimit => "content_container_size_limit": error,
        ContentZipInvalid => "content_zip_invalid": error,
        ContentZipEntryInvalid => "content_zip_entry_invalid": error,
        ContentJsonInvalid => "content_json_invalid": error,
        ContentJsonDuplicatePath => "content_json_duplicate_path": error,
        ContentJsonFileNotString => "content_json_file_not_string": error,
        // A source tree snapshot.
        SourceTreeLoaderRetired => "source_tree_loader_retired": error,
        SourceTreeDeadline => "source_tree_deadline": error,
        SourceOperationPathInvalid => "source_operation_path_invalid": error,
        SourceOperationInvalid => "source_operation_invalid": error,
        SourceEntryOperationMissing => "source_entry_operation_missing": error,
        SourceConversionMetadataMissing => "source_conversion_metadata_missing": error,
        SourceConversionEncodeFailed => "source_conversion_encode_failed": error,
        SourceLocaleMissing => "source_locale_missing": error,
        SourceGameInvalid => "source_game_invalid": error,
        SourceServerInvalid => "source_server_invalid": error,
        SourceLocaleInvalid => "source_locale_invalid": error,
        SourceResourcesInvalid => "source_resources_invalid": error,
        SourceCompiledEntryLimit => "source_compiled_entry_limit": error,
        SourceCompiledSizeLimit => "source_compiled_size_limit": error,
        SourceContainsDerivedOutput => "source_contains_derived_output": error,
        // Post-admission OCR field declarations (the message of a `package_invalid` refusal).
        OcrFieldsTargetMissingOrDuplicate => "ocr_fields_target_missing_or_duplicate": error,
        OcrFieldsDictionaryHashMismatch => "ocr_fields_dictionary_hash_mismatch": error,
        OcrModeSchemaMismatch => "ocr_mode_schema_mismatch": error,
        OcrFieldsOutcomeMissing => "ocr_fields_outcome_missing": error,
    }
}
