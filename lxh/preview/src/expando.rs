//! The interactive expanded preview ("expando"): a WM-managed toplevel that
//! mirrors one harness display at ~50% screen height and forwards mouse and
//! keyboard input into it via XTEST.
//!
//! Opened by double-clicking a preview cell; closed with Esc, the WM close
//! button, or when the target display dies. At most one expando exists at a
//! time; the panel owns the handle and toggles it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use lxh_action::raw;
use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt, CreateWindowAux, EventMask, Gcontext, ImageFormat, ModMask, PropMode,
    Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;
use x11rb::COPY_DEPTH_FROM_PARENT;

use crate::cell::scale;
use crate::x11::{connect_user, disconnected, set_title};

/// Render interval for the expando (slightly slower than a cell; input
/// events force an immediate repaint anyway).
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
/// How often pending X events are checked.
const PUMP_INTERVAL: Duration = Duration::from_millis(10);
/// Fraction of the user's screen the expando occupies.
const SCREEN_FRACTION: f64 = 0.5;
/// Esc keysym.
const KEY_ESC: u32 = 0xff1b;
/// Shift left keysym.
const KEY_SHIFT_L: u32 = 0xffe1;

/// Events the expando reports to the panel.
#[derive(Debug)]
pub(crate) enum ExpandoEvent {
    /// The user closed the expando (Esc, WM close, or display gone).
    Closed,
}

/// Handle to the running expando, held by the panel.
pub(crate) struct ExpandoHandle {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
}

impl ExpandoHandle {
    /// Create the expando window and start its render/input loop. Setup
    /// errors surface before returning.
    pub(crate) fn start(
        target_display: &str,
        title: &str,
        events: mpsc::Sender<ExpandoEvent>,
    ) -> Result<Self, LxhError> {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let stop2 = Arc::clone(&stop);
        let alive2 = Arc::clone(&alive);
        let startup = ExpandoStartup {
            target_display: target_display.to_string(),
            title: title.to_string(),
            events: events.clone(),
        };
        let close_tx = events;

        thread::spawn(move || {
            let result = run(startup, &stop2, &alive2, ready_tx);
            if let Err(e) = result {
                eprintln!("preview expando error: {e}");
            }
            alive2.store(false, Ordering::Relaxed);
            // Always report closure so the panel drops its handle.
            let _ = close_tx.send(ExpandoEvent::Closed);
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop, alive }),
            Ok(Err(msg)) => Err(LxhError::DisplayUnavailable(msg)),
            Err(_) => Err(LxhError::DisplayUnavailable(
                "preview expando thread died during startup".into(),
            )),
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for ExpandoHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ExpandoStartup {
    target_display: String,
    title: String,
    events: mpsc::Sender<ExpandoEvent>,
}

struct Expando {
    user_conn: RustConnection,
    win: Window,
    gc: Gcontext,
    target_conn: RustConnection,
    target_root: Window,
    src_w: u32,
    src_h: u32,
    win_w: u32,
    win_h: u32,
    /// Keyboard mappings of the user display and the target display.
    user_mapping: x11rb::protocol::xproto::GetKeyboardMappingReply,
    target_mapping: x11rb::protocol::xproto::GetKeyboardMappingReply,
    /// Buttons currently held by the user (target button numbers).
    held_buttons: Vec<u8>,
    /// Per-press translation: user keycode -> (target keycode, shift injected).
    pressed_keys: HashMap<u8, (u8, bool)>,
    /// Last motion position in this pump cycle (only the last is forwarded).
    pending_motion: Option<(i16, i16)>,
    /// Set by any forwarded input; forces an immediate repaint.
    dirty: bool,
}

fn run(
    startup: ExpandoStartup,
    stop: &AtomicBool,
    alive: &AtomicBool,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), LxhError> {
    let mut expando = match Expando::new(&startup) {
        Ok(e) => e,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return Ok(());
        }
    };
    let _ = ready.send(Ok(()));

    let mut last_render = Instant::now();
    while !stop.load(Ordering::Relaxed) && alive.load(Ordering::Relaxed) {
        if !expando.pump_events(&startup.events) {
            break;
        }
        expando.flush_motion();
        if expando.dirty || last_render.elapsed() >= REFRESH_INTERVAL {
            if expando.render().is_err() {
                let _ = startup.events.send(ExpandoEvent::Closed);
                break;
            }
            expando.dirty = false;
            last_render = Instant::now();
        }
        thread::sleep(PUMP_INTERVAL);
    }

    expando.shutdown();
    Ok(())
}

/// Window geometry for the expando: 50% of the screen height, width by the
/// target's aspect ratio (capped at 50% screen width), docked top-left.
fn expando_geometry(screen_w: u32, screen_h: u32, src_w: u32, src_h: u32) -> (u32, u32, i16, i16) {
    let mut h = (screen_h as f64 * SCREEN_FRACTION) as u32;
    let mut w = if src_h > 0 {
        (h as f64 * src_w as f64 / src_h as f64) as u32
    } else {
        src_w
    };
    let max_w = (screen_w as f64 * SCREEN_FRACTION) as u32;
    if w > max_w {
        w = max_w;
        h = if src_w > 0 {
            (w as f64 * src_h as f64 / src_w as f64) as u32
        } else {
            src_h
        };
    }
    // Dock top-left; the preview panel docks top-right.
    let x = 8i16;
    let y = 8i16;
    (w.max(1), h.max(1), x, y)
}

impl Expando {
    fn new(startup: &ExpandoStartup) -> Result<Self, LxhError> {
        let (user_conn, user_screen) = connect_user()?;
        let screen = user_conn.setup().roots[user_screen].clone();

        let (target_conn, target_screen) = RustConnection::connect(Some(&startup.target_display))
            .map_err(|e| {
            LxhError::DisplayUnavailable(format!("cannot connect to target display: {e}"))
        })?;
        let target_root = target_conn.setup().roots[target_screen].root;
        let geom = target_conn
            .get_geometry(target_root)
            .map_err(|e| LxhError::DisplayUnavailable(e.to_string()))?
            .reply()
            .map_err(|e| LxhError::DisplayUnavailable(e.to_string()))?;
        let src_w = geom.width as u32;
        let src_h = geom.height as u32;

        let (win_w, win_h, x, y) = expando_geometry(
            screen.width_in_pixels as u32,
            screen.height_in_pixels as u32,
            src_w,
            src_h,
        );

        let win = user_conn.generate_id().map_err(disconnected)?;
        user_conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                win,
                screen.root,
                x,
                y,
                win_w as u16,
                win_h as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .event_mask(
                        EventMask::EXPOSURE
                            | EventMask::STRUCTURE_NOTIFY
                            | EventMask::BUTTON_PRESS
                            | EventMask::BUTTON_RELEASE
                            | EventMask::KEY_PRESS
                            | EventMask::KEY_RELEASE
                            | EventMask::POINTER_MOTION,
                    )
                    .background_pixel(screen.black_pixel),
            )
            .map_err(disconnected)?;

