//! One preview cell: a child window of the panel container that mirrors a
//! single harness display.
//!
//! A cell knows nothing about the panel, the container's layout, or other
//! cells. It renders its target display in a loop, draws its own hover title
//! bar (title + close button, overlaying only its own top strip), reports
//! close/disconnect events, and accepts geometry/visibility updates from the
//! panel through shared flags.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use image::{imageops::FilterType, RgbaImage};
use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ChangeGCAux, ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, ImageFormat, Rectangle, Segment, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

use crate::x11::{connect_user, disconnected, set_opacity, set_title};

/// Render interval for a cell.
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);
/// How often pending X events (close, hover, geometry) are checked.
const PUMP_INTERVAL: Duration = Duration::from_millis(20);
/// Window opacity: ~70% opaque; needs a compositing WM.
const OPACITY: u32 = 0xB3FFFFFF;

/// Title strip height and close-button width. Derived once per process from
/// the screen metrics and passed to every cell.
#[derive(Clone, Copy)]
pub(crate) struct CellChrome {
    pub strip_h: u16,
    pub btn_w: u16,
}

/// Events a cell reports back to the panel.
#[derive(Debug)]
pub(crate) enum CellEvent {
    /// The user clicked the close button.
    Closed { display_id: String },
    /// The target display's X connection broke.
    DisplayGone { display_id: String },
}

/// Geometry for one cell, relative to the container. Set by the panel.
#[derive(Clone, Copy)]
pub(crate) struct CellRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Shared state the panel mutates and the cell thread consumes.
#[derive(Default)]
struct CellMailbox {
    pending_rect: Mutex<Option<CellRect>>,
    mapped: AtomicBool,
}

/// Handle to a running cell, held by the panel.
pub(crate) struct PreviewCell {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    mailbox: Arc<CellMailbox>,
}

