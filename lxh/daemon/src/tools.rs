use lxh_core::LxhError;
use schemars::JsonSchema;
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
pub struct DesktopOverviewArgs {
    pub display_id: String,
    /// Only return windows of this process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Only return windows that are mapped on screen. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_screen_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAppsArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InvokeMenuArgs {
    pub display_id: String,
    pub pid: u32,
    /// Menu path from the application menu bar, e.g. ["File", "Open"].
    /// Segments are matched case-sensitively after trimming whitespace.
    pub path: Vec<String>,
}

/// One state assertion. At least one of window/element should be present.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StateExpectationArgs {
    /// Window predicate for the pid's managed window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowExpectationArgs>,
    /// Element predicate against the AT-SPI tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementExpectationArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WindowExpectationArgs {
    /// The window must exist. Note: absence cannot be proven reliably;
    /// only true is accepted.
    #[serde(default)]
    pub exists: Option<bool>,
    /// The window title must contain this string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_contains: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ElementExpectationArgs {
    /// The element's AT-SPI role must equal this string (e.g. "button").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Some element's name must contain this string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_contains: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VerifyStateArgs {
    pub display_id: String,
    pub pid: u32,
    /// One to eight predicates, combined with logical AND.
    pub expect: Vec<StateExpectationArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ZoomArgs {
    pub display_id: String,
    pub window_id: u64,
    /// Region in window coordinates (screenshot pixels).
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
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
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub button: Option<ButtonArg>,
    #[serde(default)]
    pub count: Option<u64>,
    /// Interpret x/y as coordinates in the last lxh_zoom image instead of
    /// display coordinates. Requires a lxh_zoom call on this display first.
    #[serde(default)]
    pub from_zoom: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoveArgs {
    pub display_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub from_zoom: Option<bool>,
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
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    #[serde(default)]
    pub from_zoom: Option<bool>,
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

/// JSON Schema for a tool's arguments, tuned for MCP clients:
/// subschemas are inlined (no `$ref`/`definitions`) and `Option<T>`
/// does not become a `null` union, because several clients (e.g.
/// gemini-cli-based agents) reject or skip schemas they cannot flatten.
fn root_schema<T: JsonSchema>() -> Value {
    let settings = schemars::gen::SchemaSettings::draft07().with(|settings| {
        settings.inline_subschemas = true;
        settings.option_add_null_type = false;
        settings.option_nullable = false;
    });
    let mut schema =
        serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).unwrap();
    // schemars emits e.g. "format": "uint64"; some MCP clients reject unknown
    // formats, and no tool relies on format semantics.
    strip_format(&mut schema);
    schema
}

fn strip_format(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("format");
            for child in map.values_mut() {
                strip_format(child);
            }
        }
        Value::Array(items) => {
            for child in items {
                strip_format(child);
            }
        }
        _ => {}
    }
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
        "inputSchema": schema,
        "annotations": annotations_for(name),
    })
}

/// MCP behavior hints per tool. Every tool operates on harness displays the
/// daemon owns, so the world stays closed.
fn annotations_for(name: &str) -> Value {
    let read_only = matches!(
        name,
        "lxh_display_info"
            | "lxh_list_apps"
            | "lxh_get_desktop_overview"
            | "lxh_get_window_state"
            | "lxh_capture_screenshot"
            | "lxh_capture_window"
            | "lxh_zoom"
            | "lxh_clipboard_get"
            | "lxh_input_get_cursor_position"
            | "lxh_wait"
            | "lxh_verify_state"
    );
    let destructive = matches!(
        name,
        "lxh_display_destroy" | "lxh_app_terminate" | "lxh_window_close"
    );
    let idempotent = matches!(
        name,
        "lxh_display_info"
            | "lxh_list_apps"
            | "lxh_get_desktop_overview"
            | "lxh_get_window_state"
            | "lxh_capture_screenshot"
            | "lxh_capture_window"
            | "lxh_zoom"
            | "lxh_clipboard_get"
            | "lxh_clipboard_set"
            | "lxh_input_get_cursor_position"
            | "lxh_input_move"
            | "lxh_wait"
            | "lxh_verify_state"
            | "lxh_set_value"
            | "lxh_window_focus"
            | "lxh_window_set_frame"
            | "lxh_preview_open"
            | "lxh_preview_close"
            | "lxh_display_attach"
            | "lxh_display_detach"
    );
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": false,
    })
}