        set_title(&user_conn, win, &startup.title)?;
        // Advertise WM_DELETE_WINDOW so the WM close button is delivered as a
        // ClientMessage we can handle (exit cleanly instead of being killed).
        let wm_protocols = user_conn
            .intern_atom(false, b"WM_PROTOCOLS")
            .map_err(disconnected)?
            .reply()
            .map_err(disconnected)?
            .atom;
        let wm_delete_window = user_conn
            .intern_atom(false, b"WM_DELETE_WINDOW")
            .map_err(disconnected)?
            .reply()
            .map_err(disconnected)?
            .atom;
        user_conn
            .change_property32(
                PropMode::REPLACE,
                win,
                wm_protocols,
                AtomEnum::ATOM,
                &[wm_delete_window],
            )
            .map_err(disconnected)?
            .check()
            .map_err(disconnected)?;

        let gc = user_conn.generate_id().map_err(disconnected)?;
        user_conn
            .create_gc(
                gc,
                win,
                &x11rb::protocol::xproto::CreateGCAux::new().foreground(0),
            )
            .map_err(disconnected)?
            .check()
            .map_err(disconnected)?;

        user_conn.map_window(win).map_err(disconnected)?;
        user_conn.flush().map_err(disconnected)?;

        let user_mapping = raw::get_mapping(&user_conn)?;
        let target_mapping = raw::get_mapping(&target_conn)?;

        Ok(Self {
            user_conn,
            win,
            gc,
            target_conn,
            target_root,
            src_w,
            src_h,
            win_w,
            win_h,
            user_mapping,
            target_mapping,
            held_buttons: Vec::new(),
            pressed_keys: HashMap::new(),
            pending_motion: None,
            dirty: true,
        })
    }

    /// Map an event position (window coords) to target display coords.
    fn map_point(&self, x: i16, y: i16) -> (i32, i32) {
        let dx = (x.max(0) as i32 * self.src_w as i32) / self.win_w as i32;
        let dy = (y.max(0) as i32 * self.src_h as i32) / self.win_h as i32;
        (
            dx.clamp(0, self.src_w as i32 - 1),
            dy.clamp(0, self.src_h as i32 - 1),
        )
    }

