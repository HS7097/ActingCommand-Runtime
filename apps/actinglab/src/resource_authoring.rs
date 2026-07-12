// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, runtime_slice_cli};
use actingcommand_contract::{
    CorrelationId, EventPayload, EventQuery, EventType, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, PublicEventPayload, ResourceAuthoringEvent, ResourceAuthoringPhase,
};
use actingcommand_resource_tooling::{
    AuthoringDraft, AuthoringEnvironmentSnapshot, AuthoringEvent, AuthoringEventKind,
    AuthoringEventSink, AuthoringFile, AuthoringProvenance, AuthoringPublishRequest,
    AuthoringReceipt, AuthoringRecoveryContext, AuthoringRecoveryDecision, AuthoringRecoveryOracle,
    AuthoringValidationReport, AuthoringWriteMode, PackageBuildTaskRequest, PackageEnvOptions,
    PackageSource, ResourceConvertRequest, materialize_authoring_draft, prepare_package_build_task,
    publish_authoring_draft_with_recovery, resource_convert,
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
    let mut recovery = RuntimeAuthoringRecoveryOracle { client };
    let mut validator = |candidate: &Path, _draft: &AuthoringDraft| {
        validate_candidate(candidate, &input.task_id, game, server)
    };
    let receipt = publish_authoring_draft_with_recovery(
        &AuthoringPublishRequest {
            target_root: target_root.to_path_buf(),
            target_label,
            force,
        },
        &draft,
        &mut recovery,
        &mut validator,
        &mut events,
    )?;
    Ok(RecordAuthoringPublishOutput {
        runtime_correlation_id: correlation_id,
        receipt,
    })
}

struct RuntimeAuthoringRecoveryOracle<'a> {
    client: &'a RuntimeClient,
}

impl AuthoringRecoveryOracle for RuntimeAuthoringRecoveryOracle<'_> {
    fn decide(
        &mut self,
        context: &AuthoringRecoveryContext,
    ) -> CliOutcome<AuthoringRecoveryDecision> {
        let correlation_id = parse_correlation_id(context.correlation_id())?;
        let events = self
            .client
            .query_events(
                EventQuery {
                    correlation_id: Some(correlation_id),
                    ..EventQuery::default()
                },
                ProjectionProfile::Lab,
            )
            .map_err(runtime_slice_cli::map_runtime_error)?;
        decide_recovery_from_events(context, &events)
    }
}

fn parse_correlation_id(value: &str) -> CliOutcome<CorrelationId> {
    serde_json::from_value(Value::String(value.to_string())).map_err(|_| {
        CliError::safety_blocked(
            "authoring_recovery_correlation_invalid",
            "authoring recovery journal contains an invalid Runtime correlation",
            &["resource_authoring", "runtime_correlation"],
        )
    })
}

fn decide_recovery_from_events(
    context: &AuthoringRecoveryContext,
    events: &[ProjectedEvent],
) -> CliOutcome<AuthoringRecoveryDecision> {
    let mut phases = Vec::new();
    for event in events {
        let Some((phase, draft_id, target_label, target_fingerprint)) =
            projected_authoring_identity(event)?
        else {
            continue;
        };
        if draft_id != context.draft_id()
            || target_label != context.target_label()
            || target_fingerprint != context.target_fingerprint()
        {
            return Err(authoring_recovery_error(
                "authoring recovery ledger identity does not match the transaction journal",
            ));
        }
        phases.push(phase);
    }
    let prefix = [
        ResourceAuthoringPhase::AuthoringStarted,
        ResourceAuthoringPhase::DraftBuilt,
        ResourceAuthoringPhase::ValidationCompleted,
        ResourceAuthoringPhase::PromoteIntent,
    ];
    if phases.len() < prefix.len() || phases[..prefix.len()] != prefix {
        return Err(authoring_recovery_error(
            "authoring recovery ledger is missing the durable promote-intent prefix",
        ));
    }
    match &phases[prefix.len()..] {
        [] => Ok(AuthoringRecoveryDecision::RollbackCandidate),
        [ResourceAuthoringPhase::Promoted] => Ok(AuthoringRecoveryDecision::CommitCandidate),
        [ResourceAuthoringPhase::PromoteFailed] => Ok(AuthoringRecoveryDecision::RollbackCandidate),
        _ => Err(authoring_recovery_error(
            "authoring recovery ledger has an ambiguous terminal sequence",
        )),
    }
}

