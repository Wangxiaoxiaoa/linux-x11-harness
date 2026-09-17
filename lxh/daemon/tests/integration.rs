use std::env;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tokio::time::timeout;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn bin_path() -> PathBuf {
    env::var("CARGO_BIN_EXE_linux-x11-harness")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/debug/linux-x11-harness"))
}

struct DaemonGuard {
    socket: PathBuf,
    pid: PathBuf,
    child: tokio::process::Child,
}

struct Connection {
    writer: tokio::net::unix::OwnedWriteHalf,
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    next_id: i64,
}

impl Connection {
    async fn call_method(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params
        });
        self.send(&req).await
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        self.next_id += 1;
        let req = json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        let mut resp = self.send(&req).await;
        // MCP wraps tool output as CallToolResult: result.content[0].text holds
        // a JSON string. Unwrap it so tests can address the tool's own fields.
        if let Some(text) = resp["result"]["content"][0]["text"].as_str() {
            if let Ok(inner) = serde_json::from_str::<Value>(text) {
                resp["result"] = inner;
            }
        }
        resp
    }

    async fn create_display(&mut self) -> Value {
        self.call_tool("lxh_display_create", json!({"preview": false}))
            .await
    }

    async fn send(&mut self, req: &Value) -> Value {
        let line = req.to_string() + "\n";
        self.writer.write_all(line.as_bytes()).await.expect("write");
        self.writer.flush().await.expect("flush");

        let mut response = String::new();
        timeout(
            Duration::from_secs(10),
            self.reader.read_line(&mut response),
        )
        .await
        .expect("timeout")
        .expect("read");
        serde_json::from_str(&response).expect("parse response")
    }
}

impl DaemonGuard {
    async fn new() -> Self {
        let tmp = env::temp_dir();
        let unique = format!(
            "{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let socket = tmp.join(format!("lxh-test-{unique}.sock"));
        let pid = tmp.join(format!("lxh-test-{unique}.pid"));
        let _ = tokio::fs::remove_file(&socket).await;
        let _ = tokio::fs::remove_file(&pid).await;

        let mut child = Command::new(bin_path())
            .arg("serve")
            .env("LXH_SOCKET_PATH", &socket)
            .env("LXH_PID_PATH", &pid)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon");

        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if socket.exists() {
                break;
            }
        }
        if !socket.exists() {
            let _ = child.kill().await;
            panic!("daemon failed to start");
        }

        Self { socket, pid, child }
    }

    async fn connect(&self) -> Connection {
        let stream = UnixStream::connect(&self.socket).await.expect("connect");
        let (reader, writer) = stream.into_split();
        Connection {
            writer,
            reader: BufReader::new(reader),
            next_id: 0,
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let pid = self.child.id().unwrap_or(0);
        if pid > 0 {
            unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        }
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.pid);
    }
}

#[tokio::test]
async fn tools_list_returns_all_tools() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let resp = conn.call_method("tools/list", json!({})).await;
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 28, "expected 28 tools");

    for tool in tools {
        assert!(tool["name"].is_string(), "tool missing name: {tool}");
        assert!(
            tool["description"].is_string(),
            "tool missing description: {tool}"
        );
        assert!(
            tool["inputSchema"].is_object(),
            "tool missing inputSchema: {tool}"
        );
    }
}

#[tokio::test]
async fn tools_call_returns_mcp_content_envelope() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    conn.next_id += 1;
    let req = json!({
        "jsonrpc": "2.0",
        "id": conn.next_id,
        "method": "tools/call",
        "params": { "name": "lxh_display_create", "arguments": {"preview": false} }
    });
    let resp = conn.send(&req).await;
    let content = resp["result"]["content"].as_array().expect("content array");
    assert_eq!(content[0]["type"], "text");
    let text = content[0]["text"].as_str().expect("text string");
    let inner: Value = serde_json::from_str(text).expect("text is JSON");
    assert!(inner["display_id"].is_string());

    let display_id = inner["display_id"].as_str().unwrap().to_string();
    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn create_and_destroy_harness_display() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"]
        .as_str()
        .expect("display_id")
        .to_string();
    let display = create["result"]["display"]
        .as_str()
        .expect("display")
        .to_string();
    assert!(
        display_id.starts_with("d-") && display_id.len() == 34,
        "display_id should be d-<uuid>: {display_id}"
    );
    assert!(display.starts_with(":"));

    let info = conn
        .call_tool("lxh_display_info", json!({"display_id": display_id}))
        .await;
    assert!(
        info["result"]["display"].is_string(),
        "unexpected info response: {}",
        info
    );

    let destroy = conn
        .call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
    assert!(destroy["result"]["success"].as_bool().unwrap());
}

