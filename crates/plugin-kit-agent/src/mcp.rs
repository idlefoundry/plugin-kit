//! The Model Context Protocol over standard input and output: newline-delimited JSON-RPC 2.0,
//! one message a line. Its shape (the protocol revisions spoken, `initialize`, `tools/list`,
//! `tools/call` answering a failed call as a result with `isError` rather than a JSON-RPC
//! error, so that the agent reads why) follows the owner's DAW's MCP server (DAW-v3,
//! `crates/daw-api/src/mcp.rs`, GPL-3.0-or-later as this is). Every tool is carried out by a
//! [`Backend`]: the bridge's, which passes it to a instrument (`bridge.rs`), or a test's.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

use super::tools::Tool;
use super::{ApiError, Reply, codes};
use crate::Config;

/// Protocol revisions this server speaks, newest first (DAW-v3's).
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The longest line read: a request is a few hundred bytes; past this it is refused.
pub const MAX_LINE: usize = 1 << 20;

/// What carries out a tool's call.
pub trait Backend {
    fn call(&mut self, tool: &Tool, args: &Value) -> Reply;
    /// The client's name, as `initialize` gives it.
    fn client(&mut self, _name: &str) {}
}

fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// A tool's outcome as `tools/call`'s result: its JSON as text, and structured; a failure
/// with `isError`.
pub fn tool_result(result: Reply) -> Value {
    let (mut body, is_error) = match result {
        Ok(r) => (r, false),
        Err(ApiError {
            message,
            data,
            code,
        }) => {
            let mut e = json!({"error": message, "code": code});
            if let Some(d) = data {
                e["details"] = d;
            }
            (e, true)
        }
    };
    // Offline measurements can return PNGs as native MCP image content. Keep base64 out
    // of text and structuredContent so a client does not receive it three times.
    let images = body.as_object_mut().and_then(|b| b.remove("__mcp_images"));
    let text = serde_json::to_string_pretty(&body).unwrap_or_default();
    let mut content = vec![json!({"type": "text", "text": text})];
    if let Some(Value::Array(images)) = images {
        content.extend(images);
    }
    let structured = if body.is_object() {
        body
    } else {
        json!({"result": body})
    };
    json!({
        "content": content,
        "structuredContent": structured,
        "isError": is_error,
    })
}

/// Handles one message; `None` for a notification.
pub fn handle(backend: &mut dyn Backend, config: &Config, msg: &Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else {
        return Some(error(
            Value::Null,
            codes::INVALID_REQUEST,
            "expected a JSON-RPC object (batches are not taken)",
        ));
    };
    let id = obj.get("id").cloned();
    if id
        .as_ref()
        .is_some_and(|v| !v.is_null() && !v.is_string() && !v.is_number())
    {
        return Some(error(
            Value::Null,
            codes::INVALID_REQUEST,
            "invalid request id",
        ));
    }
    if obj.get("jsonrpc") != Some(&json!("2.0")) {
        return Some(error(
            id.unwrap_or(Value::Null),
            codes::INVALID_REQUEST,
            "expected JSON-RPC 2.0",
        ));
    }
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // (A response to a request of ours: we make none.)
        return id
            .filter(|_| !obj.contains_key("result") && !obj.contains_key("error"))
            .map(|id| error(id, codes::INVALID_REQUEST, "missing method"));
    };
    let args = obj.get("params").cloned().unwrap_or(json!({}));
    // Notifications (no id) get no answer: `notifications/initialized`, `/cancelled`.
    let id = id?;
    Some(match method {
        "initialize" => {
            let asked = args.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|v| PROTOCOL_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            if let Some(client) = args.pointer("/clientInfo/name").and_then(Value::as_str) {
                backend.client(client);
            }
            reply(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": config.name, "version": config.version},
                    "instructions": config.instructions,
                }),
            )
        }
        "ping" => reply(id, json!({})),
        "tools/list" => reply(
            id,
            json!({"tools": config.tools.iter().map(Tool::json).collect::<Vec<_>>()}),
        ),
        "tools/call" => {
            let name = args.get("name").and_then(Value::as_str).unwrap_or("");
            let call_args = args.get("arguments").cloned().unwrap_or(json!({}));
            match config.tools.iter().find(|t| t.name == name) {
                None => error(id, codes::INVALID_PARAMS, &format!("unknown tool `{name}`")),
                Some(_) if !call_args.is_object() => {
                    error(id, codes::INVALID_PARAMS, "`arguments` must be an object")
                }
                Some(t) => reply(id, tool_result(backend.call(t, &call_args))),
            }
        }
        "resources/list" => reply(id, json!({"resources": []})),
        "resources/templates/list" => reply(id, json!({"resourceTemplates": []})),
        "prompts/list" => reply(id, json!({"prompts": []})),
        other => error(
            id,
            codes::METHOD_NOT_FOUND,
            &format!("unknown method `{other}`"),
        ),
    })
}

