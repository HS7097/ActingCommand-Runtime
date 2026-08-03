// SPDX-License-Identifier: AGPL-3.0-only

use super::{CliError, CliOutcome, TRUSTED_REMOTE_CLIENT_CERT_ENV, TRUSTED_REMOTE_TOKEN_ENV};
use serde_json::{Value, json};
use std::env;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(super) struct RuntimeEndpointPolicy {
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) channel: RuntimeEndpointChannel,
    pub(super) auth_material: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeEndpointChannel {
    LocalDirect,
    TrustedRemote,
}

impl RuntimeEndpointChannel {
    fn as_str(self) -> &'static str {
        match self {
            RuntimeEndpointChannel::LocalDirect => "local_direct",
            RuntimeEndpointChannel::TrustedRemote => "trusted_remote",
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
    if is_loopback_host(&host) {
        return Ok(RuntimeEndpointPolicy {
            scheme,
            host,
            port,
            channel: RuntimeEndpointChannel::LocalDirect,
            auth_material: None,
        });
    }
    if scheme != "https" {
        return Err(CliError::safety_blocked(
            "trusted_remote_transport_blocked",
            "trusted remote runtime endpoints must use https:// with encryption",
            &["trusted_remote", "encryption"],
        ));
    }
    let auth_material = trusted_remote_auth_material().ok_or_else(|| {
        CliError::safety_blocked(
            "trusted_remote_auth_required",
            format!(
                "trusted remote runtime endpoints require {TRUSTED_REMOTE_TOKEN_ENV} or {TRUSTED_REMOTE_CLIENT_CERT_ENV}"
            ),
            &["trusted_remote", "authentication"],
        )
    })?;
    Ok(RuntimeEndpointPolicy {
        scheme,
        host,
        port,
        channel: RuntimeEndpointChannel::TrustedRemote,
        auth_material: Some(auth_material),
    })
}

pub(super) fn runtime_endpoint_policy_json(policy: &RuntimeEndpointPolicy) -> Value {
    json!({
        "channel": policy.channel.as_str(),
        "scheme": policy.scheme,
        "host": policy.host,
        "port": policy.port,
        "encryption_required": policy.channel == RuntimeEndpointChannel::TrustedRemote,
        "authentication_required": policy.channel == RuntimeEndpointChannel::TrustedRemote,
        "auth_material": policy.auth_material,
        "auth_env": {
            "token": TRUSTED_REMOTE_TOKEN_ENV,
            "client_certificate": TRUSTED_REMOTE_CLIENT_CERT_ENV
        }
    })
}

fn trusted_remote_auth_material() -> Option<&'static str> {
    if env_var_non_empty(TRUSTED_REMOTE_TOKEN_ENV) {
        Some("token")
    } else if env_var_non_empty(TRUSTED_REMOTE_CLIENT_CERT_ENV) {
        Some("client_certificate")
    } else {
        None
    }
}

pub(super) fn env_var_non_empty(name: &str) -> bool {
    env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
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

fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim_matches(['[', ']']).to_ascii_lowercase();
    normalized == "localhost"
        || normalized == "::1"
        || normalized == "0:0:0:0:0:0:0:1"
        || normalized.starts_with("127.")
}
