//! Shared clipboard: one canonical value in the hub, mirrored between the
//! user's desktop (`:0`) and every harness display, with per-display
//! direction policies.
//!
//! ```text
//!            canonical { text, fingerprint, version }     (hub)
//!             ↑ x11rb selection owner        ↑ x11rb selection owner
//!      desktop spoke (:0)          sandbox spoke (one per display)
//! ```
//!
//! Every display runs the same spoke loop; the desktop is just a spoke
//! whose policy is always bidirectional. Policies for sandbox displays:
//! `ToSandbox` (default — desktop changes flow in, sandbox changes never
//! touch the desktop), `Bidirectional`, `Off`. See
//! `docs/clipboard-sync.md`.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Content larger than this is truncated before entering the hub.
const MAX_CONTENT_BYTES: usize = 1024 * 1024;
/// Spoke cycle: wakes on hub changes, drains X events.
const SPOKE_TICK: Duration = Duration::from_millis(50);
/// Safety-net poll of the local clipboard (bidirectional policy).
const LOCAL_POLL: Duration = Duration::from_secs(1);
/// Conversion-request reply timeout before a requestor window is reaped.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

fn fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.len().hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

fn truncate(text: String) -> String {
    if text.len() > MAX_CONTENT_BYTES {
        eprintln!(
            "clipboard: content of {} bytes truncated to {MAX_CONTENT_BYTES}",
            text.len()
        );
        let mut cut = MAX_CONTENT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text[..cut].to_string()
    } else {
        text
    }
}

/// One spoke thread. `policy` is shared with the hub so the direction can
/// be changed while the thread runs.
struct SpokeHandle {
    stop: Arc<AtomicBool>,
}

/// Where the current canonical content came from. The desktop spoke
/// skips pushing sandbox-origin updates to the user's desktop; everything
/// else flows everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Desktop,
    Agent,
    Sandbox,
}

#[derive(Clone)]
struct Canonical {
    text: String,
    fp: u64,
    version: u64,
    origin: Origin,
}

struct HubInner {
    canonical: Canonical,
    /// display string -> spoke handle (one spoke per X display).
    spokes: HashMap<String, SpokeHandle>,
}

/// The shared clipboard. One instance per process (each display,
/// including the desktop, gets a spoke).
pub struct ClipboardHub {
    inner: Mutex<HubInner>,
    changed: Condvar,
}

impl Default for ClipboardHub {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardHub {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HubInner {
                canonical: Canonical {
                    text: String::new(),
                    fp: fingerprint(""),
                    version: 0,
                    origin: Origin::Desktop,
                },
                spokes: HashMap::new(),
            }),
            changed: Condvar::new(),
        }
    }

    /// Current shared content.
    pub fn get(&self) -> String {
        self.inner.lock().unwrap().canonical.text.clone()
    }

    fn version(&self) -> u64 {
        self.inner.lock().unwrap().canonical.version
    }

    fn canonical(&self) -> Canonical {
        self.inner.lock().unwrap().canonical.clone()
    }

    /// Replace the shared content if it differs; returns true when changed.
    /// `origin` decides where the update propagates (see `Origin`).
    fn set_if_changed(&self, text: String, origin: Origin) -> bool {
        let text = truncate(text);
        let fp = fingerprint(&text);
        let mut inner = self.inner.lock().unwrap();
        if inner.canonical.fp == fp {
            return false;
        }
        inner.canonical = Canonical {
            text,
            fp,
            version: inner.canonical.version + 1,
            origin,
        };
        self.changed.notify_all();
        true
    }

    /// Replace the shared content; every spoke pushes it to its display,
    /// including the desktop.
    pub fn set(&self, text: &str) {
        self.set_if_changed(text.to_string(), Origin::Agent);
    }

    /// Block until canonical changes or the timeout elapses.
    fn wait_for_change(&self, timeout: Duration) {
        let guard = self.inner.lock().unwrap();
        let version = guard.canonical.version;
        let _ = self
            .changed
            .wait_timeout_while(guard, timeout, |i| i.canonical.version == version)
            .unwrap();
    }

    /// Start a spoke for `display`. Call once per display; the daemon
    /// creates each display exactly once.
    pub fn start_spoke(self: &Arc<Self>, display: &str) {
        let stop = Arc::new(AtomicBool::new(false));
        self.inner.lock().unwrap().spokes.insert(
            display.to_string(),
            SpokeHandle {
                stop: Arc::clone(&stop),
            },
        );
        let hub = Arc::clone(self);
        let display = display.to_string();
        std::thread::spawn(move || run_spoke(hub, display, false, stop));
    }

    /// Start the desktop spoke (the user's `:0`): always full sharing —
    /// desktop copies flow out (via the safety-net read after
    /// SelectionClear) and shared content flows in. Idempotent.
    pub fn start_desktop_spoke(self: &Arc<Self>, display: &str) {
        let mut inner = self.inner.lock().unwrap();
        if inner.spokes.contains_key(display) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        inner.spokes.insert(
            display.to_string(),
            SpokeHandle {
                stop: Arc::clone(&stop),
            },
        );
        let hub = Arc::clone(self);
        let display = display.to_string();
        std::thread::spawn(move || run_spoke(hub, display, true, stop));
    }

    /// Stop the spoke of a destroyed display.
    pub fn stop(&self, display: &str) {
        if let Some(handle) = self.inner.lock().unwrap().spokes.remove(display) {
            handle.stop.store(true, Ordering::Relaxed);
            self.changed.notify_all();
        }
    }

    /// Stop every spoke (daemon shutdown).
    pub fn stop_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        for (_, handle) in inner.spokes.drain() {
            handle.stop.store(true, Ordering::Relaxed);
        }
        self.changed.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Spoke (one thread per X display, desktop and sandboxes alike)
