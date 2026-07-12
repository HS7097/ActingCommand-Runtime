// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, runtime_slice_cli};
use actingcommand_contract::{ResourceAuthoringEvent, ResourceAuthoringPhase};
use actingcommand_resource_tooling::{
    AuthoringDraft, AuthoringEnvironmentSnapshot, AuthoringEvent, AuthoringEventKind,
    AuthoringEventSink, AuthoringFile, AuthoringProvenance, AuthoringPublishRequest,
    AuthoringReceipt, AuthoringValidationReport, AuthoringWriteMode, PackageBuildTaskRequest,
    PackageEnvOptions, PackageSource, ResourceConvertRequest, materialize_authoring_draft,
    prepare_package_build_task, publish_authoring_draft, resource_convert,
};
use actingcommand_runtime_client::{RuntimeAuthoringSession, RuntimeClient};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static VALIDATION_PACKAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) struct RecordAuthoringAsset {
    pub source: PathBuf,
    pub relative_path: PathBuf,
}

pub(super) struct RecordAuthoringInput {
    pub record_id: String,
    pub task_id: String,
    pub task_dir_name: String,
    pub bundle: Value,
    pub assets: Vec<RecordAuthoringAsset>,
}

#[derive(Serialize)]
pub(super) struct RecordAuthoringPublishOutput {
    runtime_correlation_id: String,
    receipt: AuthoringReceipt,
}

pub(super) fn materialize_record_authoring(
    target_root: &Path,
    input: &RecordAuthoringInput,
) -> CliOutcome<()> {
    let draft_id = input.draft_id();
    let draft = input.to_draft(&draft_id)?;
    materialize_authoring_draft(target_root, &draft)
}

pub(super) fn publish_record_authoring(
    client: &RuntimeClient,
    target_root: &Path,
    target_label: String,
    input: &RecordAuthoringInput,
    game: &str,
    server: &str,
    force: bool,
) -> CliOutcome<RecordAuthoringPublishOutput> {
    let session = client
        .begin_authoring_session()
        .map_err(runtime_slice_cli::map_runtime_error)?;
    let correlation_id = typed_correlation_string(&session)?;
    let draft = input.to_draft(&correlation_id)?;
    let mut events = RuntimeAuthoringEventSink {
        session: &session,
        correlation_id: &correlation_id,
    };
    let mut validator = |candidate: &Path, _draft: &AuthoringDraft| {
        validate_candidate(candidate, &input.task_id, game, server)
    };
    let receipt = publish_authoring_draft(
        &AuthoringPublishRequest {
            target_root: target_root.to_path_buf(),
            target_label,
            force,
        },
        &draft,
        &mut validator,
        &mut events,
    )?;
    Ok(RecordAuthoringPublishOutput {
        runtime_correlation_id: correlation_id,
        receipt,
    })
}

impl RecordAuthoringInput {
    fn draft_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.record_id.as_bytes());
        hasher.update([0]);
        hasher.update(self.task_id.as_bytes());
        format!("record-{}", &format!("{:x}", hasher.finalize())[..32])
    }

    fn to_draft(&self, correlation_id: &str) -> CliOutcome<AuthoringDraft> {
        let replace_scope = PathBuf::from("operations").join(&self.task_dir_name);
        let mut files = vec![
            AuthoringFile::bytes(
                replace_scope.join("task.json"),
                serde_json::to_vec_pretty(&self.bundle).map_err(|error| {
                    CliError::usage(format!("failed to serialize authoring task: {error}"))
                })?,
                AuthoringWriteMode::Replace,
            )?,
            AuthoringFile::bytes(
                "operations/resources.json",
                serde_json::to_vec_pretty(&json!({
                    "schema_version": "1.0",
                    "resources": [],
                    "resource_count": 0
                }))
                .map_err(|error| {
                    CliError::usage(format!("failed to serialize authoring resources: {error}"))
                })?,
                AuthoringWriteMode::CreateIfMissing,
            )?,
        ];
        let mut source_artifact_ids = BTreeSet::new();
        for asset in &self.assets {
            let artifact_id = format!("sha256:{}", file_sha256(&asset.source)?);
            source_artifact_ids.insert(artifact_id.clone());
            files.push(AuthoringFile::copy(
                asset.relative_path.clone(),
                asset.source.clone(),
                artifact_id,
                AuthoringWriteMode::Replace,
            )?);
        }
        AuthoringDraft::new(
            correlation_id,
            self.draft_id(),
            replace_scope,
            files,
            AuthoringProvenance {
                record_id: self.record_id.clone(),
                source_artifact_ids: source_artifact_ids.into_iter().collect(),
            },
        )
    }
}

struct RuntimeAuthoringEventSink<'a> {
    session: &'a RuntimeAuthoringSession,
    correlation_id: &'a str,
}

