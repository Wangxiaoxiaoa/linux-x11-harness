//! The panel container window: a single override-redirect frame that hosts
//! all preview cells as children.
//!
//! The container owns only its own chrome: a pager bar (`◀ page/total ▶`)
//! drawn when more than one page exists; clicks are reported to the panel.
//!
//! All geometry decisions (size, position, cell slots) live in `layout.rs`
//! and are applied by `panel.rs` through [`PreviewContainer::set_state`];
//! the container never moves or resizes itself.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ChangeGCAux, ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, Rectangle, Segment, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

use crate::x11::{connect_user, disconnected, set_opacity, set_title};

/// How often events are polled.
const PUMP_INTERVAL: Duration = Duration::from_millis(20);
/// Window opacity: ~70% opaque; needs a compositing WM.
const OPACITY: u32 = 0xB3FFFFFF;
/// Chrome background (dark slate).
const CHROME_BG: u32 = 0x101418;

/// Events the container reports to the panel.
#[derive(Debug)]
pub(crate) enum ContainerEvent {
    /// One of the pager buttons was clicked.
    PagePrev,
    PageNext,
}

/// Chrome + geometry state the panel pushes into the container.
#[derive(Clone, Copy)]
pub(crate) struct ContainerState {
    /// Whether the pager bar is shown (more than one page).
    pub pager_visible: bool,
    /// Current 1-based page and total pages (only meaningful when visible).
    pub page: u32,
    pub pages: u32,
    /// Pager bar height in pixels.
    pub pager_h: u16,
    /// Container geometry on the user's screen.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Handle to the running container, held by the panel.
pub(crate) struct PreviewContainer {
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    window_id: u32,
}

impl PreviewContainer {
    /// Create the container window and start its event/paint loop.
    pub(crate) fn start(
        title: &str,
        state: ContainerState,
        events: mpsc::Sender<ContainerEvent>,
    ) -> Result<Self, LxhError> {
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

        let stop2 = Arc::clone(&stop);
        let alive2 = Arc::clone(&alive);
        let title = title.to_string();

        thread::spawn(move || {
            let result = run(&title, state, events, &stop2, &alive2, ready_tx);
            if let Err(e) = result {
                eprintln!("preview container error: {e}");
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
                "preview container thread died during startup".into(),
            )),
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub(crate) fn window_id(&self) -> u32 {
        self.window_id
    }

    /// Push new chrome/geometry state; the container applies and repaints.
    pub(crate) fn set_state(&self, state: ContainerState) {
        *state_mailbox().lock().unwrap() = Some(state);
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for PreviewContainer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// State updates from the panel, consumed by the container thread.
fn state_mailbox() -> &'static Mutex<Option<ContainerState>> {
    static MAILBOX: std::sync::OnceLock<Mutex<Option<ContainerState>>> = std::sync::OnceLock::new();
    MAILBOX.get_or_init(|| Mutex::new(None))
}

/// Paint state owned by the container thread.
struct Container {
    conn: RustConnection,
    win: Window,
    gc: Gcontext,
    state: ContainerState,
    events: mpsc::Sender<ContainerEvent>,
}

fn run(
    title: &str,
    state: ContainerState,
    events: mpsc::Sender<ContainerEvent>,
    stop: &AtomicBool,
    alive: &AtomicBool,
    ready: mpsc::Sender<Result<u32, String>>,
) -> Result<(), LxhError> {
    let mut container = match Container::new(title, state, events) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return Ok(());
        }
    };
    let _ = ready.send(Ok(container.win));

    while !stop.load(Ordering::Relaxed) && alive.load(Ordering::Relaxed) {
        container.apply_state();
        if !container.pump_events() {
            break;
        }
        container.paint();
        thread::sleep(PUMP_INTERVAL);
    }

    container.shutdown();
    Ok(())
}

impl Container {
    fn new(
        title: &str,
        state: ContainerState,
        events: mpsc::Sender<ContainerEvent>,
    ) -> Result<Self, LxhError> {
        let (conn, screen_idx) = connect_user()?;
        let screen = conn.setup().roots[screen_idx].clone();
        let root = screen.root;

        let win = conn.generate_id().map_err(disconnected)?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            state.x as i16,
            state.y as i16,
            state.width as u16,
            state.height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(
                    EventMask::STRUCTURE_NOTIFY | EventMask::BUTTON_PRESS | EventMask::EXPOSURE,
                )
                .background_pixel(CHROME_BG),
        )
        .map_err(disconnected)?;

        set_title(&conn, win, title)?;
        set_opacity(&conn, win, OPACITY)?;

