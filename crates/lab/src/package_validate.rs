// SPDX-License-Identifier: AGPL-3.0-only

use actingcommand_pack_containment::{Containment, ContainmentError, InstanceId, Sha256Hash};
use std::fs;

use crate::{
    JsonDocument, Lab, LabError, LabPorts, LabResult, PackageValidateRequest,
    PackageValidationResponse, RecognitionPackDiagnosticsResponse,
    UnsupportedRecognitionTargetResponse,
};

impl<P: LabPorts> Lab<P> {
    pub fn package_validate(
        &mut self,
        request: PackageValidateRequest,
    ) -> LabResult<PackageValidationResponse> {
        validate_package(request)
    }
}

fn validate_package(request: PackageValidateRequest) -> LabResult<PackageValidationResponse> {
    let bytes = fs::read(&request.zip_path).map_err(|error| {
        LabError::package_invalid(format!(
            "failed to open package {}: {error}",
            request.zip_path.display()
        ))
    })?;
    let instance = InstanceId::new("package-validate").map_err(containment_package_error)?;
    let expected = Sha256Hash::digest(&bytes);
    let mut containment = Containment::new();
    let bundle = containment
        .load(&instance, &bytes, &expected)
        .map_err(containment_package_error)?;
    Ok(PackageValidationResponse {
        status: "valid".to_string(),
        module: bundle.resource_root().to_string(),
        manifest_path: bundle.manifest_path().to_string(),
        task_count: bundle.task_count(),
        entry_count: bundle.entry_count(),
        dangerous_entries: Vec::new(),
        recognition_pack_diagnostics: bundle
            .recognition_pack_diagnostics()
            .iter()
            .map(|diagnostics| RecognitionPackDiagnosticsResponse {
                path: diagnostics.path.clone(),
                unsupported_target_count: diagnostics.unsupported_targets.len(),
                unsupported_targets: diagnostics
                    .unsupported_targets
                    .iter()
                    .map(|target| UnsupportedRecognitionTargetResponse {
                        id: target.id.clone(),
                        reason: target.reason.clone(),
                    })
                    .collect(),
            })
            .collect(),
        manifest: JsonDocument::new(bundle.manifest().clone()),
        entries: request
            .include_entries
            .then(|| bundle.entry_paths().map(str::to_string).collect()),
    })
}

fn containment_package_error(error: ContainmentError) -> LabError {
    LabError::package_invalid(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;
    use zip::ZipWriter;
    use zip::write::FileOptions;

    #[test]
    fn package_validate_accepts_safe_zip() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                ("module/manifest.json", br#"{"schema_version":"0.2"}"#),
                ("module/operations/task/task.json", br#"{"id":"task"}"#),
                ("module/operations/resources.json", br#"{}"#),
            ],
        );

        let response = validate_package(PackageValidateRequest {
            zip_path: zip,
            include_entries: false,
        })
        .unwrap();

        assert_eq!(response.module, "module");
        assert_eq!(response.task_count, 1);
        assert!(response.entries.is_none());
    }

    #[test]
    fn package_validate_reports_unsupported_recognition_targets() {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(
            &zip,
            &[
                ("module/manifest.json", br#"{"schema_version":"0.2"}"#),
                ("module/operations/task/task.json", br#"{"id":"task"}"#),
                (
                    "module/recognition/arknights.cn.pack.json",
                    br#"{
                        "schema_version":"0.5",
                        "targets":[{
                            "type":"template",
                            "id":"page/home",
                            "template_path":"templates/home.png",
                            "region":{"x":0,"y":0,"width":1,"height":1},
                            "method":"rgb_count",
                            "mask":{"type":"range","lower":1,"upper":255}
                        }]
                    }"#,
                ),
            ],
        );

        let response = validate_package(PackageValidateRequest {
            zip_path: zip,
            include_entries: false,
        })
        .unwrap();

        assert_eq!(response.recognition_pack_diagnostics.len(), 1);
        assert_eq!(
            response.recognition_pack_diagnostics[0].unsupported_target_count,
            1
        );
        assert_eq!(
            response.recognition_pack_diagnostics[0].unsupported_targets[0].id,
            "page/home"
        );
    }

    #[test]
    fn package_validate_rejects_zip_slip() {
        let error = validate_fixture(&[
            ("module/manifest.json", br#"{}"#),
            ("module/operations/task/task.json", br#"{}"#),
            ("module/../escape.json", br#"{}"#),
        ]);
        assert_eq!(error.code, "package_invalid");
    }

    #[test]
    fn package_validate_rejects_executable_entry() {
        let error = validate_fixture(&[
            ("module/manifest.json", br#"{}"#),
            ("module/operations/task/task.json", br#"{}"#),
            ("module/tools/run.ps1", b"Write-Host no"),
        ]);
        assert!(error.message.contains("executable"));
    }

    #[test]
    fn package_validate_rejects_hash_mismatch() {
        let error = validate_fixture(&[
            (
                "module/manifest.json",
                br#"{"hashes":{"operations/resources.json":"sha256:0000"}}"#,
            ),
            ("module/operations/task/task.json", br#"{}"#),
            ("module/operations/resources.json", br#"{}"#),
        ]);
        assert!(error.message.contains("hash mismatch"));
    }

    #[test]
    fn package_validate_rejects_unsafe_manifest_hash_path_without_echoing_traversal() {
        let error = validate_fixture(&[
            (
                "module/manifest.json",
                br#"{"hashes":{"../outside.json":"sha256:0000"}}"#,
            ),
            ("module/operations/task/task.json", br#"{}"#),
            ("module/operations/resources.json", br#"{}"#),
        ]);
        assert!(error.message.contains("manifest hash path is unsafe"));
        assert!(!error.message.contains(".."));
    }

    fn validate_fixture(files: &[(&str, &[u8])]) -> LabError {
        let temp = TempDir::new().unwrap();
        let zip = temp.path().join("bundle.zip");
        write_test_zip(&zip, files);
        validate_package(PackageValidateRequest {
            zip_path: zip,
            include_entries: false,
        })
        .expect_err("fixture must be rejected")
    }

    fn write_test_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }
}