#[tokio::test]
async fn launch_app_and_take_screenshot() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();
    let display = create["result"]["display"].as_str().unwrap().to_string();
    assert!(
        display_id.starts_with("d-") && display_id.len() == 34,
        "display_id should be d-<uuid>: {display_id}"
    );
    assert!(display.starts_with(":"));

    let launch = conn
        .call_tool(
            "lxh_app_launch",
            json!({"display_id": display_id, "command": "xterm", "args": ["-e", "sleep", "60"]}),
        )
        .await;
    assert!(
        launch["result"]["pid"].as_u64().unwrap() > 0,
        "unexpected launch response: {}",
        launch
    );

    tokio::time::sleep(Duration::from_millis(500)).await;

    let shot = conn
        .call_tool("lxh_capture_screenshot", json!({"display_id": display_id}))
        .await;
    assert!(!shot["result"]["data"].as_str().unwrap().is_empty());

    let _ = conn
        .call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn app_terminate_kills_process() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    let launch = conn
        .call_tool(
            "lxh_app_launch",
            json!({"display_id": display_id, "command": "sleep", "args": ["60"]}),
        )
        .await;
    let pid = launch["result"]["pid"].as_u64().unwrap() as u32;
    assert!(pid > 0);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(tokio::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .await
        .unwrap()
        .success());

    let term = conn
        .call_tool(
            "lxh_app_terminate",
            json!({"display_id": display_id, "pid": pid}),
        )
        .await;
    assert!(term["result"]["success"].as_bool().unwrap());

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!tokio::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .await
        .unwrap()
        .success());

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn input_move_and_get_cursor_position() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    let move_resp = conn
        .call_tool(
            "lxh_input_move",
            json!({"display_id": display_id, "x": 123, "y": 456}),
        )
        .await;
    assert!(move_resp["result"]["success"].as_bool().unwrap());

    tokio::time::sleep(Duration::from_millis(50)).await;

    let pos = conn
        .call_tool(
            "lxh_input_get_cursor_position",
            json!({"display_id": display_id}),
        )
        .await;
    assert_eq!(pos["result"]["x"].as_i64(), Some(123));
    assert_eq!(pos["result"]["y"].as_i64(), Some(456));

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn clipboard_roundtrip() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    let set = conn
        .call_tool(
            "lxh_clipboard_set",
            json!({"display_id": display_id, "text": "hello harness"}),
        )
        .await;
    assert!(set["result"]["success"].as_bool().unwrap());

    let get = conn
        .call_tool("lxh_clipboard_get", json!({"display_id": display_id}))
        .await;
    assert_eq!(get["result"]["text"].as_str(), Some("hello harness"));

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn invalid_display_id_returns_error() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let resp = conn
        .call_tool(
            "lxh_capture_screenshot",
            json!({"display_id": "d-doesnotexist"}),
        )
        .await;
    assert!(resp["error"].is_object(), "expected error response: {resp}");
    assert!(resp["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not found"));
}

#[tokio::test]
async fn invalid_tool_arguments_returns_error() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    let resp = conn
        .call_tool("lxh_app_launch", json!({"display_id": display_id}))
        .await;
    assert!(resp["error"].is_object(), "expected error response: {resp}");

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn display_info_returns_resolution() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    let info = conn
        .call_tool("lxh_display_info", json!({"display_id": display_id}))
        .await;
    assert!(info["result"]["width"].as_u64().unwrap() > 0);
    assert!(info["result"]["height"].as_u64().unwrap() > 0);

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn desktop_overview_lists_launched_app() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    let launch = conn
        .call_tool(
            "lxh_app_launch",
            json!({"display_id": display_id, "command": "xterm", "args": ["-e", "sleep", "60"]}),
        )
        .await;
    let pid = launch["result"]["pid"].as_u64().unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;

    let overview = conn
        .call_tool(
            "lxh_get_desktop_overview",
            json!({"display_id": display_id}),
        )
        .await;
    let processes = overview["result"]["processes"].as_array().unwrap();
    assert!(
        processes.iter().any(|p| p["pid"].as_u64() == Some(pid)),
        "launched process not in overview: {overview}"
    );

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

