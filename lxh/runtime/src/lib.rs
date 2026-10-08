#![forbid(unsafe_code)]
use std::sync::Arc;

use std::sync::atomic::{AtomicU32, Ordering};

use lxh_core::LxhError;
use uuid::Uuid;

pub mod clipboard;
pub mod display;
pub mod process;
pub mod wm;
pub mod xserver;

pub use display::{Display, DisplayConfig, DisplayKind};

fn process_scoped_display_start() -> u32 {
    // Use the process id so that concurrent daemon instances do not collide
    // on the low display numbers starting at :99. Two constraints: display
    // numbers must stay below 65536, and x11rb computes the TCP port as
    // 6000 + display in u16, so keep 6000 + display < 65536
    // (i.e. display <= 59535, hence the 59_400 range from base 100).
    let pid = std::process::id();
    100 + (pid % 59_400)
}

pub struct Runtime {
    next_display: AtomicU32,
    clipboard: Arc<crate::clipboard::ClipboardHub>,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            next_display: AtomicU32::new(process_scoped_display_start()),
            clipboard: Arc::new(crate::clipboard::ClipboardHub::new()),
        }
    }

    /// The shared clipboard hub (one per process).
    pub fn clipboard(&self) -> &Arc<crate::clipboard::ClipboardHub> {
        &self.clipboard
    }

    /// Start the desktop spoke (the user's `$DISPLAY`); called lazily so
    /// headless hosts skip it.
    fn ensure_desktop_spoke(&self) {
        let desktop = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        self.clipboard.start_desktop_spoke(&desktop);
    }

    pub async fn create_display(&self, config: DisplayConfig) -> Result<Display, LxhError> {
        let num = self.next_display.fetch_add(1, Ordering::Relaxed);
        let id = format!("d-{}", Uuid::new_v4().simple());
        let display = format!(":{}", num);
        let display = Display::create(id, display, config).await?;
        // SDK-created displays join the shared clipboard by default.
        self.ensure_desktop_spoke();
        self.clipboard
            .start_spoke(display.display(), crate::clipboard::Policy::ToSandbox);
        Ok(display)
    }

    pub async fn attach_display(&self, display: &str) -> Result<Display, LxhError> {
        self.ensure_desktop_spoke();
        let id = display.to_string();
        Display::attach(id, display.to_string()).await
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use lxh_core::{CaptureDriver, InputDriver, MouseButton};

    #[tokio::test]
    async fn sdk_driver_access() {
        let runtime = Runtime::new();
        let mut display = runtime
            .create_display(DisplayConfig {
                width: 400,
                height: 300,
                depth: 24,
            })
            .await
            .expect("create display");

        let driver = display.driver();
        driver.move_mouse(200, 150).await.expect("move");
        let (x, y) = driver.get_cursor_position().await.expect("cursor");
        assert_eq!((x, y), (200, 150));
        let shot = driver.screenshot().await.expect("screenshot");
        assert!(!shot.data.is_empty());
        driver
            .click(x, y, MouseButton::Left, 1)
            .await
            .expect("click");

        display.destroy().await.expect("destroy");
    }
}
