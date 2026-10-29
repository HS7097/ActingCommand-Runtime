// SPDX-License-Identifier: AGPL-3.0-only

//! The wire shapes of the player workbench (Workflow #429 A.2 R-0, register §1.213, since
//! 0.12.30; `contracts/runtime-interface.md`, "Workbench request kinds"). They are defined once,
//! before their behaviour, so that the console pins `runtime-client` 3 once and later slices only
//! add behaviour. Like U-1's shapes ([`super::pack_registry`]), every request kind that uses them
//! is refused `not_implemented` until the slice named on it adds the behaviour and its shape
//! checks. No member says how many times a task ran or may run (Alice's ruling 100): the routine
//! summary's buckets are health statistics only.

use crate::outcome::CodeStr;
use crate::{EventId, OwnerEpoch, RequestId};
use serde::{Deserialize, Serialize};

/// What `StopInstanceRun` answers (R-6): the run it stopped, whether the run reached its
/// terminal state within `wait_ms`, and the Runtime's own reset of the device afterwards.
/// `settled: false` is "still stopping": the request answers anyway once `wait_ms` passed, and
/// `reset` is absent until the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRunStop {
    pub request_id: RequestId,
    pub settled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<InstanceRunReset>,
}

/// How the Runtime's own reset after a stopped run went (R-6): with the run's own lease, after
/// the run's next checkpoint; the client owes no `SafeReset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceRunReset {
    Done,
    NotNeeded,
    Failed,
}

/// One row of an `EditPackSchedule` (R-8, `contracts/instance-fact-store.md`, "Pack schedule
/// facts"): from `from_unix_ms` until `until_unix_ms` (half-open; absent means open-ended) the
/// package is schedulable or not. The package is the request's `package_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackScheduleEditRow {
    pub from_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_unix_ms: Option<u64>,
    pub schedulable: bool,
}

/// The one change of an `EditResourceTarget` (R-13c): add or replace a target, pause or resume
/// one, or delete one. The target travels as JSON text, as `ApplyResourceTargets` carries its
/// document: one `actingcommand.resource-targets.v3` target object (`id`, the kind `maintain`
/// with `at_least` or `gain` with `amount`, the resource pool, optional `paused` and
/// `valid_until_unix_ms`; no `tasks`, no `on_met`), parsed and checked by R-13a. The request's
/// `expected_version` is the view's `active.version`, or `0` when the view has no `active`
/// (the first target of an instance, or one after the table was withdrawn).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceTargetEdit {
    Upsert { target_json: String },
    SetPaused { id: String, paused: bool },
    Delete { id: String },
}

/// The bucket width of a `RoutineSummary` (R-12): an hour, or an activity day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineSummaryGranularity {
    Hour,
    Day,
}

/// What `RoutineSummary` answers (R-12): health statistics from the host's memory, per instance
/// and bucket. Nothing is read from or written to the ledger. `revision` is the summary's
/// revision now, the `since_revision` of the next read; with `since_revision` only the buckets
/// changed after it are answered. The memory and its revisions belong to one owner epoch, which
/// the page names: a restart begins both again, so a client whose cursor came with another
/// `owner_epoch` drops its `since_revision` and reads again without it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummaryPage {
    pub owner_epoch: OwnerEpoch,
    pub granularity: RoutineSummaryGranularity,
    pub revision: u64,
    pub instances: Vec<RoutineSummaryInstance>,
}

/// One instance's buckets. `coverage_from_unix_ms` is when its memory begins: a restart begins
/// it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummaryInstance {
    pub instance_alias: String,
    pub coverage_from_unix_ms: u64,
    pub buckets: Vec<RoutineSummaryBucket>,
}

/// One bucket, `[start_unix_ms, end_unix_ms)`, and the summary revision of its last change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummaryBucket {
    pub start_unix_ms: u64,
    pub end_unix_ms: u64,
    pub revision: u64,
    pub tasks: Vec<RoutineSummaryTask>,
}

/// One task's health in one bucket: its runs that succeeded, that failed (by code) and that
/// were interrupted. A health statistic, never a count of runs done or allowed (ruling 100).
/// An interruption is a run that ended `contained_task_cancelled` (a stop, a drain's cancel):
/// neither a success nor a failure of the task, so never in `failures`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummaryTask {
    pub task_id: String,
    pub successes: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<RoutineSummaryFailure>,
    pub interruptions: u32,
}

/// The failures of one code in one bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummaryFailure {
    pub code: CodeStr,
    pub count: u32,
}

