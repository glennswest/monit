//! Linux framebuffer surface with a double buffer plus simple drawing
//! primitives (rects, text, bars). Pixels are packed 0x00RRGGBB which, on a
//! little-endian 32bpp XRGB framebuffer, lands in memory as B,G,R,X.

use crate::font::Font;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::os::unix::io::AsRawFd;

// ioctl request number; the `request` arg type differs by libc (c_int on musl,
// c_ulong on glibc), so cast with `as _` at the call site.
const KDSETMODE: libc::c_int = 0x4B3A;
const KD_TEXT: libc::c_int = 0x00;
const KD_GRAPHICS: libc::c_int = 0x01;

pub type Color = u32;

pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

pub struct Fb {
    pub w: usize,
    pub h: usize,
    stride: usize, // bytes per scanline
    dev: File,
    tty: Option<File>,
    buf: Vec<u32>,
}

fn read_sysfs(path: &str) -> String {
    let mut s = String::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    s.trim().to_string()
}

impl Fb {
    pub fn open() -> std::io::Result<Fb> {
        let size = read_sysfs("/sys/class/graphics/fb0/virtual_size");
        let (w, h) = size
            .split_once(',')
            .map(|(a, b)| {
                (a.parse().unwrap_or(1920), b.parse().unwrap_or(1080))
            })
            .unwrap_or((1920, 1080));
        let stride: usize = read_sysfs("/sys/class/graphics/fb0/stride")
            .parse()
            .unwrap_or(w * 4);

        let dev = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;

        // Take the active console into graphics mode so fbcon stops drawing
        // text (and the blinking cursor) over our framebuffer.
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty0").ok();
        if let Some(ref t) = tty {
            unsafe {
                libc::ioctl(t.as_raw_fd(), KDSETMODE as _, KD_GRAPHICS);
            }
        }

        Ok(Fb { w, h, stride, dev, tty, buf: vec![0u32; w * h] })
    }

    /// Restore the console to text mode. Called on shutdown.
    pub fn restore(&self) {
        if let Some(ref t) = self.tty {
            unsafe {
                libc::ioctl(t.as_raw_fd(), KDSETMODE as _, KD_TEXT);
            }
        }
    }

    pub fn clear(&mut self, c: Color) {
        for px in self.buf.iter_mut() {
            *px = c;
        }
    }

    #[inline]
    pub fn put(&mut self, x: isize, y: isize, c: Color) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        self.buf[y as usize * self.w + x as usize] = c;
    }

    pub fn rect(&mut self, x: isize, y: isize, w: usize, h: usize, c: Color) {
        for dy in 0..h as isize {
            for dx in 0..w as isize {
                self.put(x + dx, y + dy, c);
            }
        }
    }

    pub fn frame(&mut self, x: isize, y: isize, w: usize, h: usize, c: Color) {
        self.rect(x, y, w, 1, c);
        self.rect(x, y + h as isize - 1, w, 1, c);
        self.rect(x, y, 1, h, c);
        self.rect(x + w as isize - 1, y, 1, h, c);
    }

    /// Draw text; returns the x advance in pixels.
    pub fn text(
        &mut self,
        font: &Font,
        x: isize,
        y: isize,
        scale: usize,
        c: Color,
        s: &str,
    ) -> isize {
        let bpr = font.bytes_per_row();
        let mut cx = x;
        for ch in s.chars() {
            let g = font.glyph(ch);
            for row in 0..font.height {
                let rowbytes = &g[row * bpr..row * bpr + bpr];
                for col in 0..font.width {
                    let byte = rowbytes[col / 8];
                    let bit = 7 - (col % 8);
                    if (byte >> bit) & 1 == 1 {
                        let px = cx + (col * scale) as isize;
                        let py = y + (row * scale) as isize;
                        if scale == 1 {
                            self.put(px, py, c);
                        } else {
                            self.rect(px, py, scale, scale, c);
                        }
                    }
                }
            }
            cx += (font.width * scale) as isize;
        }
        cx - x
    }

    /// Width in pixels a string would occupy at the given scale.
    pub fn text_w(font: &Font, scale: usize, s: &str) -> isize {
        (s.chars().count() * font.width * scale) as isize
    }

    /// A horizontal progress bar with border and fractional fill.
    pub fn bar(
        &mut self,
        x: isize,
        y: isize,
        w: usize,
        h: usize,
        frac: f64,
        fill: Color,
        track: Color,
        border: Color,
    ) {
        self.rect(x, y, w, h, track);
        let inner = w.saturating_sub(4);
        let fw = ((inner as f64) * frac.clamp(0.0, 1.0)).round() as usize;
        if fw > 0 {
            self.rect(x + 2, y + 2, fw, h.saturating_sub(4), fill);
        }
        self.frame(x, y, w, h, border);
    }

    /// Area/line graph of a 0..1 series (oldest first), right-aligned so the
    /// newest sample is at the right edge. Draws faint quarter gridlines.
    pub fn graph(
        &mut self,
        x: isize,
        y: isize,
        w: usize,
        h: usize,
        series: &[f64],
        line: Color,
        fill: Color,
        track: Color,
        border: Color,
    ) {
        self.rect(x, y, w, h, track);
        for g in [0.25f64, 0.5, 0.75] {
            let gy = y + h as isize - 1 - (g * (h as f64 - 2.0)) as isize;
            self.rect(x + 1, gy, w.saturating_sub(2), 1, border);
        }
        let inner_h = h.saturating_sub(2) as f64;
        let n = series.len();
        let vis = if n > w { &series[n - w..] } else { series };
        let off = w.saturating_sub(vis.len());
        for (i, &v) in vis.iter().enumerate() {
            let col = x + (off + i) as isize;
            let bh = (v.clamp(0.0, 1.0) * inner_h) as usize;
            if bh > 0 {
                self.rect(col, y + h as isize - 1 - bh as isize, 1, bh, fill);
            }
            // brighter cap on the line
            self.put(col, y + h as isize - 1 - bh as isize, line);
        }
        self.frame(x, y, w, h, border);
    }

    /// Flush the double buffer to the framebuffer device, honoring stride.
    pub fn present(&self) {
        let row_bytes = self.w * 4;
        if self.stride == row_bytes {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    self.buf.as_ptr() as *const u8,
                    self.buf.len() * 4,
                )
            };
            let _ = self.dev.write_all_at(bytes, 0);
        } else {
            for row in 0..self.h {
                let line = &self.buf[row * self.w..row * self.w + self.w];
                let bytes = unsafe {
                    std::slice::from_raw_parts(line.as_ptr() as *const u8, row_bytes)
                };
                let _ = self.dev.write_all_at(bytes, (row * self.stride) as u64);
            }
        }
    }
}