fn projected_authoring_identity(
    event: &ProjectedEvent,
) -> CliOutcome<Option<(ResourceAuthoringPhase, &str, &str, &str)>> {
    let projected = match &event.payload {
        ProjectionPayload::Full(EventPayload::ResourceAuthoring(payload)) => Some((
            payload.phase(),
            payload.draft_id(),
            payload.target_label(),
            payload.target_fingerprint(),
        )),
        ProjectionPayload::Public(PublicEventPayload::ResourceAuthoring(payload)) => Some((
            payload.authoring_phase().ok_or_else(|| {
                authoring_recovery_error("authoring recovery projection is missing its phase")
            })?,
            payload.draft_id().ok_or_else(|| {
                authoring_recovery_error("authoring recovery projection is missing its draft")
            })?,
            payload.target_label().ok_or_else(|| {
                authoring_recovery_error("authoring recovery projection is missing its target")
            })?,
            payload.target_fingerprint().ok_or_else(|| {
                authoring_recovery_error(
                    "authoring recovery projection is missing its target fingerprint",
                )
            })?,
        )),
        _ => None,
    };
    if projected.is_none() && is_resource_authoring_event(event.event_type) {
        return Err(authoring_recovery_error(
            "authoring recovery event has no inspectable authoring payload",
        ));
    }
    Ok(projected)
}

const fn is_resource_authoring_event(event_type: EventType) -> bool {
    matches!(
        event_type,
        EventType::ResourceAuthoringStarted
            | EventType::ResourceDraftBuilt
            | EventType::ResourceValidationCompleted
            | EventType::ResourcePromoteIntent
            | EventType::ResourcePromoted
            | EventType::ResourcePromoteFailed
    )
}