use x11rb::connection::Connection as X11Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, MapState, QueryTreeReply, Window};
use x11rb::rust_connection::RustConnection;

fn find_xterm_window(display: &str) -> Option<u64> {
    find_window_by_title(display, "lxh-window-test")
}

fn find_window_by_title(display: &str, title: &str) -> Option<u64> {
    let (conn, screen_num) = RustConnection::connect(Some(display)).ok()?;
    let root = conn.setup().roots[screen_num].root;
    find_window_recursive(&conn, root, title)
}

fn find_window_recursive(conn: &RustConnection, window: Window, title: &str) -> Option<u64> {
    let reply: QueryTreeReply = conn.query_tree(window).ok()?.reply().ok()?;
    for &child in &reply.children {
        let attrs = match conn.get_window_attributes(child) {
            Ok(c) => c.reply().ok()?,
            Err(_) => continue,
        };
        if attrs.map_state != MapState::VIEWABLE {
            continue;
        }
        let name =
            match conn.get_property(false, child, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024) {
                Ok(c) => c.reply().ok()?,
                Err(_) => continue,
            };
        let s = String::from_utf8_lossy(&name.value);
        if s.contains(title) {
            return Some(child as u64);
        }
        if let Some(id) = find_window_recursive(conn, child, title) {
            return Some(id);
        }
    }
    None
}

fn window_size(display: &str, window_id: u64) -> Option<(u64, u64)> {
    let (conn, _) = RustConnection::connect(Some(display)).ok()?;
    let geom = conn.get_geometry(window_id as u32).ok()?.reply().ok()?;
    Some((geom.width as u64, geom.height as u64))
}

#[tokio::test]
async fn window_lifecycle_focus_set_frame_close() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();
    let display = create["result"]["display"].as_str().unwrap().to_string();

    let _ = conn
        .call_tool(
            "lxh_app_launch",
            json!({
                "display_id": display_id,
                "command": "xterm",
                "args": ["-title", "lxh-window-test", "-e", "sleep", "60"]
            }),
        )
        .await;

    let mut window_id = None;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        window_id = find_xterm_window(&display);
        if window_id.is_some() {
            break;
        }
    }
    let window_id = window_id.expect("xterm window not found");

    let focus = conn
        .call_tool(
            "lxh_window_focus",
            json!({"display_id": display_id, "window_id": window_id}),
        )
        .await;
    assert!(focus["result"]["success"].as_bool().unwrap());

    let set_frame = conn
        .call_tool(
            "lxh_window_set_frame",
            json!({
                "display_id": display_id,
                "window_id": window_id,
                "x": 10,
                "y": 20,
                "width": 500,
                "height": 400
            }),
        )
        .await;
    assert!(set_frame["result"]["success"].as_bool().unwrap());

    tokio::time::sleep(Duration::from_millis(300)).await;

    let (w, h) = tokio::task::spawn_blocking({
        let display = display.clone();
        move || window_size(&display, window_id).expect("window size")
    })
    .await
    .unwrap();
    assert!(w > 400, "width not resized: {w}");
    assert!(h > 350, "height not resized: {h}");

    let capture = conn
        .call_tool(
            "lxh_capture_window",
            json!({"display_id": display_id, "window_id": window_id}),
        )
        .await;
    assert!(!capture["result"]["data"].as_str().unwrap().is_empty());

    let close = conn
        .call_tool(
            "lxh_window_close",
            json!({"display_id": display_id, "window_id": window_id}),
        )
        .await;
    assert!(close["result"]["success"].as_bool().unwrap());

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        find_xterm_window(&display).is_none(),
        "window should be closed"
    );

    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