/// What an instance does now (`RuntimeInstanceStatus.activity`), kept in memory, never read
/// from the ledger: the console's second progress line and its stop button. Absent while the
/// instance does none of these, and until the slice that fills each kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceActivity {
    pub kind: InstanceActivityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    pub since_unix_ms: u64,
}

/// The kinds of [`InstanceActivity`], a closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceActivityKind {
    /// A run the scheduler dispatched.
    PolicyRun,
    /// A run a client started (`RunInstanceTask`, `RunContainedTask`).
    DirectRun,
    /// A run the host started for itself (a startup package, a ladder rung's package).
    HostRun,
    /// A reconnect (a resume's preparation).
    Reconnect,
    /// A self-check.
    Selfcheck,
    /// An emulator start, stop or restart.
    EmulatorControl,
    /// A recovery ladder.
    Ladder,
}

/// What `EditPackSchedule` answers (R-8): the list's version after the edit, the next
/// `expected_version` (an opaque token compared only for equality), and the event of the
/// `fact.published` that holds the scope's whole list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackScheduleEdited {
    pub version: u64,
    pub event_id: EventId,
}

/// What `PackScheduleView` answers (Workflow #429 R-0c, register §1.216 R0C-3, since 0.12.30;
/// `runtime-client` 3; until R-8 the host refuses the read `not_implemented`; R-8 fills it, R-9 the
/// pack defaults): for one instance, the version of each scope's `session.pack.schedule` list
/// that applies to it and, per package, whether it is schedulable now and which layer decided.
/// Each package is resolved on its own (`contracts/instance-fact-store.md`, "Pack schedule
/// facts", SR-11 items 1 to 3): of the layers that have a row of that package, the most specific
/// (instance over server over game) decides with its row that covers now; when none covers now,
/// a default row of the pack that covers now decides, and otherwise the package is schedulable.
/// A layer without a row of it decides nothing for it. Each package also names the layer below's
/// decision (`below`, R-0d, register §1.217), which R-8 fills too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackScheduleView {
    pub instance_alias: String,
    /// One entry per scope that applies to the instance: the list's version, the next
    /// `EditPackSchedule.expected_version` of that scope (an opaque token compared only for
    /// equality), `0` while the scope has no list.
    pub scopes: Vec<PackScheduleScopeVersion>,
    pub packages: Vec<PackSchedulePackage>,
}

/// One scope's `session.pack.schedule` list version in a [`PackScheduleView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackScheduleScopeVersion {
    pub scope: crate::FactScope,
    pub version: u64,
}

/// One package in a [`PackScheduleView`]: whether it is schedulable now, the layer that
/// decided, the row that decided (absent when no row covers now: `source` `none`), when the
/// decision next changes (absent when no row bounds it; a row that takes effect later gives its
/// start), and the layer below's decision (`below`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackSchedulePackage {
    pub package_id: String,
    pub schedulable: bool,
    pub source: PackScheduleSource,
    /// The row that decided, in the shape an `EditPackSchedule` writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<PackScheduleEditRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_change_unix_ms: Option<u64>,
    /// What the package would be now if the deciding layer had no row of it (Workflow #429 R-0d,
    /// register §1.217, since 0.12.30): the next layer down, ending at the pack default or
    /// `none`. Absent when `source` is already `pack_default` or `none`. Runtime 0.12.30 answers
    /// the read `not_implemented`; R-8 fills it with the rest of the view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<PackScheduleBelow>,
}

/// The layer below's decision for one package in a [`PackScheduleView`] (Workflow #429 R-0d,
/// register §1.217, since 0.12.30): SR-11 resolved for the package again without the deciding
/// layer's rows of it, so whether the package would be schedulable now and which layer would
/// decide: `server`, `game`, `pack_default` or `none` (no lower row covers now, so it is
/// schedulable), never `instance`. The console reads it to see, before it edits, what
/// withdrawing the deciding layer's rows of the package would give, so turning the package on or
/// off is one `EditPackSchedule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackScheduleBelow {
    pub schedulable: bool,
    pub source: PackScheduleSource,
}

