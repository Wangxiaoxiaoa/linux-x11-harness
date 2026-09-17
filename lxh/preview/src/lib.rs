use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use image::{imageops::FilterType, RgbaImage};
use lxh_core::LxhError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Gcontext, GetImageReply,
    ImageFormat, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperExt;
use x11rb::COPY_DEPTH_FROM_PARENT;

/// Preview window size is relative to the user's screen. The harness display
/// is scaled to fit within 1/8 of the user's screen dimensions while preserving
/// its aspect ratio.
const PREVIEW_SCREEN_FRACTION: u32 = 8;

/// Default window opacity for the preview. 0xB3FFFFFF is ~70% opaque
/// (~30% transparent). Requires a compositing window manager.
const PREVIEW_OPACITY: u32 = 0xB3FFFFFF;

/// A read-only preview window showing the contents of a harness Xvfb display
/// on the user's default DISPLAY. Closing the preview window only stops the
/// viewer; the underlying harness display keeps running.
pub struct PreviewWindow {
    stop: Arc<AtomicBool>,
}

impl Drop for PreviewWindow {
    fn drop(&mut self) {
        self.stop();
    }
}

impl PreviewWindow {
    /// Start a preview window. The returned handle can be used to stop it.
    pub fn start(
        target_display: &str,
        title: &str,
        refresh_interval: Duration,
    ) -> Result<Self, LxhError> {
        let target_display = target_display.to_string();
        let title = title.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);

        thread::spawn(move || {
            let _ = run_preview_loop(&target_display, &title, refresh_interval, stop2);
        });

        Ok(Self { stop })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run_preview_loop(
    target_display: &str,
    title: &str,
    refresh_interval: Duration,
    stop: Arc<AtomicBool>,
) -> Result<(), LxhError> {
    let (target_conn, target_screen) =
        RustConnection::connect(Some(target_display)).map_err(|e| {
            LxhError::DisplayUnavailable(format!("cannot connect to target display: {e}"))
        })?;
    let target_root = target_conn.setup().roots[target_screen].root;
    let target_geom = target_conn
        .get_geometry(target_root)
        .map_err(|e| LxhError::DisplayUnavailable(format!("get_geometry failed: {e}")))?
        .reply()
        .map_err(|e| LxhError::DisplayUnavailable(format!("get_geometry reply failed: {e}")))?;
    let width = target_geom.width as u32;
    let height = target_geom.height as u32;

    let user_display = std::env::var("DISPLAY")
        .ok()
        .unwrap_or_else(|| ":0".to_string());
    let (user_conn, user_screen) = RustConnection::connect(Some(&user_display)).map_err(|e| {
        LxhError::DisplayUnavailable(format!("cannot connect to user display: {e}"))
    })?;
    let user_root = user_conn.setup().roots[user_screen].root;
    let user_geom = user_conn
        .get_geometry(user_root)
        .map_err(|e| LxhError::DisplayUnavailable(format!("get_geometry user failed: {e}")))?
        .reply()
        .map_err(|e| {
            LxhError::DisplayUnavailable(format!("get_geometry user reply failed: {e}"))
        })?;
    let max_preview_w = user_geom.width as u32 / PREVIEW_SCREEN_FRACTION;
    let max_preview_h = user_geom.height as u32 / PREVIEW_SCREEN_FRACTION;
    let (target_w, target_h) = fit_inside(width, height, max_preview_w, max_preview_h);

    let screen = &user_conn.setup().roots[user_screen];
    let root = screen.root;
    let visual = screen.root_visual;
    let win_id = user_conn
        .generate_id()
        .map_err(|e| LxhError::InvalidArgument(format!("generate_id failed: {e}")))?;
    user_conn
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            win_id,
            root,
            0,
            0,
            target_w as u16,
            target_h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .event_mask(EventMask::EXPOSURE)
                .background_pixel(screen.black_pixel),
        )
        .map_err(|e| LxhError::InvalidArgument(format!("create_window failed: {e}")))?;

    set_window_title(&user_conn, win_id, title)?;
    set_window_opacity(&user_conn, win_id, PREVIEW_OPACITY)?;

    let gc = create_gc(&user_conn, win_id)?;

    user_conn
        .map_window(win_id)
        .map_err(|e| LxhError::InvalidArgument(format!("map_window failed: {e}")))?;
    user_conn
        .flush()
        .map_err(|e| LxhError::InvalidArgument(format!("flush failed: {e}")))?;

    while !stop.load(Ordering::Relaxed) {
        // Drain expose events to keep the connection responsive.
        while let Ok(Some(_)) = user_conn.poll_for_event() {}

        let image = match grab_screen(&target_conn, target_root, width, height) {
            Ok(img) => img,
            Err(_) => {
                thread::sleep(refresh_interval);
                continue;
            }
        };

        if image.depth != 24 || target_w == width && target_h == height {
            let _ = draw_image(&user_conn, win_id, width, height, &image, gc);
        } else {
            let scaled = match scale_image(&image.data, width, height, target_w, target_h) {
                Some(data) => data,
                None => {
                    thread::sleep(refresh_interval);
                    continue;
                }
            };
            let _ = draw_scaled_image(&user_conn, win_id, target_w, target_h, &scaled, gc);
        }

        thread::sleep(refresh_interval);
    }

