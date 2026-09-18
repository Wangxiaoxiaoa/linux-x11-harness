use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use image::{imageops::FilterType, RgbaImage};
use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::properties::{WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, ImageFormat,
    PropMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;
use x11rb::COPY_DEPTH_FROM_PARENT;

/// Refresh interval for the preview render loop.
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// How often pending X events (close, double click) are checked.
const PUMP_INTERVAL: Duration = Duration::from_millis(20);

/// Maximum gap between two clicks (in X server milliseconds) that still counts
/// as a double click.
const DOUBLE_CLICK_MS: u32 = 400;

/// Window opacity: ~70% opaque (~30% transparent). Requires a compositing
/// window manager.
const OPACITY: u32 = 0xB3FFFFFF;

/// Geometry and sizes for a single preview window, computed by the manager.
#[derive(Clone, Copy)]
pub(crate) struct PreviewConfig {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub zoom_x: i32,
    pub zoom_y: i32,
    pub zoom_width: u32,
    pub zoom_height: u32,
}

/// A single read-only preview window. Dropping or stopping it closes only the
/// window; the target display keeps running.
pub(crate) struct PreviewWindow {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    window_id: u32,
}

impl PreviewWindow {
    /// Create the window and start the render loop. Setup happens before this
    /// returns, so connection or window creation failures are reported here.
    pub(crate) fn start(
        target_display: &str,
        title: &str,
        config: PreviewConfig,
    ) -> Result<Self, LxhError> {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

        let stop2 = Arc::clone(&stop);
        let alive2 = Arc::clone(&alive);
        let target_display = target_display.to_string();
        let title = title.to_string();

        thread::spawn(move || {
            let result = run(&target_display, &title, config, &stop2, &alive2, ready_tx);
            if let Err(e) = result {
                eprintln!("preview thread error: {e}");
            }
            alive2.store(false, Ordering::Relaxed);
        });

        match ready_rx.recv() {
            Ok(Ok(window_id)) => Ok(Self {
                stop,
                alive,
                window_id,
            }),
            Ok(Err(msg)) => Err(LxhError::DisplayUnavailable(msg)),
            Err(_) => Err(LxhError::DisplayUnavailable(
                "preview thread died during startup".into(),
            )),
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// The X window id of the preview, for external liveness probes.
    pub(crate) fn window_id(&self) -> u32 {
        self.window_id
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Immutable per-window X resources held by the render loop.
struct Preview {
    target_conn: RustConnection,
    target_root: Window,
    source_width: u32,
    source_height: u32,
    user_conn: RustConnection,
    win: Window,
    gc: Gcontext,
    wm_delete_window: u32,
    config: PreviewConfig,
    zoomed: bool,
    last_click: Option<(u32, i16, i16)>,
}

fn run(
    target_display: &str,
    title: &str,
    config: PreviewConfig,
    stop: &AtomicBool,
    alive: &AtomicBool,
    ready: mpsc::Sender<Result<u32, String>>,
) -> Result<(), LxhError> {
    let mut preview = match Preview::new(target_display, title, config) {
        Ok(p) => p,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return Ok(());
        }
    };
    let _ = ready.send(Ok(preview.win));

    // Events are pumped frequently so user-initiated closes are noticed
    // promptly; the screen is only re-grabbed at the refresh interval.
    let mut last_render = Instant::now();
    while !stop.load(Ordering::Relaxed) && alive.load(Ordering::Relaxed) {
        if !preview.pump_events()? {
            break;
        }
        if last_render.elapsed() >= REFRESH_INTERVAL {
            if let Err(e) = preview.render() {
                let _ = e;
            }
            last_render = Instant::now();
        }
        thread::sleep(PUMP_INTERVAL);
    }

    preview.shutdown();
    Ok(())
}

impl Preview {
    fn new(target_display: &str, title: &str, config: PreviewConfig) -> Result<Self, LxhError> {
        let (target_conn, target_screen) =
            RustConnection::connect(Some(target_display)).map_err(|e| {
                LxhError::DisplayUnavailable(format!("cannot connect to target display: {e}"))
            })?;
        let target_root = target_conn.setup().roots[target_screen].root;
        let target_geom = target_conn
            .get_geometry(target_root)
            .map_err(disconnected)?
            .reply()
            .map_err(disconnected)?;
        let source_width = target_geom.width as u32;
        let source_height = target_geom.height as u32;

        let user_display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        let (user_conn, user_screen) =
            RustConnection::connect(Some(&user_display)).map_err(|e| {
                LxhError::DisplayUnavailable(format!("cannot connect to {user_display}: {e}"))
            })?;
        let screen = user_conn.setup().roots[user_screen].clone();
        let root = screen.root;
        let visual = screen.root_visual;

        let win = user_conn.generate_id().map_err(disconnected)?;
        user_conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                win,
                root,
                config.x as i16,
                config.y as i16,
                config.width as u16,
                config.height as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                visual,
                &CreateWindowAux::new()
                    .event_mask(
                        EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::BUTTON_PRESS,
                    )
                    .background_pixel(screen.black_pixel),
            )
            .map_err(disconnected)?;

        set_title(&user_conn, win, title)?;
        set_opacity(&user_conn, win, OPACITY)?;
        set_geometry_hints(
            &user_conn,
            win,
            config.x,
            config.y,
            config.width,
            config.height,
        )?;

        let wm_delete_window = {
            let wm_protocols = user_conn
                .intern_atom(false, b"WM_PROTOCOLS")
                .map_err(disconnected)?
                .reply()
                .map_err(disconnected)?
                .atom;
            let atom = user_conn
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
                    &[atom],
                )
                .map_err(disconnected)?;
            atom
        };

        let gc = user_conn.generate_id().map_err(disconnected)?;
        user_conn
            .create_gc(
                gc,
                win,
                &CreateGCAux::new().foreground(0).background(u32::MAX),
            )
            .map_err(disconnected)?
            .check()
            .map_err(disconnected)?;

        user_conn.map_window(win).map_err(disconnected)?;
        user_conn.flush().map_err(disconnected)?;

        Ok(Self {
            target_conn,
            target_root,
            source_width,
            source_height,
            user_conn,
            win,
            gc,
            wm_delete_window,
            config,
            zoomed: false,
            last_click: None,
        })
    }

    /// Process pending X events. Returns `false` when the window was closed.
    fn pump_events(&mut self) -> Result<bool, LxhError> {
        loop {
            let event = match self.user_conn.poll_for_event().map_err(disconnected)? {
                Some(event) => event,
                None => return Ok(true),
            };
            match event {
                Event::DestroyNotify(_) => return Ok(false),
                Event::ClientMessage(event) => {
                    if event.format == 32
                        && event.window == self.win
                        && event.data.as_data32()[0] == self.wm_delete_window
                    {
                        return Ok(false);
                    }
                }
                Event::ButtonPress(event) if event.detail == 1 => {
                    self.handle_click(event.time, event.event_x, event.event_y)?;
                }
                _ => {}
            }
        }
    }

    fn handle_click(&mut self, time: u32, x: i16, y: i16) -> Result<(), LxhError> {
        let is_double = match self.last_click {
            Some((last_time, last_x, last_y)) => {
                time.wrapping_sub(last_time) < DOUBLE_CLICK_MS
                    && (x - last_x).abs() < 8
                    && (y - last_y).abs() < 8
            }
            None => false,
        };
        if is_double {
            self.last_click = None;
            self.toggle_zoom()?;
        } else {
            self.last_click = Some((time, x, y));
        }
        Ok(())
    }

    fn toggle_zoom(&mut self) -> Result<(), LxhError> {
        self.zoomed = !self.zoomed;
        let (x, y, width, height) = if self.zoomed {
            (
                self.config.zoom_x,
                self.config.zoom_y,
                self.config.zoom_width,
                self.config.zoom_height,
            )
        } else {
            (
                self.config.x,
                self.config.y,
                self.config.width,
                self.config.height,
            )
        };
        self.user_conn
            .configure_window(
                self.win,
                &x11rb::protocol::xproto::ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(width)
                    .height(height),
            )
            .map_err(disconnected)?;
        set_geometry_hints(&self.user_conn, self.win, x, y, width, height)?;
        self.user_conn.flush().map_err(disconnected)?;
        Ok(())
    }

    fn current_size(&self) -> (u32, u32) {
        if self.zoomed {
            (self.config.zoom_width, self.config.zoom_height)
        } else {
            (self.config.width, self.config.height)
        }
    }

    fn render(&mut self) -> Result<(), LxhError> {
        let (dst_w, dst_h) = self.current_size();
        let image = self
            .target_conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.target_root,
                0,
                0,
                self.source_width as u16,
                self.source_height as u16,
                u32::MAX,
            )
            .map_err(disconnected)?
            .reply()
            .map_err(disconnected)?;

        let scaled = scale(
            &image.data,
            self.source_width,
            self.source_height,
            dst_w,
            dst_h,
        );
        let data = scaled.as_deref().unwrap_or(&image.data);
        let (put_w, put_h, depth) = if scaled.is_some() {
            (dst_w, dst_h, 24)
        } else {
            (self.source_width, self.source_height, image.depth)
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
            .map_err(disconnected)?
            .check()
            .map_err(disconnected)?;
        self.user_conn.flush().map_err(disconnected)
    }

    fn shutdown(&mut self) {
        let _ = self.user_conn.destroy_window(self.win);
        let _ = self.user_conn.flush();
    }
}

fn set_title(conn: &RustConnection, win: Window, title: &str) -> Result<(), LxhError> {
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

fn set_opacity(conn: &RustConnection, win: Window, opacity: u32) -> Result<(), LxhError> {
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

/// Hint the WM to keep this position and disallow user resizing.
fn set_geometry_hints(
    conn: &RustConnection,
    win: Window,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), LxhError> {
    let hints = WmSizeHints {
        position: Some((WmSizeHintsSpecification::UserSpecified, x, y)),
        size: Some((
            WmSizeHintsSpecification::UserSpecified,
            width as i32,
            height as i32,
        )),
        min_size: Some((width as i32, height as i32)),
        max_size: Some((width as i32, height as i32)),
        ..Default::default()
    };
    hints
        .set_normal_hints(conn, win)
        .map_err(disconnected)?
        .check()
        .map_err(disconnected)
}

/// Scale a 24-bit ZPixmap (BGRA, 4 bytes per pixel) to the target size.
fn scale(bgra: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Vec<u8>> {
    if src_w == dst_w && src_h == dst_h {
        return None;
    }
    let mut rgba = vec![0u8; (src_w * src_h * 4) as usize];
    let (src, _) = bgra.as_chunks::<4>();
    let (dst, _) = rgba.as_chunks_mut::<4>();
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }

    let src = RgbaImage::from_raw(src_w, src_h, rgba)?;
    let resized = image::imageops::resize(&src, dst_w, dst_h, FilterType::Triangle);

    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    let (src, _) = resized.as_raw().as_chunks::<4>();
    let (dst, _) = out.as_chunks_mut::<4>();
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
    Some(out)
}

fn disconnected<E: std::fmt::Display>(e: E) -> LxhError {
    LxhError::DisplayUnavailable(e.to_string())
}
