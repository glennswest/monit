//! Dashboard layout: two host panels (pve, ai) drawn onto the framebuffer.

use crate::collect::{Host, Mem};
use crate::fb::{rgb, Color, Fb};
use crate::font::Font;

const BG: Color = rgb(13, 17, 23);
const PANEL: Color = rgb(22, 27, 34);
const BORDER: Color = rgb(48, 54, 61);
const TRACK: Color = rgb(33, 38, 45);
const TEXT: Color = rgb(201, 209, 217);
const DIM: Color = rgb(139, 148, 158);
const ACCENT: Color = rgb(88, 166, 255);
const GPU_CLR: Color = rgb(188, 140, 255);
const GREEN: Color = rgb(63, 185, 80);
const YELLOW: Color = rgb(210, 153, 34);
const RED: Color = rgb(248, 81, 73);

fn level_color(frac: f64) -> Color {
    if frac < 0.70 {
        GREEN
    } else if frac < 0.85 {
        YELLOW
    } else {
        RED
    }
}

fn gb(kb: u64) -> f64 {
    kb as f64 / 1_048_576.0
}

/// Human-readable size from kB: "1.4G" or "812M".
fn human_kb(kb: u64) -> String {
    if kb >= 1_048_576 / 10 {
        format!("{:.1}G", gb(kb))
    } else {
        format!("{}M", kb / 1024)
    }
}

pub struct Fonts {
    pub small: Font,
    pub big: Font,
}

pub fn render(fb: &mut Fb, f: &Fonts, pve: &Host, ai: &Host, clock: &str) {
    fb.clear(BG);
    let w = fb.w as isize;
    let margin = 40isize;

    // Title bar.
    fb.text(&f.big, margin, 24, 1, ACCENT, "g8 memory monitor");
    let cw = Fb::text_w(&f.small, 2, clock);
    fb.text(&f.small, w - margin - cw, 32, 2, DIM, clock);
    fb.rect(margin, 78, (w - 2 * margin) as usize, 2, BORDER);

    // Two panels.
    let top = 100isize;
    let gap = 30isize;
    let panel_w = ((w - 2 * margin - gap) / 2) as usize;
    let panel_h = (fb.h as isize - top - margin) as usize;
    panel(fb, f, margin, top, panel_w, panel_h, pve);
    panel(fb, f, margin + panel_w as isize + gap, top, panel_w, panel_h, ai);
}

fn panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host) {
    fb.rect(x, y, w, h, PANEL);
    fb.frame(x, y, w, h, BORDER);
    let pad = 26isize;
    let ix = x + pad;
    let iw = w as isize - 2 * pad;
    let mut cy = y + pad;

    // Header: hostname + status dot.
    let dot = if host.ok { GREEN } else { RED };
    fb.rect(ix, cy + 6, 16, 16, dot);
    fb.text(&f.big, ix + 28, cy, 1, TEXT, &host.label);
    cy += 44;

    if !host.ok {
        fb.text(&f.small, ix, cy + 20, 2, RED, "OFFLINE");
        let msg = if host.err.is_empty() { "unreachable" } else { &host.err };
        fb.text(&f.small, ix, cy + 60, 1, DIM, msg);
        return;
    }

    // RAM block.
    cy = mem_block(fb, f, ix, cy, iw, &host.mem);
    cy += 18;

    // Top consumers.
    fb.text(&f.small, ix, cy, 1, DIM, "TOP MEMORY CONSUMERS");
    cy += 24;
    let maxrss = host.procs.first().map(|p| p.rss_kb).unwrap_or(1).max(1);
    for p in &host.procs {
        cy = consumer_row(fb, f, ix, cy, iw, &p.name, human_kb(p.rss_kb), p.rss_kb as f64 / maxrss as f64, TEXT);
        if cy > y + h as isize - 60 {
            break;
        }
    }

    // GPU block (if present).
    if !host.gpus.is_empty() {
        cy += 14;
        fb.rect(ix, cy, iw as usize, 2, BORDER);
        cy += 16;
        for g in &host.gpus {
            cy = gpu_block(fb, f, ix, cy, iw, g);
        }
        if !host.gpu_procs.is_empty() {
            cy += 6;
            fb.text(&f.small, ix, cy, 1, DIM, "GPU PROCESSES");
            cy += 24;
            let gmax = host.gpu_procs.first().map(|p| p.mem_mb).unwrap_or(1).max(1);
            for gp in &host.gpu_procs {
                let val = format!("{:.1}G", gp.mem_mb as f64 / 1024.0);
                cy = consumer_row(fb, f, ix, cy, iw, &gp.name, val, gp.mem_mb as f64 / gmax as f64, GPU_CLR);
                if cy > y + h as isize - 30 {
                    break;
                }
            }
        }
    }
}

/// RAM used bar plus the used/total headline and a free/cache/swap footnote.
fn mem_block(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, m: &Mem) -> isize {
    let frac = m.frac();
    let clr = level_color(frac);
    let headline = format!("{:.1} / {:.1} GB", gb(m.used_kb()), gb(m.total_kb));
    fb.text(&f.big, x, y, 1, TEXT, &headline);
    let pct = format!("{:.0}%", frac * 100.0);
    let pw = Fb::text_w(&f.big, 1, &pct);
    fb.text(&f.big, x + w - pw, y, 1, clr, &pct);
    let by = y + 40;
    fb.bar(x, by, w as usize, 30, frac, clr, TRACK, BORDER);
    let foot = format!(
        "free {:.1}   cache {:.1}   swap {:.1}/{:.1} GB",
        gb(m.free_kb),
        gb(m.buffers_kb + m.cached_kb),
        gb(m.swap_total_kb.saturating_sub(m.swap_free_kb)),
        gb(m.swap_total_kb),
    );
    fb.text(&f.small, x, by + 38, 1, DIM, &foot);
    by + 60
}

fn gpu_block(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, g: &crate::collect::Gpu) -> isize {
    let frac = g.frac();
    let clr = level_color(frac);
    let head = format!("GPU {}  {}", g.idx, g.name);
    fb.text(&f.small, x, y, 1, GPU_CLR, &head);
    let cy = y + 22;
    fb.bar(x, cy, w as usize, 26, frac, clr, TRACK, BORDER);
    let label = format!(
        "{:.1} / {:.1} GB   util {}%",
        g.used_mb as f64 / 1024.0,
        g.total_mb as f64 / 1024.0,
        g.util
    );
    fb.text(&f.small, x, cy + 32, 1, TEXT, &label);
    cy + 54
}

/// A single "name .......... value" row with a thin proportional underbar.
fn consumer_row(
    fb: &mut Fb,
    f: &Fonts,
    x: isize,
    y: isize,
    w: isize,
    name: &str,
    value: String,
    frac: f64,
    clr: Color,
) -> isize {
    fb.text(&f.small, x, y, 1, clr, name);
    let vw = Fb::text_w(&f.small, 1, &value);
    fb.text(&f.small, x + w - vw, y, 1, TEXT, &value);
    // thin proportional bar under the row
    let bw = ((w as f64) * frac.clamp(0.0, 1.0)) as usize;
    fb.rect(x, y + 18, w as usize, 2, TRACK);
    if bw > 0 {
        fb.rect(x, y + 18, bw, 2, clr);
    }
    y + 26
}