fn authoring_recovery_error(message: impl Into<String>) -> CliError {
    CliError::safety_blocked(
        "authoring_recovery_ledger_ambiguous",
        message,
        &["resource_authoring", "runtime_ledger"],
    )
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
    let build = prepared
        .build(&AuthoringEnvironmentSnapshot::default())
        .and_then(|built| {
            let bytes = fs::read(&package_path).map_err(|error| {
                CliError::package_invalid(format!(
                    "failed to hash validation package {}: {error}",
                    package_path.display()
                ))
            })?;
            Ok((built, format!("{:x}", Sha256::digest(bytes))))
        });
    let cleanup = remove_validation_package(&package_path);
    let (built, package_sha256) = match (build, cleanup) {
        (Ok(result), Ok(())) => result,
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
    AuthoringValidationReport::new(
        vec![
            "draft_schema".to_string(),
            "resource_convert".to_string(),
            "repository_references".to_string(),
            "package_build".to_string(),
            "containment_round_trip".to_string(),
        ],
        package_sha256,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_contract::{EventActor, EventSource, IdentifierIssuer, InstanceId};
    use actingcommand_device::{CaptureBackend, DeviceError, DeviceResult, InputBackend};
    use actingcommand_runtime_client::RuntimeClientConfig;
    use actingcommand_runtime_host::{
        ExecutionBackendProvider, ResolvedExecutionInstance, RuntimeHost, RuntimeHostConfig,
    };
    use std::sync::Arc;
    use tempfile::TempDir;

    struct AuthoringTestProvider {
        instance_id: InstanceId,
    }

    impl ExecutionBackendProvider for AuthoringTestProvider {
        fn instance_aliases(&self) -> Vec<String> {
            vec!["ak.cn".to_string()]
        }

        fn resolve(&self, instance_alias: &str) -> Option<ResolvedExecutionInstance> {
            (instance_alias == "ak.cn")
                .then(|| ResolvedExecutionInstance::new(self.instance_id, "test-device"))
        }

        fn open_input(&self, _instance_alias: &str) -> DeviceResult<Box<dyn InputBackend>> {
            Err(DeviceError::fatal("authoring test must not open input"))
        }

        fn open_capture(&self, _instance_alias: &str) -> DeviceResult<Box<dyn CaptureBackend>> {
            Err(DeviceError::fatal("authoring test must not open capture"))
        }
    }

    fn start_runtime(root: &TempDir) -> (RuntimeHost, RuntimeClient) {
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let host = RuntimeHost::start(
            RuntimeHostConfig::new(root.path(), b"actinglab-authoring-recovery-test"),
            Arc::new(AuthoringTestProvider { instance_id }),
        )
        .expect("runtime host");
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            EventActor::Lab,
            EventSource::Lab,
        ))
        .expect("runtime client");
        (host, client)
    }

    fn append_phase(
        session: &RuntimeAuthoringSession,
        phase: ResourceAuthoringPhase,
        draft_id: &str,
        target_fingerprint: &str,
    ) {
        session
            .append(
                ResourceAuthoringEvent::new(
                    phase,
                    draft_id,
                    "resource-root",
                    target_fingerprint,
                    vec!["operations/task-a/task.json".to_string()],
                    None,
                )
                .expect("authoring event"),
            )
            .expect("durable authoring event");
    }

    fn recovery_context(
        session: &RuntimeAuthoringSession,
        draft_id: &str,
        target_fingerprint: &str,
    ) -> AuthoringRecoveryContext {
        AuthoringRecoveryContext::new(
            typed_correlation_string(session).expect("correlation"),
            draft_id,
            "resource-root",
            target_fingerprint,
            "c".repeat(64),
        )
        .expect("recovery context")
    }

    #[test]
    fn runtime_recovery_oracle_rolls_back_intent_and_commits_durable_success() {
        let root = TempDir::new().expect("tempdir");
        let (host, client) = start_runtime(&root);
        let session = client.begin_authoring_session().expect("authoring session");
        let fingerprint = "b".repeat(64);
        let context = recovery_context(&session, "draft-a", &fingerprint);
        for phase in [
            ResourceAuthoringPhase::AuthoringStarted,
            ResourceAuthoringPhase::DraftBuilt,
            ResourceAuthoringPhase::ValidationCompleted,
            ResourceAuthoringPhase::PromoteIntent,
        ] {
            append_phase(&session, phase, "draft-a", &fingerprint);
        }

        let mut oracle = RuntimeAuthoringRecoveryOracle { client: &client };
        assert_eq!(
            oracle.decide(&context).expect("rollback decision"),
            AuthoringRecoveryDecision::RollbackCandidate
        );

        append_phase(
            &session,
            ResourceAuthoringPhase::Promoted,
            "draft-a",
            &fingerprint,
        );
        assert_eq!(
            oracle.decide(&context).expect("commit decision"),
            AuthoringRecoveryDecision::CommitCandidate
        );

        drop(client);
        host.close().expect("close runtime host");
    }

    #[test]
    fn runtime_recovery_oracle_rejects_ledger_identity_mismatch() {
        let root = TempDir::new().expect("tempdir");
        let (host, client) = start_runtime(&root);
        let session = client.begin_authoring_session().expect("authoring session");
        let fingerprint = "b".repeat(64);
        let context = recovery_context(&session, "draft-a", &fingerprint);
        append_phase(
            &session,
            ResourceAuthoringPhase::AuthoringStarted,
            "draft-b",
            &fingerprint,
        );

        let error = RuntimeAuthoringRecoveryOracle { client: &client }
            .decide(&context)
            .expect_err("mismatched ledger identity must fail closed");
        assert_eq!(error.code, "authoring_recovery_ledger_ambiguous");

        drop(client);
        host.close().expect("close runtime host");
    }
}
