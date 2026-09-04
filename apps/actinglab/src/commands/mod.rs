pub(super) mod capabilities;
pub(super) mod device_commands;
pub(super) mod environment_report;
pub(super) mod navigation_recovery;
pub(super) mod path_io;
pub(super) mod semantic_ledger;
pub(super) mod session_contracts;
pub(super) mod session_record;
pub(super) mod session_transport;

#[cfg(test)]
pub(crate) use capabilities::session_layer_capability_contract;
pub(crate) use capabilities::{command_capabilities, run_capabilities};
pub(crate) use device_commands::{
    CaptureFreshProbeReport, CaptureFreshProbeStatus, CaptureFreshnessExpectation,
    StreamInputRelayAction, capture_diagnosis_recovery_json, capture_for_command,
    capture_fresh_probe_report, capture_fresh_probe_report_json, open_cli_runtime_input_proxy,
    reject_legacy_session_routing, run_capture, run_direct_input, run_direct_touch,
    run_stream_input_relay, run_touch_probe,
};
#[cfg(test)]
pub(crate) use device_commands::{
    DirectInputCommand, DirectTouchCommand, classify_capture_freshness, instance_health_status,
};
#[cfg(test)]
pub(crate) use environment_report::env_needs_detection_json;
pub(crate) use environment_report::{
    attach_env_resolved, record_env_needs_detection, record_env_resolved,
};
#[cfg(test)]
pub(crate) use navigation_recovery::stale_capture_recovery_json;
pub(crate) use navigation_recovery::{
    DestructiveClick, NavigationEdge, NavigationGraph, PageDetectionOutcome, SemanticInput,
    canonical_navigation_page, detect_current_page, find_navigation_route, navigation_edge_json,
    page_detection_json, parse_navigation_graph_value, parse_point_pair, point_json, rect_center,
    rect_json, rects_intersect, reject_dangerous_semantic_id, reject_destructive_overlap,
    reject_destructive_overlap_input, run_current_page, run_detect_page, run_is_visible,
    run_locate, run_navigate, run_recognize, run_session_recover, run_tap_target,
    semantic_input_json, target_eval_json, target_evaluation_rect,
};
#[cfg(test)]
pub(crate) use path_io::{JSON_TMP_SEQ, write_json_file};
pub(crate) use path_io::{ensure_path_within, read_json_file, write_json_file_atomic};
pub(crate) use semantic_ledger::{finish_semantic_result_with_ledger, semantic_ledger_context};
#[cfg(test)]
pub(crate) use session_contracts::{
    SESSION_DAEMON_REQUEST_TIMEOUT_MS, SESSION_LEASE_STALE_MS, session_access_contract,
    session_api_contract, session_capture_policy_payload, session_self_heal_policy_payload,
};
pub(crate) use session_contracts::{
    run_session_api, run_session_capture_policy, run_session_contract, run_session_record_policy,
    run_session_self_heal_policy, run_session_throat_policy,
};
#[cfg(test)]
pub(crate) use session_record::{
    SessionRecordAnchorArtifact, SessionRecordAnchorBacktest, SessionRecordAnchorRegionResolution,
    SessionRecordContext, SessionRecordFrameProvenance, SessionRecordSourceFrame,
    SessionRecordStep, SessionRecordStepData, SessionRecordStepEvaluation, find_drift_amend_step,
    materialize_anchor_artifact_from_source, parse_session_record_drift_diagnostics,
    session_record_build_draft,
};
pub(crate) use session_record::{SessionRecordRect, SessionRecordRegion, run_session_record};
pub(crate) use session_transport::run_session_transport;