pub fn tool_definitions() -> Vec<Value> {
    vec![
        tool_def(
            "lxh_display_create",
            "Create a new X11 display. Set persistent to keep it after disconnect.",
            root_schema::<DisplayCreateArgs>(),
        ),
        tool_def(
            "lxh_display_destroy",
            "Destroy an X11 display",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_display_attach",
            "Attach to an existing X11 display (e.g. :0)",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_display_detach",
            "Detach from an existing X11 display without destroying it",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_app_launch",
            "Launch an application on a display",
            root_schema::<AppLaunchArgs>(),
        ),
        tool_def(
            "lxh_app_terminate",
            "Terminate an application by PID",
            root_schema::<AppTerminateArgs>(),
        ),
        tool_def(
            "lxh_get_window_state",
            "Get window state: metadata, optional AT-SPI tree, optional screenshot",
            root_schema::<GetWindowStateArgs>(),
        ),
        tool_def(
            "lxh_input_click",
            "Click at screen coordinates. Supports left/right/middle buttons and multiple clicks.",
            root_schema::<ClickArgs>(),
        ),
        tool_def(
            "lxh_input_move",
            "Move the mouse cursor",
            root_schema::<MoveArgs>(),
        ),
        tool_def("lxh_input_type", "Type text", root_schema::<TypeArgs>()),
        tool_def(
            "lxh_input_key",
            "Press a key or key combination",
            root_schema::<KeyArgs>(),
        ),
        tool_def(
            "lxh_input_scroll",
            "Scroll by a delta",
            root_schema::<ScrollArgs>(),
        ),
        tool_def(
            "lxh_input_drag",
            "Drag from (x1, y1) to (x2, y2)",
            root_schema::<DragArgs>(),
        ),
        tool_def(
            "lxh_input_get_cursor_position",
            "Get the current mouse cursor position",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_capture_screenshot",
            "Take a screenshot",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_zoom",
            "Capture a cropped image of a region (x1,y1)-(x2,y2) in display coordinates (the \
             same space as a11y frames and input coordinates), padded by 20% and scaled to \
             at most 500 px wide. The result includes display_origin and scale; pass \
             from_zoom=true to lxh_input_click / lxh_input_move / lxh_input_drag to \
             translate coordinates from the zoom image back to display space.",
            root_schema::<ZoomArgs>(),
        ),
        tool_def(
            "lxh_capture_window",
            "Take a screenshot of a specific window",
            root_schema::<WindowIdArgs>(),
        ),
        tool_def(
            "lxh_list_apps",
            "List applications: running processes merged with installed XDG desktop \
             entries. Each entry includes name, running (with pid when live), launch_path \
             (pass to lxh_app_launch) and bundle_id. Use it to answer \"is X installed?\" \
             and \"is X running?\".",
            root_schema::<ListAppsArgs>(),
        ),
        tool_def(
            "lxh_invoke_menu",
            "Resolve an application menu path (e.g. [\"File\", \"Open\"]) through AT-SPI \
             and invoke the final item. Path segments are matched case-sensitively \
             against direct children of the menu bar; fails with the exact segment \
             when not found.",
            root_schema::<InvokeMenuArgs>(),
        ),
        tool_def(
            "lxh_verify_state",
            "Assert read-only state predicates for a process: window (exists, \
             title_contains) and AT-SPI element (role, label_contains), combined with \
             logical AND. Returns per-predicate outcomes with the observed state. \
             Single sample; call lxh_wait between attempts if the UI is still settling.",
            root_schema::<VerifyStateArgs>(),
        ),
        tool_def(
            "lxh_get_desktop_overview",
            "Return desktop overview: running processes and windows. Stale windows from \
             exited processes are never returned. Each window record includes window_id, pid, \
             title, bounds, z_index (higher = closer to front) and on_screen.",
            root_schema::<DesktopOverviewArgs>(),
        ),
        tool_def(
            "lxh_set_value",
            "Set the value of an AT-SPI editable element",
            root_schema::<SetValueArgs>(),
        ),
        tool_def(
            "lxh_click_element",
            "Click an AT-SPI element by pid and index",
            root_schema::<ClickElementArgs>(),
        ),
        tool_def(
            "lxh_wait",
            "Wait for a number of milliseconds",
            root_schema::<WaitArgs>(),
        ),
        tool_def(
            "lxh_window_focus",
            "Focus a window by id",
            root_schema::<WindowIdArgs>(),
        ),
        tool_def(
            "lxh_window_set_frame",
            "Set a window's position and size",
            root_schema::<SetWindowFrameArgs>(),
        ),
        tool_def(
            "lxh_window_close",
            "Close a window by id",
            root_schema::<WindowIdArgs>(),
        ),
        tool_def(
            "lxh_clipboard_get",
            "Get text from the clipboard",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_clipboard_set",
            "Set text on the clipboard",
            root_schema::<ClipboardSetArgs>(),
        ),
        tool_def(
            "lxh_display_info",
            "Get display metadata",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_preview_open",
            "Open a preview window for a display if not already open",
            root_schema::<DisplayIdArgs>(),
        ),
        tool_def(
            "lxh_preview_close",
            "Close the preview window for a display",
            root_schema::<DisplayIdArgs>(),
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
        assert_eq!(defs.len(), 32, "expected 32 tool definitions");

        let names: Vec<&str> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"lxh_display_create"));
        assert!(names.contains(&"lxh_list_apps"));
        assert!(names.contains(&"lxh_zoom"));
        assert!(names.contains(&"lxh_invoke_menu"));
        assert!(names.contains(&"lxh_verify_state"));
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
        assert_eq!(parsed.x, 10.0);
        assert_eq!(parsed.y, 20.0);
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

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn schemas_are_client_friendly() {
        for def in tool_definitions() {
            let schema = &def["inputSchema"];
            let raw = serde_json::to_string(schema).unwrap();
            assert!(!raw.contains("\"$ref\""), "{} has $ref: {raw}", def["name"]);
            assert!(
                !raw.contains("definitions"),
                "{} has definitions",
                def["name"]
            );
            assert!(!raw.contains("format"), "{} has format: {raw}", def["name"]);
            let Some(props) = schema["properties"].as_object() else {
                // Parameter-less tools (e.g. lxh_list_apps) have no properties.
                continue;
            };
            for (name, prop) in props {
                assert!(
                    prop.get("type").is_some(),
                    "{}.{} has no type: {prop}",
                    def["name"],
                    name
                );
            }
        }
    }
}