#[tokio::test]
async fn persistent_display_survives_client_disconnect() {
    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn
        .call_tool(
            "lxh_display_create",
            json!({"persistent": true, "preview": false}),
        )
        .await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();

    // Drop the first connection.
    drop(conn);

    // Reconnect to the same daemon and verify the display is still there.
    let mut conn2 = daemon.connect().await;
    let info = conn2
        .call_tool("lxh_display_info", json!({"display_id": display_id}))
        .await;
    assert!(info["result"]["display"].is_string());

    conn2
        .call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}

fn python_path() -> PathBuf {
    env::var("PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("python3"))
}

fn atspi_test_app_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("atspi_test_app.py");
    path
}

#[tokio::test]
async fn atspi_interaction_with_gtk_app() {
    let python = python_path();
    if !tokio::process::Command::new(&python)
        .args([
            "-c",
            "import gi; gi.require_version('Gtk', '3.0'); from gi.repository import Gtk",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("skipping atspi test: python gi/Gtk3 not available");
        return;
    }

    let daemon = DaemonGuard::new().await;
    let mut conn = daemon.connect().await;
    let create = conn.create_display().await;
    let display_id = create["result"]["display_id"].as_str().unwrap().to_string();
    let display = create["result"]["display"].as_str().unwrap().to_string();

    let app_path = atspi_test_app_path();
    let launch = conn
        .call_tool(
            "lxh_app_launch",
            json!({
                "display_id": display_id,
                "command": python.to_string_lossy(),
                "args": [app_path.to_string_lossy()]
            }),
        )
        .await;
    let pid = launch["result"]["pid"].as_u64().unwrap();

    tokio::time::sleep(Duration::from_millis(1500)).await;

    // Find the window via x11rb (desktop overview title may be null).
    let mut window_id = None;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        window_id = find_window_by_title(&display, "lxh-atspi-test");
        if window_id.is_some() {
            break;
        }
    }
    let window_id = window_id.expect("gtk window not found");

    // Read window state including accessibility tree.
    let state = conn
        .call_tool(
            "lxh_get_window_state",
            json!({
                "display_id": display_id,
                "pid": pid,
                "window_id": window_id,
                "include_tree": true
            }),
        )
        .await;
    let tree = state["result"]["tree"]["elements"].as_array().unwrap();
    assert!(!tree.is_empty(), "accessibility tree should not be empty");

    // Find the entry element and set its value.
    let entry_index = tree
        .iter()
        .position(|e| {
            e["role"].as_str() == Some("text")
                || e["name"]
                    .as_str()
                    .map(|n| n.contains("test-entry"))
                    .unwrap_or(false)
        })
        .expect("entry element not found in tree");
    let set = conn
        .call_tool(
            "lxh_set_value",
            json!({
                "display_id": display_id,
                "pid": pid,
                "index": entry_index,
                "value": "hello-atspi"
            }),
        )
        .await;
    assert!(set["result"]["success"].as_bool().unwrap());

    // Find the button element and click it.
    let button_index = tree
        .iter()
        .position(|e| {
            e["role"].as_str() == Some("push button")
                || e["name"]
                    .as_str()
                    .map(|n| n.contains("Click me"))
                    .unwrap_or(false)
        })
        .expect("button element not found in tree");
    let click = conn
        .call_tool(
            "lxh_click_element",
            json!({
                "display_id": display_id,
                "pid": pid,
                "index": button_index
            }),
        )
        .await;
    assert!(click["result"]["success"].as_bool().unwrap());

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify the label changed to "clicked".
    let state2 = conn
        .call_tool(
            "lxh_get_window_state",
            json!({
                "display_id": display_id,
                "pid": pid,
                "window_id": window_id,
                "include_tree": true
            }),
        )
        .await;
    let tree2 = state2["result"]["tree"]["elements"].as_array().unwrap();
    let label_changed = tree2.iter().any(|e| {
        e["name"]
            .as_str()
            .map(|n| n.contains("clicked"))
            .unwrap_or(false)
            || e["value"]
                .as_str()
                .map(|v| v.contains("clicked"))
                .unwrap_or(false)
    });
    assert!(label_changed, "button click did not update label: {state2}");

    conn.call_tool(
        "lxh_app_terminate",
        json!({"display_id": display_id, "pid": pid}),
    )
    .await;
    conn.call_tool("lxh_display_destroy", json!({"display_id": display_id}))
        .await;
}
