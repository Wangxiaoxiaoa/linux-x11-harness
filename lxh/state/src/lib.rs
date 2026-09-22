use async_trait::async_trait;
use image::{ImageEncoder, RgbImage};
use lxh_core::{
    x11, A11yDriver, Bounds, CaptureDriver, DesktopOverview, GetWindowStateResult, LxhError,
    ProcessEntry, RegionCapture, Screenshot, WindowEntry,
};
use tokio::task;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
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

    async fn capture_region(
        &self,
        window_id: u32,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> Result<RegionCapture, LxhError> {
        let display = self.display.clone();
        task::spawn_blocking(move || {
            let (conn, screen) = x11::open_connection(&display)?;
            let root = conn.setup().roots[screen].root;

            let geom = conn
                .get_geometry(window_id)
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?;
            let (win_w, win_h) = (f64::from(geom.width), f64::from(geom.height));

            // Window's absolute position (it is a child of the WM frame).
            let win_origin = conn
                .translate_coordinates(window_id, root, 0, 0)
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?;
            let (win_x, win_y) = (f64::from(win_origin.dst_x), f64::from(win_origin.dst_y));

            // Region is in display coordinates (the same space as a11y
            // frames and input coordinates); normalize, pad by 20%, then
            // clamp to the window.
            let (x1, x2) = (x1.min(x2), x1.max(x2));
            let (y1, y2) = (y1.min(y2), y1.max(y2));
            let pad_x = (x2 - x1) * 0.2;
            let pad_y = (y2 - y1) * 0.2;
            let mut lx1 = (x1 - win_x - pad_x).clamp(0.0, win_w);
            let mut ly1 = (y1 - win_y - pad_y).clamp(0.0, win_h);
            let lx2 = (x2 - win_x + pad_x).clamp(0.0, win_w);
            let ly2 = (y2 - win_y + pad_y).clamp(0.0, win_h);
            // Keep the crop inside the drawable even for degenerate regions.
            lx1 = lx1.min((win_w - 1.0).max(0.0));
            ly1 = ly1.min((win_h - 1.0).max(0.0));
            let crop_w = (lx2 - lx1).max(1.0).min(win_w - lx1);
            let crop_h = (ly2 - ly1).max(1.0).min(win_h - ly1);

            // Display-absolute origin of the crop.
            let display_x = win_x + lx1;
            let display_y = win_y + ly1;

            // Scale down so the output is at most 500 px wide; never upscale.
            let scale = (500.0 / crop_w).min(1.0);
            let out_w = (crop_w * scale).round().max(1.0) as u32;
            let out_h = (crop_h * scale).round().max(1.0) as u32;

            let mut shot = capture_rect(
                &conn,
                window_id,
                lx1 as i16,
                ly1 as i16,
                crop_w as u16,
                crop_h as u16,
            )?;

            if scale < 1.0 {
                let decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(&shot.data))
                    .map_err(|e| LxhError::InvalidArgument(e.to_string()))?;
                use image::ImageDecoder;
                let (out_cols, out_rows) = decoder.dimensions();
                let mut buf = vec![0u8; decoder.total_bytes() as usize];
                decoder
                    .read_image(&mut buf)
                    .map_err(|e| LxhError::InvalidArgument(e.to_string()))?;
                let img = image::RgbImage::from_raw(out_cols, out_rows, buf)
                    .ok_or_else(|| LxhError::InvalidArgument("failed to decode capture".into()))?;
                let resized = image::imageops::resize(
                    &img,
                    out_w,
                    out_h,
                    image::imageops::FilterType::Triangle,
                );
                let mut png = Vec::new();
                image::codecs::png::PngEncoder::new(&mut png)
                    .write_image(&resized, out_w, out_h, image::ExtendedColorType::Rgb8)
                    .map_err(|e| LxhError::InvalidArgument(e.to_string()))?;
                shot = Screenshot { data: png };
            }

            Ok(RegionCapture {
                screenshot: shot,
                display_x: display_x as i32,
                display_y: display_y as i32,
                scale: out_w as f64 / crop_w,
            })
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

            // Managed windows in stacking order (bottom to top), maintained
            // by the WM. Tree-walking the root would also surface helper
            // windows (e.g. GTK CSD shadows) that are not real app windows.
            let client_list = conn
                .intern_atom(false, b"_NET_CLIENT_LIST_STACKING")
                .map_err(x11::xerr)?
                .reply()
                .map_err(x11::xerr)?
                .atom;
            let client_windows = conn
                .get_property(false, root, client_list, AtomEnum::WINDOW, 0, 4096)
                .map_err(x11::xerr)?
                .reply()
                .ok()
                .map(|r| {
                    r.value
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| u32::from_ne_bytes(*b))
                        .collect::<Vec<u32>>()
                })
                .unwrap_or_default();

            // Exited applications can keep their windows alive in X11 as
            // zombies; never return stale targets to callers.
            let processes = list_processes();

            let mut windows = Vec::new();
            for (z_index, &window) in client_windows.iter().enumerate() {
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

                // Display-absolute geometry (client geometry is relative to
                // the WM frame).
                let translated = conn
                    .translate_coordinates(window, root, 0, 0)
                    .map_err(x11::xerr)?
                    .reply()
                    .map_err(x11::xerr)?;
                let geom = conn
                    .get_geometry(window)
                    .map_err(x11::xerr)?
                    .reply()
                    .map_err(x11::xerr)?;
                let bounds = Bounds {
                    x: translated.dst_x as i32,
                    y: translated.dst_y as i32,
                    w: geom.width as u32,
                    h: geom.height as u32,
                };

                windows.push(WindowEntry {
                    id: window,
                    pid,
                    title,
                    bounds: Some(bounds),
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