// ---------------------------------------------------------------------------

struct SpokeAtoms {
    clipboard: u32,
    targets: u32,
    utf8_string: u32,
    data_prop: u32,
}

fn intern_atoms(conn: &x11rb::rust_connection::RustConnection) -> Option<SpokeAtoms> {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let intern = |name: &[u8]| -> Option<u32> {
        conn.intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    };
    Some(SpokeAtoms {
        clipboard: intern(b"CLIPBOARD")?,
        targets: intern(b"TARGETS")?,
        utf8_string: intern(b"UTF8_STRING")?,
        data_prop: intern(b"LXH_CLIPBOARD_DATA")?,
    })
}

fn run_spoke(hub: Arc<ClipboardHub>, display: String, desktop: bool, stop: Arc<AtomicBool>) {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, SelectionNotifyEvent, WindowClass,
    };
    use x11rb::protocol::Event;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;
    use x11rb::COPY_DEPTH_FROM_PARENT;

    let Ok((conn, screen)) = RustConnection::connect(Some(&display)) else {
        return; // display gone: nothing to sync
    };
    let root = conn.setup().roots[screen].root;
    let Some(atoms) = intern_atoms(&conn) else {
        return;
    };

    // Unmapped 1x1 owner window; selection events are delivered to it.
    let Ok(win) = conn.generate_id() else {
        return;
    };
    if conn
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            conn.setup().roots[screen].root_visual,
            &CreateWindowAux::new(),
        )
        .is_err()
    {
        return;
    }
    let _ = conn.flush();

    // The version of the canonical value this display currently serves.
    // 0 == serves nothing yet. The desktop spoke additionally keeps the
    // text it serves on the display: sandbox-origin updates must not
    // reach the desktop clipboard, so it serves a snapshot that only
    // desktop/agent-origin updates replace.
    let mut served = 0u64;
    let mut owned = false;
    let mut last_poll = Instant::now() - LOCAL_POLL; // first read is immediate
    let mut serving = String::new();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Drain X events: serve paste requests; watch for takeovers.
        while let Ok(Some(event)) = conn.poll_for_event() {
            match event {
                // An app wants the clipboard: serve the shared content.
                Event::SelectionRequest(req) if req.selection == atoms.clipboard => {
                    let property = if req.property == x11rb::NONE {
                        req.target
                    } else {
                        req.property
                    };
                    if req.target == atoms.targets {
                        let _ = conn.change_property32(
                            PropMode::REPLACE,
                            req.requestor,
                            property,
                            AtomEnum::ATOM,
                            &[atoms.targets, atoms.utf8_string, AtomEnum::STRING.into()],
                        );
                    } else if req.target == atoms.utf8_string
                        || req.target == AtomEnum::STRING.into()
                    {
                        let text = if desktop { &serving } else { &hub.get() };
                        let _ = conn.change_property8(
                            PropMode::REPLACE,
                            req.requestor,
                            property,
                            req.target,
                            text.as_bytes(),
                        );
                    } else {
                        // Unsupported target: refuse with an empty property.
                        let _ = conn.send_event(
                            false,
                            req.requestor,
                            x11rb::protocol::xproto::EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: req.time,
                                requestor: req.requestor,
                                selection: req.selection,
                                target: req.target,
                                property: x11rb::NONE,
                            },
                        );
                        let _ = conn.flush();
                        continue;
                    }
                    let _ = conn.send_event(
                        false,
                        req.requestor,
                        x11rb::protocol::xproto::EventMask::NO_EVENT,
                        SelectionNotifyEvent {
                            response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: req.time,
                            requestor: req.requestor,
                            selection: req.selection,
                            target: req.target,
                            property,
                        },
                    );
                    let _ = conn.flush();
                }
                // We lost ownership: an app on this display copied. Pull the
                // new content into the hub (bidirectional only); the content
                // already lives on this display, so no re-grab is needed.
                Event::SelectionClear(clear) if clear.selection == atoms.clipboard => {
                    owned = false;
                    let origin = if desktop {
                        Origin::Desktop
                    } else {
                        Origin::Sandbox
                    };
                    if let Some(local) = read_display_clipboard(&display) {
                        if hub.set_if_changed(local, origin) {
                            served = hub.version();
                        }
                    }
                }
                _ => {}
            }
        }

        // Safety net: an app may have copied while we were not the owner
        // (e.g. before our first grab) — SelectionClear only fires when we
        // DID own it. One read per second covers that gap.
        if !owned && last_poll.elapsed() >= LOCAL_POLL {
            last_poll = Instant::now();
            let origin = if desktop {
                Origin::Desktop
            } else {
                Origin::Sandbox
            };
            if let Some(local) = read_display_clipboard(&display) {
                if hub.set_if_changed(local, origin) {
                    served = hub.version();
                    // The content is already on this display; serving it
                    // needs no ownership change.
                    continue;
                }
            }
        }

        // Canonical changed from elsewhere: take ownership so this
        // display serves the shared content. The desktop spoke skips
        // sandbox-origin updates and keeps serving its previous snapshot
        // — sandbox content never overwrites the user's desktop clipboard
        // (agent writes are Origin::Agent and always flow through).
        let snapshot = hub.canonical();
        let skip_desktop = snapshot.origin == Origin::Sandbox;
        if hub.version() != served && !(desktop && skip_desktop) {
            let _ = conn.set_selection_owner(win, atoms.clipboard, x11rb::CURRENT_TIME);
            let _ = conn.flush();
            owned = true;
            served = hub.version();
            if desktop {
                serving = snapshot.text.clone();
            }
        }

        hub.wait_for_change(SPOKE_TICK);
    }

    let _ = conn.destroy_window(win);
    let _ = conn.flush();
}

