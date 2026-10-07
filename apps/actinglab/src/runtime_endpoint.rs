// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome};
use serde_json::{Value, json};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(super) struct RuntimeEndpointPolicy {
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) channel: RuntimeEndpointChannel,
}

/// Workflow #355 D4: the trusted-remote channel was retired; ActingLab talks only to a Runtime
/// on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeEndpointChannel {
    LocalDirect,
}

impl RuntimeEndpointChannel {
    fn as_str(self) -> &'static str {
        match self {
            RuntimeEndpointChannel::LocalDirect => "local_direct",
        }
    }
}

pub(super) fn runtime_endpoint_check(endpoint: &str) -> Value {
    match runtime_endpoint_policy(endpoint) {
        Ok(policy) => {
            let reachable = runtime_tcp_available(endpoint);
            json!({
                "ok": reachable,
                "endpoint": endpoint,
                "reachable": reachable,
                "policy": runtime_endpoint_policy_json(&policy)
            })
        }
        Err(err) => json!({
            "ok": false,
            "endpoint": endpoint,
            "error_code": err.code,
            "error": err.message,
            "blocked_by": err.blocked_by
        }),
    }
}

pub(super) fn runtime_endpoint_policy(endpoint: &str) -> CliOutcome<RuntimeEndpointPolicy> {
    let (scheme, host, port) = parse_endpoint_parts(endpoint).ok_or_else(|| {
        CliError::runtime_not_running(format!(
            "runtime endpoint is invalid; expected host:port, http://host:port, or https://host:port, got {endpoint}"
        ))
    })?;
    // The Runtime binds a loopback IP only (runtime-host refuses any bind address whose IP is
    // not loopback), so the endpoint host must be a loopback IP literal. Names, `localhost`
    // included, are refused rather than resolved.
    if !host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) {
        return Err(CliError::safety_blocked(
            "runtime_endpoint_not_local",
            format!(
                "ActingLab connects only to a Runtime on this machine; use 127.0.0.1:<port> or [::1]:<port>, got {host}"
            ),
            &["local_runtime_only"],
        ));
    }
    Ok(RuntimeEndpointPolicy {
        scheme,
        host,
        port,
        channel: RuntimeEndpointChannel::LocalDirect,
    })
}

pub(super) fn runtime_endpoint_policy_json(policy: &RuntimeEndpointPolicy) -> Value {
    json!({
        "channel": policy.channel.as_str(),
        "scheme": policy.scheme,
        "host": policy.host,
        "port": policy.port
    })
}

pub(super) fn runtime_tcp_available(endpoint: &str) -> bool {
    let Some((host, port)) = parse_endpoint_host_port(endpoint) else {
        return false;
    };
    let Ok(mut addrs) = (host.as_str(), port).to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok())
}

fn parse_endpoint_host_port(endpoint: &str) -> Option<(String, u16)> {
    parse_endpoint_parts(endpoint).map(|(_scheme, host, port)| (host, port))
}

fn parse_endpoint_parts(endpoint: &str) -> Option<(String, String, u16)> {
    let (scheme, trimmed) = if let Some(rest) = endpoint.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(rest) = endpoint.strip_prefix("https://") {
        ("https", rest)
    } else {
        ("tcp", endpoint)
    };
    let host_port = trimmed.split('/').next()?;
    let (host, port) = host_port.rsplit_once(':')?;
    Some((
        scheme.to_string(),
        host.trim_matches(['[', ']']).to_string(),
        port.parse().ok()?,
    ))
}