    /// Process pending X events. Returns `false` when the expando should
    /// close.
    fn pump_events(&mut self, events: &mpsc::Sender<ExpandoEvent>) -> bool {
        loop {
            let event = match self.user_conn.poll_for_event() {
                Ok(event) => event,
                Err(_) => return false,
            };
            let Some(event) = event else { return true };
            match event {
                Event::DestroyNotify(_) => {
                    let _ = events.send(ExpandoEvent::Closed);
                    return false;
                }
                Event::ClientMessage(_) => {
                    // WM_DELETE_WINDOW.
                    let _ = events.send(ExpandoEvent::Closed);
                    return false;
                }
                Event::Expose(_) => self.dirty = true,
                Event::ButtonPress(event) => match event.detail {
                    1..=3 => {
                        let (dx, dy) = self.map_point(event.event_x, event.event_y);
                        if raw::button(
                            &self.target_conn,
                            self.target_root,
                            dx,
                            dy,
                            event.detail,
                            true,
                        )
                        .is_ok()
                        {
                            self.held_buttons.push(event.detail);
                        }
                        self.dirty = true;
                    }
                    4 | 5 => {
                        // Wheel up/down at the pointer position.
                        let (dx, dy) = self.map_point(event.event_x, event.event_y);
                        let _ = raw::move_to(&self.target_conn, self.target_root, dx, dy);
                        let _ = raw::wheel(
                            &self.target_conn,
                            self.target_root,
                            0,
                            if event.detail == 4 { -1 } else { 1 },
                        );
                        self.dirty = true;
                    }
                    _ => {}
                },
                Event::ButtonRelease(event) => {
                    if (1..=3).contains(&event.detail) {
                        let (dx, dy) = self.map_point(event.event_x, event.event_y);
                        let _ = raw::button(
                            &self.target_conn,
                            self.target_root,
                            dx,
                            dy,
                            event.detail,
                            false,
                        );
                        self.held_buttons.retain(|b| *b != event.detail);
                        self.dirty = true;
                    }
                }
                Event::MotionNotify(event) => {
                    self.pending_motion = Some((event.event_x, event.event_y));
                }
                Event::KeyPress(event) => {
                    if !self.forward_key(&event, true) {
                        let _ = events.send(ExpandoEvent::Closed);
                        return false;
                    }
                    self.dirty = true;
                }
                Event::KeyRelease(event) => {
                    self.release_key(&event);
                    self.dirty = true;
                }
                _ => {}
            }
        }
    }

    /// Forward at most one motion per pump cycle (the last one wins).
    fn flush_motion(&mut self) {
        let Some((x, y)) = self.pending_motion.take() else {
            return;
        };
        let (dx, dy) = self.map_point(x, y);
        if raw::move_to(&self.target_conn, self.target_root, dx, dy).is_ok() {
            self.dirty = true;
        }
    }

    /// Translate a user key event to the target display. Returns `false`
    /// when the key was Escape (the exit gesture).
    fn forward_key(&mut self, event: &x11rb::protocol::xproto::KeyPressEvent, _down: bool) -> bool {
        let level = usize::from(event.state.contains(ModMask::SHIFT));
        let Some(keysym) = raw::keysym_at(&self.user_mapping, event.detail, level) else {
            return true;
        };
        if keysym == KEY_ESC {
            return false;
        }
        let Some((keycode, needs_shift)) = raw::keycode_for_keysym(&self.target_mapping, keysym)
        else {
            return true;
        };
        let mut shift_injected = false;
        if needs_shift {
            if let Some((shift_kc, _)) = raw::keycode_for_keysym(&self.target_mapping, KEY_SHIFT_L)
            {
                if raw::press(&self.target_conn, shift_kc, true).is_ok() {
                    shift_injected = true;
                }
            }
        }
        let _ = raw::press(&self.target_conn, keycode, true);
        self.pressed_keys
            .insert(event.detail, (keycode, shift_injected));
        true
    }

    /// Release a previously forwarded key using the press-time translation.
    fn release_key(&mut self, event: &x11rb::protocol::xproto::KeyReleaseEvent) {
        let Some((keycode, shift_injected)) = self.pressed_keys.remove(&event.detail) else {
            return;
        };
        let _ = raw::press(&self.target_conn, keycode, false);
        if shift_injected {
            if let Some((shift_kc, _)) = raw::keycode_for_keysym(&self.target_mapping, KEY_SHIFT_L)
            {
                let _ = raw::press(&self.target_conn, shift_kc, false);
            }
        }
    }

    fn render(&mut self) -> Result<(), ()> {
        let image = self
            .target_conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.target_root,
                0,
                0,
                self.src_w as u16,
                self.src_h as u16,
                u32::MAX,
            )
            .map_err(|_| ())?
            .reply()
            .map_err(|_| ())?;

        let scaled = scale(&image.data, self.src_w, self.src_h, self.win_w, self.win_h);
        let data = scaled.as_deref().unwrap_or(&image.data);
        let (put_w, put_h, depth) = if scaled.is_some() {
            (self.win_w, self.win_h, 24)
        } else {
            (self.src_w, self.src_h, image.depth)
        };

        self.user_conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.win,
                self.gc,
                put_w as u16,
                put_h as u16,
                0,
                0,
                0,
                depth,
                data,
            )
            .map_err(|_| ())?
            .check()
            .map_err(|_| ())?;
        self.user_conn.flush().map_err(|_| ())?;
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.user_conn.destroy_window(self.win);
        let _ = self.user_conn.flush();
    }
}
