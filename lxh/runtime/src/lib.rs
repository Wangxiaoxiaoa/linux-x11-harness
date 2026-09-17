use std::sync::atomic::{AtomicU32, Ordering};

use lxh_core::LxhError;
use uuid::Uuid;

pub mod display;
pub mod process;
pub mod wm;
pub mod xserver;

pub use display::{Display, DisplayConfig, DisplayKind};

fn process_scoped_display_start() -> u32 {
    // Use the process id so that concurrent daemon instances do not collide
    // on the low display numbers starting at :99.
    let pid = std::process::id();
    100 + (pid % 90_000)
}

pub struct Runtime {
    next_display: AtomicU32,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            next_display: AtomicU32::new(process_scoped_display_start()),
        }
    }

    pub async fn create_display(&self, config: DisplayConfig) -> Result<Display, LxhError> {
        let num = self.next_display.fetch_add(1, Ordering::Relaxed);
        let id = format!("d-{}", Uuid::new_v4().simple());
        let display = format!(":{}", num);
        Display::create(id, display, config).await
    }

    pub async fn attach_display(&self, display: &str) -> Result<Display, LxhError> {
        let id = display.to_string();
        Display::attach(id, display.to_string()).await
    }
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
