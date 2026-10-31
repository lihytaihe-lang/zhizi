use crate::{layer2, storage::AppStorage};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    pub enabled: bool,
    pub endpoint: Option<String>,
}

struct RunningServer {
    address: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
}

impl RunningServer {
    fn endpoint(&self) -> String {
        format!("http://{}/mcp/{}", self.address, self.token)
    }
}

/// 每次启动 server 轮换一次性访问令牌：熵取自标准库 OS 随机种子的哈希键、
/// 纳秒时钟与进程号，经 SHA-256 展开为 256 位十六进制，不引入新依赖。
fn generate_access_token() -> String {
    let mut hasher = Sha256::new();
    for round in 0_u64..4 {
        let mut seeder = RandomState::new().build_hasher();
        seeder.write_u64(round);
        hasher.update(&seeder.finish().to_le_bytes());
    }
    hasher.update(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
            .to_le_bytes(),
    );
    hasher.update(&std::process::id().to_le_bytes());
    format!("{:x}", hasher.finalize())
}

/// 恒定时间比较：不因逐字节提前返回而泄露令牌前缀。
fn constant_time_eq(expected: &str, provided: &str) -> bool {
    if expected.len() != provided.len() {
        return false;
    }
    expected
        .bytes()
        .zip(provided.bytes())
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn request_path_is_authorized(path: &str, token: &str) -> bool {
    path.strip_prefix("/mcp/")
        .is_some_and(|provided| constant_time_eq(provided, token))
}

#[derive(Default)]
pub struct McpServerState {
    running: Mutex<Option<RunningServer>>,
}

impl McpServerState {
    pub fn status(&self) -> Result<McpServerStatus, String> {
        let running = self
            .running
            .lock()
            .map_err(|_| "MCP 状态锁已损坏。".to_string())?;
        Ok(McpServerStatus {
            enabled: running.is_some(),
            endpoint: running.as_ref().map(|server| server.endpoint()),
        })
    }

    pub fn set_enabled(&self, app: AppHandle, enabled: bool) -> Result<McpServerStatus, String> {
        let mut running = self
            .running
            .lock()
            .map_err(|_| "MCP 状态锁已损坏。".to_string())?;
        if !enabled {
            if let Some(server) = running.take() {
                server.stop.store(true, Ordering::Release);
                let _ = TcpStream::connect_timeout(&server.address, Duration::from_millis(100));
            }
            return Ok(McpServerStatus {
                enabled: false,
                endpoint: None,
            });
        }
        if let Some(server) = running.as_ref() {
            return Ok(McpServerStatus {
                enabled: true,
                endpoint: Some(server.endpoint()),
            });
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("无法启动本地 MCP server：{error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("无法读取 MCP 地址：{error}"))?;
        if !address.ip().is_loopback() {
            return Err("MCP server 拒绝监听非 loopback 地址。".into());
        }
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("无法设置 MCP server：{error}"))?;
        // 每次启动都生成新令牌；重启后旧令牌随之失效。
        let token = generate_access_token();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_token = token.clone();
        thread::Builder::new()
            .name("agent-input-mcp".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, peer)) if peer.ip().is_loopback() => {
                            handle_connection(&app, &worker_token, stream)
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| format!("无法创建 MCP server 线程：{error}"))?;
        let endpoint = format!("http://{address}/mcp/{token}");
        *running = Some(RunningServer {
            address,
            token,
            stop,
        });
        Ok(McpServerStatus {
            enabled: true,
            endpoint: Some(endpoint),
        })
    }
}

enum ReadRequest {
    Unauthorized,
    Body(String),
}

fn read_http_body(stream: &mut TcpStream, token: &str) -> Result<ReadRequest, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected = None;
    loop {
        let count = stream
            .read(&mut buffer)
            .map_err(|error| format!("MCP 请求读取失败：{error}"))?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if expected.is_none() {
            if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let request_line = headers.lines().next().unwrap_or_default();
                if !request_line.starts_with("POST ") {
                    return Err("MCP 只接受 POST 请求。".into());
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or_default();
                // 路径或令牌不对时直接拒绝，不处理请求体。
                if !request_path_is_authorized(path, token) {
                    return Ok(ReadRequest::Unauthorized);
                }
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .ok_or_else(|| "MCP 请求缺少 Content-Length。".to_string())?;
                expected = Some(header_end + 4 + length);
            }
        }
        if expected.is_some_and(|length| request.len() >= length) {
            break;
        }
        if request.len() > 1024 * 1024 {
            return Err("MCP 请求过大。".into());
        }
    }
    let header_end = request
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .ok_or_else(|| "MCP HTTP 请求不完整。".to_string())?;
    let body = String::from_utf8(request[header_end + 4..].to_vec())
        .map_err(|_| "MCP 请求必须是 UTF-8。".to_string())?;
    Ok(ReadRequest::Body(body))
}

