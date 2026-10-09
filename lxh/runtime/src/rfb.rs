//! Minimal RFB (VNC) 3.8 protocol client for the interactive preview
//! window. Connects to the per-display x11vnc server, receives
//! framebuffer updates, and sends pointer/key events. No third-party
//! dependencies — pure `std::net::TcpStream`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Pixel format we request: 32bpp BGRA little-endian (matches X11 ZPixmap).
#[derive(Clone, Debug)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    fn bgra32() -> Self {
        Self {
            bits_per_pixel: 32,
            depth: 24,
            big_endian: false,
            true_colour: true,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }
    }

    fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0] = self.bits_per_pixel;
        b[1] = self.depth;
        b[2] = self.big_endian as u8;
        b[3] = self.true_colour as u8;
        b[4..6].copy_from_slice(&self.red_max.to_be_bytes());
        b[6..8].copy_from_slice(&self.green_max.to_be_bytes());
        b[8..10].copy_from_slice(&self.blue_max.to_be_bytes());
        b[10] = self.red_shift;
        b[11] = self.green_shift;
        b[12] = self.blue_shift;
        b[13..16].copy_from_slice(&[0, 0, 0]);
        b
    }
}

/// The remote framebuffer state received from the server.
pub struct Framebuffer {
    pub width: u16,
    pub height: u16,
    /// BGRA pixels (row-major).
    pub pixels: Vec<u8>,
}

/// A connected RFB client session.
pub struct RfbClient {
    stream: Mutex<TcpStream>,
    pub framebuffer: Arc<Mutex<Framebuffer>>,
    stop: Arc<AtomicBool>,
    update_thread: Option<std::thread::JoinHandle<()>>,
}

impl RfbClient {
    /// Connect to an RFB server (e.g. x11vnc on localhost:5901),
    /// perform the 3.8 handshake, and start the framebuffer update loop.
    pub fn connect(addr: &str) -> Result<Self, String> {
        let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;

        // ── ProtocolVersion handshake ──
        let mut server_ver = [0u8; 12];
        stream
            .read_exact(&mut server_ver)
            .map_err(|e| e.to_string())?;
        assert!(server_ver.starts_with(b"RFB "), "not RFB");
        stream
            .write_all(b"RFB 003.008\n")
            .map_err(|e| e.to_string())?;

        // ── Security (None) ──
        let types_len = read_u8(&mut stream)?;
        let mut types = vec![0u8; types_len as usize];
        stream.read_exact(&mut types).map_err(|e| e.to_string())?;
        if !types.contains(&1) {
            return Err("server does not support None auth".into());
        }
        stream
            .write_all(&[1]) // None (RFB 3.8: u8, not u32)
            .map_err(|e| e.to_string())?;
        let result = read_u32(&mut stream)?;
        if result != 0 {
            return Err("auth failed".into());
        }

        // ── ClientInit ──
        stream.write_all(&[1]).map_err(|e| e.to_string())?; // shared

        // ── ServerInit ──
        let mut init = [0u8; 24];
        stream.read_exact(&mut init).map_err(|e| e.to_string())?;
        let width = u16::from_be_bytes([init[0], init[1]]);
        let height = u16::from_be_bytes([init[2], init[3]]);
        // skip pixel format (bytes 4-19)
        let name_len = u32::from_be_bytes([init[20], init[21], init[22], init[23]]);
        let mut name = vec![0u8; name_len as usize];
        stream.read_exact(&mut name).map_err(|e| e.to_string())?;
        let _name = String::from_utf8_lossy(&name).into_owned();

        // ── SetPixelFormat (BGRA 32bpp) ──
        let pf = PixelFormat::bgra32();
        let mut msg = vec![0u8]; // message type 0
        msg.extend_from_slice(&[0, 0, 0]); // padding
        msg.extend_from_slice(&pf.to_bytes());
        stream.write_all(&msg).map_err(|e| e.to_string())?;

        // ── SetEncodings (Raw only) ──
        // u8 type + u8 padding + u16 count + i32 encoding = 8 bytes
        let enc: [u8; 8] = [2, 0, 0, 1, 0, 0, 0, 0];
        stream.write_all(&enc).map_err(|e| e.to_string())?;

        // ── FramebufferUpdateRequest ──
        let mut fur = vec![3u8]; // message type 3
        fur.extend_from_slice(&[0]); // incremental = false (full first)
        fur.extend_from_slice(&0u16.to_be_bytes()); // x
        fur.extend_from_slice(&0u16.to_be_bytes()); // y
        fur.extend_from_slice(&width.to_be_bytes());
        fur.extend_from_slice(&height.to_be_bytes());
        stream.write_all(&fur).map_err(|e| e.to_string())?;

        let framebuffer = Arc::new(Mutex::new(Framebuffer {
            width,
            height,
            pixels: vec![0u8; (width as usize) * (height as usize) * 4],
        }));

        let stop = Arc::new(AtomicBool::new(false));
        let stream_clone = stream.try_clone().map_err(|e| e.to_string())?;

        let fb = Arc::clone(&framebuffer);
        let stop2 = Arc::clone(&stop);

        let update_thread = std::thread::spawn(move || {
            let mut stream = stream_clone;
            loop {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                // Request incremental update
                let mut req = vec![3u8];
                req.extend_from_slice(&[1]); // incremental
                req.extend_from_slice(&0u16.to_be_bytes());
                req.extend_from_slice(&0u16.to_be_bytes());
                req.extend_from_slice(&width_u16(&fb));
                req.extend_from_slice(&height_u16(&fb));
                if stream.write_all(&req).is_err() {
                    break;
                }
                // Read FramebufferUpdate
                if !read_framebuffer_update(&mut stream, &fb) {
                    break;
                }
            }
        });

        Ok(Self {
            stream: Mutex::new(stream.try_clone().map_err(|e| e.to_string())?),
            framebuffer,
            stop,
            update_thread: Some(update_thread),
        })
    }