    let _ = user_conn.destroy_window(win_id);
    let _ = user_conn.flush();
    Ok(())
}

fn grab_screen(
    conn: &RustConnection,
    root: Window,
    width: u32,
    height: u32,
) -> Result<GetImageReply, LxhError> {
    conn.get_image(
        ImageFormat::Z_PIXMAP,
        root,
        0,
        0,
        width as u16,
        height as u16,
        u32::MAX,
    )
    .map_err(|e| LxhError::InvalidArgument(format!("get_image failed: {e}")))?
    .reply()
    .map_err(|e| LxhError::InvalidArgument(format!("get_image reply failed: {e}")))
}

fn draw_image(
    conn: &RustConnection,
    win: Window,
    width: u32,
    height: u32,
    image: &GetImageReply,
    gc: Gcontext,
) -> Result<(), LxhError> {
    conn.put_image(
        ImageFormat::Z_PIXMAP,
        win,
        gc,
        width as u16,
        height as u16,
        0,
        0,
        0,
        image.depth,
        &image.data,
    )
    .map_err(|e| LxhError::InvalidArgument(format!("put_image failed: {e}")))?
    .check()
    .map_err(|e| LxhError::InvalidArgument(format!("put_image check failed: {e}")))?;
    conn.flush()
        .map_err(|e| LxhError::InvalidArgument(format!("flush failed: {e}")))
}

fn create_gc(conn: &RustConnection, win: Window) -> Result<Gcontext, LxhError> {
    let gc = conn
        .generate_id()
        .map_err(|e| LxhError::InvalidArgument(format!("generate_id failed: {e}")))?;
    conn.create_gc(
        gc,
        win,
        &CreateGCAux::new().foreground(0).background(u32::MAX),
    )
    .map_err(|e| LxhError::InvalidArgument(format!("create_gc failed: {e}")))?
    .check()
    .map_err(|e| LxhError::InvalidArgument(format!("create_gc check failed: {e}")))?;
    Ok(gc)
}

fn set_window_title(conn: &RustConnection, win: Window, title: &str) -> Result<(), LxhError> {
    WrapperExt::change_property8(
        conn,
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .map_err(|e| LxhError::InvalidArgument(format!("change_property failed: {e}")))?
    .check()
    .map_err(|e| LxhError::InvalidArgument(format!("change_property check failed: {e}")))
}

fn set_window_opacity(conn: &RustConnection, win: Window, opacity: u32) -> Result<(), LxhError> {
    let atom = conn
        .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")
        .map_err(|e| LxhError::InvalidArgument(format!("intern_atom failed: {e}")))?
        .reply()
        .map_err(|e| LxhError::InvalidArgument(format!("intern_atom reply failed: {e}")))?
        .atom;
    conn.change_property32(PropMode::REPLACE, win, atom, AtomEnum::CARDINAL, &[opacity])
        .map_err(|e| LxhError::InvalidArgument(format!("change_property32 failed: {e}")))?
        .check()
        .map_err(|e| LxhError::InvalidArgument(format!("change_property32 check failed: {e}")))
}

fn fit_inside(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale_w = max_w as f64 / src_w as f64;
    let scale_h = max_h as f64 / src_h as f64;
    let scale = scale_w.min(scale_h).min(1.0);
    ((src_w as f64 * scale) as u32, (src_h as f64 * scale) as u32)
}

fn scale_image(bgra: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Option<Vec<u8>> {
    let mut rgba = vec![0u8; (src_w * src_h * 4) as usize];
    for (src_chunk, dst_chunk) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        dst_chunk[0] = src_chunk[2];
        dst_chunk[1] = src_chunk[1];
        dst_chunk[2] = src_chunk[0];
        dst_chunk[3] = src_chunk[3];
    }
    let src = RgbaImage::from_raw(src_w, src_h, rgba)?;
    let dst = image::imageops::resize(&src, dst_w, dst_h, FilterType::Triangle);
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    for (src_chunk, dst_chunk) in dst.as_raw().chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        dst_chunk[0] = src_chunk[2];
        dst_chunk[1] = src_chunk[1];
        dst_chunk[2] = src_chunk[0];
        dst_chunk[3] = src_chunk[3];
    }
    Some(out)
}

fn draw_scaled_image(
    conn: &RustConnection,
    win: Window,
    width: u32,
    height: u32,
    data: &[u8],
    gc: Gcontext,
) -> Result<(), LxhError> {
    conn.put_image(
        ImageFormat::Z_PIXMAP,
        win,
        gc,
        width as u16,
        height as u16,
        0,
        0,
        0,
        24,
        data,
    )
    .map_err(|e| LxhError::InvalidArgument(format!("put_image failed: {e}")))?
    .check()
    .map_err(|e| LxhError::InvalidArgument(format!("put_image check failed: {e}")))?;
    conn.flush()
        .map_err(|e| LxhError::InvalidArgument(format!("flush failed: {e}")))
}
