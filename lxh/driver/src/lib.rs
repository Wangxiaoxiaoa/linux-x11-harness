use async_trait::async_trait;
use lxh_action::{ArboardClipboard, X11WindowManager, XtestInput};
use lxh_core::{
    A11yDriver, Bounds, CaptureDriver, ClipboardDriver, DesktopOverview, Driver,
    GetWindowStateResult, InputDriver, LxhError, MouseButton, RegionCapture, Screenshot,
    StateExpectation, VerificationResult, WindowDriver,
};
use lxh_state::{AtspiA11y, X11Capture};

pub struct DefaultDriver {
    input: XtestInput,
    capture: X11Capture,
    a11y: AtspiA11y,
    window: X11WindowManager,
    clipboard: ArboardClipboard,
}

impl DefaultDriver {
    pub fn new(display: &str) -> Result<Self, LxhError> {
        Ok(Self {
            input: XtestInput::new(display)?,
            capture: X11Capture::new(display)?,
            a11y: AtspiA11y::new(display)?,
            window: X11WindowManager::new(display)?,
            clipboard: ArboardClipboard::new()?,
        })
    }
}

#[async_trait]
impl InputDriver for DefaultDriver {
    async fn click(&self, x: i32, y: i32, button: MouseButton, count: u32) -> Result<(), LxhError> {
        self.input.click(x, y, button, count).await
    }

    async fn move_mouse(&self, x: i32, y: i32) -> Result<(), LxhError> {
        self.input.move_mouse(x, y).await
    }

    async fn scroll(&self, dx: i32, dy: i32) -> Result<(), LxhError> {
        self.input.scroll(dx, dy).await
    }

    async fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), LxhError> {
        self.input.drag(x1, y1, x2, y2).await
    }

    async fn type_text(&self, text: &str) -> Result<(), LxhError> {
        self.input.type_text(text).await
    }

    async fn key(&self, key: &str, modifiers: &[&str]) -> Result<(), LxhError> {
        self.input.key(key, modifiers).await
    }

    async fn get_cursor_position(&self) -> Result<(i32, i32), LxhError> {
        self.input.get_cursor_position().await
    }
}

#[async_trait]
impl CaptureDriver for DefaultDriver {
    async fn screenshot(&self) -> Result<Screenshot, LxhError> {
        self.capture.screenshot().await
    }

    async fn screenshot_window(&self, window_id: u32) -> Result<Screenshot, LxhError> {
        self.capture.screenshot_window(window_id).await
    }

    async fn capture_region(
        &self,
        window_id: u32,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> Result<RegionCapture, LxhError> {
        self.capture.capture_region(window_id, x1, y1, x2, y2).await
    }
}

#[async_trait]
impl WindowDriver for DefaultDriver {
    async fn focus_window(&self, window_id: u32) -> Result<(), LxhError> {
        self.window.focus_window(window_id).await
    }

    async fn set_window_frame(
        &self,
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), LxhError> {
        self.window
            .set_window_frame(window_id, x, y, width, height)
            .await
    }

    async fn close_window(&self, window_id: u32) -> Result<(), LxhError> {
        self.window.close_window(window_id).await
    }
}

#[async_trait]
impl ClipboardDriver for DefaultDriver {
    async fn clipboard_get(&self) -> Result<String, LxhError> {
        self.clipboard.clipboard_get().await
    }

    async fn clipboard_set(&self, text: &str) -> Result<(), LxhError> {
        self.clipboard.clipboard_set(text).await
    }
}

#[async_trait]
impl A11yDriver for DefaultDriver {
    async fn get_window_state(
        &self,
        pid: u32,
        window_id: u32,
        include_tree: bool,
        include_screenshot: bool,
    ) -> Result<GetWindowStateResult, LxhError> {
        self.a11y
            .get_window_state(pid, window_id, include_tree, include_screenshot)
            .await
    }

    async fn get_desktop_overview(&self) -> Result<DesktopOverview, LxhError> {
        self.a11y.get_desktop_overview().await
    }

    async fn set_value(&self, pid: u32, index: usize, value: &str) -> Result<(), LxhError> {
        self.a11y.set_value(pid, index, value).await
    }

    async fn element_frame(&self, pid: u32, index: usize) -> Result<Bounds, LxhError> {
        self.a11y.element_frame(pid, index).await
    }

    async fn invoke_menu(&self, pid: u32, path: &[String]) -> Result<(), LxhError> {
        self.a11y.invoke_menu(pid, path).await
    }

    async fn verify_state(
        &self,
        pid: u32,
        expect: &[StateExpectation],
    ) -> Result<VerificationResult, LxhError> {
        self.a11y.verify_state(pid, expect).await
    }
}

#[async_trait]
impl Driver for DefaultDriver {
    async fn click_element(
        &self,
        pid: u32,
        index: usize,
        button: MouseButton,
    ) -> Result<(), LxhError> {
        let Bounds { x, y, w, h } = self.a11y.element_frame(pid, index).await?;
        let cx = x + w as i32 / 2;
        let cy = y + h as i32 / 2;
        self.input.click(cx, cy, button, 1).await
    }
}
