use lxh_core::LxhError;
use schemars::{schema_for, JsonSchema};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ButtonArg {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DisplayCreateArgs {
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub preview: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DisplayIdArgs {
    pub display_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AppLaunchArgs {
    pub display_id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AppTerminateArgs {
    pub display_id: String,
    pub pid: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetWindowStateArgs {
    pub display_id: String,
    pub pid: u64,
    pub window_id: u64,
    #[serde(default)]
    pub include_tree: bool,
    #[serde(default)]
    pub include_screenshot: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickArgs {
    pub display_id: String,
    pub x: i64,
    pub y: i64,
    #[serde(default)]
    pub button: Option<ButtonArg>,
    #[serde(default)]
    pub count: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoveArgs {
    pub display_id: String,
    pub x: i64,
    pub y: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeArgs {
    pub display_id: String,
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KeyArgs {
    pub display_id: String,
    pub key: String,
    #[serde(default)]
    pub modifiers: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScrollArgs {
    pub display_id: String,
    #[serde(default)]
    pub dx: i64,
    #[serde(default)]
    pub dy: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DragArgs {
    pub display_id: String,
    pub x1: i64,
    pub y1: i64,
    pub x2: i64,
    pub y2: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WindowIdArgs {
    pub display_id: String,
    pub window_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetWindowFrameArgs {
    pub display_id: String,
    pub window_id: u64,
    pub x: i64,
    pub y: i64,
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetValueArgs {
    pub display_id: String,
    pub pid: u64,
    pub index: u64,
    pub value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickElementArgs {
    pub display_id: String,
    pub pid: u64,
    pub index: u64,
    #[serde(default)]
    pub button: Option<ButtonArg>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitArgs {
    pub ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClipboardSetArgs {
    pub display_id: String,
    pub text: String,
}

fn tool_def(name: &str, description: &str, schema: Value) -> Value {
    let mut schema = schema;
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
        obj.remove("title");
    }
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema
    })
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool_def(
            "lxh_display_create",
            "Create a new X11 display. Set persistent to keep it after disconnect.",
            serde_json::to_value(schema_for!(DisplayCreateArgs)).unwrap(),
        ),
        tool_def(
            "lxh_display_destroy",
            "Destroy an X11 display",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_display_attach",
            "Attach to an existing X11 display (e.g. :0)",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_display_detach",
            "Detach from an existing X11 display without destroying it",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_app_launch",
            "Launch an application on a display",
            serde_json::to_value(schema_for!(AppLaunchArgs)).unwrap(),
        ),
        tool_def(
            "lxh_app_terminate",
            "Terminate an application by PID",
            serde_json::to_value(schema_for!(AppTerminateArgs)).unwrap(),
        ),
        tool_def(
            "lxh_get_window_state",
            "Get window state: metadata, optional AT-SPI tree, optional screenshot",
            serde_json::to_value(schema_for!(GetWindowStateArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_click",
            "Click at screen coordinates. Supports left/right/middle buttons and multiple clicks.",
            serde_json::to_value(schema_for!(ClickArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_move",
            "Move the mouse cursor",
            serde_json::to_value(schema_for!(MoveArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_type",
            "Type text",
            serde_json::to_value(schema_for!(TypeArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_key",
            "Press a key or key combination",
            serde_json::to_value(schema_for!(KeyArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_scroll",
            "Scroll by a delta",
            serde_json::to_value(schema_for!(ScrollArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_drag",
            "Drag from (x1, y1) to (x2, y2)",
            serde_json::to_value(schema_for!(DragArgs)).unwrap(),
        ),
        tool_def(
            "lxh_input_get_cursor_position",
            "Get the current mouse cursor position",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_capture_screenshot",
            "Take a screenshot",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_capture_window",
            "Take a screenshot of a specific window",
            serde_json::to_value(schema_for!(WindowIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_get_desktop_overview",
            "Return desktop overview: running processes and visible windows",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_set_value",
            "Set the value of an AT-SPI editable element",
            serde_json::to_value(schema_for!(SetValueArgs)).unwrap(),
        ),
        tool_def(
            "lxh_click_element",
            "Click an AT-SPI element by pid and index",
            serde_json::to_value(schema_for!(ClickElementArgs)).unwrap(),
        ),
        tool_def(
            "lxh_wait",
            "Wait for a number of milliseconds",
            serde_json::to_value(schema_for!(WaitArgs)).unwrap(),
        ),
        tool_def(
            "lxh_window_focus",
            "Focus a window by id",
            serde_json::to_value(schema_for!(WindowIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_window_set_frame",
            "Set a window's position and size",
            serde_json::to_value(schema_for!(SetWindowFrameArgs)).unwrap(),
        ),
        tool_def(
            "lxh_window_close",
            "Close a window by id",
            serde_json::to_value(schema_for!(WindowIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_clipboard_get",
            "Get text from the clipboard",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_clipboard_set",
            "Set text on the clipboard",
            serde_json::to_value(schema_for!(ClipboardSetArgs)).unwrap(),
        ),
        tool_def(
            "lxh_display_info",
            "Get display metadata",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_preview_open",
            "Open a preview window for a display if not already open",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
        tool_def(
            "lxh_preview_close",
            "Close the preview window for a display",
            serde_json::to_value(schema_for!(DisplayIdArgs)).unwrap(),
        ),
    ]
}

pub fn parse_args<T: for<'de> Deserialize<'de>>(args: &Value) -> Result<T, LxhError> {
    serde_json::from_value(args.clone())
        .map_err(|e| LxhError::InvalidArgument(format!("invalid arguments: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_has_expected_tools() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 28, "expected 28 tool definitions");

        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"lxh_display_create"));
        assert!(names.contains(&"lxh_input_click"));
        assert!(names.contains(&"lxh_capture_screenshot"));
        assert!(names.contains(&"lxh_clipboard_get"));
        assert!(names.contains(&"lxh_window_set_frame"));
        assert!(names.contains(&"lxh_preview_open"));
        assert!(names.contains(&"lxh_preview_close"));
    }

    #[test]
    fn tool_definitions_have_schema() {
        for def in tool_definitions() {
            assert!(def["name"].is_string(), "tool name missing: {def}");
            assert!(
                def["description"].is_string(),
                "tool description missing: {def}"
            );
            assert!(
                def["inputSchema"].is_object(),
                "tool inputSchema missing: {def}"
            );
        }
    }

    #[test]
    fn parse_display_create_args() {
        let args = json!({"persistent": true});
        let parsed = parse_args::<DisplayCreateArgs>(&args).unwrap();
        assert!(parsed.persistent);

        let default = parse_args::<DisplayCreateArgs>(&json!({})).unwrap();
        assert!(!default.persistent);
    }

    #[test]
    fn parse_click_args_with_button() {
        let args = json!({"display_id": "d-1", "x": 10, "y": 20, "button": "right", "count": 2});
        let parsed = parse_args::<ClickArgs>(&args).unwrap();
        assert_eq!(parsed.x, 10);
        assert_eq!(parsed.y, 20);
        assert!(matches!(parsed.button, Some(ButtonArg::Right)));
        assert_eq!(parsed.count, Some(2));
    }

    #[test]
    fn parse_missing_required_field_is_rejected() {
        let args = json!({"display_id": "d-1"}); // missing command
        let err = parse_args::<AppLaunchArgs>(&args).unwrap_err();
        assert!(matches!(err, LxhError::InvalidArgument(_)));
    }
}
