//! Read-only preview windows for harness displays.
//!
//! A harness display is a headless Xvfb server. `PreviewManager` shows its
//! contents in a small window on the user's desktop. The preview is fully
//! decoupled from the display lifecycle: closing a preview window only stops
//! that viewer, never the display.

mod manager;
mod window;

pub use manager::PreviewManager;
