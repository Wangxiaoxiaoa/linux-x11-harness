use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::tools::{
    parse_args, tool_definitions as tools_tool_definitions, AppLaunchArgs, AppTerminateArgs,
    ClickArgs, ClickElementArgs, ClipboardSetArgs, DisplayCreateArgs, DisplayIdArgs, DragArgs,
    GetWindowStateArgs, KeyArgs, MoveArgs, ScrollArgs, SetValueArgs, SetWindowFrameArgs, TypeArgs,
    WaitArgs, WindowIdArgs,
};
use lxh_core::{Driver, LxhError, MouseButton};
use lxh_driver::DefaultDriver;
use lxh_preview::PreviewPanel;
use lxh_runtime::{Display, DisplayConfig, Runtime};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};

pub struct DaemonState {
    pub runtime: Arc<Runtime>,
    pub displays: Arc<RwLock<HashMap<String, Arc<Mutex<Display>>>>>,
    pub drivers: Arc<RwLock<HashMap<String, Arc<dyn Driver>>>>,
    pub previews: Arc<PreviewPanel>,
}

pub struct ClientSession {
    pub owned_displays: HashSet<String>,
    pub client_name: Option<String>,
}

impl Default for ClientSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientSession {
    pub fn new() -> Self {
        Self {
            owned_displays: HashSet::new(),
            client_name: None,
        }
    }
}

fn parse_button(b: Option<crate::tools::ButtonArg>) -> Result<MouseButton, LxhError> {
    match b.unwrap_or(crate::tools::ButtonArg::Left) {
        crate::tools::ButtonArg::Left => Ok(MouseButton::Left),
        crate::tools::ButtonArg::Right => Ok(MouseButton::Right),
        crate::tools::ButtonArg::Middle => Ok(MouseButton::Middle),
    }
}

pub async fn create_display(
    state: &DaemonState,
    session: &mut ClientSession,
    args: &Value,
) -> Result<Value, LxhError> {
    let args: DisplayCreateArgs = parse_args(args)?;
    let config = DisplayConfig::default();
    let preview = args.preview.unwrap_or(true);

    let display = state.runtime.create_display(config).await?;
    let id = display.id().to_string();
    let display_str = display.display().to_string();
    let driver = Arc::new(DefaultDriver::new(&display_str)?);

    if preview {
        let title = preview_title(session, &args.name);
        if let Err(e) = state.previews.open(&id, &display_str, &title) {
            eprintln!("preview unavailable for {id}: {e}");
        }
    }

    state
        .displays
        .write()
        .await
        .insert(id.clone(), Arc::new(Mutex::new(display)));
    state.drivers.write().await.insert(id.clone(), driver);
    if !args.persistent {
        session.owned_displays.insert(id.clone());
    }

    Ok(json!({ "display_id": id, "display": display_str }))
}

pub async fn attach_display(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let display = state.runtime.attach_display(&args.display_id).await?;
    let id = display.id().to_string();
    let display_str = display.display().to_string();
    let driver = Arc::new(DefaultDriver::new(&display_str)?);

    state
        .displays
        .write()
        .await
        .insert(id.clone(), Arc::new(Mutex::new(display)));
    state.drivers.write().await.insert(id.clone(), driver);

    Ok(json!({ "display_id": id, "display": display_str }))
}

pub async fn destroy_display(
    state: &DaemonState,
    session: &mut ClientSession,
    args: &Value,
) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let display = find_display(state, &args.display_id).await?;
    state.previews.close(&args.display_id);
    display.lock().await.destroy().await?;

    state.displays.write().await.remove(&args.display_id);
    state.drivers.write().await.remove(&args.display_id);
    session.owned_displays.remove(&args.display_id);

    Ok(json!({ "success": true }))
}

pub async fn preview_open(
    state: &DaemonState,
    session: &ClientSession,
    args: &Value,
) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let display = find_display(state, &args.display_id).await?;
    let display_str = display.lock().await.display().to_string();
    let title = preview_title(session, &None);
    state
        .previews
        .open(&args.display_id, &display_str, &title)?;
    Ok(json!({ "success": true }))
}

pub async fn preview_close(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    state.previews.close(&args.display_id);
    Ok(json!({ "success": true }))
}

fn preview_title(session: &ClientSession, name: &Option<String>) -> String {
    let who = name
        .clone()
        .or_else(|| session.client_name.clone())
        .unwrap_or_else(|| "Agent".to_string());
    format!("LXH {who}")
}

pub async fn detach_display(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let display = find_display(state, &args.display_id).await?;
    if !display.lock().await.is_external() {
        return Err(LxhError::InvalidArgument(
            "cannot detach a harness display; use lxh_display_destroy".into(),
        ));
    }

    state.displays.write().await.remove(&args.display_id);
    state.drivers.write().await.remove(&args.display_id);

    Ok(json!({ "success": true }))
}

pub async fn app_launch(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: AppLaunchArgs = parse_args(args)?;
    let arg_refs: Vec<&str> = args.args.iter().map(|s| s.as_str()).collect();
    let display = find_display(state, &args.display_id).await?;
    let pid = display
        .lock()
        .await
        .launch_app(&args.command, &arg_refs)
        .await?;
    Ok(json!({ "pid": pid }))
}

pub async fn app_terminate(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: AppTerminateArgs = parse_args(args)?;
    let display = find_display(state, &args.display_id).await?;
    display.lock().await.terminate_app(args.pid as u32).await?;
    Ok(json!({ "success": true }))
}

pub async fn click(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: ClickArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .click(
            args.x as i32,
            args.y as i32,
            parse_button(args.button)?,
            args.count.unwrap_or(1) as u32,
        )
        .await?;
    Ok(json!({ "success": true }))
}

