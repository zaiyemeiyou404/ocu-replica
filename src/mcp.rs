//! MCP stdio front-end (JSON-RPC 2.0, protocol versions 2024-11-05 / 2025-03-26).
//!
//! Exposes the retained native window and control tools. Unsupported methods
//! are rejected by the shared dispatcher as well as omitted from discovery.

use serde_json::{json, Map, Value};

pub const RETAINED_TOOLS: &[&str] = &[
    "list_apps", "list_windows", "get_window", "get_window_state",
    "click_element", "type_text", "set_value", "diagnostic_state",
    "end_turn", "close", "list_displays",
];

pub const PROTOCOL_LATEST: &str = "2025-03-26";
pub const PROTOCOL_FALLBACK: &str = "2024-11-05";
pub const SERVER_NAME: &str = "ocu-replica";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn supported_protocol(client: Option<&str>) -> &'static str {
    match client {
        Some(PROTOCOL_LATEST) => PROTOCOL_LATEST,
        Some(PROTOCOL_FALLBACK) => PROTOCOL_FALLBACK,
        _ => PROTOCOL_LATEST,
    }
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

fn window_prop() -> Value {
    json!({
        "type": "object",
        "required": ["app", "id"],
        "properties": {
            "app": {"type": "string"},
            "id": {"type": "integer", "minimum": 0},
            "title": {"type": "string"}
        }
    })
}

/// Build the tools/list payload. Schemas were derived from the window2 .d.ts
/// and the original helper's serde field table.
pub fn tools_list() -> Value {
    let tools = vec![
        tool("list_apps", "List installed apps and their currently open targetable windows.", json!({"type":"object","properties":{}})),
        tool("list_windows", "List open windows that can be targeted.", json!({"type":"object","properties":{}})),
        tool("get_window", "Rehydrate a currently open window by id.", json!({
            "type":"object","required":["id"],
            "properties":{"id":{"type":"integer","minimum":0},"app":{"type":"string"}}
        })),
        tool("get_window_state", "Capture a screenshot and/or accessibility tree for a window.", json!({
            "type":"object","required":["window"],
            "properties":{
                "window": window_prop(),
                "include_screenshot":{"type":"boolean","default":true},
                "include_text":{"type":"boolean","default":false},
                "input_mode":{"type":"string","enum":["foreground","background","auto"]}
            }
        })),
        tool("click_element", "Click an indexed element from the latest get_window_state tree.", json!({
            "type":"object","required":["window","element_index"],
            "properties":{
                "window": window_prop(),
                "element_index":{"type":"integer","minimum":0},
                "mouse_button":{"type":"string"},
                "click_count":{"type":"integer","minimum":1}
            }
        })),
        tool("type_text", "Type text into the current focus in a window.", json!({
            "type":"object","required":["window","text"],
            "properties":{"window": window_prop(),"text":{"type":"string"},"element_index":{"type":"integer","minimum":0}}
        })),
        tool("set_value", "Replace the value of an indexed editable element.", json!({
            "type":"object","required":["window","element_index","value"],
            "properties":{
                "window": window_prop(),
                "element_index":{"type":"integer","minimum":0},
                "value":{"type":"string"}
            }
        })),
        tool("diagnostic_state", "Return internal cache / capture diagnostics.", json!({
            "type":"object",
            "properties":{"window": window_prop()}
        })),
        tool("end_turn", "Mark the current turn finished; clears Esc interruption for this turn.", json!({"type":"object","properties":{}})),
        tool("close", "Shut down the helper.", json!({"type":"object","properties":{}})),
        tool("list_displays", "List monitors (index, device name, rect, work area, DPI, primary).", json!({"type":"object","properties":{}})),
    ];
    let mut tools = tools;
    for tool in &mut tools {
        let schema = &mut tool["inputSchema"];
        if let Some(props) = schema["properties"].as_object_mut() {
            if props.contains_key("input_mode") {
                props.insert("input_mode".into(), json!({"type":"string","enum":["background"],"default":"background"}));
            }
        }
        if tool["name"] == "type_text" {
            tool["inputSchema"]["required"] = json!(["window", "text", "element_index"]);
            tool["description"] = json!("Insert text at the selected native edit control's caret. Requires element_index; unsupported control classes fail without a foreground fallback.");
        }
        if tool["name"] == "click_element" {
            tool["description"] = json!("Single left click on a known native Button/CheckBox control. Unsupported controls fail without a UIA or foreground fallback.");
            tool["inputSchema"]["properties"]["mouse_button"] = json!({"type":"string","enum":["left"]});
            tool["inputSchema"]["properties"]["click_count"] = json!({"type":"integer","const":1});
        }
    }
    json!({"tools": tools})
}

pub fn initialize_result(client_protocol: Option<&str>) -> Value {
    json!({
        "protocolVersion": supported_protocol(client_protocol),
        "capabilities": {
            "tools": {"listChanged": false}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        },
        "instructions": "Windows computer-use helper. Prefer element_index from get_window_state over raw coordinates. Call get_window_state again after any window change or Esc interrupt."
    })
}

