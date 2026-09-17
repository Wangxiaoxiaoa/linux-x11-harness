use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use lxh_core::LxhError;
use lxh_preview::PreviewManager;
use lxh_runtime::Runtime;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;

use crate::handlers::{self, ClientSession, DaemonState};
use crate::protocol::{Request, Response};

pub struct DaemonServer {
    state: Arc<DaemonState>,
    socket_path: PathBuf,
}

impl DaemonServer {
    pub fn new(runtime: Arc<Runtime>, socket_path: PathBuf) -> Self {
        Self {
            state: Arc::new(DaemonState {
                runtime,
                displays: Arc::new(RwLock::new(HashMap::new())),
                drivers: Arc::new(RwLock::new(HashMap::new())),
                previews: Arc::new(PreviewManager::new()),
            }),
            socket_path,
        }
    }

    pub async fn run(&self) -> Result<(), LxhError> {
        let _ = std::fs::remove_file(&self.socket_path);
        let listener = UnixListener::bind(&self.socket_path)
            .map_err(|e| LxhError::InvalidArgument(format!("cannot bind socket: {e}")))?;

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|e| LxhError::InvalidArgument(format!("accept failed: {e}")))?;
            let state = Arc::clone(&self.state);
            tokio::spawn(async move {
                let mut session = ClientSession::new();
                session.client_name = peer_name(&stream);
                let state2 = Arc::clone(&state);
                if let Err(e) = handle_client(state, &mut session, stream).await {
                    eprintln!("client handler error: {e}");
                }
                cleanup_session(state2, &mut session).await;
            });
        }
    }
}

async fn handle_client(
    state: Arc<DaemonState>,
    session: &mut ClientSession,
    stream: UnixStream,
) -> Result<(), LxhError> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| LxhError::InvalidArgument(format!("read failed: {e}")))?;
        if n == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let req = match serde_json::from_str::<Request>(&line) {
            Ok(req) => req,
            Err(e) => {
                let resp = Response::error(None, -32700, &format!("invalid json: {e}"));
                let msg = serde_json::to_string(&resp)
                    .map_err(|e| LxhError::InvalidArgument(format!("serialize failed: {e}")))?;
                write_half.write_all(msg.as_bytes()).await.ok();
                write_half.write_all(b"\n").await.ok();
                continue;
            }
        };

        let resp = dispatch(&state, session, req).await;
        let msg = serde_json::to_string(&resp)
            .map_err(|e| LxhError::InvalidArgument(format!("serialize failed: {e}")))?;
        write_half
            .write_all(msg.as_bytes())
            .await
            .map_err(|e| LxhError::InvalidArgument(format!("write failed: {e}")))?;
        write_half
            .write_all(b"\n")
            .await
            .map_err(|e| LxhError::InvalidArgument(format!("write failed: {e}")))?;
    }

    Ok(())
}

async fn dispatch(state: &DaemonState, session: &mut ClientSession, req: Request) -> Response {
    let result = match req.method.as_str() {
        "initialize" => {
            let supported = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
            let requested = req.params.get("protocolVersion").and_then(|v| v.as_str());
            let protocol_version = requested
                .and_then(|r| supported.iter().find(|&&s| s == r).copied())
                .unwrap_or("2025-11-25");
            Ok(json!({
                "protocolVersion": protocol_version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "linux-x11-harness", "version": "0.1.0" }
            }))
        }
        "tools/list" => Ok(json!({ "tools": handlers::tool_definitions() })),
        "tools/call" => dispatch_tool_call(state, session, &req.params).await,
        _ => Err(LxhError::InvalidArgument(format!(
            "unknown method: {}",
            req.method
        ))),
    };

    match result {
        Ok(value) => Response::result(req.id, value),
        Err(e) => Response::error(req.id, -32603, &e.to_string()),
    }
}

async fn dispatch_tool_call(
    state: &DaemonState,
    session: &mut ClientSession,
    params: &Value,
) -> Result<Value, LxhError> {
    let name = params["name"]
        .as_str()
        .ok_or_else(|| LxhError::InvalidArgument("tool name required".into()))?;
    let args = &params["arguments"];
    match name {
        "lxh_display_create" => handlers::create_display(state, session, args).await,
        "lxh_display_destroy" => handlers::destroy_display(state, session, args).await,
        "lxh_display_attach" => handlers::attach_display(state, args).await,
        "lxh_display_detach" => handlers::detach_display(state, args).await,
        "lxh_app_launch" => handlers::app_launch(state, args).await,
        "lxh_app_terminate" => handlers::app_terminate(state, args).await,
        "lxh_input_click" => handlers::click(state, args).await,
        "lxh_input_move" => handlers::move_mouse(state, args).await,
        "lxh_input_type" => handlers::input_type(state, args).await,
        "lxh_input_key" => handlers::input_key(state, args).await,
        "lxh_input_scroll" => handlers::scroll(state, args).await,
        "lxh_input_drag" => handlers::drag(state, args).await,
        "lxh_input_get_cursor_position" => handlers::get_cursor_position(state, args).await,
        "lxh_capture_screenshot" => handlers::screenshot(state, args).await,
        "lxh_capture_window" => handlers::screenshot_window(state, args).await,
        "lxh_window_focus" => handlers::window_focus(state, args).await,
        "lxh_window_set_frame" => handlers::window_set_frame(state, args).await,
        "lxh_window_close" => handlers::window_close(state, args).await,
        "lxh_click_element" => handlers::click_element(state, args).await,
        "lxh_wait" => handlers::wait(state, args).await,
        "lxh_clipboard_get" => handlers::clipboard_get(state, args).await,
        "lxh_clipboard_set" => handlers::clipboard_set(state, args).await,
        "lxh_display_info" => handlers::display_info(state, args).await,
        "lxh_preview_open" => handlers::preview_open(state, session, args).await,
        "lxh_preview_close" => handlers::preview_close(state, args).await,
        "lxh_get_window_state" => handlers::get_window_state(state, args).await,
        "lxh_get_desktop_overview" => handlers::get_desktop_overview(state, args).await,
        "lxh_set_value" => handlers::set_value(state, args).await,
        _ => Err(LxhError::InvalidArgument(format!("unknown tool: {name}"))),
    }
}

async fn cleanup_session(state: Arc<DaemonState>, session: &mut ClientSession) {
    let mut displays = state.displays.write().await;
    for id in session.owned_displays.drain() {
        state.previews.close(&id);
        if let Some(display) = displays.remove(&id) {
            let _ = display.lock().await.destroy().await;
        }
    }
}

fn peer_name(stream: &UnixStream) -> Option<String> {
    let cred = stream.peer_cred().ok()?;
    let pid = cred.pid()?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = comm.trim();
    let friendly = match name {
        "claude" | "claude-code" => "Claude",
        "codex" => "Codex",
        "kimi" | "kimi-code" => "Kimi",
        "qwen" => "Qwen",
        "opencode" => "OpenCode",
        "pi" => "Pi",
        _ => return None,
    };
    Some(friendly.to_string())
}