impl PreviewCell {
    /// Create the cell window (a child of `parent`) and start its render
    /// loop. Setup errors surface before returning.
    pub(crate) fn start(
        parent: Window,
        display_id: String,
        target_display: &str,
        title: &str,
        rect: CellRect,
        chrome: CellChrome,
        events: mpsc::Sender<CellEvent>,
    ) -> Result<Self, LxhError> {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let mailbox = Arc::new(CellMailbox {
            pending_rect: Mutex::new(Some(rect)),
            mapped: AtomicBool::new(true),
        });
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let stop2 = Arc::clone(&stop);
        let alive2 = Arc::clone(&alive);
        let mailbox2 = Arc::clone(&mailbox);
        let startup = CellStartup {
            parent,
            display_id: display_id.clone(),
            target_display: target_display.to_string(),
            title: title.to_string(),
            chrome,
            events,
        };

        thread::spawn(move || {
            let result = run(startup, &stop2, &alive2, &mailbox2, ready_tx);
            if let Err(e) = result {
                eprintln!("preview cell error: {e}");
            }
            alive2.store(false, Ordering::Relaxed);
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                alive,
                mailbox,
            }),
            Ok(Err(msg)) => Err(LxhError::DisplayUnavailable(msg)),
            Err(_) => Err(LxhError::DisplayUnavailable(
                "preview cell thread died during startup".into(),
            )),
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Move/resize the cell (coordinates relative to the container).
    pub(crate) fn set_rect(&self, rect: CellRect) {
        *self.mailbox.pending_rect.lock().unwrap() = Some(rect);
    }

    /// Show or hide the cell (pagination). Hidden cells skip rendering.
    pub(crate) fn set_mapped(&self, mapped: bool) {
        self.mailbox.mapped.store(mapped, Ordering::Relaxed);
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for PreviewCell {
    fn drop(&mut self) {
        self.stop();
    }
}

/// X resources and state owned by the cell's render thread.
struct Cell {
    display_id: String,
    target_conn: RustConnection,
    target_root: Window,
    source_width: u32,
    source_height: u32,
    user_conn: RustConnection,
    win: Window,
    gc: Gcontext,
    title: String,
    rect: CellRect,
    chrome: CellChrome,
    hover: bool,
    /// Last map state actually applied to the X window.
    mapped_applied: bool,
    mailbox: Arc<CellMailbox>,
    events: mpsc::Sender<CellEvent>,
}

/// Everything the cell thread needs to bootstrap one cell.
struct CellStartup {
    parent: Window,
    display_id: String,
    target_display: String,
    title: String,
    chrome: CellChrome,
    events: mpsc::Sender<CellEvent>,
}

fn run(
    startup: CellStartup,
    stop: &AtomicBool,
    alive: &AtomicBool,
    mailbox: &Arc<CellMailbox>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), LxhError> {
    let rect = mailbox
        .pending_rect
        .lock()
        .unwrap()
        .take()
        .expect("cell starts with an initial rect");
    let mut cell = match Cell::new(&startup, rect, mailbox) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return Ok(());
        }
    };
    let _ = ready.send(Ok(()));

    let mut last_render = Instant::now();
    while !stop.load(Ordering::Relaxed) && alive.load(Ordering::Relaxed) {
        cell.apply_pending_rect();
        cell.apply_mapped();
        if !cell.pump_events() {
            break;
        }
        if cell.mapped_applied && last_render.elapsed() >= REFRESH_INTERVAL {
            match cell.render() {
                Ok(()) => {}
                Err(CellRenderError::DisplayGone) => {
                    let _ = cell.events.send(CellEvent::DisplayGone {
                        display_id: cell.display_id.clone(),
                    });
                    break;
                }
                Err(CellRenderError::Transient) => {}
            }
            last_render = Instant::now();
        }
        thread::sleep(PUMP_INTERVAL);
    }

    cell.shutdown();
    Ok(())
}

enum CellRenderError {
    /// The target display's connection is broken.
    DisplayGone,
    /// Skip this frame.
    Transient,
}

impl Cell {
    fn new(
        startup: &CellStartup,
        rect: CellRect,
        mailbox: &Arc<CellMailbox>,
    ) -> Result<Self, LxhError> {
        let CellStartup {
            parent,
            display_id,
            target_display,
            title,
            chrome,
            events,
        } = startup;
        let (target_conn, target_screen) =
            RustConnection::connect(Some(target_display)).map_err(|e| {
                LxhError::DisplayUnavailable(format!("cannot connect to target display: {e}"))
            })?;
        let target_root = target_conn.setup().roots[target_screen].root;
        let geom = target_conn
            .get_geometry(target_root)
            .map_err(|e| LxhError::DisplayUnavailable(e.to_string()))?
            .reply()
            .map_err(|e| LxhError::DisplayUnavailable(e.to_string()))?;
        let source_width = geom.width as u32;
        let source_height = geom.height as u32;

        let (user_conn, user_screen) = connect_user()?;
        let screen = user_conn.setup().roots[user_screen].clone();

        let win = user_conn.generate_id().map_err(disconnected)?;
        // Child of the OR container: the WM never manages this window.
        user_conn
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                win,
                *parent,
                rect.x as i16,
                rect.y as i16,
                rect.width as u16,
                rect.height as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new()
                    .event_mask(
                        EventMask::EXPOSURE
                            | EventMask::STRUCTURE_NOTIFY
                            | EventMask::BUTTON_PRESS
                            | EventMask::ENTER_WINDOW
                            | EventMask::LEAVE_WINDOW,
                    )
                    .background_pixel(screen.black_pixel),
            )
            .map_err(disconnected)?;

