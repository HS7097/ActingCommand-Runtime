// SPDX-License-Identifier: AGPL-3.0-only

//! JSON-RPC 2.0 framing and the two MCP protocol eras (#338 §四 双代协议). The era of a
//! request is read from its shape alone, never from a client name: a request whose
//! `_meta` carries `io.modelcontextprotocol/protocolVersion` is a stateless 2026-07-28
//! request; any other request needs a legacy `initialize` first.

use super::tools;
use serde_json::{Map, Value, json};

pub(super) const MODERN_VERSION: &str = "2026-07-28";
/// Every version this server speaks, the modern one first; `server/discover` and -32022
/// list all four.
pub(super) const SUPPORTED_VERSIONS: [&str; 4] =
    [MODERN_VERSION, "2025-11-25", "2025-06-18", "2025-03-26"];
const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

pub(super) const PARSE_ERROR: i64 = -32700;
pub(super) const INVALID_REQUEST: i64 = -32600;
pub(super) const METHOD_NOT_FOUND: i64 = -32601;
pub(super) const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// The -32602 answer to a request that neither follows a legacy `initialize` nor carries a
/// `_meta` protocol version.
pub(super) const DUAL_ERA_MESSAGE: &str = "actingctl-mcp is a dual-era MCP server: send initialize first (protocol 2025-11-25, 2025-06-18 or 2025-03-26), or carry params._meta[\"io.modelcontextprotocol/protocolVersion\"] = \"2026-07-28\" and params._meta[\"io.modelcontextprotocol/clientCapabilities\"] on every request";

/// The legacy versions reachable through `initialize`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LegacyVersion {
    V20250326,
    V20250618,
    V20251125,
}

impl LegacyVersion {
    /// The version the client asked for when this server speaks it; any other request
    /// gets 2025-11-25, as the legacy negotiation prescribes.
    pub(super) fn negotiate(requested: &str) -> Self {
        match requested {
            "2025-03-26" => Self::V20250326,
            "2025-06-18" => Self::V20250618,
            _ => Self::V20251125,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::V20250326 => "2025-03-26",
            Self::V20250618 => "2025-06-18",
            Self::V20251125 => "2025-11-25",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Era {
    Legacy(LegacyVersion),
    Modern,
}

impl Era {
    /// 2025-03-26 knows neither tool `title` / `outputSchema` nor `structuredContent`.
    pub(super) const fn structured(self) -> bool {
        !matches!(self, Self::Legacy(LegacyVersion::V20250326))
    }
}

/// What a request's `_meta` says about its era.
pub(super) enum MetaVersion {
    /// No `_meta` protocol version: a legacy request.
    Absent,
    /// A 2026-07-28 request with its client capabilities.
    Modern,
    /// A version string this server does not take through `_meta` (-32022).
    Unsupported(String),
    /// `_meta` or one of its required keys is malformed (-32602).
    Malformed(&'static str),
}

pub(super) fn meta_version(params: Option<&Value>) -> MetaVersion {
    let Some(meta) = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("_meta"))
    else {
        return MetaVersion::Absent;
    };
    let Some(meta) = meta.as_object() else {
        return MetaVersion::Malformed("params._meta must be an object");
    };
    let Some(version) = meta.get(META_PROTOCOL_VERSION) else {
        return MetaVersion::Absent;
    };
    let Some(version) = version.as_str() else {
        return MetaVersion::Malformed(
            "params._meta io.modelcontextprotocol/protocolVersion must be a string",
        );
    };
    if version != MODERN_VERSION {
        return MetaVersion::Unsupported(version.to_owned());
    }
    if !meta
        .get(META_CLIENT_CAPABILITIES)
        .is_some_and(Value::is_object)
    {
        return MetaVersion::Malformed(
            "request _meta is missing or has a malformed io.modelcontextprotocol/clientCapabilities",
        );
    }
    MetaVersion::Modern
}

/// One parsed JSON-RPC message.
pub(super) enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    /// A response from the client. This server sends no requests, so it is ignored.
    Response,
    /// Not a JSON-RPC 2.0 request or notification (-32600); `id` is the request's id when
    /// it is a valid one, otherwise null.
    Invalid { id: Value },
}

pub(super) fn classify(message: Value) -> Incoming {
    let Value::Object(mut message) = message else {
        return Incoming::Invalid { id: Value::Null };
    };
    if !message.contains_key("method")
        && (message.contains_key("result") || message.contains_key("error"))
    {
        return Incoming::Response;
    }
    let id = message.remove("id");
    // MCP request ids are strings or integers, never null.
    let id_valid = match &id {
        None | Some(Value::String(_)) => true,
        Some(Value::Number(number)) => number.is_i64() || number.is_u64(),
        Some(_) => false,
    };
    let reply_id = match &id {
        Some(id) if id_valid => id.clone(),
        _ => Value::Null,
    };
    let params = message.remove("params");
    if !id_valid
        || message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || params
            .as_ref()
            .is_some_and(|params| !params.is_object() && !params.is_array())
    {
        return Incoming::Invalid { id: reply_id };
    }
    match (message.remove("method"), id) {
        (Some(Value::String(method)), Some(id)) => Incoming::Request { id, method, params },
        (Some(Value::String(method)), None) => Incoming::Notification { method, params },
        _ => Incoming::Invalid { id: reply_id },
    }
}

pub(super) fn server_info() -> Value {
    json!({"name": "actingctl-mcp", "version": env!("CARGO_PKG_VERSION")})
}

pub(super) fn response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub(super) fn error_response(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// -32022 with every supported version, the modern one first.
pub(super) fn unsupported_version(id: &Value, requested: &str) -> Value {
    error_response(
        id,
        UNSUPPORTED_PROTOCOL_VERSION,
        "Unsupported protocol version",
        Some(json!({"supported": SUPPORTED_VERSIONS, "requested": requested})),
    )
}

/// Every 2026-07-28 result carries `resultType` and the server identity in `_meta`; a
/// legacy result is returned unchanged.
pub(super) fn complete(era: Era, result: Value) -> Value {
    if era != Era::Modern {
        return result;
    }
    let Value::Object(fields) = result else {
        return result;
    };
    let mut decorated = Map::new();
    decorated.insert("resultType".to_owned(), json!("complete"));
    decorated.extend(fields);
    let mut meta = Map::new();
    meta.insert(META_SERVER_INFO.to_owned(), server_info());
    decorated.insert("_meta".to_owned(), Value::Object(meta));
    Value::Object(decorated)
}

/// `server/discover`. `ttlMs` 0 and `cacheScope` private mark the answer as not cacheable.
pub(super) fn discover(id: &Value) -> Value {
    response(
        id,
        complete(
            Era::Modern,
            json!({
                "supportedVersions": SUPPORTED_VERSIONS,
                "capabilities": {"tools": {}},
                "instructions": tools::INSTRUCTIONS,
                "ttlMs": 0,
                "cacheScope": "private",
            }),
        ),
    )
}

/// The legacy `initialize` answer for the negotiated version.
pub(super) fn initialize_result(version: LegacyVersion) -> Value {
    json!({
        "protocolVersion": version.as_str(),
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": server_info(),
        "instructions": tools::INSTRUCTIONS,
    })
}
