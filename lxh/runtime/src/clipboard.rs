//! Shared clipboard: one canonical value in the hub, mirrored between the
//! user's desktop and every harness display, with per-display direction
//! policies.
//!
//! ```text
//!            canonical { text, fingerprint, version }     (hub)
//!             ↑ arboard persistent instance   ↑ x11rb selection owner
//!      desktop bridge (:0)          sandbox spoke (one per display)
//! ```
//!
//! Policies: `ToSandbox` (default — desktop changes flow in, sandbox
//! changes never touch the desktop), `Bidirectional`, `Off`. See
//! `docs/clipboard-sync.md`.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Content larger than this is truncated before entering the hub.
const MAX_CONTENT_BYTES: usize = 1024 * 1024;
/// Sandbox spoke cycle: wakes on hub changes, drains X events.
const SPOKE_TICK: Duration = Duration::from_millis(50);
/// Safety-net poll of the sandbox clipboard (bidirectional policy).
const LOCAL_POLL: Duration = Duration::from_secs(1);
/// How often the desktop bridge re-reads the desktop clipboard.
const DESKTOP_POLL: Duration = Duration::from_millis(500);
/// Conversion-request reply timeout before a requestor window is reaped.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Direction policy for one display's clipboard sync.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    /// Not part of the shared clipboard.
    Off,
    /// Desktop changes flow in; sandbox changes never reach the desktop.
    ToSandbox,
    /// Full VM-style sharing.
    Bidirectional,
}

impl Policy {
    pub fn parse(s: &str) -> Option<Policy> {
        match s {
            "off" => Some(Policy::Off),
            "to_sandbox" => Some(Policy::ToSandbox),
            "bidirectional" => Some(Policy::Bidirectional),
            _ => None,
        }
    }

    fn from_u8(v: u8) -> Policy {
        match v {
            1 => Policy::ToSandbox,
            2 => Policy::Bidirectional,
            _ => Policy::Off,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Policy::Off => 0,
            Policy::ToSandbox => 1,
            Policy::Bidirectional => 2,
        }
    }
}

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

struct SpokeHandle {
    stop: Arc<AtomicBool>,
    policy: AtomicU8,
}

#[derive(Clone)]
struct Canonical {
    text: String,
    fp: u64,
    version: u64,
}

struct HubInner {
    canonical: Canonical,
    /// display string -> spoke handle (one spoke per X display).
    spokes: HashMap<String, SpokeHandle>,
    desktop_stop: Option<Arc<AtomicBool>>,
}

