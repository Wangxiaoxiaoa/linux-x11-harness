use async_trait::async_trait;
use lxh_core::{x11, InputDriver, LxhError, MouseButton};
use std::{thread, time::Duration};
use tokio::task;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

fn keysym_for_char(ch: char) -> Option<u32> {
    x11_keysymdef::lookup_by_codepoint(ch).map(|r| r.keysym)
}

fn keysym_for_name(name: &str) -> Option<u32> {
    x11_keysymdef::lookup_by_name(name).map(|r| r.keysym)
}

fn keycode_for_keysym(
    mapping: &x11rb::protocol::xproto::GetKeyboardMappingReply,
    keysym: u32,
) -> Option<(u8, bool)> {
    let per = mapping.keysyms_per_keycode as usize;
    if per == 0 {
        return None;
    }
    for (i, syms) in mapping.keysyms.chunks(per).enumerate() {
        if syms.first() == Some(&keysym) {
            return Some(((8 + i) as u8, false));
        }
        if per > 1 && syms.get(1) == Some(&keysym) {
            return Some(((8 + i) as u8, true));
        }
    }
    None
}

/// Round-trip to the X server so every buffered request (key/button
/// transitions) is actually delivered before this short-lived connection
/// closes. X11 requests are client-buffered: without the round trip the
/// server may process a press and its release back to back, erasing any
/// intended hold interval, and a closing connection loses requests that
/// are still in flight.
fn deliver(conn: &RustConnection) -> Result<(), LxhError> {
    conn.flush().map_err(x11::xerr)?;
    conn.get_input_focus()
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?;
    Ok(())
}

fn get_mapping(
    conn: &RustConnection,
) -> Result<x11rb::protocol::xproto::GetKeyboardMappingReply, LxhError> {
    conn.get_keyboard_mapping(8, 248)
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)
}

pub struct XtestInput {
    display: String,
}

impl XtestInput {
    pub fn new(display: &str) -> Result<Self, LxhError> {
        let _ = x11::open_connection(display)?;
        Ok(Self {
            display: display.to_string(),
        })
    }
}

fn mouse_button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    }
}