pub async fn move_mouse(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: MoveArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.move_mouse(args.x as i32, args.y as i32).await?;
    Ok(json!({ "success": true }))
}

pub async fn input_type(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: TypeArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.type_text(&args.text).await?;
    Ok(json!({ "success": true }))
}

pub async fn input_key(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: KeyArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let modifiers: Vec<&str> = args.modifiers.iter().map(|s| s.as_str()).collect();
    driver.key(&args.key, &modifiers).await?;
    Ok(json!({ "success": true }))
}

pub async fn scroll(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: ScrollArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.scroll(args.dx as i32, args.dy as i32).await?;
    Ok(json!({ "success": true }))
}

pub async fn drag(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DragArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .drag(
            args.x1 as i32,
            args.y1 as i32,
            args.x2 as i32,
            args.y2 as i32,
        )
        .await?;
    Ok(json!({ "success": true }))
}

pub async fn get_cursor_position(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let (x, y) = driver.get_cursor_position().await?;
    Ok(json!({ "x": x, "y": y }))
}

pub async fn screenshot(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let shot = driver.screenshot().await?;
    Ok(json!({ "mimeType": "image/png", "data": encode_png(&shot.data) }))
}

pub async fn screenshot_window(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: WindowIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let shot = driver.screenshot_window(args.window_id as u32).await?;
    Ok(json!({ "mimeType": "image/png", "data": encode_png(&shot.data) }))
}

pub async fn window_focus(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: WindowIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.focus_window(args.window_id as u32).await?;
    Ok(json!({ "success": true }))
}

pub async fn window_set_frame(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: SetWindowFrameArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .set_window_frame(
            args.window_id as u32,
            args.x as i32,
            args.y as i32,
            args.width as u32,
            args.height as u32,
        )
        .await?;
    Ok(json!({ "success": true }))
}

pub async fn window_close(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: WindowIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.close_window(args.window_id as u32).await?;
    Ok(json!({ "success": true }))
}

pub async fn click_element(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: ClickElementArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .click_element(
            args.pid as u32,
            args.index as usize,
            parse_button(args.button)?,
        )
        .await?;
    Ok(json!({ "success": true }))
}

pub async fn wait(_state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: WaitArgs = parse_args(args)?;
    tokio::time::sleep(std::time::Duration::from_millis(args.ms)).await;
    Ok(json!({ "success": true }))
}

pub async fn clipboard_get(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let text = driver.clipboard_get().await?;
    Ok(json!({ "text": text }))
}

pub async fn clipboard_set(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: ClipboardSetArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.clipboard_set(&args.text).await?;
    Ok(json!({ "success": true }))
}

pub async fn display_info(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let display = find_display(state, &args.display_id).await?;
    let info = display.lock().await.info();
    Ok(json!({
        "display": info.display,
        "width": info.width,
        "height": info.height,
        "app_count": info.app_count,
    }))
}

pub async fn get_window_state(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: GetWindowStateArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let state = driver
        .get_window_state(
            args.pid as u32,
            args.window_id as u32,
            args.include_tree,
            args.include_screenshot,
        )
        .await?;

    let mut result = json!({
        "window_id": state.window_id,
        "title": state.title,
        "app_name": state.app_name,
        "bounds": {
            "x": state.bounds.x,
            "y": state.bounds.y,
            "w": state.bounds.w,
            "h": state.bounds.h,
        },
    });

    if let Some(tree) = state.tree {
        let elements: Vec<Value> = tree
            .elements
            .iter()
            .map(|e| {
                let mut node = json!({
                    "index": e.index,
                    "role": e.role,
                    "name": e.name,
                    "actions": e.actions,
                    "depth": e.depth,
                });
                if let Some(p) = e.parent_index {
                    node["parent_index"] = json!(p);
                }
                if let Some(b) = &e.frame {
                    node["frame"] = json!({ "x": b.x, "y": b.y, "w": b.w, "h": b.h });
                }
                node
            })
            .collect();
        result["tree"] = json!({ "elements": elements });
    }

    if let Some(screenshot) = state.screenshot {
        result["screenshot"] = json!(encode_png(&screenshot.data));
    }

    Ok(result)
}

pub async fn get_desktop_overview(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DisplayIdArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let overview = driver.get_desktop_overview().await?;

    let processes: Vec<Value> = overview
        .processes
        .iter()
        .map(|p| json!({ "pid": p.pid, "name": p.name }))
        .collect();
    let windows: Vec<Value> = overview
        .windows
        .iter()
        .map(|w| {
            let mut node = json!({
                "window_id": w.id,
                "pid": w.pid,
                "title": w.title,
            });
            if let Some(b) = &w.bounds {
                node["bounds"] = json!({ "x": b.x, "y": b.y, "w": b.w, "h": b.h });
            }
            node
        })
        .collect();

    Ok(json!({ "processes": processes, "windows": windows }))
}

pub async fn set_value(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: SetValueArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .set_value(args.pid as u32, args.index as usize, &args.value)
        .await?;
    Ok(json!({ "success": true }))
}

pub fn tool_definitions() -> Vec<Value> {
    tools_tool_definitions()
}

async fn find_display(state: &DaemonState, id: &str) -> Result<Arc<Mutex<Display>>, LxhError> {
    state
        .displays
        .read()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| LxhError::DisplayNotFound(id.into()))
}

async fn find_driver(state: &DaemonState, id: &str) -> Result<Arc<dyn Driver>, LxhError> {
    state
        .drivers
        .read()
        .await
        .get(id)
        .cloned()
        .ok_or_else(|| LxhError::DisplayNotFound(id.into()))
}

fn encode_png(data: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data)
}