        set_title(&user_conn, win, title)?;
        set_opacity(&user_conn, win, OPACITY)?;

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
            display_id: display_id.clone(),
            target_conn,
            target_root,
            source_width,
            source_height,
            user_conn,
            win,
            gc,
            title: title.clone(),
            rect,
            chrome: *chrome,
            hover: false,
            mapped_applied: true,
            mailbox: Arc::clone(mailbox),
            events: events.clone(),
        })
    }

    /// Map/unmap the window to match the panel's visibility flag.
    fn apply_mapped(&mut self) {
        let want = self.mailbox.mapped.load(Ordering::Relaxed);
        if want != self.mapped_applied {
            self.mapped_applied = want;
            let result = if want {
                self.user_conn.map_window(self.win)
            } else {
                self.user_conn.unmap_window(self.win)
            };
            if let Err(e) = result {
                eprintln!("preview cell map/unmap failed: {e}");
            }
            let _ = self.user_conn.flush();
        }
    }

    /// Apply a geometry update posted by the panel, if any.
    fn apply_pending_rect(&mut self) {
        let rect = self.mailbox.pending_rect.lock().unwrap().take();
        if let Some(rect) = rect {
            self.rect = rect;
            let _ = self
                .user_conn
                .configure_window(
                    self.win,
                    &ConfigureWindowAux::new()
                        .x(rect.x)
                        .y(rect.y)
                        .width(rect.width)
                        .height(rect.height),
                )
                .map_err(disconnected);
            let _ = self.user_conn.flush();
        }
    }

    /// Process pending X events. Returns `false` when the cell was closed.
    fn pump_events(&mut self) -> bool {
        loop {
            let event = match self.user_conn.poll_for_event() {
                Ok(event) => event,
                Err(_) => return false,
            };
            let Some(event) = event else { return true };
            match event {
                // External destroy (or any teardown we did not initiate):
                // report it so the panel can re-layout or tear itself down.
                Event::DestroyNotify(_) => {
                    let _ = self.events.send(CellEvent::Closed {
                        display_id: self.display_id.clone(),
                    });
                    return false;
                }
                Event::EnterNotify(_) => self.hover = true,
                Event::LeaveNotify(_) => self.hover = false,
                Event::ButtonPress(event) if event.detail == 1 => {
                    let in_close = self.hover
                        && event.event_y < self.chrome.strip_h as i16
                        && event.event_x >= self.rect.width as i16 - self.chrome.btn_w as i16;
                    if in_close {
                        let _ = self.events.send(CellEvent::Closed {
                            display_id: self.display_id.clone(),
                        });
                        return false;
                    }
                }
                _ => {}
            }
        }
    }

    fn render(&mut self) -> Result<(), CellRenderError> {
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
            .map_err(|_| CellRenderError::DisplayGone)?
            .reply()
            .map_err(classify_reply)?;

        let scaled = scale(
            &image.data,
            self.source_width,
            self.source_height,
            self.rect.width,
            self.rect.height,
        );
        let data = scaled.as_deref().unwrap_or(&image.data);
        let (put_w, put_h, depth) = if scaled.is_some() {
            (self.rect.width, self.rect.height, 24)
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
            .map_err(|_| CellRenderError::DisplayGone)?
            .check()
            .map_err(|_| CellRenderError::DisplayGone)?;

        if self.hover {
            self.draw_title_bar();
        }
        self.user_conn
            .flush()
            .map_err(|_| CellRenderError::DisplayGone)?;
        Ok(())
    }

    /// Draw the hover title bar: dark strip + title text + close button,
    /// overlaying the top strip of this cell only.
    fn draw_title_bar(&self) {
        let gc = self.gc;
        let h = self.chrome.strip_h;
        let w = self.rect.width as u16;

        let _ = self
            .user_conn
            .change_gc(gc, &ChangeGCAux::new().foreground(0xE6202020));
        let _ = self.user_conn.poly_fill_rectangle(
            self.win,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }],
        );

        if let Ok(fid) = self.user_conn.generate_id() {
            if self.user_conn.open_font(fid, b"10x20").is_ok() {
                let _ = self.user_conn.change_gc(
                    gc,
                    &ChangeGCAux::new()
                        .font(fid)
                        .foreground(0xFFFFFFFF)
                        .background(0xE6202020),
                );
                let ascii: String = self.title.chars().filter(|c| c.is_ascii()).collect();
                let _ = self.user_conn.image_text8(
                    self.win,
                    gc,
                    10,
                    h as i16 / 2 + 7,
                    ascii.as_bytes(),
                );
                let _ = self.user_conn.close_font(fid);
            }
        }

        let _ = self
            .user_conn
            .change_gc(gc, &ChangeGCAux::new().foreground(0xFFFFFFFF).line_width(2));

        // Close button: an X at the right end of the strip.
        let cx = w as i16 - self.chrome.btn_w as i16 / 2;
        let cy = h as i16 / 2;
        for dx in [-6i16, 6] {
            let _ = self.user_conn.poly_segment(
                self.win,
                gc,
                &[Segment {
                    x1: cx - dx,
                    y1: cy - 6,
                    x2: cx + dx,
                    y2: cy + 6,
                }],
            );
        }
    }

    fn shutdown(&mut self) {
        let _ = self.user_conn.destroy_window(self.win);
        let _ = self.user_conn.flush();
    }
}

/// Scale a 24-bit ZPixmap (BGRA) to the target size.
fn scale(bgra: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Vec<u8>> {
    if src_w == dst_w && src_h == dst_h {
        return None;
    }
    if src_w == 0 || src_h == 0 {
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

/// X error codes that are transient frame failures, not display death
/// (BadMatch = 8, BadValue = 2 per the X protocol).
const TRANSIENT_X_ERRORS: [u8; 2] = [8, 2];

fn classify_error_code(code: u8) -> CellRenderError {
    if TRANSIENT_X_ERRORS.contains(&code) {
        CellRenderError::Transient
    } else {
        CellRenderError::DisplayGone
    }
}

fn classify_reply(e: x11rb::errors::ReplyError) -> CellRenderError {
    match e {
        x11rb::errors::ReplyError::ConnectionError(_) => CellRenderError::DisplayGone,
        x11rb::errors::ReplyError::X11Error(err) => classify_error_code(err.error_code),
    }
}
