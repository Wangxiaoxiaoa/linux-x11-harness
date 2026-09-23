use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::tools::{
    parse_args, tool_definitions as tools_tool_definitions, AppLaunchArgs, AppTerminateArgs,
    ClickArgs, ClickElementArgs, ClipboardSetArgs, DesktopOverviewArgs, DisplayCreateArgs,
    DisplayIdArgs, DragArgs, GetWindowStateArgs, InvokeMenuArgs, KeyArgs, MoveArgs, ScrollArgs,
    SetValueArgs, SetWindowFrameArgs, TypeArgs, VerifyStateArgs, WaitArgs, WindowIdArgs, ZoomArgs,
};
use lxh_core::{
    Driver, ElementExpectation, LxhError, MouseButton, StateExpectation, WindowExpectation,
};
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
    /// Last zoom context per display, for `from_zoom` coordinate
    /// translation in the coordinate-taking input tools.
    pub zooms: Arc<std::sync::Mutex<HashMap<String, ZoomContext>>>,
}

/// Mapping from zoom-image coordinates back to display coordinates: the
/// zoom output's top-left sits at (`display_x`, `display_y`) and one zoom
/// pixel spans `scale` display pixels.
#[derive(Clone, Copy)]
pub struct ZoomContext {
    pub display_x: i32,
    pub display_y: i32,
    pub scale: f64,
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
    let (x, y) = resolve_coord(
        state,
        &args.display_id,
        args.x,
        args.y,
        args.from_zoom.unwrap_or(false),
    )?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .click(
            x as i32,
            y as i32,
            parse_button(args.button)?,
            args.count.unwrap_or(1) as u32,
        )
        .await?;
    Ok(json!({ "success": true }))
}

pub async fn move_mouse(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: MoveArgs = parse_args(args)?;
    let (x, y) = resolve_coord(
        state,
        &args.display_id,
        args.x,
        args.y,
        args.from_zoom.unwrap_or(false),
    )?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.move_mouse(x as i32, y as i32).await?;
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
    let from_zoom = args.from_zoom.unwrap_or(false);
    let (x1, y1) = resolve_coord(state, &args.display_id, args.x1, args.y1, from_zoom)?;
    let (x2, y2) = resolve_coord(state, &args.display_id, args.x2, args.y2, from_zoom)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver
        .drag(x1 as i32, y1 as i32, x2 as i32, y2 as i32)
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

pub async fn zoom(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: ZoomArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    let capture = driver
        .capture_region(args.window_id as u32, args.x1, args.y1, args.x2, args.y2)
        .await?;

    let scale = capture.scale;
    state.zooms.lock().unwrap().insert(
        args.display_id.clone(),
        ZoomContext {
            display_x: capture.display_x,
            display_y: capture.display_y,
            scale,
        },
    );

    Ok(json!({
        "mimeType": "image/png",
        "data": encode_png(&capture.screenshot.data),
        "zoom": {
            "display_origin": { "x": capture.display_x, "y": capture.display_y },
            "scale": scale,
        }
    }))
}

/// Translate zoom-image coordinates to display coordinates.
fn resolve_coord(
    state: &DaemonState,
    display_id: &str,
    x: f64,
    y: f64,
    from_zoom: bool,
) -> Result<(i64, i64), LxhError> {
    if !from_zoom {
        return Ok((x as i64, y as i64));
    }
    let ctx = state
        .zooms
        .lock()
        .unwrap()
        .get(display_id)
        .copied()
        .ok_or_else(|| {
            LxhError::InvalidArgument(
                "from_zoom requires a lxh_zoom call on this display first".into(),
            )
        })?;
    let dx = ctx.display_x + (x / ctx.scale).round() as i32;
    let dy = ctx.display_y + (y / ctx.scale).round() as i32;
    Ok((dx as i64, dy as i64))
}

pub async fn invoke_menu(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: InvokeMenuArgs = parse_args(args)?;
    let driver = find_driver(state, &args.display_id).await?;
    driver.invoke_menu(args.pid, &args.path).await?;
    Ok(json!({ "success": true }))
}

pub async fn verify_state(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: VerifyStateArgs = parse_args(args)?;
    for (i, e) in args.expect.iter().enumerate() {
        if let Some(w) = &e.window {
            if w.exists == Some(false) {
                return Err(LxhError::InvalidArgument(format!(
                    "expect[{i}].window.exists=false cannot be verified (absence is not \
                     provable); only true is accepted"
                )));
            }
        }
    }
    if args.expect.is_empty() || args.expect.len() > 8 {
        return Err(LxhError::InvalidArgument(
            "expect requires 1 to 8 predicates".into(),
        ));
    }

    let driver = find_driver(state, &args.display_id).await?;
    let expect: Vec<StateExpectation> = args
        .expect
        .iter()
        .map(|e| StateExpectation {
            window: e.window.as_ref().map(|w| WindowExpectation {
                exists: w.exists.unwrap_or(true),
                title_contains: w.title_contains.clone(),
            }),
            element: e.element.as_ref().map(|el| ElementExpectation {
                role: el.role.clone(),
                label_contains: el.label_contains.clone(),
            }),
        })
        .collect();

    let result = driver.verify_state(args.pid, &expect).await?;
    let results: Vec<Value> = result
        .results
        .iter()
        .map(|(satisfied, observed)| json!({ "satisfied": satisfied, "observed": observed }))
        .collect();
    Ok(json!({ "status": result.status, "results": results }))
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
                if let Some(v) = &e.value {
                    node["value"] = json!(v);
                }
                if let Some(c) = e.checked {
                    node["checked"] = json!(c);
                }
                if let Some(en) = e.enabled {
                    node["enabled"] = json!(en);
                }
                if let Some(se) = e.selected {
                    node["selected"] = json!(se);
                }
                if let Some(d) = &e.description {
                    node["description"] = json!(d);
                }
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

pub fn list_apps() -> Value {
    let apps: Vec<Value> = crate::apps::list_apps()
        .iter()
        .map(|a| {
            let mut node = json!({
                "name": a.name,
                "running": a.running,
            });
            if let Some(pid) = a.pid {
                node["pid"] = json!(pid);
            }
            if let Some(p) = &a.launch_path {
                node["launch_path"] = json!(p);
            }
            if let Some(b) = &a.bundle_id {
                node["bundle_id"] = json!(b);
            }
            node
        })
        .collect();
    json!({ "apps": apps, "count": apps.len() })
}

pub async fn get_desktop_overview(state: &DaemonState, args: &Value) -> Result<Value, LxhError> {
    let args: DesktopOverviewArgs = parse_args(args)?;
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
        .filter(|w| args.pid.is_none_or(|pid| w.pid == Some(pid)))
        .filter(|w| !args.on_screen_only.unwrap_or(false) || w.on_screen)
        .map(|w| {
            let mut node = json!({
                "window_id": w.id,
                "pid": w.pid,
                "title": w.title,
                "z_index": w.z_index,
                "on_screen": w.on_screen,
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
