//! Thin stdio MCP adapter over `ravnpad-cli`.

use serde_json::{Value, json};
use std::io::{self, BufRead as _, Write as _};
use std::process::{Command, Stdio};

const MCP_VERSION: &str = "2025-11-25";

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("stdin: {error}");
                break;
            }
        };
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_message(
                    &mut stdout,
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":error.to_string()}}),
                );
                continue;
            }
        };
        if message.get("id").is_none() {
            continue;
        }
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let response = match message.get("method").and_then(Value::as_str) {
            Some("initialize") => success(
                id,
                json!({
                    "protocolVersion": MCP_VERSION,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "ravnpad", "version": env!("CARGO_PKG_VERSION")}
                }),
            ),
            Some("ping") => success(id, json!({})),
            Some("tools/list") => success(id, json!({"tools": tools()})),
            Some("tools/call") => call_tool(
                id,
                message.pointer("/params/name").and_then(Value::as_str),
                message.pointer("/params/arguments"),
            ),
            Some(method) => failure(id, -32601, format!("unknown method: {method}")),
            None => failure(id, -32600, "missing method"),
        };
        write_message(&mut stdout, response);
    }
}

fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "ravnpad_document_status",
            "description": "Return the current document identity for a RavnPad instance whose agent access is set to Explore or Edit.",
            "inputSchema": object_schema(json!({"instance_id":{"type":"string"}}), &["instance_id"]),
            "annotations": {"readOnlyHint": true}
        }),
        json!({
            "name": "ravnpad_document_read",
            "description": "Read the live RavnPad buffer. This never falls back to reading the file on disk.",
            "inputSchema": object_schema(json!({
                "instance_id":{"type":"string"}, "document_id":{"type":"string"},
                "offset":{"type":"integer","minimum":0}, "limit":{"type":"integer","minimum":0}
            }), &["instance_id","document_id"]),
            "annotations": {"readOnlyHint": true}
        }),
        json!({
            "name": "ravnpad_document_propose",
            "description": "Submit a revision-bound UTF-8 byte patch for direct application to the active buffer. This requires Agent > Edit in the target RavnPad app. Validation rejects stale or ambiguous edits, and the target file is not saved.",
            "inputSchema": object_schema(json!({
                "instance_id":{"type":"string"}, "document_id":{"type":"string"},
                "operation_id":{"type":"string"}, "base_revision":{"type":"integer","minimum":0},
                "base_hash":{"type":"string","pattern":"^sha256:"},
                "edits":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"properties":{
                    "start_byte":{"type":"integer","minimum":0}, "end_byte":{"type":"integer","minimum":0},
                    "expected_text":{"type":"string"}, "replacement":{"type":"string"}
                },"required":["start_byte","end_byte","expected_text","replacement"]}}
            }), &["instance_id","document_id","operation_id","base_revision","base_hash","edits"]),
            "annotations": {"readOnlyHint": false, "destructiveHint": true}
        }),
    ]
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","additionalProperties":false,"properties":properties,"required":required})
}

fn call_tool(id: Value, name: Option<&str>, arguments: Option<&Value>) -> Value {
    let Some(name) = name else {
        return failure(id, -32602, "missing tool name");
    };
    let arguments = arguments.cloned().unwrap_or_else(|| json!({}));
    match execute_tool(name, &arguments) {
        Ok(value) => success(id, tool_result(value, false)),
        Err(error) => success(id, tool_result(json!({"error": error}), true)),
    }
}

fn execute_tool(name: &str, arguments: &Value) -> Result<Value, String> {
    let result = match name {
        "ravnpad_document_status" => run_cli(
            &[
                "document",
                "status",
                "--instance",
                string(&arguments, "instance_id")?,
                "--json",
            ],
            None,
        ),
        "ravnpad_document_read" => {
            let mut args = vec![
                "document",
                "read",
                "--instance",
                string(&arguments, "instance_id")?,
                "--document",
                string(&arguments, "document_id")?,
                "--json",
            ];
            let offset;
            if let Some(value) = arguments.get("offset").and_then(Value::as_u64) {
                offset = value.to_string();
                args.extend(["--offset", &offset]);
            }
            let limit;
            if let Some(value) = arguments.get("limit").and_then(Value::as_u64) {
                limit = value.to_string();
                args.extend(["--limit", &limit]);
            }
            run_cli(&args, None)
        }
        "ravnpad_document_propose" => {
            let instance = string(&arguments, "instance_id")?;
            let document = string(&arguments, "document_id")?;
            let patch = json!({
                "operation_id": arguments.get("operation_id"), "document_id": document,
                "base_revision": arguments.get("base_revision"), "base_hash": arguments.get("base_hash"),
                "edits": arguments.get("edits")
            });
            run_cli(
                &[
                    "document",
                    "propose",
                    "--instance",
                    instance,
                    "--document",
                    document,
                    "--stdin",
                    "--json",
                ],
                Some(patch.to_string().as_bytes()),
            )
        }
        _ => return Err(format!("unknown tool: {name}")),
    };
    result
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or invalid {key}"))
}

fn run_cli(args: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let suffix = std::env::consts::EXE_SUFFIX;
    let cli = executable.with_file_name(format!("ravnpad-cli{suffix}"));
    let mut child = Command::new(cli)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .ok_or("cannot open CLI stdin")?
            .write_all(input)
            .map_err(|error| error.to_string())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
    json!({"content":[{"type":"text","text":text}],"structuredContent":value,"isError":is_error})
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn failure(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.into()}})
}
fn write_message(output: &mut impl io::Write, message: Value) {
    if serde_json::to_writer(&mut *output, &message).is_ok() {
        let _ = output.write_all(b"\n");
        let _ = output.flush();
    }
}
