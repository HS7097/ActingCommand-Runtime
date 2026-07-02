// SPDX-License-Identifier: AGPL-3.0-only

//! Declarative recovery execution primitives for ActingLab self-heal plans.
//!
//! This module is intentionally independent from device control. It evaluates a
//! recovery graph against a caller-provided runtime so recovery plans can be
//! tested without opening ADB, capture, MaaTouch, OCR, resources, or game logic.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryGraph {
    pub schema_version: String,
    pub entry: String,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_max_node_visits")]
    pub max_node_visits: u32,
    pub nodes: Vec<RecoveryNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryNode {
    pub id: String,
    #[serde(default)]
    pub detect: Option<DetectKind>,
    #[serde(default)]
    pub action_escalation: Vec<RecoveryAction>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default)]
    pub on_error: Option<String>,
    #[serde(default)]
    pub save_on_error: bool,
    #[serde(default, with = "duration_millis")]
    pub reco_timeout: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DetectKind {
    Always,
    Signal { name: String },
    Page { page_id: String },
    Freeze { stable_frames: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveryAction {
    WaitFreezes {
        stable_frames: u32,
        #[serde(default, with = "duration_millis")]
        interval: Option<Duration>,
    },
    Signal {
        name: String,
    },
    Tap {
        x: i32,
        y: i32,
    },
    AppRestart {
        package: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverySignal {
    Matched,
    NotMatched,
    ActionSucceeded,
    ActionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStatus {
    Completed,
    Failed,
    MaxAttempts,
    LoopDetected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryExecutionReport {
    pub status: RecoveryStatus,
    pub visited_nodes: Vec<String>,
    pub attempts: u32,
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryExecError {
    message: String,
}

impl RecoveryExecError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RecoveryExecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for RecoveryExecError {}

pub type RecoveryResult<T> = Result<T, RecoveryExecError>;

pub trait RecoveryRuntime {
    fn detect(&mut self, detect: &DetectKind) -> RecoveryResult<RecoverySignal>;
    fn run_action(
        &mut self,
        action: &RecoveryAction,
        timeout: Option<Duration>,
    ) -> RecoveryResult<RecoverySignal>;
    fn wait_freezes(
        &mut self,
        stable_frames: u32,
        interval: Option<Duration>,
        timeout: Option<Duration>,
    ) -> RecoveryResult<RecoverySignal>;
    fn save_on_error(&mut self, node_id: &str, reason: &str) -> RecoveryResult<()>;
}

pub fn execute_recovery_graph(
    graph: &RecoveryGraph,
    runtime: &mut impl RecoveryRuntime,
) -> RecoveryResult<RecoveryExecutionReport> {
    let nodes = validate_graph(graph)?;
    let mut attempts = 0;
    let mut current = graph.entry.as_str();
    let mut visited_nodes = Vec::new();
    let mut node_visits: BTreeMap<String, u32> = BTreeMap::new();

    loop {
        if attempts >= graph.max_attempts {
            return Ok(RecoveryExecutionReport {
                status: RecoveryStatus::MaxAttempts,
                visited_nodes,
                attempts,
                terminal_reason: Some(format!(
                    "recovery reached max_attempts={}",
                    graph.max_attempts
                )),
            });
        }
        let node = nodes.get(current).ok_or_else(|| {
            RecoveryExecError::fatal(format!("recovery node '{current}' is missing"))
        })?;
        let visits = node_visits.entry(current.to_string()).or_insert(0);
        *visits += 1;
        if *visits > graph.max_node_visits {
            return Ok(RecoveryExecutionReport {
                status: RecoveryStatus::LoopDetected,
                visited_nodes,
                attempts,
                terminal_reason: Some(format!("recovery loop detected at node '{current}'")),
            });
        }
        attempts += 1;
        visited_nodes.push(current.to_string());

        let outcome = run_recovery_node(node, runtime)?;
        if outcome == NodeOutcome::Succeeded {
            match node.next.as_deref() {
                Some(next) => current = next,
                None => {
                    return Ok(RecoveryExecutionReport {
                        status: RecoveryStatus::Completed,
                        visited_nodes,
                        attempts,
                        terminal_reason: None,
                    });
                }
            }
        } else if let Some(next) = node.on_error.as_deref() {
            if node.save_on_error {
                runtime.save_on_error(&node.id, outcome.reason())?;
            }
            current = next;
        } else {
            if node.save_on_error {
                runtime.save_on_error(&node.id, outcome.reason())?;
            }
            return Ok(RecoveryExecutionReport {
                status: RecoveryStatus::Failed,
                visited_nodes,
                attempts,
                terminal_reason: Some(format!("node '{}' failed: {}", node.id, outcome.reason())),
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeOutcome {
    Succeeded,
    DetectFailed,
    ActionFailed,
}

impl NodeOutcome {
    fn reason(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::DetectFailed => "detect_failed",
            Self::ActionFailed => "action_failed",
        }
    }
}

fn run_recovery_node(
    node: &RecoveryNode,
    runtime: &mut impl RecoveryRuntime,
) -> RecoveryResult<NodeOutcome> {
    if let Some(detect) = node.detect.as_ref() {
        match runtime.detect(detect)? {
            RecoverySignal::Matched => {}
            RecoverySignal::NotMatched => return Ok(NodeOutcome::DetectFailed),
            signal => {
                return Err(RecoveryExecError::fatal(format!(
                    "detect for node '{}' returned invalid signal {:?}",
                    node.id, signal
                )));
            }
        }
    }
    if node.action_escalation.is_empty() {
        return Ok(NodeOutcome::Succeeded);
    }
    for action in &node.action_escalation {
        let signal = match action {
            RecoveryAction::WaitFreezes {
                stable_frames,
                interval,
            } => runtime.wait_freezes(*stable_frames, *interval, node.reco_timeout)?,
            _ => runtime.run_action(action, node.reco_timeout)?,
        };
        match signal {
            RecoverySignal::ActionSucceeded => return Ok(NodeOutcome::Succeeded),
            RecoverySignal::ActionFailed => {}
            invalid => {
                return Err(RecoveryExecError::fatal(format!(
                    "action for node '{}' returned invalid signal {:?}",
                    node.id, invalid
                )));
            }
        }
    }
    Ok(NodeOutcome::ActionFailed)
}

fn validate_graph(graph: &RecoveryGraph) -> RecoveryResult<BTreeMap<&str, &RecoveryNode>> {
    if graph.max_attempts == 0 {
        return Err(RecoveryExecError::fatal(
            "recovery graph max_attempts must be greater than 0",
        ));
    }
    if graph.max_node_visits == 0 {
        return Err(RecoveryExecError::fatal(
            "recovery graph max_node_visits must be greater than 0",
        ));
    }
    if graph.entry.trim().is_empty() {
        return Err(RecoveryExecError::fatal(
            "recovery graph entry must not be empty",
        ));
    }

    let mut nodes = BTreeMap::new();
    for node in &graph.nodes {
        if node.id.trim().is_empty() {
            return Err(RecoveryExecError::fatal(
                "recovery node id must not be empty",
            ));
        }
        if nodes.insert(node.id.as_str(), node).is_some() {
            return Err(RecoveryExecError::fatal(format!(
                "duplicate recovery node id '{}'",
                node.id
            )));
        }
    }
    if !nodes.contains_key(graph.entry.as_str()) {
        return Err(RecoveryExecError::fatal(format!(
            "recovery entry node '{}' is missing",
            graph.entry
        )));
    }

    let ids = nodes.keys().copied().collect::<BTreeSet<_>>();
    for node in &graph.nodes {
        validate_edge(&ids, &node.id, "next", node.next.as_deref())?;
        validate_edge(&ids, &node.id, "on_error", node.on_error.as_deref())?;
        validate_node(node)?;
    }
    Ok(nodes)
}

fn validate_edge(
    ids: &BTreeSet<&str>,
    source: &str,
    field: &str,
    target: Option<&str>,
) -> RecoveryResult<()> {
    if let Some(target) = target
        && !ids.contains(target)
    {
        return Err(RecoveryExecError::fatal(format!(
            "recovery node '{source}' {field} target '{target}' is missing"
        )));
    }
    Ok(())
}

fn validate_node(node: &RecoveryNode) -> RecoveryResult<()> {
    for action in &node.action_escalation {
        if let RecoveryAction::WaitFreezes { stable_frames, .. } = action
            && *stable_frames == 0
        {
            return Err(RecoveryExecError::fatal(format!(
                "recovery node '{}' wait_freezes stable_frames must be greater than 0",
                node.id
            )));
        }
    }
    if matches!(node.detect, Some(DetectKind::Freeze { stable_frames: 0 })) {
        return Err(RecoveryExecError::fatal(format!(
            "recovery node '{}' freeze detect stable_frames must be greater than 0",
            node.id
        )));
    }
    Ok(())
}

fn default_max_attempts() -> u32 {
    8
}

fn default_max_node_visits() -> u32 {
    3
}

mod duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(value: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value
            .map(|duration| duration.as_millis() as u64)
            .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<u64>::deserialize(deserializer)?;
        Ok(value.map(Duration::from_millis))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_follows_on_error_edge() {
        let graph = graph_with_nodes(vec![
            RecoveryNode {
                id: "check".to_string(),
                detect: Some(DetectKind::Signal {
                    name: "missing".to_string(),
                }),
                action_escalation: Vec::new(),
                next: None,
                on_error: Some("recover".to_string()),
                save_on_error: true,
                reco_timeout: None,
            },
            RecoveryNode {
                id: "recover".to_string(),
                detect: None,
                action_escalation: vec![RecoveryAction::Signal {
                    name: "light".to_string(),
                }],
                next: None,
                on_error: None,
                save_on_error: false,
                reco_timeout: None,
            },
        ]);
        let mut runtime = FakeRuntime::default()
            .with_detect("missing", RecoverySignal::NotMatched)
            .with_action("light", RecoverySignal::ActionSucceeded);

        let report = execute_recovery_graph(&graph, &mut runtime).expect("recovery should run");

        assert_eq!(report.status, RecoveryStatus::Completed);
        assert_eq!(report.visited_nodes, ["check", "recover"]);
        assert_eq!(
            runtime.saved_errors,
            [("check".to_string(), "detect_failed".to_string())]
        );
    }

    #[test]
    fn recovery_wait_freezes_waits_until_stable() {
        let graph = graph_with_nodes(vec![RecoveryNode {
            id: "wait".to_string(),
            detect: None,
            action_escalation: vec![RecoveryAction::WaitFreezes {
                stable_frames: 3,
                interval: Some(Duration::from_millis(25)),
            }],
            next: None,
            on_error: None,
            save_on_error: false,
            reco_timeout: Some(Duration::from_millis(500)),
        }]);
        let mut runtime =
            FakeRuntime::default().with_wait_freezes(3, RecoverySignal::ActionSucceeded);

        let report = execute_recovery_graph(&graph, &mut runtime).expect("wait should run");

        assert_eq!(report.status, RecoveryStatus::Completed);
        assert_eq!(
            runtime.wait_calls,
            [(
                3,
                Some(Duration::from_millis(25)),
                Some(Duration::from_millis(500))
            )]
        );
    }

    #[test]
    fn recovery_stops_at_max_attempts() {
        let mut graph = graph_with_nodes(vec![RecoveryNode {
            id: "retry".to_string(),
            detect: None,
            action_escalation: vec![RecoveryAction::Signal {
                name: "fail".to_string(),
            }],
            next: None,
            on_error: Some("retry".to_string()),
            save_on_error: false,
            reco_timeout: None,
        }]);
        graph.max_attempts = 2;
        graph.max_node_visits = 10;
        let mut runtime = FakeRuntime::default().with_action("fail", RecoverySignal::ActionFailed);

        let report =
            execute_recovery_graph(&graph, &mut runtime).expect("max attempts should stop");

        assert_eq!(report.status, RecoveryStatus::MaxAttempts);
        assert_eq!(report.visited_nodes, ["retry", "retry"]);
        assert_eq!(report.attempts, 2);
    }

    #[test]
    fn recovery_detects_loop_before_unbounded_retry() {
        let mut graph = graph_with_nodes(vec![RecoveryNode {
            id: "loop".to_string(),
            detect: None,
            action_escalation: Vec::new(),
            next: Some("loop".to_string()),
            on_error: None,
            save_on_error: false,
            reco_timeout: None,
        }]);
        graph.max_attempts = 10;
        graph.max_node_visits = 2;
        let mut runtime = FakeRuntime::default();

        let report = execute_recovery_graph(&graph, &mut runtime).expect("loop should be reported");

        assert_eq!(report.status, RecoveryStatus::LoopDetected);
        assert_eq!(report.visited_nodes, ["loop", "loop"]);
    }

    fn graph_with_nodes(nodes: Vec<RecoveryNode>) -> RecoveryGraph {
        RecoveryGraph {
            schema_version: "actinglab.recovery.v0.1".to_string(),
            entry: nodes[0].id.clone(),
            max_attempts: 8,
            max_node_visits: 3,
            nodes,
        }
    }

    #[derive(Default)]
    struct FakeRuntime {
        detects: BTreeMap<String, RecoverySignal>,
        actions: BTreeMap<String, RecoverySignal>,
        waits: BTreeMap<u32, RecoverySignal>,
        saved_errors: Vec<(String, String)>,
        wait_calls: Vec<(u32, Option<Duration>, Option<Duration>)>,
    }

    impl FakeRuntime {
        fn with_detect(mut self, name: &str, signal: RecoverySignal) -> Self {
            self.detects.insert(name.to_string(), signal);
            self
        }

        fn with_action(mut self, name: &str, signal: RecoverySignal) -> Self {
            self.actions.insert(name.to_string(), signal);
            self
        }

        fn with_wait_freezes(mut self, stable_frames: u32, signal: RecoverySignal) -> Self {
            self.waits.insert(stable_frames, signal);
            self
        }
    }

    impl RecoveryRuntime for FakeRuntime {
        fn detect(&mut self, detect: &DetectKind) -> RecoveryResult<RecoverySignal> {
            match detect {
                DetectKind::Always => Ok(RecoverySignal::Matched),
                DetectKind::Signal { name } => Ok(*self
                    .detects
                    .get(name)
                    .unwrap_or(&RecoverySignal::NotMatched)),
                DetectKind::Page { page_id } => Ok(*self
                    .detects
                    .get(page_id)
                    .unwrap_or(&RecoverySignal::NotMatched)),
                DetectKind::Freeze { stable_frames } => Ok(*self
                    .waits
                    .get(stable_frames)
                    .unwrap_or(&RecoverySignal::NotMatched)),
            }
        }

        fn run_action(
            &mut self,
            action: &RecoveryAction,
            _timeout: Option<Duration>,
        ) -> RecoveryResult<RecoverySignal> {
            match action {
                RecoveryAction::Signal { name } => Ok(*self
                    .actions
                    .get(name)
                    .unwrap_or(&RecoverySignal::ActionFailed)),
                RecoveryAction::Tap { .. } | RecoveryAction::AppRestart { .. } => {
                    Ok(RecoverySignal::ActionFailed)
                }
                RecoveryAction::WaitFreezes { .. } => Err(RecoveryExecError::fatal(
                    "wait_freezes must use wait_freezes",
                )),
            }
        }

        fn wait_freezes(
            &mut self,
            stable_frames: u32,
            interval: Option<Duration>,
            timeout: Option<Duration>,
        ) -> RecoveryResult<RecoverySignal> {
            self.wait_calls.push((stable_frames, interval, timeout));
            Ok(*self
                .waits
                .get(&stable_frames)
                .unwrap_or(&RecoverySignal::ActionFailed))
        }

        fn save_on_error(&mut self, node_id: &str, reason: &str) -> RecoveryResult<()> {
            self.saved_errors
                .push((node_id.to_string(), reason.to_string()));
            Ok(())
        }
    }
}
