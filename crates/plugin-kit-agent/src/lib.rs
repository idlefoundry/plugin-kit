//! Local MCP transport for instruments. The application owns a [`server::Server`] and
//! drains its inbox on the window thread. A separate binary runs [`mcp::serve`] with a
//! [`bridge::Bridge`]. No audio-thread code belongs here. Adapted from CA-72 (GPL-3.0-or-later).
mod awake;
pub mod bridge;
pub mod discovery;
pub mod mcp;
pub mod server;
pub mod setup;
pub mod tools;

/// Product identity and capabilities supplied by the instrument, shared by bridge and window.
#[derive(Clone, Debug)]
pub struct Config {
    pub name: &'static str,
    pub version: &'static str,
    pub instructions: &'static str,
    pub tools: Vec<tools::Tool>,
}

use serde_json::{Value, json};

/// The local socket an instrument listens on and a bridge reaches it by: Unix's, and on Windows the
/// same AF_UNIX socket (Windows 10 1803 and later), so that one path serves both.
#[cfg(unix)]
pub(crate) use std::os::unix::net::{UnixListener as Listener, UnixStream as Stream};
#[cfg(windows)]
pub(crate) use uds_windows::{UnixListener as Listener, UnixStream as Stream};

/// The version of the conversation between the bridge and an instrument (`hello`'s `protocol`): a
/// bridge refuses an instance that speaks another.
pub const PROTOCOL: u64 = 1;

/// JSON-RPC's error codes, and ours (-32000 to -32099, the range it leaves to servers).
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    /// The instance's window did not take the request in its time.
    pub const TIMEOUT: i64 = -32001;
    /// Too many requests waiting, or too many clients.
    pub const BUSY: i64 = -32002;
    /// No instrument to talk to (none open with agents allowed, or the one chosen has closed).
    pub const NO_INSTANCE: i64 = -32003;
    /// Refused as the panel would refuse it (a control unavailable in the current mode).
    pub const REFUSED: i64 = -32004;
    /// Several instruments are open and none is chosen.
    pub const AMBIGUOUS: i64 = -32005;
}

/// An error a request ends in: its code, what to say, and anything that helps put it right
/// (the range a value must be in, the choices a selector has).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl ApiError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn params(message: impl Into<String>) -> Self {
        ApiError::new(codes::INVALID_PARAMS, message)
    }

    pub fn refused(message: impl Into<String>) -> Self {
        ApiError::new(codes::REFUSED, message)
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// As JSON-RPC's `error` member.
    pub fn to_json(&self) -> Value {
        let mut e = json!({"code": self.code, "message": self.message});
        if let Some(d) = &self.data {
            e["data"] = d.clone();
        }
        e
    }

    /// From JSON-RPC's `error` member (what an instance answered).
    pub fn from_json(v: &Value) -> Self {
        ApiError {
            code: v
                .get("code")
                .and_then(Value::as_i64)
                .unwrap_or(codes::INTERNAL),
            message: v
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_owned(),
            data: v.get("data").cloned(),
        }
    }
}

pub type Reply = Result<Value, ApiError>;

/// An argument of a call: `args[key]`, absent or null as none.
pub fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    args.get(key).filter(|v| !v.is_null())
}

/// A string argument that must be there.
pub fn text_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    match arg(args, key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(ApiError::params(format!("`{key}` must be a string"))),
        None => Err(ApiError::params(format!("`{key}` is required"))),
    }
}

/// A boolean argument, `default` when absent.
pub fn flag_arg(args: &Value, key: &str, default: bool) -> Result<bool, ApiError> {
    match arg(args, key) {
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ApiError::params(format!("`{key}` must be true or false"))),
        None => Ok(default),
    }
}

#[cfg(test)]
fn test_config() -> Config {
    use tools::{Runs, Tool};
    Config {
        name: "test",
        version: env!("CARGO_PKG_VERSION"),
        instructions: "Test instrument",
        tools: [
            ("test_instances", "instances", Runs::Bridge),
            ("test_select", "select", Runs::Bridge),
            ("test_describe", "describe", Runs::Instance),
            ("test_get", "parameters.get", Runs::Instance),
            ("test_set", "parameters.set", Runs::Instance),
        ]
        .into_iter()
        .map(|(name, method, runs)| Tool {
            name,
            method,
            runs,
            description: "Test",
            schema: serde_json::json!({"type":"object"}),
            read_only: false,
        })
        .collect(),
    }
}