/// The layer that decided a package in a [`PackScheduleView`], a closed vocabulary: the
/// instance's, server's or game's `session.pack.schedule` row, the pack's default row (R-9), or
/// none (no row covers now, SR-11 item 3, so it is schedulable; a row that takes effect later
/// gives `next_change_unix_ms` its start).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackScheduleSource {
    Instance,
    Server,
    Game,
    PackDefault,
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConfigSettingChange, EventActor, EventSource, FactScope, IdentifierIssuer,
        RuntimeOperation, RuntimeOperationKind, RuntimeRequest, RuntimeResult,
    };
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    fn round_trips<T>(value: &T) -> Value
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let encoded = serde_json::to_value(value).expect("shape encodes");
        let decoded: T = serde_json::from_value(encoded.clone()).expect("shape decodes");
        assert_eq!(&decoded, value);
        encoded
    }

    fn refuses_unknown<T: DeserializeOwned + std::fmt::Debug>(mut encoded: Value, path: &[&str]) {
        let mut object = &mut encoded;
        for member in path {
            object = object.get_mut(*member).expect("member present");
        }
        object
            .as_object_mut()
            .expect("an object")
            .insert("unknown_member".to_owned(), json!(1));
        assert!(
            serde_json::from_value::<T>(encoded.clone()).is_err(),
            "{encoded}"
        );
    }

    // One new request shape, its kind and the nested objects that refuse unknown members too.
    type Case = (RuntimeOperation, RuntimeOperationKind, Vec<&'static str>);

    fn operations() -> Vec<Case> {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let holder_id = *ids.mint_holder_id().expect("holder id").transport();
        let request_id = *ids.mint_request_id().expect("request id").transport();
        vec![
            (
                RuntimeOperation::RunInstanceTask {
                    instance_alias: "node.a".to_owned(),
                    package_id: "package.example".to_owned(),
                    holder_id,
                    response_deadline_ms: Some(60_000),
                },
                RuntimeOperationKind::RunInstanceTask,
                vec![],
            ),
            (
                RuntimeOperation::StopInstanceRun {
                    instance_alias: "node.a".to_owned(),
                    request_id,
                    wait_ms: 25_000,
                },
                RuntimeOperationKind::StopInstanceRun,
                vec![],
            ),
            (
                RuntimeOperation::EditPackSchedule {
                    scope: FactScope::Instance {
                        instance_id: "node.a".to_owned(),
                    },
                    package_id: "package.example".to_owned(),
                    rows: vec![
                        PackScheduleEditRow {
                            from_unix_ms: 1_000,
                            until_unix_ms: Some(2_000),
                            schedulable: true,
                        },
                        PackScheduleEditRow {
                            from_unix_ms: 2_000,
                            until_unix_ms: None,
                            schedulable: false,
                        },
                    ],
                    expected_version: Some(7),
                },
                RuntimeOperationKind::EditPackSchedule,
                vec![],
            ),
            (
                RuntimeOperation::EditResourceTarget {
                    instance_alias: "node.a".to_owned(),
                    expected_version: 7,
                    op: ResourceTargetEdit::Upsert {
                        target_json: r#"{"id":"t1"}"#.to_owned(),
                    },
                },
                RuntimeOperationKind::EditResourceTarget,
                vec!["op"],
            ),
            (
                RuntimeOperation::EditResourceTarget {
                    instance_alias: "node.a".to_owned(),
                    expected_version: 7,
                    op: ResourceTargetEdit::SetPaused {
                        id: "t1".to_owned(),
                        paused: true,
                    },
                },
                RuntimeOperationKind::EditResourceTarget,
                vec!["op"],
            ),
            (
                RuntimeOperation::EditResourceTarget {
                    instance_alias: "node.a".to_owned(),
                    expected_version: 7,
                    op: ResourceTargetEdit::Delete {
                        id: "t1".to_owned(),
                    },
                },
                RuntimeOperationKind::EditResourceTarget,
                vec!["op"],
            ),
            (
                RuntimeOperation::RoutineSummary {
                    instance: Some("node.a".to_owned()),
                    granularity: RoutineSummaryGranularity::Hour,
                    since_revision: Some(3),
                },
                RuntimeOperationKind::RoutineSummary,
                vec![],
            ),
            (
                RuntimeOperation::RoutineSummary {
                    instance: None,
                    granularity: RoutineSummaryGranularity::Day,
                    since_revision: None,
                },
                RuntimeOperationKind::RoutineSummary,
                vec![],
            ),
            (
                RuntimeOperation::ConfigureSettings {
                    changes: vec![ConfigSettingChange {
                        key: "scheduling.parallel.enabled".to_owned(),
                        value: Some(json!(true)),
                    }],
                    expected_version: Some(12),
                },
                RuntimeOperationKind::ConfigureSettings,
                vec![],
            ),
            (
                RuntimeOperation::ApplyResourceTargets {
                    document_json: "{}".to_owned(),
                    expected_version: Some(12),
                },
                RuntimeOperationKind::ApplyResourceTargets,
                vec![],
            ),
        ]
    }

    // R-0: every new request shape round-trips, refuses an unknown member at each object level,
    // and each new kind's operation takes its own kind.
    #[test]
    fn r0_every_workbench_request_shape_round_trips_and_refuses_an_unknown_member() {
        for (operation, kind, nested) in operations() {
            assert_eq!(operation.kind(), kind);
            assert!(operation.validate().is_ok(), "{operation:?}");
            let encoded = round_trips(&operation);
            refuses_unknown::<RuntimeOperation>(encoded.clone(), &[]);
            for member in nested {
                refuses_unknown::<RuntimeOperation>(encoded.clone(), &[member]);
            }
        }
        // An EditPackSchedule row refuses an unknown member too.
        let (operation, _, _) = operations().remove(2);
        let mut encoded = serde_json::to_value(&operation).expect("encodes");
        encoded["rows"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeOperation>(encoded).is_err());

        // The optional members are absent when unused, so ConfigureSettings and
        // ApplyResourceTargets without them serialize as before.
        let configure = serde_json::to_value(RuntimeOperation::ConfigureSettings {
            changes: vec![ConfigSettingChange {
                key: "scheduling.parallel.enabled".to_owned(),
                value: None,
            }],
            expected_version: None,
        })
        .expect("encodes");
        assert_eq!(
            configure,
            json!({
                "operation": "configure_settings",
                "changes": [{"key": "scheduling.parallel.enabled"}],
            })
        );
        let apply = serde_json::to_value(RuntimeOperation::ApplyResourceTargets {
            document_json: "{}".to_owned(),
            expected_version: None,
        })
        .expect("encodes");
        assert_eq!(
            apply,
            json!({"operation": "apply_resource_targets", "document_json": "{}"})
        );

        // A request envelope carrying a new kind validates (the host refuses it later).
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let (operation, _, _) = operations().remove(0);
        assert!(
            RuntimeRequest::new(
                ids.mint_request_id().expect("request id"),
                ids.mint_correlation_id().expect("correlation id"),
                None,
                EventActor::Cli,
                EventSource::Cli,
                1,
                operation,
            )
            .is_ok()
        );
    }

    // R-0: every new answer shape round-trips and refuses an unknown member, the status member
    // `activity` included.
    #[test]
    fn r0_every_workbench_answer_shape_round_trips_and_refuses_an_unknown_member() {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let request_id = *ids.mint_request_id().expect("request id").transport();
        let event_id = *ids.mint_event_id().expect("event id").transport();

        let stopped = RuntimeResult::InstanceRunStopped {
            stopped: InstanceRunStop {
                request_id,
                settled: true,
                reset: Some(InstanceRunReset::NotNeeded),
            },
        };
        let encoded = round_trips(&stopped);
        refuses_unknown::<RuntimeResult>(encoded.clone(), &[]);
        refuses_unknown::<RuntimeResult>(encoded, &["stopped"]);
        let still_stopping = RuntimeResult::InstanceRunStopped {
            stopped: InstanceRunStop {
                request_id,
                settled: false,
                reset: None,
            },
        };
        let encoded = round_trips(&still_stopping);
        assert!(encoded["stopped"].get("reset").is_none());

        let edited = RuntimeResult::PackScheduleEdited {
            edited: PackScheduleEdited {
                version: 8,
                event_id,
            },
        };
        let encoded = round_trips(&edited);
        refuses_unknown::<RuntimeResult>(encoded.clone(), &[]);
        refuses_unknown::<RuntimeResult>(encoded, &["edited"]);

        let owner_epoch = *ids.mint_owner_epoch().expect("owner epoch").transport();
        let summary = RuntimeResult::RoutineSummary {
            summary: Box::new(RoutineSummaryPage {
                owner_epoch,
                granularity: RoutineSummaryGranularity::Hour,
                revision: 4,
                instances: vec![RoutineSummaryInstance {
                    instance_alias: "node.a".to_owned(),
                    coverage_from_unix_ms: 1_000,
                    buckets: vec![RoutineSummaryBucket {
                        start_unix_ms: 3_600_000,
                        end_unix_ms: 7_200_000,
                        revision: 4,
                        tasks: vec![RoutineSummaryTask {
                            task_id: "task.a".to_owned(),
                            successes: 2,
                            // A failure is a run that failed; a stop (`contained_task_cancelled`)
                            // counts only as an interruption (review L1).
                            failures: vec![RoutineSummaryFailure {
                                code: serde_json::from_value(json!(
                                    "contained_task_deadline_exceeded"
                                ))
                                .expect("code spelling"),
                                count: 1,
                            }],
                            interruptions: 1,
                        }],
                    }],
                }],
            }),
        };
        let encoded = round_trips(&summary);
        // The page names the owner epoch its revisions belong to (review M8).
        assert_eq!(
            encoded["summary"]["owner_epoch"],
            serde_json::to_value(owner_epoch).expect("epoch encodes")
        );
        let mut without_epoch = encoded.clone();
        without_epoch["summary"]
            .as_object_mut()
            .expect("an object")
            .remove("owner_epoch");
        assert!(serde_json::from_value::<RuntimeResult>(without_epoch).is_err());
        refuses_unknown::<RuntimeResult>(encoded.clone(), &[]);
        refuses_unknown::<RuntimeResult>(encoded.clone(), &["summary"]);
        let mut nested = encoded.clone();
        nested["summary"]["instances"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["summary"]["instances"][0]["buckets"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["summary"]["instances"][0]["buckets"][0]["tasks"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded;
        nested["summary"]["instances"][0]["buckets"][0]["tasks"][0]["failures"][0]["unknown_member"] =
            json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());

        // No member of a bucket says how many times a task ran or may run (ruling 100): a task
        // row has exactly its health statistics.
        let row = serde_json::to_value(RoutineSummaryTask {
            task_id: "task.a".to_owned(),
            successes: 0,
            failures: Vec::new(),
            interruptions: 0,
        })
        .expect("encodes");
        let members = row
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            members,
            ["interruptions", "successes", "task_id"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );

        for kind in [
            InstanceActivityKind::PolicyRun,
            InstanceActivityKind::DirectRun,
            InstanceActivityKind::HostRun,
            InstanceActivityKind::Reconnect,
            InstanceActivityKind::Selfcheck,
            InstanceActivityKind::EmulatorControl,
            InstanceActivityKind::Ladder,
        ] {
            let activity = InstanceActivity {
                kind,
                request_id: Some(request_id),
                package_id: Some("package.example".to_owned()),
                since_unix_ms: 5_000,
            };
            let encoded = round_trips(&activity);
            refuses_unknown::<InstanceActivity>(encoded, &[]);
        }
        assert_eq!(
            serde_json::to_value(InstanceActivityKind::Selfcheck).expect("encodes"),
            json!("selfcheck")
        );
        assert_eq!(
            serde_json::to_value(InstanceActivityKind::EmulatorControl).expect("encodes"),
            json!("emulator_control")
        );

        // Review M1: the settings' version beside the change's answer and the fact snapshot,
        // absent when not filled (so both serialize as before), refused beside an unknown member.
        let setting = crate::ConfiguredSetting {
            key: "scheduling.parallel.enabled".to_owned(),
            value: crate::FactScalar::Boolean(true),
            source: crate::ConfigParameterSource::Instruction,
        };
        let configured = RuntimeResult::SettingsConfigured {
            settings: vec![setting.clone()],
            config_version: Some(13),
        };
        let encoded = round_trips(&configured);
        assert_eq!(encoded["config_version"], json!(13));
        refuses_unknown::<RuntimeResult>(encoded, &[]);
        let encoded = round_trips(&RuntimeResult::SettingsConfigured {
            settings: vec![setting],
            config_version: None,
        });
        assert!(encoded.get("config_version").is_none());
        let snapshot = crate::RuntimeFactSnapshot {
            schema_version: crate::RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
            ledger_position: 9,
            taken_at_unix_ms: 1_000,
            snapshot_id: 9,
            part: 1,
            parts: 1,
            records: Vec::new(),
        };
        let encoded = round_trips(&RuntimeResult::RuntimeFactSnapshot {
            snapshot: snapshot.clone(),
            config_version: Some(13),
        });
        assert_eq!(encoded["config_version"], json!(13));
        // Beside the snapshot, never in the persisted snapshot.
        assert!(encoded["snapshot"].get("config_version").is_none());
        refuses_unknown::<RuntimeResult>(encoded.clone(), &[]);
        refuses_unknown::<RuntimeResult>(encoded, &["snapshot"]);
        let encoded = round_trips(&RuntimeResult::RuntimeFactSnapshot {
            snapshot,
            config_version: None,
        });
        assert!(encoded.get("config_version").is_none());

        // Review M2: A.5's read model in `ResourceTargetView`'s `active`, one row per target,
        // every `state` spelled, each level refusing an unknown member; absent when not filled.
        let states = [
            (crate::ResourceTargetState::Active, "active"),
            (crate::ResourceTargetState::Met, "met"),
            (crate::ResourceTargetState::Completed, "completed"),
            (crate::ResourceTargetState::Paused, "paused"),
            (
                crate::ResourceTargetState::AwaitingBaseline,
                "awaiting_baseline",
            ),
            (crate::ResourceTargetState::Unknown, "unknown"),
            (crate::ResourceTargetState::Unmapped, "unmapped"),
        ];
        let rows = states
            .iter()
            .enumerate()
            .map(|(index, (state, _))| crate::ResourceTargetRow {
                id: format!("t{index}"),
                kind: if index % 2 == 0 {
                    crate::ResourceTargetKind::Maintain
                } else {
                    crate::ResourceTargetKind::Gain
                },
                resource: "pool.example".to_owned(),
                goal: 100,
                baseline: Some(10),
                current: Some(40),
                remaining: 70,
                progress_milli: 300,
                state: *state,
                reading: crate::ResourceTargetReading {
                    observed_at_unix_ms: Some(2_000),
                    fresh_until_unix_ms: Some(3_000),
                    pending_reason: Some(crate::ResourceTargetPendingReason::Expired),
                },
                completed_at_unix_ms: Some(2_500),
                updated_at_unix_ms: 1_500,
            })
            .collect::<Vec<_>>();
        let view = |rows: Vec<crate::ResourceTargetRow>| RuntimeResult::ResourceTargetView {
            view: Box::new(crate::ResourceTargetView {
                instance_alias: "node.a".to_owned(),
                policy_instance: "node.a".to_owned(),
                evaluated_at_unix_ms: 9_000,
                as_of_ledger_position: 12,
                catalog_hash: None,
                targetable: Vec::new(),
                not_targetable: Vec::new(),
                active: Some(crate::ActiveResourceTargets {
                    policy_sha256: format!("sha256:{}", "a".repeat(64)),
                    schema_version: "actingcommand.resource-targets.v2".to_owned(),
                    version: 12,
                    event_id,
                    valid_until_unix_ms: None,
                    expired: false,
                    targets: vec![json!({"id": "t0"})],
                    conditions: Vec::new(),
                    rows,
                }),
                stale_schema: None,
            }),
        };
        let encoded = round_trips(&view(rows));
        for (index, (_, spelling)) in states.iter().enumerate() {
            assert_eq!(
                encoded["view"]["active"]["rows"][index]["state"],
                json!(*spelling)
            );
        }
        assert_eq!(encoded["view"]["active"]["rows"][1]["kind"], json!("gain"));
        refuses_unknown::<RuntimeResult>(encoded.clone(), &["view"]);
        refuses_unknown::<RuntimeResult>(encoded.clone(), &["view", "active"]);
        let mut nested = encoded.clone();
        nested["view"]["active"]["rows"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded;
        nested["view"]["active"]["rows"][0]["reading"]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let encoded = round_trips(&view(Vec::new()));
        assert!(encoded["view"]["active"].get("rows").is_none());
    }

    // Fix round 2 (register §1.213 R0-10): A.5's `stale_schema` mark at `ResourceTargetView`'s
    // top level. Filled, it round-trips beside a view with no `active` (a stored v1 or v2 record
    // reads as no table); absent, the view has no `stale_schema` key; either way the view still
    // refuses an unknown member.
    #[test]
    fn r0_resource_target_view_carries_stale_schema_only_when_filled() {
        let view = |stale_schema: Option<bool>| RuntimeResult::ResourceTargetView {
            view: Box::new(crate::ResourceTargetView {
                instance_alias: "node.a".to_owned(),
                policy_instance: "node.a".to_owned(),
                evaluated_at_unix_ms: 9_000,
                as_of_ledger_position: 12,
                catalog_hash: None,
                targetable: Vec::new(),
                not_targetable: Vec::new(),
                active: None,
                stale_schema,
            }),
        };
        let encoded = round_trips(&view(Some(true)));
        assert_eq!(encoded["view"]["stale_schema"], json!(true));
        assert!(encoded["view"].get("active").is_none());
        refuses_unknown::<RuntimeResult>(encoded.clone(), &["view"]);

        let encoded = round_trips(&view(None));
        assert!(encoded["view"].get("stale_schema").is_none());
        refuses_unknown::<RuntimeResult>(encoded, &["view"]);
    }

    // R-0c (register §1.216 R0C-3): `PackScheduleView`'s request and answer round-trip, each
    // object level refuses an unknown member, every source is spelled, the optional members are
    // absent when not filled, and the request takes its own kind.
    #[test]
    fn r0c_pack_schedule_view_round_trips_and_refuses_an_unknown_member() {
        let operation = RuntimeOperation::PackScheduleView {
            instance_alias: "node.a".to_owned(),
        };
        assert_eq!(operation.kind(), RuntimeOperationKind::PackScheduleView);
        assert!(operation.validate().is_ok());
        let encoded = round_trips(&operation);
        assert_eq!(
            encoded,
            json!({"operation": "pack_schedule_view", "instance_alias": "node.a"})
        );
        refuses_unknown::<RuntimeOperation>(encoded, &[]);

        let sources = [
            (PackScheduleSource::Instance, "instance"),
            (PackScheduleSource::Server, "server"),
            (PackScheduleSource::Game, "game"),
            (PackScheduleSource::PackDefault, "pack_default"),
            (PackScheduleSource::None, "none"),
        ];
        let packages = sources
            .iter()
            .enumerate()
            .map(|(index, (source, _))| PackSchedulePackage {
                package_id: format!("package.{index}"),
                schedulable: index % 2 == 0,
                source: *source,
                row: (*source != PackScheduleSource::None).then_some(PackScheduleEditRow {
                    from_unix_ms: 1_000,
                    until_unix_ms: Some(2_000),
                    schedulable: index % 2 == 0,
                }),
                next_change_unix_ms: (*source != PackScheduleSource::None).then_some(2_000),
                below: None,
            })
            .collect::<Vec<_>>();
        let viewed = RuntimeResult::PackScheduleViewed {
            view: Box::new(PackScheduleView {
                instance_alias: "node.a".to_owned(),
                scopes: vec![
                    PackScheduleScopeVersion {
                        scope: FactScope::Instance {
                            instance_id: "node.a".to_owned(),
                        },
                        version: 0,
                    },
                    PackScheduleScopeVersion {
                        scope: FactScope::Server {
                            server_id: "server.a".to_owned(),
                        },
                        version: 7,
                    },
                    PackScheduleScopeVersion {
                        scope: FactScope::Game {
                            game_id: "game.a".to_owned(),
                        },
                        version: 9,
                    },
                ],
                packages,
            }),
        };
        let encoded = round_trips(&viewed);
        for (index, (_, spelling)) in sources.iter().enumerate() {
            assert_eq!(
                encoded["view"]["packages"][index]["source"],
                json!(*spelling)
            );
        }
        // A package no layer decides has no row and no next change.
        let undecided = &encoded["view"]["packages"][4];
        assert!(undecided.get("row").is_none(), "{undecided}");
        assert!(
            undecided.get("next_change_unix_ms").is_none(),
            "{undecided}"
        );
        refuses_unknown::<RuntimeResult>(encoded.clone(), &[]);
        refuses_unknown::<RuntimeResult>(encoded.clone(), &["view"]);
        let mut nested = encoded.clone();
        nested["view"]["scopes"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["view"]["scopes"][1]["scope"]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["view"]["packages"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["view"]["packages"][0]["row"]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded;
        nested["view"]["packages"][0]["source"] = json!("default");
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
    }

    // R-0d (register §1.217 R0D-1): a package's `below`, the layer below's decision. Filled, it
    // round-trips as `{schedulable, source}` under the package; absent (the deciding layer is the
    // pack default or `none`), the package has no `below` key on the wire. Every object level, the
    // `below` object included, refuses an unknown member.
    #[test]
    fn r0d_pack_schedule_package_carries_below_only_when_filled_and_refuses_an_unknown_member() {
        let row = |schedulable: bool| PackScheduleEditRow {
            from_unix_ms: 1_000,
            until_unix_ms: Some(2_000),
            schedulable,
        };
        // (deciding source, its decision, the layer below's decision when there is one)
        let cases = [
            (
                PackScheduleSource::Instance,
                false,
                Some((true, PackScheduleSource::Server, "server")),
            ),
            (
                PackScheduleSource::Instance,
                false,
                Some((true, PackScheduleSource::None, "none")),
            ),
            (
                PackScheduleSource::Server,
                true,
                Some((false, PackScheduleSource::Game, "game")),
            ),
            (
                PackScheduleSource::Game,
                false,
                Some((true, PackScheduleSource::PackDefault, "pack_default")),
            ),
            (PackScheduleSource::PackDefault, false, None),
            (PackScheduleSource::None, true, None),
        ];
        let packages = cases
            .iter()
            .enumerate()
            .map(
                |(index, (source, schedulable, below))| PackSchedulePackage {
                    package_id: format!("package.{index}"),
                    schedulable: *schedulable,
                    source: *source,
                    row: (*source != PackScheduleSource::None).then_some(row(*schedulable)),
                    next_change_unix_ms: (*source != PackScheduleSource::None).then_some(2_000),
                    below: below.map(|(schedulable, source, _)| PackScheduleBelow {
                        schedulable,
                        source,
                    }),
                },
            )
            .collect::<Vec<_>>();
        let viewed = RuntimeResult::PackScheduleViewed {
            view: Box::new(PackScheduleView {
                instance_alias: "node.a".to_owned(),
                scopes: vec![PackScheduleScopeVersion {
                    scope: FactScope::Instance {
                        instance_id: "node.a".to_owned(),
                    },
                    version: 3,
                }],
                packages,
            }),
        };
        let encoded = round_trips(&viewed);
        for (index, (_, _, below)) in cases.iter().enumerate() {
            let package = &encoded["view"]["packages"][index];
            match below {
                Some((schedulable, _, spelling)) => assert_eq!(
                    package["below"],
                    json!({"schedulable": schedulable, "source": spelling}),
                    "{package}"
                ),
                None => assert!(package.get("below").is_none(), "{package}"),
            }
        }

        // A package without `below` is exactly R-0c's package on the wire, and one written
        // without the key reads as `None`.
        let without = PackSchedulePackage {
            package_id: "package.a".to_owned(),
            schedulable: true,
            source: PackScheduleSource::None,
            row: None,
            next_change_unix_ms: None,
            below: None,
        };
        assert_eq!(
            round_trips(&without),
            json!({"package_id": "package.a", "schedulable": true, "source": "none"})
        );
        let below = PackScheduleBelow {
            schedulable: true,
            source: PackScheduleSource::Game,
        };
        let encoded_below = round_trips(&below);
        assert_eq!(
            encoded_below,
            json!({"schedulable": true, "source": "game"})
        );
        refuses_unknown::<PackScheduleBelow>(encoded_below, &[]);

        // Every level refuses an unknown member, with `below` present.
        refuses_unknown::<RuntimeResult>(encoded.clone(), &[]);
        refuses_unknown::<RuntimeResult>(encoded.clone(), &["view"]);
        let mut nested = encoded.clone();
        nested["view"]["scopes"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["view"]["packages"][0]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["view"]["packages"][0]["row"]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded.clone();
        nested["view"]["packages"][0]["below"]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
        let mut nested = encoded;
        nested["view"]["packages"][0]["below"]["source"] = json!("default");
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
    }

    // Review L6: the status member `activity` at the Status level. Filled, it round-trips in
    // the instance's row; absent, the row has no `activity` key, which is why 0.12.30 keeps
    // `runtime-client` `min` at 2 (a revision-2 reader refuses the member).
    #[test]
    fn r0_status_carries_activity_only_when_filled() {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let owner_epoch = *ids.mint_owner_epoch().expect("owner epoch").transport();
        let request_id = *ids.mint_request_id().expect("request id").transport();
        let instance = |alias: &str| {
            crate::RuntimeInstanceStatus::new(
                alias,
                *ids.mint_instance_id().expect("instance id").transport(),
                false,
                0,
                false,
                false,
                false,
            )
            .expect("instance status")
        };
        let activity = InstanceActivity {
            kind: InstanceActivityKind::DirectRun,
            request_id: Some(request_id),
            package_id: Some("package.example".to_owned()),
            since_unix_ms: 5_000,
        };
        let status = crate::RuntimeControlPlaneStatus::new(
            owner_epoch,
            vec![
                instance("node.a").with_activity(Some(activity.clone())),
                instance("node.b"),
            ],
        )
        .expect("status");
        let encoded = round_trips(&RuntimeResult::Status { status });
        let rows = encoded["status"]["instances"]
            .as_array()
            .expect("instance rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["instance_alias"], json!("node.a"));
        assert_eq!(
            rows[0]["activity"],
            serde_json::to_value(&activity).expect("activity encodes")
        );
        assert_eq!(rows[1]["instance_alias"], json!("node.b"));
        assert!(rows[1].get("activity").is_none());

        let decoded: crate::RuntimeControlPlaneStatus =
            serde_json::from_value(encoded["status"].clone()).expect("status decodes");
        decoded.validate().expect("status validates");
        assert_eq!(decoded.instances()[0].activity(), Some(&activity));
        assert_eq!(decoded.instances()[1].activity(), None);

        let mut nested = encoded;
        nested["status"]["instances"][0]["activity"]["unknown_member"] = json!(1);
        assert!(serde_json::from_value::<RuntimeResult>(nested).is_err());
    }
}