        let gc = conn.generate_id().map_err(disconnected)?;
        conn.create_gc(gc, win, &CreateGCAux::new().foreground(0xFFFFFFFF))
            .map_err(disconnected)?
            .check()
            .map_err(disconnected)?;

        conn.map_window(win).map_err(disconnected)?;
        conn.flush().map_err(disconnected)?;

        Ok(Self {
            conn,
            win,
            gc,
            state,
            events,
        })
    }

    /// Apply chrome/geometry updates posted by the panel.
    fn apply_state(&mut self) {
        let update = state_mailbox().lock().unwrap().take();
        if let Some(state) = update {
            if state.x != self.state.x
                || state.y != self.state.y
                || state.width != self.state.width
                || state.height != self.state.height
            {
                let _ = self
                    .conn
                    .configure_window(
                        self.win,
                        &ConfigureWindowAux::new()
                            .x(state.x)
                            .y(state.y)
                            .width(state.width)
                            .height(state.height),
                    )
                    .map_err(disconnected);
            }
            self.state = state;
        }
    }

    fn pump_events(&mut self) -> bool {
        loop {
            let Ok(event) = self.conn.poll_for_event() else {
                return false;
            };
            let Some(event) = event else { return true };
            match event {
                Event::DestroyNotify(_) => return false,
                Event::ButtonPress(event) if event.detail == 1 => {
                    if !self.state.pager_visible {
                        continue;
                    }
                    let pager_top = self.state.height - self.state.pager_h as u32;
                    if event.event_y as u32 >= pager_top {
                        if (event.event_x as u32) < self.state.width / 2 {
                            let _ = self.events.send(ContainerEvent::PagePrev);
                        } else {
                            let _ = self.events.send(ContainerEvent::PageNext);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Repaint the chrome (pager bar when needed). Cell content is drawn by
    /// the cells themselves.
    fn paint(&mut self) {
        let s = self.state;

        // Stay above full-screen input/grab layers some desktop components
        // raise over our overlay; this panel is an always-visible overlay.
        let _ = self.conn.configure_window(
            self.win,
            &ConfigureWindowAux::new().stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
        );

        if s.pager_visible {
            let pager_top = s.height - s.pager_h as u32;
            let _ = self
                .conn
                .change_gc(self.gc, &ChangeGCAux::new().foreground(CHROME_BG));
            let _ = self.conn.poly_fill_rectangle(
                self.win,
                self.gc,
                &[Rectangle {
                    x: 0,
                    y: pager_top as i16,
                    width: s.width as u16,
                    height: s.pager_h,
                }],
            );

            let _ = self.conn.change_gc(
                self.gc,
                &ChangeGCAux::new().foreground(0xFFFFFFFF).line_width(2),
            );
            let cy = pager_top as i16 + s.pager_h as i16 / 2;
            // ◀ arrow (left half is the previous-page hit area).
            let _ = self.conn.poly_segment(
                self.win,
                self.gc,
                &[
                    Segment {
                        x1: 26,
                        y1: cy - 6,
                        x2: 14,
                        y2: cy,
                    },
                    Segment {
                        x1: 14,
                        y1: cy,
                        x2: 26,
                        y2: cy + 6,
                    },
                ],
            );
            // ▶ arrow (right half is the next-page hit area).
            let rx = s.width as i16 - 14;
            let _ = self.conn.poly_segment(
                self.win,
                self.gc,
                &[
                    Segment {
                        x1: rx - 12,
                        y1: cy - 6,
                        x2: rx,
                        y2: cy,
                    },
                    Segment {
                        x1: rx,
                        y1: cy,
                        x2: rx - 12,
                        y2: cy + 6,
                    },
                ],
            );
            // page/pages text, centered.
            if let Ok(fid) = self.conn.generate_id() {
                if self.conn.open_font(fid, b"10x20").is_ok() {
                    let _ = self.conn.change_gc(
                        self.gc,
                        &ChangeGCAux::new()
                            .font(fid)
                            .foreground(0xFFFFFFFF)
                            .background(CHROME_BG),
                    );
                    let text = format!("{}/{}", s.page, s.pages);
                    let _ = self.conn.image_text8(
                        self.win,
                        self.gc,
                        s.width as i16 / 2 - 14,
                        cy + 7,
                        text.as_bytes(),
                    );
                    let _ = self.conn.close_font(fid);
                }
            }
            let _ = self
                .conn
                .change_gc(self.gc, &ChangeGCAux::new().line_width(0));
        }

        let _ = self.conn.flush();
    }

    fn shutdown(&mut self) {
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}