fn has_net_wm_pid(conn: &RustConnection, window: u32) -> bool {
    let atom = match conn.intern_atom(false, b"_NET_WM_PID") {
        Ok(c) => match c.reply() {
            Ok(r) => r.atom,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    let cardinal = match conn.intern_atom(false, b"CARDINAL") {
        Ok(c) => match c.reply() {
            Ok(r) => r.atom,
            Err(_) => return false,
        },
        Err(_) => return false,
    };
    conn.get_property(false, window, atom, cardinal, 0, 1)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| r.format == 32 && r.value.len() >= 4)
        .unwrap_or(false)
}

fn client_window(conn: &RustConnection, window: u32) -> u32 {
    if has_net_wm_pid(conn, window) {
        return window;
    }
    if let Ok(tree) = conn.query_tree(window).map_err(x11::xerr) {
        if let Ok(reply) = tree.reply() {
            for &child in &reply.children {
                if has_net_wm_pid(conn, child) {
                    return child;
                }
            }
        }
    }
    window
}

#[async_trait]
impl InputDriver for XtestInput {
    async fn click(&self, x: i32, y: i32, button: MouseButton, count: u32) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = x11::root_window(&conn, screen);
            let x16 = x as i16;
            let y16 = y as i16;
            let btn = mouse_button_number(button);

            conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, x16, y16)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;

            for i in 0..count.max(1) {
                if i > 0 {
                    thread::sleep(Duration::from_millis(50));
                }
                conn.xtest_fake_input(BUTTON_PRESS_EVENT, btn, 0, root, x16, y16, 0)
                    .map_err(x11::xerr)?
                    .check()
                    .map_err(x11::xerr)?;
                conn.xtest_fake_input(BUTTON_RELEASE_EVENT, btn, 0, root, x16, y16, 0)
                    .map_err(x11::xerr)?
                    .check()
                    .map_err(x11::xerr)?;
            }

            deliver(&conn)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn move_mouse(&self, x: i32, y: i32) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = x11::root_window(&conn, screen);
            conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, x as i16, y as i16)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;
            deliver(&conn)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn scroll(&self, dx: i32, dy: i32) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = x11::root_window(&conn, screen);

            let click = |button: u8| -> Result<(), LxhError> {
                conn.xtest_fake_input(BUTTON_PRESS_EVENT, button, 0, root, 0, 0, 0)
                    .map_err(x11::xerr)?
                    .check()
                    .map_err(x11::xerr)?;
                conn.xtest_fake_input(BUTTON_RELEASE_EVENT, button, 0, root, 0, 0, 0)
                    .map_err(x11::xerr)?
                    .check()
                    .map_err(x11::xerr)?;
                Ok(())
            };

            for _ in 0..dy.abs() {
                click(if dy > 0 { 5 } else { 4 })?;
            }
            for _ in 0..dx.abs() {
                click(if dx > 0 { 7 } else { 6 })?;
            }

            deliver(&conn)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = x11::root_window(&conn, screen);

            conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, x1 as i16, y1 as i16)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;
            conn.xtest_fake_input(BUTTON_PRESS_EVENT, 1, 0, root, x1 as i16, y1 as i16, 0)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;
            conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, 0, root, x2 as i16, y2 as i16, 0)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;
            conn.xtest_fake_input(BUTTON_RELEASE_EVENT, 1, 0, root, x2 as i16, y2 as i16, 0)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;

            deliver(&conn)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn type_text(&self, _pid: Option<u32>, text: &str) -> Result<(), LxhError> {
        let display = self.display.clone();
        let text = text.to_string();
        task::spawn_blocking(move || {
            let (conn, _) = x11::open_connection(&display)?;
            let mapping = get_mapping(&conn)?;

            for ch in text.chars() {
                let keysym = keysym_for_char(ch).ok_or_else(|| {
                    LxhError::InvalidArgument(format!("unknown character: {}", ch))
                })?;
                let (keycode, needs_shift) =
                    keycode_for_keysym(&mapping, keysym).ok_or_else(|| {
                        LxhError::InvalidArgument(format!("no keycode for keysym 0x{:x}", keysym))
                    })?;

                if needs_shift {
                    let (shift_kc, _) = keycode_for_keysym(&mapping, 0xffe1)
                        .ok_or_else(|| LxhError::InvalidArgument("no Shift keycode".into()))?;
                    press(&conn, shift_kc, true)?;
                    press(&conn, keycode, true)?;
                    press(&conn, keycode, false)?;
                    press(&conn, shift_kc, false)?;
                } else {
                    press(&conn, keycode, true)?;
                    press(&conn, keycode, false)?;
                }
            }

            deliver(&conn)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn key(&self, key: &str, modifiers: &[&str]) -> Result<(), LxhError> {
        let display = self.display.clone();
        let key = key.to_string();
        let modifiers: Vec<String> = modifiers.iter().map(|s| s.to_string()).collect();
        task::spawn_blocking(move || {
            let (conn, _) = x11::open_connection(&display)?;
            let mapping = get_mapping(&conn)?;

            let keysym = keysym_for_name(&key)
                .or_else(|| keysym_for_char(key.chars().next().unwrap_or('\0')))
                .ok_or_else(|| LxhError::InvalidArgument(format!("unknown key: {}", key)))?;
            let (keycode, _) = keycode_for_keysym(&mapping, keysym)
                .ok_or_else(|| LxhError::InvalidArgument(format!("no keycode for key: {}", key)))?;

            let mut codes = Vec::new();
            for m in &modifiers {
                let mk = keysym_for_name(m)
                    .ok_or_else(|| LxhError::InvalidArgument(format!("unknown modifier: {}", m)))?;
                let (mkc, _) = keycode_for_keysym(&mapping, mk).ok_or_else(|| {
                    LxhError::InvalidArgument(format!("no keycode for modifier: {}", m))
                })?;
                codes.push(mkc);
            }
            codes.push(keycode);

            for &code in &codes {
                press(&conn, code, true)?;
            }
            for &code in codes.iter().rev() {
                press(&conn, code, false)?;
            }

            deliver(&conn)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn get_cursor_position(&self) -> Result<(i32, i32), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = x11::root_window(&conn, screen);
            let reply = conn
                .query_pointer(root)
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?;
            Ok((reply.root_x as i32, reply.root_y as i32))
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }
}

fn press(conn: &RustConnection, keycode: u8, down: bool) -> Result<(), LxhError> {
    let event = if down {
        KEY_PRESS_EVENT
    } else {
        KEY_RELEASE_EVENT
    };
    conn.xtest_fake_input(event, keycode, 0, x11rb::NONE, 0, 0, 0)
        .map_err(x11::xerr)?
        .check()
        .map_err(x11::xerr)
}

pub struct X11WindowManager {
    display: String,
}

impl X11WindowManager {
    pub fn new(display: &str) -> Result<Self, LxhError> {
        let _ = x11::open_connection(display)?;
        Ok(Self {
            display: display.to_string(),
        })
    }
}

#[async_trait]
impl lxh_core::WindowDriver for X11WindowManager {
    async fn focus_window(&self, window_id: u32) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = x11::root_window(&conn, screen);
            let target = client_window(&conn, window_id);

            let net_active_window = conn
                .intern_atom(false, b"_NET_ACTIVE_WINDOW")
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?
                .atom;

            let event = x11rb::protocol::xproto::ClientMessageEvent::new(
                32,
                target,
                net_active_window,
                [2, x11rb::CURRENT_TIME, 0, 0, 0],
            );
            conn.send_event(
                false,
                root,
                x11rb::protocol::xproto::EventMask::SUBSTRUCTURE_REDIRECT,
                event,
            )
            .map_err(x11::xerr)?
            .check()
            .map_err(x11::xerr)?;
            conn.flush().map_err(x11::xerr)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn set_window_frame(
        &self,
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, _) = x11::open_connection(&display)?;
            let aux = x11rb::protocol::xproto::ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(width)
                .height(height);
            conn.configure_window(window_id, &aux)
                .map_err(x11::xerr)?
                .check()
                .map_err(x11::xerr)?;
            conn.flush().map_err(x11::xerr)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn close_window(&self, window_id: u32) -> Result<(), LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, _) = x11::open_connection(&display)?;

            let wm_protocols = conn
                .intern_atom(false, b"WM_PROTOCOLS")
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?
                .atom;
            let wm_delete_window = conn
                .intern_atom(false, b"WM_DELETE_WINDOW")
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?
                .atom;

            let target = client_window(&conn, window_id);
            let event = x11rb::protocol::xproto::ClientMessageEvent::new(
                32,
                target,
                wm_protocols,
                [wm_delete_window, x11rb::CURRENT_TIME, 0, 0, 0],
            );
            conn.send_event(
                false,
                target,
                x11rb::protocol::xproto::EventMask::NO_EVENT,
                event,
            )
            .map_err(x11::xerr)?
            .check()
            .map_err(x11::xerr)?;
            conn.flush().map_err(x11::xerr)?;
            Ok(())
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }
}

pub struct ArboardClipboard;

impl ArboardClipboard {
    pub fn new() -> Result<Self, LxhError> {
        let _ = arboard::Clipboard::new().map_err(|e| LxhError::InvalidArgument(e.to_string()))?;
        Ok(Self)
    }
}

#[async_trait]
impl lxh_core::ClipboardDriver for ArboardClipboard {
    async fn clipboard_get(&self) -> Result<String, LxhError> {
        task::spawn_blocking(|| {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| LxhError::InvalidArgument(e.to_string()))?;
            clipboard
                .get_text()
                .map_err(|e| LxhError::InvalidArgument(e.to_string()))
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn clipboard_set(&self, text: &str) -> Result<(), LxhError> {
        let text = text.to_string();
        task::spawn_blocking(move || {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| LxhError::InvalidArgument(e.to_string()))?;
            clipboard
                .set_text(text)
                .map_err(|e| LxhError::InvalidArgument(e.to_string()))
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysym_lookup() {
        assert!(keysym_for_name("Return").is_some());
        assert!(keysym_for_name("Shift_L").is_some());
        assert!(keysym_for_char('a').is_some());
    }

    #[test]
    fn keycode_for_keysym_finds_slot_zero() {
        let (conn, _) = x11::open_connection(":0").expect("need local display");
        let mapping = get_mapping(&conn).expect("keyboard mapping");
        let keysym = keysym_for_name("a").unwrap();
        let (code, shifted) = keycode_for_keysym(&mapping, keysym).unwrap();
        assert!(code > 0);
        assert!(!shifted);
    }

    #[test]
    fn keycode_for_keysym_finds_shifted_slot() {
        let (conn, _) = x11::open_connection(":0").expect("need local display");
        let mapping = get_mapping(&conn).expect("keyboard mapping");
        let keysym = keysym_for_name("A").unwrap();
        let (code, shifted) = keycode_for_keysym(&mapping, keysym).unwrap();
        assert!(code > 0);
        assert!(shifted);
    }
}