fn http_response(status: &str, body: Option<&str>) -> Vec<u8> {
    let body = body.unwrap_or("");
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn handle_connection(app: &AppHandle, token: &str, mut stream: TcpStream) {
    let storage = app.state::<AppStorage>();
    handle_connection_with_storage(&storage, token, &mut stream);
}

fn handle_connection_with_storage(storage: &AppStorage, token: &str, stream: &mut TcpStream) {
    let response = match read_http_body(stream, token) {
        Ok(ReadRequest::Unauthorized) => http_response(
            "401 Unauthorized",
            Some(
                &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32001,"message":"MCP 访问令牌无效或已过期。"}})
                    .to_string(),
            ),
        ),
        Ok(ReadRequest::Body(body)) => {
            match serde_json::from_str::<Value>(&body)
                .map_err(|_| "MCP JSON-RPC 请求无效。".to_string())
                .and_then(|request| dispatch(storage, request))
            {
                Ok(Some(value)) => http_response("200 OK", Some(&value.to_string())),
                Ok(None) => http_response("202 Accepted", None),
                Err(message) => http_response(
                    "400 Bad Request",
                    Some(
                        &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":message}})
                            .to_string(),
                    ),
                ),
            }
        }
        Err(message) => http_response(
            "400 Bad Request",
            Some(
                &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":message}})
                    .to_string(),
            ),
        ),
    };
    let _ = stream.write_all(&response);
}

fn tool_result(id: Value, value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".into());
    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":text}],"structuredContent":value,"isError":false}})
}

