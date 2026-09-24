use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum LxhError {
    #[error("process spawn failed: {0}")]
    ProcessSpawnFailed(String),
    #[error("process kill failed: {0}")]
    ProcessKillFailed(String),
    #[error("display not found: {0}")]
    DisplayNotFound(String),
    #[error("display unavailable: {0}")]
    DisplayUnavailable(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("not supported")]
    NotSupported,
}

pub mod x11 {
    use super::LxhError;
    use x11rb::connection::Connection;
    use x11rb::rust_connection::{ConnectError, RustConnection};

    pub fn xerr<E: std::fmt::Display>(e: E) -> LxhError {
        LxhError::DisplayUnavailable(e.to_string())
    }

    pub fn open_connection(display: &str) -> Result<(RustConnection, usize), LxhError> {
        RustConnection::connect(Some(display)).map_err(|e: ConnectError| {
            LxhError::DisplayUnavailable(format!("cannot open display: {e}"))
        })
    }

    pub fn root_window(conn: &RustConnection, screen: usize) -> u32 {
        conn.setup().roots[screen].root
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn open_bad_display_returns_display_unavailable() {
            let result = open_connection(":99999");
            assert!(
                matches!(result, Err(LxhError::DisplayUnavailable(_))),
                "expected DisplayUnavailable, got {result:?}"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

pub struct Screenshot {
    pub data: Vec<u8>,
}

/// A cropped, scaled window capture with the mapping back to display
/// coordinates: the output's top-left sits at (`display_x`, `display_y`)
/// on the display, and one output pixel spans `scale` window pixels.
pub struct RegionCapture {
    pub screenshot: Screenshot,
    pub display_x: i32,
    pub display_y: i32,
    pub scale: f64,
}

pub struct ProcessEntry {
    pub pid: u32,
    pub name: String,
}

pub struct WindowEntry {
    pub id: u32,
    pub pid: Option<u32>,
    pub title: Option<String>,
    pub bounds: Option<Bounds>,
    /// Higher is closer to the front; `query_tree` returns bottom-to-top,
    /// so the position in the root's child list is the z index.
    pub z_index: usize,
    /// Whether the window is currently viewable (mapped and on screen).
    pub on_screen: bool,
}

pub struct DesktopOverview {
    pub processes: Vec<ProcessEntry>,
    pub windows: Vec<WindowEntry>,
}

pub struct GetWindowStateResult {
    pub window_id: u32,
    pub title: Option<String>,
    pub app_name: Option<String>,
    pub bounds: Bounds,
    pub tree: Option<AccessibilityTree>,
    pub screenshot: Option<Screenshot>,
}

pub struct DisplayInfo {
    pub display: String,
    pub width: u32,
    pub height: u32,
    pub app_count: usize,
}

pub struct A11yElement {
    pub index: usize,
    /// True when this element is part of the actionable set.
    pub actionable: bool,
    pub role: String,
    pub name: Option<String>,
    /// Text-interface content for entries and text views.
    pub value: Option<String>,
    /// Toggle state when the role exposes one.
    pub checked: Option<bool>,
    pub enabled: Option<bool>,
    /// Selection state for selectable controls.
    pub selected: Option<bool>,
    pub description: Option<String>,
    pub frame: Option<Bounds>,
    pub actions: Vec<String>,
    pub parent_index: Option<usize>,
    pub depth: usize,
}

pub struct AccessibilityTree {
    pub elements: Vec<A11yElement>,
}

#[derive(Clone, Debug)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[async_trait]
pub trait InputDriver: Send + Sync {
    async fn click(&self, x: i32, y: i32, button: MouseButton, count: u32) -> Result<(), LxhError>;
    async fn move_mouse(&self, x: i32, y: i32) -> Result<(), LxhError>;
    async fn scroll(&self, dx: i32, dy: i32) -> Result<(), LxhError>;
    async fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), LxhError>;
    /// `pid` (when known) enables the AT-SPI write fallback: XTEST
    /// delivers to the focused widget; when nothing editable holds focus
    /// the text is written through the pid's tree instead.
    async fn type_text(&self, pid: Option<u32>, text: &str) -> Result<(), LxhError>;
    async fn key(&self, key: &str, modifiers: &[&str]) -> Result<(), LxhError>;
    async fn get_cursor_position(&self) -> Result<(i32, i32), LxhError>;
}

#[async_trait]
pub trait CaptureDriver: Send + Sync {
    async fn screenshot(&self) -> Result<Screenshot, LxhError>;
    async fn screenshot_window(&self, window_id: u32) -> Result<Screenshot, LxhError>;
    /// Capture the window under the cursor (or the full display). Returns
    /// (window_id, screenshot); window_id is 0 for full display.
    async fn capture_at_cursor(&self) -> Result<(u32, Screenshot), LxhError>;
    /// Capture a window region (window coordinates, padded by 20% and
    /// clamped to the window), scaled so the output is at most 500 px wide.
    async fn capture_region(
        &self,
        window_id: u32,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> Result<RegionCapture, LxhError>;
}

#[async_trait]
pub trait WindowDriver: Send + Sync {
    async fn focus_window(&self, window_id: u32) -> Result<(), LxhError>;
    async fn set_window_frame(
        &self,
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), LxhError>;
    async fn close_window(&self, window_id: u32) -> Result<(), LxhError>;
}

#[async_trait]
pub trait ClipboardDriver: Send + Sync {
    async fn clipboard_get(&self) -> Result<String, LxhError>;
    async fn clipboard_set(&self, text: &str) -> Result<(), LxhError>;
}

/// One state assertion from `verify_state`.
#[derive(Debug, Clone)]
pub struct StateExpectation {
    /// Window-level predicate for the process's window.
    pub window: Option<WindowExpectation>,
    /// Element-level predicate against the AT-SPI tree.
    pub element: Option<ElementExpectation>,
}

#[derive(Debug, Clone, Default)]
pub struct WindowExpectation {
    /// The window must exist.
    pub exists: bool,
    /// The window title must contain this string.
    pub title_contains: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ElementExpectation {
    /// The element's AT-SPI role must equal this string.
    pub role: Option<String>,
    /// Some element's name must contain this string.
    pub label_contains: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VerificationResult {
    /// "satisfied" when every predicate holds, "unsatisfied" otherwise.
    pub status: String,
    /// Per-predicate outcome with a short observed-state description.
    pub results: Vec<(bool, String)>,
}

#[async_trait]
pub trait A11yDriver: Send + Sync {
    async fn get_window_state(
        &self,
        pid: u32,
        window_id: u32,
        include_tree: bool,
        include_screenshot: bool,
    ) -> Result<GetWindowStateResult, LxhError>;
    async fn get_desktop_overview(&self) -> Result<DesktopOverview, LxhError>;
    async fn set_value(&self, pid: u32, index: usize, value: &str) -> Result<(), LxhError>;
    async fn element_frame(&self, pid: u32, index: usize) -> Result<Bounds, LxhError>;
    /// Resolve an application menu path (e.g. ["File", "Open"]) through
    /// AT-SPI and invoke the final item.
    async fn invoke_menu(&self, pid: u32, path: &[String]) -> Result<(), LxhError>;
    /// Assert read-only state predicates for a process's window and its
    /// accessibility tree. Single sample; use lxh_wait between calls.
    async fn verify_state(
        &self,
        pid: u32,
        expect: &[StateExpectation],
    ) -> Result<VerificationResult, LxhError>;
}

#[async_trait]
pub trait Driver:
    InputDriver + CaptureDriver + WindowDriver + ClipboardDriver + A11yDriver
{
    async fn click_element(
        &self,
        pid: u32,
        index: usize,
        button: MouseButton,
    ) -> Result<(), LxhError>;
}