impl AuthoringEventSink for RuntimeAuthoringEventSink<'_> {
    fn append(&mut self, event: &AuthoringEvent) -> CliOutcome<()> {
        if event.correlation_id != self.correlation_id {
            return Err(CliError::safety_blocked(
                "authoring_correlation_mismatch",
                "resource authoring event correlation differs from its Runtime session",
                &["resource_authoring", "runtime_correlation"],
            ));
        }
        let event = ResourceAuthoringEvent::new(
            authoring_phase(event.kind),
            event.draft_id.clone(),
            event.target_label.clone(),
            event.target_fingerprint.clone(),
            event.changed_paths.clone(),
            event.failure_code.clone(),
        )
        .map_err(|_| {
            CliError::safety_blocked(
                "authoring_event_invalid",
                "resource authoring event failed the Runtime contract",
                &["resource_authoring", "runtime_contract"],
            )
        })?;
        self.session
            .append(event)
            .map(|_| ())
            .map_err(runtime_slice_cli::map_runtime_error)
    }
}

const fn authoring_phase(kind: AuthoringEventKind) -> ResourceAuthoringPhase {
    match kind {
        AuthoringEventKind::AuthoringStarted => ResourceAuthoringPhase::AuthoringStarted,
        AuthoringEventKind::DraftBuilt => ResourceAuthoringPhase::DraftBuilt,
        AuthoringEventKind::ValidationCompleted => ResourceAuthoringPhase::ValidationCompleted,
        AuthoringEventKind::PromoteIntent => ResourceAuthoringPhase::PromoteIntent,
        AuthoringEventKind::Promoted => ResourceAuthoringPhase::Promoted,
        AuthoringEventKind::PromoteFailed => ResourceAuthoringPhase::PromoteFailed,
    }
}

fn typed_correlation_string(session: &RuntimeAuthoringSession) -> CliOutcome<String> {
    let value = serde_json::to_value(session.correlation_id()).map_err(|error| {
        CliError::usage(format!("failed to serialize Runtime correlation: {error}"))
    })?;
    value.as_str().map(str::to_owned).ok_or_else(|| {
        CliError::safety_blocked(
            "runtime_correlation_invalid",
            "Runtime correlation did not serialize as a canonical identifier",
            &["resource_authoring", "runtime_correlation"],
        )
    })
}

fn validate_candidate(
    candidate_root: &Path,
    task_id: &str,
    game: &str,
    server: &str,
) -> CliOutcome<AuthoringValidationReport> {
    let converted = resource_convert(ResourceConvertRequest {
        repo: candidate_root.to_path_buf(),
        game: Some(game.to_string()),
        server: Some(server.to_string()),
        locale: None,
        maa_tasks_root: None,
        dry_run: false,
    })?;
    if converted.status != "written" {
        return Err(CliError::package_invalid(
            "resource convert did not produce a written candidate",
        ));
    }

    let package_path = validation_package_path(candidate_root)?;
    let prepared = prepare_package_build_task(PackageBuildTaskRequest {
        source: PackageSource::Local(candidate_root.to_path_buf()),
        temporary_root: candidate_root
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        task_id: task_id.to_string(),
        game: Some(game.to_string()),
        server: Some(server.to_string()),
        locale: None,
        package_id: None,
        execution_mode: None,
        resolution: None,
        include_recovery: false,
        out: package_path.clone(),
        dry_run: false,
        env: PackageEnvOptions::default(),
    })?;
    let required = prepared.required_environment_keys()?;
    if !required.is_empty() {
        return Err(CliError::safety_blocked(
            "authoring_environment_unresolved",
            format!(
                "resource package requires resolved environment keys: {}",
                required.join(",")
            ),
            &["resource_authoring", "environment"],
        ));
    }
    let build = prepared.build(&AuthoringEnvironmentSnapshot::default());
    let cleanup = remove_validation_package(&package_path);
    let built = match (build, cleanup) {
        (Ok(built), Ok(())) => built,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(mut error), Err(cleanup_error)) => {
            error.message = format!(
                "{}; validation package cleanup also failed: {}",
                error.message, cleanup_error.message
            );
            return Err(error);
        }
    };
    if built.validation.status != "valid" {
        return Err(CliError::package_invalid(
            "containment round-trip did not report a valid package",
        ));
    }
    AuthoringValidationReport::new(vec![
        "draft_schema".to_string(),
        "resource_convert".to_string(),
        "repository_references".to_string(),
        "package_build".to_string(),
        "containment_round_trip".to_string(),
    ])
}

fn validation_package_path(candidate_root: &Path) -> CliOutcome<PathBuf> {
    let parent = candidate_root.parent().ok_or_else(|| {
        CliError::package_invalid("authoring candidate has no parent for validation output")
    })?;
    let sequence = VALIDATION_PACKAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".actingcommand-authoring-validation-{}-{sequence}.zip",
        std::process::id()
    )))
}

fn remove_validation_package(path: &Path) -> CliOutcome<()> {
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(path).map_err(|error| {
        CliError::package_invalid(format!(
            "failed to remove validation package {}: {error}",
            path.display()
        ))
    })
}

fn file_sha256(path: &Path) -> CliOutcome<String> {
    let bytes = fs::read(path).map_err(|error| {
        CliError::usage(format!(
            "failed to read authoring source {}: {error}",
            path.display()
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
