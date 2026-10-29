// SPDX-License-Identifier: AGPL-3.0-only

//! The wire shapes of the player workbench (Workflow #429 A.2 R-0, register §1.213, since
//! 0.12.30; `contracts/runtime-interface.md`, "Workbench request kinds"). They are defined once,
//! before their behaviour, so that the console pins `runtime-client` 3 once and later slices only
//! add behaviour. Like U-1's shapes ([`super::pack_registry`]), every request kind that uses them
//! is refused `not_implemented` until the slice named on it adds the behaviour and its shape
//! checks. No member says how many times a task ran or may run (Alice's ruling 100): the routine
//! summary's buckets are health statistics only.

use crate::outcome::CodeStr;
use crate::{EventId, RequestId};
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
/// `valid_until_unix_ms`; no `tasks`, no `on_met`), parsed and checked by R-13a.
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
/// changed after it are answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSummaryPage {
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

/// What `EditPackSchedule` answers (R-8): the sequence and event of the `fact.published` that
/// holds the scope's whole list after the edit; `version` is the next `expected_version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackScheduleEdited {
    pub version: u64,
    pub event_id: EventId,
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
                    expected_revision: Some(12),
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
            expected_revision: None,
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

        let summary = RuntimeResult::RoutineSummary {
            summary: Box::new(RoutineSummaryPage {
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
                            failures: vec![RoutineSummaryFailure {
                                code: serde_json::from_value(json!("contained_task_cancelled"))
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
    }
}
