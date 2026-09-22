use async_trait::async_trait;
use image::{ImageEncoder, RgbImage};
use lxh_core::{
    x11, A11yDriver, Bounds, CaptureDriver, DesktopOverview, GetWindowStateResult, LxhError,
    ProcessEntry, Screenshot, WindowEntry,
};
use tokio::task;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

pub mod atspi;

pub struct X11Capture {
    display: String,
}

impl X11Capture {
    pub fn new(display: &str) -> Result<Self, LxhError> {
        let _ = x11::open_connection(display)?;
        Ok(Self {
            display: display.to_string(),
        })
    }
}

#[async_trait]
impl CaptureDriver for X11Capture {
    async fn screenshot(&self) -> Result<Screenshot, LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = conn.setup().roots[screen].root;
            let w = conn.setup().roots[screen].width_in_pixels;
            let h = conn.setup().roots[screen].height_in_pixels;
            capture_rect(&conn, root, 0, 0, w, h)
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn screenshot_window(&self, window_id: u32) -> Result<Screenshot, LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, _) = x11::open_connection(&display)?;
            let geom = conn
                .get_geometry(window_id)
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?;
            capture_rect(&conn, window_id, 0, 0, geom.width, geom.height)
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }
}

fn capture_rect(
    conn: &RustConnection,
    root: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
) -> Result<Screenshot, LxhError> {
    let reply = conn
        .get_image(
            x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
            root,
            x,
            y,
            w,
            h,
            u32::MAX,
        )
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?;

    let data = reply.data;
    let stride = data.len() / h as usize;
    let mut buf = vec![0u8; (w as u32 * h as u32 * 3) as usize];

    for row in 0..h as usize {
        for col in 0..w as usize {
            let offset = row * stride + col * 4;
            let pixel = u32::from_ne_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let idx = (row * w as usize + col) * 3;
            buf[idx] = ((pixel >> 16) & 0xff) as u8;
            buf[idx + 1] = ((pixel >> 8) & 0xff) as u8;
            buf[idx + 2] = (pixel & 0xff) as u8;
        }
    }

    let img = RgbImage::from_raw(w as u32, h as u32, buf)
        .ok_or_else(|| LxhError::InvalidArgument("failed to create image buffer".into()))?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&img, w as u32, h as u32, image::ExtendedColorType::Rgb8)
        .map_err(|e| LxhError::InvalidArgument(e.to_string()))?;

    Ok(Screenshot { data: png })
}

pub struct AtspiA11y {
    display: String,
}

impl AtspiA11y {
    pub fn new(display: &str) -> Result<Self, LxhError> {
        let _ = x11::open_connection(display)?;
        Ok(Self {
            display: display.to_string(),
        })
    }
}

#[async_trait]
impl A11yDriver for AtspiA11y {
    async fn get_window_state(
        &self,
        pid: u32,
        window_id: u32,
        include_tree: bool,
        include_screenshot: bool,
    ) -> Result<GetWindowStateResult, LxhError> {
        let display = self.display.clone();
        let tree_future = async {
            if include_tree {
                let walked = atspi::walk_tree(pid).await?;
                Ok(Some(atspi::accessibility_tree(&walked)))
            } else {
                Ok(None)
            }
        };

        let (state, tree) = tokio::join!(
            task::spawn_blocking(move || get_window_state_sync(
                &display,
                window_id,
                pid,
                include_screenshot
            )),
            tree_future
        );

        let (title, app_name, bounds, screenshot) =
            state.map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))??;

        Ok(GetWindowStateResult {
            window_id,
            title,
            app_name,
            bounds,
            tree: tree?,
            screenshot,
        })
    }

    async fn get_desktop_overview(&self) -> Result<DesktopOverview, LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = conn.setup().roots[screen].root;

            let net_wm_name = conn
                .intern_atom(false, b"_NET_WM_NAME")
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?
                .atom;
            let utf8 = conn
                .intern_atom(false, b"UTF8_STRING")
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?
                .atom;

            let tree_reply = conn
                .query_tree(root)
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?;

            // Exited applications can keep their windows alive in X11 as
            // zombies; never return stale targets to callers.
            let processes = list_processes();

            let mut windows = Vec::new();
            for (z_index, &window) in tree_reply.children.iter().enumerate() {
                let pid = pid_of_window(&conn, window).ok();
                if let Some(pid) = pid {
                    if !processes.iter().any(|p| p.pid == pid) {
                        continue;
                    }
                }

                let title = if let Ok(cookie) =
                    conn.get_property(false, window, net_wm_name, utf8, 0, 1024)
                {
                    cookie.reply().ok().and_then(|r| {
                        if r.value.is_empty() {
                            None
                        } else {
                            String::from_utf8(r.value).ok()
                        }
                    })
                } else {
                    None
                };

                let on_screen = conn
                    .get_window_attributes(window)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .is_some_and(|a| a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE);

                let bounds = conn
                    .get_geometry(window)
                    .map_err(x11::xerr)?
                    .reply()
                    .ok()
                    .map(|g| Bounds {
                        x: g.x as i32,
                        y: g.y as i32,
                        w: g.width as u32,
                        h: g.height as u32,
                    });

                windows.push(WindowEntry {
                    id: window,
                    pid,
                    title,
                    bounds,
                    z_index,
                    on_screen,
                });
            }

            Ok(DesktopOverview { processes, windows })
        })
        .await
        .map_err(|e| LxhError::ProcessSpawnFailed(e.to_string()))?
    }

    async fn set_value(&self, pid: u32, index: usize, value: &str) -> Result<(), LxhError> {
        atspi::set_value(pid, index, value).await
    }

    async fn element_frame(&self, pid: u32, index: usize) -> Result<Bounds, LxhError> {
        let elements = atspi::walk_tree(pid).await?;
        elements
            .into_iter()
            .find(|e| e.index == index)
            .and_then(|e| e.frame)
            .ok_or_else(|| {
                LxhError::InvalidArgument(format!("element {index} not found for pid {pid}"))
            })
    }
}

