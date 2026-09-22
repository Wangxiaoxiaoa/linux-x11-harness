//! Read-only preview panel for harness displays.
//!
//! A harness display is a headless Xvfb server. [`PreviewPanel`] shows the
//! contents of one or more displays inside a single override-redirect
//! container window on the user's desktop.
//!
//! The module is split by responsibility:
//!
//! * [`geometry`] — screen-adaptive sizes (pure math)
//! * [`layout`] — grid and pagination layout (pure math)
//! * [`cell`] — one preview cell (render loop, hover title bar, close)
//! * [`container`] — the container window frame (pager bar)
//! * [`panel`] — orchestration: open/close, events, re-layout
//! * [`x11`] — small X11 helpers shared by the above
//!
//! The preview is fully decoupled from the display lifecycle: closing a
//! preview cell only stops that viewer, never the display.

mod cell;
mod container;
mod geometry;
mod layout;
mod panel;
mod x11;

pub use panel::PreviewPanel;