fn dispatch(storage: &AppStorage, request: Value) -> Result<Option<Value>, String> {
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err("MCP 仅接受 JSON-RPC 2.0。".into());
    }
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "MCP 请求缺少 method。".to_string())?;
    let Some(id) = request.get("id").cloned() else {
        return Ok(None);
    };
    let response = match method {
        "initialize" => {
            json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"agent-input-method-local","version":env!("CARGO_PKG_VERSION")}}})
        }
        "tools/list" => json!({"jsonrpc":"2.0","id":id,"result":{"tools":[
            {"name":"list_personal_terms","description":"列出本地个人词与用词偏好；不含打字历史或事件原文。","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"list_project_terms","description":"列出当前 Project Profile 的本地术语与缩写。","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"list_action_modes","description":"列出九种本地动作模式与模板用途。","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"render_prompt","description":"仅渲染本地模板；不会调用 Provider 或发起网络请求。","inputSchema":{"type":"object","properties":{"raw_input":{"type":"string"},"mode":{"type":"string"},"compact":{"type":"boolean"}},"required":["raw_input","mode"],"additionalProperties":false}},
            {"name":"search_knowledge","description":"只读检索 owner 已授权、已批准且敏感开关开启的本地项目知识；不会写入或发起网络请求。","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"project_id":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":50,"default":5}},"required":["query","project_id"],"additionalProperties":false}}
        ]}}),
        "tools/call" => {
            let params = request
                .get("params")
                .and_then(Value::as_object)
                .ok_or_else(|| "tools/call 缺少 params。".to_string())?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "tools/call 缺少工具名。".to_string())?;
            let arguments = params
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match name {
                "list_personal_terms" => tool_result(
                    id,
                    serde_json::to_value(layer2::list_personal_terms(storage)?)
                        .map_err(|error| error.to_string())?,
                ),
                "list_project_terms" => tool_result(
                    id,
                    serde_json::to_value(layer2::list_project_terms(storage)?)
                        .map_err(|error| error.to_string())?,
                ),
                "list_action_modes" => tool_result(
                    id,
                    serde_json::to_value(layer2::ACTION_MODES)
                        .map_err(|error| error.to_string())?,
                ),
                "render_prompt" => {
                    let raw = arguments
                        .get("raw_input")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "render_prompt 缺少 raw_input。".to_string())?;
                    let mode = arguments
                        .get("mode")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "render_prompt 缺少 mode。".to_string())?;
                    let compact = arguments
                        .get("compact")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    tool_result(
                        id,
                        json!({"prompt":layer2::render_local_prompt(mode, raw, &[], compact)?}),
                    )
                }
                "search_knowledge" => {
                    let query = arguments
                        .get("query")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "search_knowledge 缺少 query。".to_string())?;
                    let project_id = arguments
                        .get("project_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "search_knowledge 缺少 project_id。".to_string())?;
                    let limit = arguments
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map(usize::try_from)
                        .transpose()
                        .map_err(|_| "search_knowledge limit 超出范围。".to_string())?
                        .unwrap_or(5);
                    tool_result(
                        id,
                        serde_json::to_value(storage.search_knowledge(query, project_id, limit)?)
                            .map_err(|error| error.to_string())?,
                    )
                }
                _ => {
                    return Ok(Some(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"只读 MCP server 不支持该工具。"}}),
                    ))
                }
            }
        }
        _ => json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"MCP 方法不存在。"}}),
    };
    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::KnowledgeSourceInput;
    use std::fs;

    fn storage() -> (AppStorage, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "agent-input-mcp-{}-{}",
            std::process::id(),
            thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("test.sqlite3");
        (AppStorage::open(path.clone()).unwrap(), path)
    }

    #[test]
    fn exposes_exactly_five_read_only_tools_and_local_render() {
        let (storage, path) = storage();
        let list = dispatch(
            &storage,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .unwrap()
        .unwrap();
        let names = list
            .pointer("/result/tools")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "list_personal_terms",
                "list_project_terms",
                "list_action_modes",
                "render_prompt",
                "search_knowledge"
            ]
        );
        assert!(!list.to_string().contains("write"));
        assert!(!list.to_string().contains("events.jsonl"));
        let rendered = dispatch(&storage, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"render_prompt","arguments":{"raw_input":"本地原文","mode":"rewrite"}}})).unwrap().unwrap();
        assert!(rendered.to_string().contains("本地原文"));

        let source_root = path.parent().unwrap().join("project-docs");
        fs::create_dir_all(&source_root).unwrap();
        fs::write(
            source_root.join("reference.md"),
            "McpNeedle | mcp needle\nMcpNeedle 只通过本地只读工具检索。\n",
        )
        .unwrap();
        storage
            .save_knowledge_source(KnowledgeSourceInput {
                id: "mcp-source".into(),
                name: "MCP fixture".into(),
                source_type: "local_path".into(),
                authorized_scope: source_root.to_string_lossy().into_owned(),
                authorization: "test explicitly authorizes this fixture".into(),
                project_id: "mcp-project".into(),
                sensitive_enabled: true,
                updated_at_ms: 1,
            })
            .unwrap();
        storage.ingest_knowledge_source("mcp-source", 2).unwrap();
        let term = storage
            .list_knowledge_terms()
            .unwrap()
            .into_iter()
            .find(|term| term.text == "McpNeedle")
            .unwrap();
        storage
            .review_knowledge_term(&term.id, "approved", "mcp needle", true, 3)
            .unwrap();
        let searched = dispatch(&storage, json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_knowledge","arguments":{"query":"McpNeedle","project_id":"mcp-project"}}})).unwrap().unwrap();
        assert!(searched.to_string().contains("reference.md"));
        assert!(searched.to_string().contains("MCP fixture"));
        assert!(!searched.to_string().contains("http://"));
        drop(storage);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn status_is_disabled_by_default() {
        let state = McpServerState::default();
        assert_eq!(state.status().unwrap().enabled, false);
        assert_eq!(state.status().unwrap().endpoint, None);
    }

    /// 用给定令牌串行服务 count 个连接；线程结束后归还 storage。
    fn spawn_token_server(
        storage: AppStorage,
        token: &str,
        count: usize,
    ) -> (SocketAddr, thread::JoinHandle<AppStorage>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let token = token.to_string();
        let worker = thread::spawn(move || {
            for _ in 0..count {
                let (mut stream, peer) = listener.accept().unwrap();
                assert!(peer.ip().is_loopback());
                handle_connection_with_storage(&storage, &token, &mut stream);
            }
            storage
        });
        (address, worker)
    }

    fn post_json(address: SocketAddr, path: &str, body: &str) -> String {
        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(
                format!(
                    "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        response
    }

    fn render_body(text: &str) -> String {
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"render_prompt","arguments":{"raw_input":text,"mode":"summarize"}}}).to_string()
    }

    fn assert_unauthorized(response: &str) {
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        assert!(!response.contains("本地原文"), "{response}");
    }

    #[test]
    fn access_tokens_are_long_unique_and_hexadecimal() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..8 {
            let token = generate_access_token();
            // 256 位十六进制，远高于 128 位下限。
            assert!(token.len() * 4 >= 128, "{token}");
            assert!(
                token.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{token}"
            );
            assert!(seen.insert(token.clone()), "重复令牌：{token}");
        }
    }

    #[test]
    fn constant_time_eq_compares_bytes_without_length_leak_inside_match() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc123", "abc1234"));
        assert!(request_path_is_authorized("/mcp/abc123", "abc123"));
        assert!(!request_path_is_authorized("/mcp", "abc123"));
        assert!(!request_path_is_authorized("/mcp/", "abc123"));
        assert!(!request_path_is_authorized("/mcp/abc123/x", "abc123"));
        assert!(!request_path_is_authorized("/other/abc123", "abc123"));
    }

    #[test]
    fn loopback_http_client_calls_read_only_mcp_server_with_current_token() {
        let (storage, path) = storage();
        let token = generate_access_token();
        let (address, worker) = spawn_token_server(storage, &token, 1);
        let response = post_json(
            address,
            &format!("/mcp/{token}"),
            &render_body("loopback 本地原文"),
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("loopback 本地原文"));
        assert!(!response.contains("events.jsonl"));
        drop(worker.join().unwrap());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn requests_with_missing_or_wrong_token_get_401_without_processing_the_body() {
        let (storage, path) = storage();
        let token = generate_access_token();
        let wrong = format!(
            "{}{}",
            &token[..token.len() - 1],
            if token.ends_with('0') { '1' } else { '0' }
        );
        let (address, worker) = spawn_token_server(storage, &token, 3);
        assert_unauthorized(&post_json(address, "/mcp", &render_body("本地原文")));
        assert_unauthorized(&post_json(address, "/mcp/", &render_body("本地原文")));
        assert_unauthorized(&post_json(
            address,
            &format!("/mcp/{wrong}"),
            &render_body("本地原文"),
        ));
        drop(worker.join().unwrap());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn token_rotates_on_restart_and_old_token_is_rejected() {
        let (storage, path) = storage();
        // 第一次启动。
        let first = generate_access_token();
        let (address, worker) = spawn_token_server(storage, &first, 1);
        let response = post_json(
            address,
            &format!("/mcp/{first}"),
            &render_body("第一代令牌"),
        );
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        let storage = worker.join().unwrap();
        // 重启：生成第二代令牌，旧令牌随之失效。
        let second = generate_access_token();
        assert_ne!(first, second);
        let (address, worker) = spawn_token_server(storage, &second, 2);
        assert_unauthorized(&post_json(
            address,
            &format!("/mcp/{first}"),
            &render_body("旧令牌"),
        ));
        let response = post_json(address, &format!("/mcp/{second}"), &render_body("新令牌"));
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        drop(worker.join().unwrap());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