/// The shared clipboard. One instance per process (the desktop bridge
/// holds the desktop side).
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
                },
                spokes: HashMap::new(),
                desktop_stop: None,
            }),
            changed: Condvar::new(),
        }
    }

    /// Current shared content.
    pub fn get(&self) -> String {
        self.inner.lock().unwrap().canonical.text.clone()
    }

    fn snapshot_fp(&self) -> u64 {
        self.inner.lock().unwrap().canonical.fp
    }

    pub fn version(&self) -> u64 {
        self.inner.lock().unwrap().canonical.version
    }

    fn policy_of(&self, display: &str) -> Option<u8> {
        self.inner
            .lock()
            .unwrap()
            .spokes
            .get(display)
            .map(|h| h.policy.load(Ordering::Relaxed))
    }

    /// Replace the shared content; every spoke pushes it to its display
    /// and the desktop bridge writes it to the desktop.
    pub fn set(&self, text: &str) {
        self.update(text.to_string());
    }

    fn update(&self, text: String) -> bool {
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
        };
        self.changed.notify_all();
        true
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

    /// Start the desktop bridge (lazily, on the first spoke).
    fn ensure_desktop_bridge(self: &Arc<Self>) {
        let mut inner = self.inner.lock().unwrap();
        if inner.desktop_stop.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        inner.desktop_stop = Some(Arc::clone(&stop));
        let hub = Arc::clone(self);
        std::thread::spawn(move || run_desktop_bridge(hub, stop));
    }

    /// Start a spoke for `display`, or update its policy if one already
    /// exists (the daemon's explicit policy overrides the SDK default).
    pub fn start_spoke(self: &Arc<Self>, display: &str, policy: Policy) {
        if policy == Policy::Off {
            self.stop(display);
            return;
        }
        self.ensure_desktop_bridge();
        let mut inner = self.inner.lock().unwrap();
        if let Some(handle) = inner.spokes.get_mut(display) {
            handle.policy.store(policy.to_u8(), Ordering::Relaxed);
            self.changed.notify_all();
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        inner.spokes.insert(
            display.to_string(),
            SpokeHandle {
                stop: Arc::clone(&stop),
                policy: AtomicU8::new(policy.to_u8()),
            },
        );
        let hub = Arc::clone(self);
        let display = display.to_string();
        std::thread::spawn(move || run_spoke(hub, display, stop));
    }

    /// Change a spoke's direction policy live.
    pub fn set_policy(&self, display: &str, policy: Policy) {
        if let Some(handle) = self.inner.lock().unwrap().spokes.get(display) {
            handle.policy.store(policy.to_u8(), Ordering::Relaxed);
            self.changed.notify_all();
        }
    }

    /// Stop the spoke of a destroyed display.
    pub fn stop(&self, display: &str) {
        if let Some(handle) = self.inner.lock().unwrap().spokes.remove(display) {
            handle.stop.store(true, Ordering::Relaxed);
            self.changed.notify_all();
        }
    }

    /// Stop every thread (daemon shutdown).
    pub fn stop_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        for (_, handle) in inner.spokes.drain() {
            handle.stop.store(true, Ordering::Relaxed);
        }
        if let Some(stop) = inner.desktop_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.changed.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Desktop bridge (:0) — arboard owns the desktop side.
// ---------------------------------------------------------------------------

fn run_desktop_bridge(hub: Arc<ClipboardHub>, stop: Arc<AtomicBool>) {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            return; // headless: nothing to bridge
        }
    };

    // The user's current clipboard is the initial canonical value.
    if let Ok(text) = clipboard.get_text() {
        hub.update(text);
    }
    let mut last_desktop_fp = hub.snapshot_fp();

    while !stop.load(Ordering::Relaxed) {
        hub.wait_for_change(DESKTOP_POLL);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let snapshot_fp = hub.snapshot_fp();
        let snapshot_text = hub.get();
        if snapshot_fp != last_desktop_fp {
            // Canonical changed (agent set or a bidirectional sandbox):
            // push it to the desktop.
            if clipboard.set_text(&snapshot_text).is_ok() {
                last_desktop_fp = snapshot_fp;
            }
        } else if let Ok(text) = clipboard.get_text() {
            // Detect a user-side copy.
            let fp = fingerprint(&text);
            if fp != last_desktop_fp {
                last_desktop_fp = fp;
                hub.update(text);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sandbox spoke (x11rb): owns the CLIPBOARD selection on its display.
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

fn run_spoke(hub: Arc<ClipboardHub>, display: String, stop: Arc<AtomicBool>) {
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

    let mut last_version = 0u64;
    let mut need_refresh = false; // canonical changed; (re)take ownership
    let mut owned = false;
    let mut last_local_poll = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let policy = Policy::from_u8(hub.policy_of(&display).unwrap_or(Policy::ToSandbox.to_u8()));

        if hub.version() != last_version {
            need_refresh = true;
            last_version = hub.version();
        }

        // Drain X events: serve paste requests; watch for takeovers.
        while let Ok(Some(event)) = conn.poll_for_event() {
            match event {
                // A sandbox app wants the clipboard: serve canonical text.
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
                        let text = hub.get();
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
                // We lost ownership: an app inside the sandbox copied. The
                // new content is picked up by the periodic local read below.
                Event::SelectionClear(clear) if clear.selection == atoms.clipboard => {
                    owned = false;
                    last_local_poll = Instant::now() - LOCAL_POLL;
                }
                _ => {}
            }
        }

        // Ownership refresh: when canonical changed, retake ownership so
        // sandbox apps paste the shared content. Under Bidirectional the
        // display's own content is read and reported first so nothing is
        // lost; under ToSandbox it is discarded by design.
        if need_refresh {
            if policy == Policy::Bidirectional && !owned {
                if let Some(local) = read_display_clipboard(&display) {
                    hub.update(local);
                }
            }
            let _ = conn.set_selection_owner(win, atoms.clipboard, x11rb::CURRENT_TIME);
            let _ = conn.flush();
            owned = true;
            need_refresh = false;
        }

        // Bidirectional safety net: an app may copy while we own the
        // selection (SelectionClear covers it) or while we do not (this
        // poll covers it). Read via a fresh connection — the proven path.
        if policy == Policy::Bidirectional && !owned && last_local_poll.elapsed() >= LOCAL_POLL {
            last_local_poll = Instant::now();
            if let Some(local) = read_display_clipboard(&display) {
                hub.update(local);
            }
        }

        hub.wait_for_change(SPOKE_TICK);
    }

    let _ = conn.destroy_window(win);
    let _ = conn.flush();
}

/// Read the CLIPBOARD selection of one display with a fresh connection.
/// Used by `lxh_clipboard_get` for a specific display.
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