pub fn rpc_ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
pub fn rpc_err(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

/// Wrap a successful call result into MCP tool content blocks.
/// If `result` contains screenshots with data-URL PNGs, each becomes an image block.
pub fn tool_result_content(result: &Value) -> Value {
    let mut content = Vec::new();
    // Always include a compact JSON text summary (without huge data URLs).
    let mut summary = result.clone();
    if let Some(shots) = summary.get_mut("screenshots").and_then(Value::as_array_mut) {
        for s in shots.iter_mut() {
            if let Some(url) = s.get("url").and_then(Value::as_str) {
                if let Some(b64) = url.strip_prefix("data:image/png;base64,") {
                    content.push(json!({
                        "type":"image",
                        "data": b64,
                        "mimeType":"image/png"
                    }));
                }
            }
            if let Some(obj) = s.as_object_mut() {
                // Replace the data URL with a short placeholder in the text summary.
                if let Some(id) = obj.get("id").cloned() {
                    obj.insert("url".into(), json!(format!("(image:{})", id.as_str().unwrap_or("?"))));
                }
            }
        }
    }
    content.insert(0, json!({
        "type":"text",
        "text": serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".into())
    }));
    json!({"content": content, "isError": false})
}

pub fn tool_error_content(message: &str) -> Value {
    json!({
        "content":[{"type":"text","text": message}],
        "isError": true
    })
}

/// Parse an inbound MCP line. Returns None for notifications (no id).
pub fn parse_rpc_line(line: &str) -> Result<Option<(Value, String, Value)>, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("malformed JSON-RPC: {e}"))?;
    let obj = v.as_object().ok_or("JSON-RPC message must be an object")?;
    let method = obj.get("method").and_then(Value::as_str).unwrap_or("").to_string();
    let id = obj.get("id").cloned();
    let params = obj.get("params").cloned().unwrap_or(Value::Object(Map::new()));
    if id.is_none() {
        // notification
        return Ok(None);
    }
    if method.is_empty() {
        return Err("JSON-RPC method is required".into());
    }
    Ok(Some((id.unwrap(), method, params)))
}

/// Extract (tool name, arguments, meta) from a tools/call params object.
pub fn tools_call_parts(params: &Value) -> Result<(String, Value, Map<String, Value>), String> {
    let obj = params.as_object().ok_or("tools/call params must be an object")?;
    let name = obj.get("name").and_then(Value::as_str).ok_or("tools/call requires name")?.to_string();
    let args = obj.get("arguments").cloned().unwrap_or(Value::Object(Map::new()));
    let meta = match obj.get("_meta").or_else(|| obj.get("meta")) {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    // Also accept turn metadata nested under arguments._meta for clients that put it there.
    let mut meta = meta;
    if let Some(Value::Object(m)) = args.get("_meta").or_else(|| args.get("meta")) {
        for (k, v) in m {
            meta.entry(k.clone()).or_insert(v.clone());
        }
    }
    Ok((name, args, meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_exposes_only_the_retained_native_tools() {
        let list = tools_list();
        let names: Vec<&str> = list["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let expected = ["list_apps", "list_windows", "get_window", "get_window_state", "click_element", "type_text", "set_value", "diagnostic_state", "end_turn", "close", "list_displays"];
        assert_eq!(names.len(), expected.len());
        for name in expected { assert!(names.contains(&name), "missing retained tool {name}"); }
    }

    #[test]
    fn tools_cover_retained_methods() {
        let list = tools_list();
        let names: Vec<&str> = list["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for m in RETAINED_TOOLS {
            assert!(names.contains(&m), "missing tool {m}");
        }
        assert_eq!(names.len(), RETAINED_TOOLS.len());
    }

    #[test]
    fn initialize_negotiates() {
        assert_eq!(supported_protocol(Some(PROTOCOL_FALLBACK)), PROTOCOL_FALLBACK);
        assert_eq!(supported_protocol(None), PROTOCOL_LATEST);
        let r = initialize_result(Some(PROTOCOL_LATEST));
        assert_eq!(r["serverInfo"]["name"], SERVER_NAME);
    }

    #[test]
    fn parse_and_call_parts() {
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#;
        let (id, method, params) = parse_rpc_line(line).unwrap().unwrap();
        assert_eq!(id, json!(1));
        assert_eq!(method, "tools/call");
        let (name, args, _) = tools_call_parts(&params).unwrap();
        assert_eq!(name, "list_apps");
        assert_eq!(args, json!({}));
        assert!(parse_rpc_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap().is_none());
    }

    #[test]
    fn tool_result_splits_images() {
        let result = json!({
            "window":{"app":"a","id":1},
            "screenshots":[{"id":"screenshot-0","zIndex":0,"url":"data:image/png;base64,QUJD","width":1,"height":1}],
            "accessibility": null
        });
        let content = tool_result_content(&result);
        let blocks = content["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["type"], "image");
        assert_eq!(blocks[1]["data"], "QUJD");
    }
}