/// Serves MCP over line-delimited JSON until `input` closes.
pub fn serve(
    backend: &mut dyn Backend,
    config: &Config,
    input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    let mut input = input;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n =
            std::io::Read::take(&mut input, MAX_LINE as u64 + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        let response = if line.len() > MAX_LINE {
            // The rest of the line is let go.
            if !line.ends_with(b"\n") {
                loop {
                    let available = input.fill_buf()?;
                    if available.is_empty() {
                        break;
                    }
                    let end = available.iter().position(|b| *b == b'\n');
                    let n = end.map_or(available.len(), |i| i + 1);
                    input.consume(n);
                    if end.is_some() {
                        break;
                    }
                }
            }
            Some(error(
                Value::Null,
                codes::INVALID_REQUEST,
                "the message is too long",
            ))
        } else {
            let text = String::from_utf8_lossy(&line);
            if text.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&text) {
                Ok(msg) => handle(backend, config, &msg),
                Err(e) => Some(error(Value::Null, codes::PARSE_ERROR, &e.to_string())),
            }
        };
        if let Some(r) = response {
            writeln!(output, "{r}")?;
            output.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers every tool with its method and arguments, or fails test_get.
    #[derive(Default)]
    struct Echo {
        client: String,
    }

    impl Backend for Echo {
        fn call(&mut self, tool: &Tool, args: &Value) -> Reply {
            if tool.name == "test_get" {
                return Err(ApiError::refused("no").with_data(json!({"why": "test"})));
            }
            Ok(json!({"method": tool.method, "args": args}))
        }
        fn client(&mut self, name: &str) {
            self.client = name.to_owned();
        }
    }

    fn run(lines: &[&str]) -> (Vec<Value>, Echo) {
        let mut e = Echo::default();
        let input = lines.join("\n");
        let mut out = Vec::new();
        serve(&mut e, &crate::test_config(), input.as_bytes(), &mut out).expect("serve");
        let replies = String::from_utf8(out)
            .expect("utf-8")
            .lines()
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect();
        (replies, e)
    }

    /// The handshake: the revision asked for if spoken (else the newest), the tools offered,
    /// notifications unanswered, the client named.
    #[test]
    fn it_initializes_and_lists_its_tools() {
        let (r, e) = run(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"tester"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
            r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#,
        ]);
        assert_eq!(r.len(), 4, "{r:?}");
        assert_eq!(r[0]["result"]["protocolVersion"], json!("2025-06-18"));
        assert_eq!(
            r[0]["result"]["serverInfo"]["version"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        let names: Vec<&str> = r[1]["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"test_describe") && names.contains(&"test_set"));
        assert_eq!(
            r[2]["result"]["protocolVersion"],
            json!(PROTOCOL_VERSIONS[0])
        );
        assert_eq!(r[3]["id"], json!("p"));
        assert_eq!(e.client, "tester");
    }

    /// A call reaches the backend with its arguments; its failure is a result the agent reads
    /// (`isError`), an unknown tool or method a JSON-RPC error, bad JSON a parse error.
    #[test]
    fn calls_and_their_errors() {
        let (r, _) = run(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"test_set","arguments":{"values":{"cutoff":1}}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"test_get","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"test_nothing","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"test_set","arguments":[1]}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"sing"}"#,
            r#"{"jsonrpc":"2.0","id":6"#,
            r#"[{"jsonrpc":"2.0","id":7,"method":"ping"}]"#,
            r#"{"jsonrpc":"2.0","id":8}"#,
        ]);
        assert_eq!(r.len(), 8, "{r:?}");
        let ok = &r[0]["result"];
        assert_eq!(ok["isError"], json!(false));
        assert_eq!(ok["structuredContent"]["method"], json!("parameters.set"));
        assert_eq!(
            ok["structuredContent"]["args"]["values"]["cutoff"],
            json!(1)
        );
        let text = ok["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("parameters.set"));
        let failed = &r[1]["result"];
        assert_eq!(failed["isError"], json!(true));
        assert_eq!(failed["structuredContent"]["error"], json!("no"));
        assert_eq!(failed["structuredContent"]["details"]["why"], json!("test"));
        assert_eq!(r[2]["error"]["code"], json!(codes::INVALID_PARAMS));
        assert_eq!(r[3]["error"]["code"], json!(codes::INVALID_PARAMS));
        assert_eq!(r[4]["error"]["code"], json!(codes::METHOD_NOT_FOUND));
        assert_eq!(r[5]["error"]["code"], json!(codes::PARSE_ERROR));
        assert_eq!(r[6]["error"]["code"], json!(codes::INVALID_REQUEST));
        assert_eq!(r[7]["error"]["code"], json!(codes::INVALID_REQUEST));
    }

    /// A line longer than it takes is refused, and the next is read as usual.
    #[test]
    fn a_line_too_long_is_refused() {
        let long = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"ping","x":"{}"}}"#,
            "a".repeat(MAX_LINE)
        );
        let (r, _) = run(&[&long, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["error"]["code"], json!(codes::INVALID_REQUEST));
        assert_eq!(r[1]["id"], json!(2));
    }
    #[test]
    fn an_oversized_line_ending_at_the_read_limit_keeps_the_next_message() {
        let long = " ".repeat(MAX_LINE);
        let (r, _) = run(&[&long, r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["error"]["code"], json!(codes::INVALID_REQUEST));
        assert_eq!(r[1]["id"], json!(2));
    }
}