/// Read the CLIPBOARD selection of one display with a fresh connection.
/// Used by `lxh_clipboard_get` for a specific display and by the local
/// change detection above.
pub fn read_display_clipboard(display: &str) -> Option<String> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::{
        xproto::{AtomEnum, ConnectionExt as _},
        Event,
    };

    let (conn, screen) = x11rb::rust_connection::RustConnection::connect(Some(display)).ok()?;
    let root = conn.setup().roots[screen].root;
    let atoms = intern_atoms(&conn)?;

    let owner = conn
        .get_selection_owner(atoms.clipboard)
        .ok()?
        .reply()
        .ok()?
        .owner;
    if owner == x11rb::NONE {
        return None;
    }

    let req = conn.generate_id().ok()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        req,
        root,
        -1,
        -1,
        1,
        1,
        0,
        x11rb::protocol::xproto::WindowClass::INPUT_OUTPUT,
        conn.setup().roots[screen].root_visual,
        &x11rb::protocol::xproto::CreateWindowAux::new(),
    )
    .ok()?;
    conn.convert_selection(
        req,
        atoms.clipboard,
        atoms.utf8_string,
        atoms.data_prop,
        x11rb::CURRENT_TIME,
    )
    .ok()?;
    conn.flush().ok()?;

    // Wait for the SelectionNotify reply.
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut text = None;
    while Instant::now() < deadline {
        match conn.wait_for_event() {
            Ok(Event::SelectionNotify(notify)) if notify.requestor == req => {
                let reply = conn
                    .get_property(false, req, atoms.data_prop, AtomEnum::ANY, 0, u32::MAX)
                    .ok()
                    .and_then(|c| c.reply().ok());
                if let Some(reply) = reply {
                    if !reply.value.is_empty() {
                        text = Some(String::from_utf8_lossy(&reply.value).into_owned());
                    }
                }
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let _ = conn.destroy_window(req);
    let _ = conn.flush();
    text
}