#[allow(clippy::type_complexity)]
fn get_window_state_sync(
    display: &str,
    window_id: u32,
    pid: u32,
    include_screenshot: bool,
) -> Result<(Option<String>, Option<String>, Bounds, Option<Screenshot>), LxhError> {
    let (conn, _screen) = x11::open_connection(display)?;

    let net_name = conn
        .intern_atom(false, b"_NET_WM_NAME")
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?
        .atom;
    let utf8 = conn
        .intern_atom(false, b"UTF8_STRING")
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?
        .atom;

    let title = conn
        .get_property(false, window_id, net_name, utf8, 0, 1024)
        .map_err(x11::xerr)?
        .reply()
        .ok()
        .and_then(|r| {
            if r.value.is_empty() {
                None
            } else {
                String::from_utf8(r.value).ok()
            }
        });

    let geom = conn
        .get_geometry(window_id)
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?;
    let bounds = Bounds {
        x: geom.x as i32,
        y: geom.y as i32,
        w: geom.width as u32,
        h: geom.height as u32,
    };

    let screenshot = if include_screenshot {
        Some(capture_rect(
            &conn,
            window_id,
            0,
            0,
            geom.width,
            geom.height,
        )?)
    } else {
        None
    };

    let app_name = read_process_name(pid);

    Ok((title, app_name, bounds, screenshot))
}

fn read_process_name(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{}/comm", pid))
        .ok()
        .map(|s| s.trim().to_string())
}

fn list_processes() -> Vec<ProcessEntry> {
    let mut processes = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                if let Ok(pid) = name.parse::<u32>() {
                    if let Some(proc_name) = read_process_name(pid) {
                        processes.push(ProcessEntry {
                            pid,
                            name: proc_name,
                        });
                    }
                }
            }
        }
    }
    processes.sort_by_key(|p| p.pid);
    processes
}

fn pid_of_window(conn: &RustConnection, window: u32) -> Result<u32, LxhError> {
    let atom = conn
        .intern_atom(false, b"_NET_WM_PID")
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?
        .atom;
    let card = conn
        .intern_atom(false, b"CARDINAL")
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?
        .atom;
    let reply = conn
        .get_property(false, window, atom, card, 0, 1)
        .map_err(x11::xerr)?
        .reply()
        .map_err(x11::xerr)?;
    if reply.format == 32 && reply.value.len() >= 4 {
        Ok(u32::from_ne_bytes([
            reply.value[0],
            reply.value[1],
            reply.value[2],
            reply.value[3],
        ]))
    } else {
        Err(LxhError::InvalidArgument("no _NET_WM_PID".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::atspi::{accessibility_tree, Element};
    use lxh_core::Bounds;

    #[test]
    fn accessibility_tree_maps_elements() {
        let elements = vec![
            Element {
                index: 0,
                role: "frame".into(),
                name: Some("window".into()),
                frame: Some(Bounds {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 100,
                }),
                actions: vec!["click".into()],
                parent_index: None,
                depth: 0,
            },
            Element {
                index: 1,
                role: "button".into(),
                name: Some("ok".into()),
                frame: None,
                actions: vec![],
                parent_index: Some(0),
                depth: 1,
            },
        ];

        let tree = accessibility_tree(&elements);
        assert_eq!(tree.elements.len(), 2);
        assert_eq!(tree.elements[0].index, 0);
        assert_eq!(tree.elements[0].role, "frame");
        assert_eq!(tree.elements[1].name, Some("ok".into()));
        assert_eq!(tree.elements[1].parent_index, Some(0));
        assert_eq!(tree.elements[1].depth, 1);
    }
}
