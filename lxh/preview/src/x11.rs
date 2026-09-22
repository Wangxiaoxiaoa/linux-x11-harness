//! Small X11 helpers shared by the panel, container and cells.

use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;

/// Connect to the user's desktop display (`$DISPLAY`).
pub(crate) fn connect_user() -> Result<(RustConnection, usize), LxhError> {
    let user_display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
    RustConnection::connect(Some(&user_display))
        .map_err(|e| LxhError::DisplayUnavailable(format!("cannot connect to {user_display}: {e}")))
}

/// Resolution of a display's root window.
pub(crate) fn root_size(conn: &RustConnection, screen_idx: usize) -> (u32, u32) {
    let screen = &conn.setup().roots[screen_idx];
    (
        screen.width_in_pixels as u32,
        screen.height_in_pixels as u32,
    )
}

pub(crate) fn set_title(conn: &RustConnection, win: Window, title: &str) -> Result<(), LxhError> {
    WrapperExt::change_property8(
        conn,
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .map_err(disconnected)?
    .check()
    .map_err(disconnected)
}

pub(crate) fn set_opacity(
    conn: &RustConnection,
    win: Window,
    opacity: u32,
) -> Result<(), LxhError> {
    let atom = conn
        .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")
        .map_err(disconnected)?
        .reply()
        .map_err(disconnected)?
        .atom;
    conn.change_property32(PropMode::REPLACE, win, atom, AtomEnum::CARDINAL, &[opacity])
        .map_err(disconnected)?
        .check()
        .map_err(disconnected)
}

pub(crate) fn disconnected<E: std::fmt::Display>(e: E) -> LxhError {
    LxhError::DisplayUnavailable(e.to_string())
}
