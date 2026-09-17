use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::rust_connection::RustConnection;

use crate::window::{PreviewConfig, PreviewWindow};

/// Preview window size relative to the user's screen: 1/8 for the normal
/// state, 1/3 when zoomed.
const NORMAL_FRACTION: u32 = 8;
const ZOOM_FRACTION: u32 = 3;

const MARGIN: i32 = 24;
const GAP: i32 = 12;

/// Owns all preview windows for a daemon. Callers only open, close and query
/// previews by display id; layout and X11 details stay inside.
#[derive(Default)]
pub struct PreviewManager {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    windows: HashMap<String, Entry>,
    used_slots: HashSet<u32>,
}

struct Entry {
    slot: u32,
    window: PreviewWindow,
}

impl PreviewManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a preview for `display_id`, or do nothing if one is already open.
    pub fn open(
        &self,
        display_id: &str,
        target_display: &str,
        title: &str,
    ) -> Result<(), LxhError> {
        let mut inner = self.inner.lock().unwrap();
        inner.prune_dead();

        if inner.windows.contains_key(display_id) {
            return Ok(());
        }

        let (screen_w, screen_h) = root_geometry(&user_display())?;
        let (target_w, target_h) = root_geometry(target_display)?;
        let (width, height) = fit_inside(
            target_w,
            target_h,
            screen_w / NORMAL_FRACTION,
            screen_h / NORMAL_FRACTION,
        );
        let (zoom_width, zoom_height) = fit_inside(
            target_w,
            target_h,
            screen_w / ZOOM_FRACTION,
            screen_h / ZOOM_FRACTION,
        );

        let slot = inner.alloc_slot();
        let x = MARGIN;
        let y = MARGIN + slot as i32 * (height as i32 + GAP);
        let config = PreviewConfig {
            x,
            y,
            width,
            height,
            zoom_x: MARGIN,
            zoom_y: MARGIN,
            zoom_width,
            zoom_height,
        };

        let window =
            PreviewWindow::start(target_display, &format!("{title} {}", slot + 1), config)?;
        inner
            .windows
            .insert(display_id.to_string(), Entry { slot, window });
        Ok(())
    }

    /// Close the preview for `display_id`, if open.
    pub fn close(&self, display_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.remove(display_id);
    }
}

impl Inner {
    fn alloc_slot(&mut self) -> u32 {
        let mut slot = 0;
        while self.used_slots.contains(&slot) {
            slot += 1;
        }
        self.used_slots.insert(slot);
        slot
    }

    fn remove(&mut self, display_id: &str) -> Option<Entry> {
        let entry = self.windows.remove(display_id)?;
        self.used_slots.remove(&entry.slot);
        Some(entry)
    }

    /// Drop entries whose window was closed by the user.
    fn prune_dead(&mut self) {
        let dead: Vec<String> = self
            .windows
            .iter()
            .filter(|(_, entry)| !entry.window.is_alive())
            .map(|(id, _)| id.clone())
            .collect();
        for id in dead {
            self.remove(&id);
        }
    }
}

fn user_display() -> String {
    std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string())
}

fn root_geometry(display: &str) -> Result<(u32, u32), LxhError> {
    let (conn, screen) = RustConnection::connect(Some(display))
        .map_err(|e| LxhError::DisplayUnavailable(format!("cannot connect to {display}: {e}")))?;
    let root = conn.setup().roots[screen].root;
    let geom = conn
        .get_geometry(root)
        .map_err(|e| LxhError::DisplayUnavailable(e.to_string()))?
        .reply()
        .map_err(|e| LxhError::DisplayUnavailable(e.to_string()))?;
    Ok((geom.width as u32, geom.height as u32))
}

/// Fit `src` inside `max` while preserving aspect ratio. Never upscales.
fn fit_inside(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f64 / src_w as f64)
        .min(max_h as f64 / src_h as f64)
        .min(1.0);
    (
        ((src_w as f64 * scale) as u32).max(1),
        ((src_h as f64 * scale) as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_inside_preserves_aspect_and_caps() {
        // 1280x800 into 2560/8 x 1600/8 = 320x200 keeps the 1.6 ratio.
        assert_eq!(fit_inside(1280, 800, 320, 200), (320, 200));
        // Limited by height.
        assert_eq!(fit_inside(1280, 800, 320, 100), (160, 100));
        // Never upscales beyond the source.
        assert_eq!(fit_inside(100, 50, 1000, 1000), (100, 50));
    }

    #[test]
    fn slots_are_reused_after_removal() {
        let mut inner = Inner::default();
        let a = inner.alloc_slot();
        let b = inner.alloc_slot();
        assert_eq!((a, b), (0, 1));
        inner.used_slots.remove(&a);
        assert_eq!(inner.alloc_slot(), 0);
    }
}