    /// Send a pointer event (button mask + position).
    pub fn send_pointer(&self, button_mask: u8, x: u16, y: u16) -> Result<(), String> {
        let mut msg = vec![5u8]; // message type 5
        msg.push(button_mask);
        msg.extend_from_slice(&x.to_be_bytes());
        msg.extend_from_slice(&y.to_be_bytes());
        self.stream
            .lock()
            .unwrap()
            .write_all(&msg)
            .map_err(|e| e.to_string())?;
        self.stream
            .lock()
            .unwrap()
            .flush()
            .map_err(|e| e.to_string())
    }

    /// Send a key event.
    pub fn send_key(&self, keysym: u32, down: bool) -> Result<(), String> {
        let mut msg = vec![4u8]; // message type 4
        msg.push(down as u8);
        msg.extend_from_slice(&[0, 0]); // padding
        msg.extend_from_slice(&keysym.to_be_bytes());
        self.stream
            .lock()
            .unwrap()
            .write_all(&msg)
            .map_err(|e| e.to_string())?;
        self.stream
            .lock()
            .unwrap()
            .flush()
            .map_err(|e| e.to_string())
    }

    /// Stop the update loop and close the connection.
    pub fn disconnect(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.update_thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for RfbClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}

// ── helpers ──

fn width_u16(fb: &Mutex<Framebuffer>) -> [u8; 2] {
    fb.lock().unwrap().width.to_be_bytes()
}

fn height_u16(fb: &Mutex<Framebuffer>) -> [u8; 2] {
    fb.lock().unwrap().height.to_be_bytes()
}

fn read_u8(s: &mut TcpStream) -> Result<u8, String> {
    let mut b = [0u8];
    s.read_exact(&mut b).map_err(|e| e.to_string())?;
    Ok(b[0])
}

fn read_u32(s: &mut TcpStream) -> Result<u32, String> {
    let mut b = [0u8; 4];
    s.read_exact(&mut b).map_err(|e| e.to_string())?;
    Ok(u32::from_be_bytes(b))
}

/// Read and apply a FramebufferUpdate message.
fn read_framebuffer_update(stream: &mut TcpStream, fb: &Mutex<Framebuffer>) -> bool {
    // message type + padding
    let mut header = [0u8; 2];
    if stream.read_exact(&mut header).is_err() {
        return false;
    }
    // number of rectangles
    let mut nrect_buf = [0u8; 2];
    if stream.read_exact(&mut nrect_buf).is_err() {
        return false;
    }
    let nrect = u16::from_be_bytes(nrect_buf);

    for _ in 0..nrect {
        // rect header: x(2) y(2) w(2) h(2) encoding(4) = 12 bytes
        let mut rh = [0u8; 12];
        if stream.read_exact(&mut rh).is_err() {
            return false;
        }
        let x = u16::from_be_bytes([rh[0], rh[1]]) as usize;
        let y = u16::from_be_bytes([rh[2], rh[3]]) as usize;
        let w = u16::from_be_bytes([rh[4], rh[5]]) as usize;
        let h = u16::from_be_bytes([rh[6], rh[7]]) as usize;
        let encoding = i32::from_be_bytes([rh[8], rh[9], rh[10], rh[11]]);

        if encoding == 0 {
            // Raw encoding: w * h * 4 bytes BGRA
            let mut data = vec![0u8; w * h * 4];
            if stream.read_exact(&mut data).is_err() {
                return false;
            }
            {
                let mut fb = fb.lock().unwrap();
                let stride = fb.width as usize;
                for row in 0..h {
                    let dst_y = y + row;
                    if dst_y >= fb.height as usize {
                        break;
                    }
                    let src = &data[row * w * 4..(row + 1) * w * 4];
                    for col in 0..w {
                        let dx = x + col;
                        if dx >= stride {
                            break;
                        }
                        let offset = (dst_y * stride + dx) * 4;
                        fb.pixels[offset..offset + 4].copy_from_slice(&src[col * 4..col * 4 + 4]);
                    }
                }
            }
        } else {
            // Unsupported encoding: skip (read available data is not
            // possible without knowing the size; disconnect).
            return false;
        }
    }
    true
}
