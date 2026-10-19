//! Chromium DevTools Protocol helpers (localhost only).
//!
//! Message builders are pure and unit-tested. Discovery scans 127.0.0.1 ports
//! (OCU_CDP_PORT or 9222..=9229). WebSocket traffic uses tungstenite without TLS.

use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const DEFAULT_PORTS: &[u16] = &[9222, 9223, 9224, 9225, 9226, 9227, 9228, 9229];

#[derive(Debug, Clone)]
pub struct CdpTarget {
    pub port: u16,
    pub ws_url: String,
    pub title: String,
    pub url: String,
    pub id: String,
    pub typ: String,
}

/// Build a JSON-RPC CDP request body.
pub fn build_request(id: u64, method: &str, params: Value) -> String {
    let mut obj = Map::new();
    obj.insert("id".into(), json!(id));
    obj.insert("method".into(), json!(method));
    if !params.is_null() {
        obj.insert("params".into(), params);
    }
    serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| "{}".into())
}

pub fn click_params(x: f64, y: f64, button: &str, click_count: u32) -> Value {
    json!({
        "type": "mousePressed",
        "x": x,
        "y": y,
        "button": button,
        "clickCount": click_count.max(1)
    })
}

pub fn click_release_params(x: f64, y: f64, button: &str, click_count: u32) -> Value {
    json!({
        "type": "mouseReleased",
        "x": x,
        "y": y,
        "button": button,
        "clickCount": click_count.max(1)
    })
}

pub fn insert_text_params(text: &str) -> Value {
    json!({"text": text})
}

pub fn key_params(key: &str, type_: &str) -> Value {
    json!({"type": type_, "key": key, "text": if key.len() == 1 { key } else { "" }})
}

pub fn navigate_params(url: &str) -> Value {
    json!({"url": url})
}

pub fn screenshot_params() -> Value {
    json!({"format": "png", "fromSurface": true, "captureBeyondViewport": false})
}

pub fn scroll_params(x: f64, y: f64, delta_x: f64, delta_y: f64) -> Value {
    json!({
        "type": "mouseWheel",
        "x": x,
        "y": y,
        "deltaX": delta_x,
        "deltaY": delta_y
    })
}

/// Ports to probe: OCU_CDP_PORT if set, else DEFAULT_PORTS.
pub fn discovery_ports() -> Vec<u16> {
    if let Ok(p) = std::env::var("OCU_CDP_PORT") {
        if let Ok(n) = p.trim().parse::<u16>() {
            return vec![n];
        }
    }
    DEFAULT_PORTS.to_vec()
}

fn http_get(host_port: &str, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(host_port).map_err(|e| format!("cdp connect {host_port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    if !text.starts_with("HTTP/1.1 200") && !text.starts_with("HTTP/1.0 200") {
        return Err(format!("cdp HTTP error from {host_port}{path}"));
    }
    Ok(body)
}

/// Discover page targets on localhost CDP ports.
pub fn discover_targets() -> Vec<CdpTarget> {
    let mut out = Vec::new();
    for port in discovery_ports() {
        let host = format!("127.0.0.1:{port}");
        let body = match http_get(&host, "/json/list") {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(arr) = serde_json::from_str::<Value>(&body) else { continue };
        let Some(list) = arr.as_array() else { continue };
        for t in list {
            let typ = t.get("type").and_then(Value::as_str).unwrap_or("");
            if typ != "page" && typ != "webview" {
                continue;
            }
            let ws = t.get("webSocketDebuggerUrl").and_then(Value::as_str).unwrap_or("").to_string();
            // Only accept localhost endpoints.
            if !(ws.starts_with("ws://127.0.0.1") || ws.starts_with("ws://localhost")) {
                continue;
            }
            out.push(CdpTarget {
                port,
                ws_url: ws,
                title: t.get("title").and_then(Value::as_str).unwrap_or("").into(),
                url: t.get("url").and_then(Value::as_str).unwrap_or("").into(),
                id: t.get("id").and_then(Value::as_str).unwrap_or("").into(),
                typ: typ.into(),
            });
        }
    }
    out
}

pub struct CdpSession {
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    next_id: u64,
}

impl CdpSession {
    pub fn connect(ws_url: &str) -> Result<Self, String> {
        // Enforce localhost.
        if !(ws_url.starts_with("ws://127.0.0.1:") || ws_url.starts_with("ws://localhost:")) {
            return Err("CDP websocket must be ws://127.0.0.1 or ws://localhost".into());
        }
        let (socket, _) = tungstenite::connect(ws_url).map_err(|e| format!("cdp ws connect: {e}"))?;
        Ok(Self { socket, next_id: 1 })
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = build_request(id, method, params);
        self.socket
            .send(tungstenite::Message::Text(msg.into()))
            .map_err(|e| format!("cdp send: {e}"))?;
        // Read until matching id (skip events).
        for _ in 0..50 {
            let msg = self.socket.read().map_err(|e| format!("cdp read: {e}"))?;
            let text = match msg {
                tungstenite::Message::Text(t) => t.to_string(),
                tungstenite::Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                tungstenite::Message::Ping(p) => {
                    let _ = self.socket.send(tungstenite::Message::Pong(p));
                    continue;
                }
                tungstenite::Message::Close(_) => return Err("cdp connection closed".into()),
                _ => continue,
            };
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("cdp json: {e}"))?;
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(format!("cdp error: {err}"));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        Err("cdp response timed out".into())
    }
}

/// Flatten a subset of Accessibility.getFullAXTree into indexed lines.
pub fn format_ax_tree(result: &Value) -> (String, Vec<Value>) {
    let nodes = result
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = Vec::new();
    let mut indexed = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let role = n
            .pointer("/role/value")
            .or_else(|| n.get("role"))
            .and_then(|v| v.as_str().or_else(|| v.get("value").and_then(Value::as_str)))
            .unwrap_or("node");
        let name = n
            .pointer("/name/value")
            .or_else(|| n.get("name"))
            .and_then(|v| v.as_str().or_else(|| v.get("value").and_then(Value::as_str)))
            .unwrap_or("");
        let line = if name.is_empty() {
            format!("{i} {role}")
        } else {
            format!("{i} {role} \"{name}\"")
        };
        lines.push(line);
        indexed.push(n.clone());
    }
    (lines.join("\n"), indexed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_requests() {
        let s = build_request(3, "Page.navigate", navigate_params("https://example.com"));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"], 3);
        assert_eq!(v["method"], "Page.navigate");
        assert_eq!(v["params"]["url"], "https://example.com");
        let c = click_params(10.0, 20.0, "left", 1);
        assert_eq!(c["type"], "mousePressed");
        assert_eq!(c["x"], 10.0);
        let t = insert_text_params("hi");
        assert_eq!(t["text"], "hi");
        let sc = screenshot_params();
        assert_eq!(sc["format"], "png");
        let whe = scroll_params(1.0, 2.0, 0.0, 100.0);
        assert_eq!(whe["type"], "mouseWheel");
    }

    #[test]
    fn formats_ax_tree() {
        let result = json!({"nodes":[
            {"role":{"value":"RootWebArea"},"name":{"value":"Example"}},
            {"role":{"value":"button"},"name":{"value":"OK"}}
        ]});
        let (tree, idx) = format_ax_tree(&result);
        assert!(tree.contains("0 RootWebArea \"Example\""));
        assert!(tree.contains("1 button \"OK\""));
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn discovery_ports_env() {
        // Just ensure default list is non-empty; env override is runtime.
        assert!(DEFAULT_PORTS.contains(&9222));
        assert!(!discovery_ports().is_empty());
    }
}
