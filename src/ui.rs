//! Dashboard rendering: a rotating set of pages, each drawn onto the
//! framebuffer. Most pages are two panels (pve | ai); the AI workload page is
//! a single full-width panel.

use crate::collect::{Gpu, Host, Mem};
use crate::fb::{rgb, Color, Fb};
use crate::font::Font;
use crate::history::History;
use std::sync::atomic::{AtomicBool, Ordering};

/// Display temperatures in Fahrenheit (set from MONIT_TEMP_UNIT). Sensor values
/// and color thresholds stay in Celsius internally; only the label converts.
pub static FAHRENHEIT: AtomicBool = AtomicBool::new(false);

fn fmt_temp(c: f64) -> String {
    if FAHRENHEIT.load(Ordering::Relaxed) {
        format!("{:.0} F", c * 9.0 / 5.0 + 32.0)
    } else {
        format!("{:.0} C", c)
    }
}

const BG: Color = rgb(13, 17, 23);
const PANEL: Color = rgb(22, 27, 34);
const BORDER: Color = rgb(48, 54, 61);
const GRID: Color = rgb(33, 38, 45);
const TRACK: Color = rgb(33, 38, 45);
const TEXT: Color = rgb(201, 209, 217);
const DIM: Color = rgb(139, 148, 158);
const ACCENT: Color = rgb(88, 166, 255);
const GPU_CLR: Color = rgb(188, 140, 255);
const GREEN: Color = rgb(63, 185, 80);
const YELLOW: Color = rgb(210, 153, 34);
const RED: Color = rgb(248, 81, 73);

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Mem,
    Cpu,
    Temp,
    Disk,
    Ai,
    Logs,
}

impl Page {
    pub const ALL: [Page; 6] = [Page::Mem, Page::Cpu, Page::Temp, Page::Disk, Page::Ai, Page::Logs];
    fn title(self) -> &'static str {
        match self {
            Page::Mem => "MEMORY",
            Page::Cpu => "CPU",
            Page::Temp => "TEMPERATURES",
            Page::Disk => "DISK",
            Page::Ai => "AI WORKLOAD",
            Page::Logs => "KERNEL / LOG ERRORS",
        }
    }
}

pub struct Fonts {
    pub small: Font,
    pub big: Font,
}

fn level_color(frac: f64) -> Color {
    if frac < 0.70 { GREEN } else if frac < 0.85 { YELLOW } else { RED }
}
fn temp_color(c: f64) -> Color {
    if c < 60.0 { GREEN } else if c < 80.0 { YELLOW } else { RED }
}
fn temp_frac(c: f64) -> f64 {
    ((c - 20.0) / 75.0).clamp(0.0, 1.0)
}
fn gb(kb: u64) -> f64 {
    kb as f64 / 1_048_576.0
}
fn human_kb(kb: u64) -> String {
    if kb >= 1_048_576 / 10 { format!("{:.1}G", gb(kb)) } else { format!("{}M", kb / 1024) }
}
fn human_bytes(b: u64) -> String {
    let t = 1u64 << 40;
    let g = 1u64 << 30;
    if b >= t { format!("{:.1}T", b as f64 / t as f64) } else { format!("{:.0}G", b as f64 / g as f64) }
}

pub fn render(fb: &mut Fb, f: &Fonts, page: Page, pve: &Host, ai: &Host, hist: &History, clock: &str) {
    fb.clear(BG);
    let w = fb.w as isize;
    let margin = 40isize;

    // Title bar: app + page name, page dots, clock.
    let title = format!("g8 monitor   {}", page.title());
    fb.text(&f.big, margin, 22, 1, ACCENT, &title);
    let cw = Fb::text_w(&f.small, 2, clock);
    fb.text(&f.small, w - margin - cw, 30, 2, DIM, clock);
    // page dots under the title text
    let mut dx = margin;
    let dy = 64isize;
    for p in Page::ALL {
        let c = if p == page { ACCENT } else { GRID };
        fb.rect(dx, dy, 22, 6, c);
        dx += 30;
    }
    fb.rect(margin, 78, (w - 2 * margin) as usize, 2, BORDER);

    let top = 100isize;
    let gap = 30isize;
    let h = (fb.h as isize - top - margin) as usize;
    let pw = ((w - 2 * margin - gap) / 2) as usize;
    let lx = margin;
    let rx = margin + pw as isize + gap;

    match page {
        Page::Ai => {
            let fw = (w - 2 * margin) as usize;
            panel_bg(fb, lx, top, fw, h);
            ai_panel(fb, f, lx + 26, top + 26, fw as isize - 52, h as isize - 52, ai, hist);
        }
        Page::Mem => {
            mem_panel(fb, f, lx, top, pw, h, pve, &hist.pve_mem.slice());
            mem_panel(fb, f, rx, top, pw, h, ai, &hist.ai_mem.slice());
        }
        Page::Cpu => {
            cpu_panel(fb, f, lx, top, pw, h, pve, &hist.pve_cpu.slice());
            cpu_panel(fb, f, rx, top, pw, h, ai, &hist.ai_cpu.slice());
        }
        Page::Temp => {
            temp_panel(fb, f, lx, top, pw, h, pve, &hist.pve_temp.slice());
            temp_panel(fb, f, rx, top, pw, h, ai, &hist.ai_temp.slice());
        }
        Page::Disk => {
            disk_panel(fb, f, lx, top, pw, h, pve);
            disk_panel(fb, f, rx, top, pw, h, ai);
        }
        Page::Logs => {
            log_panel(fb, f, lx, top, pw, h, pve);
            log_panel(fb, f, rx, top, pw, h, ai);
        }
    }
}

fn panel_bg(fb: &mut Fb, x: isize, y: isize, w: usize, h: usize) {
    fb.rect(x, y, w, h, PANEL);
    fb.frame(x, y, w, h, BORDER);
}

/// Common panel chrome: background, hostname header, status dot. Returns the
/// inner content cursor, or None if the host is offline (message drawn).
fn begin_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host) -> Option<(isize, isize, isize)> {
    panel_bg(fb, x, y, w, h);
    let pad = 26isize;
    let ix = x + pad;
    let iw = w as isize - 2 * pad;
    let dot = if host.ok { GREEN } else { RED };
    fb.rect(ix, y + pad + 6, 16, 16, dot);
    fb.text(&f.big, ix + 28, y + pad, 1, TEXT, &host.label);
    let cy = y + pad + 44;
    if !host.ok {
        fb.text(&f.small, ix, cy + 16, 2, RED, "OFFLINE");
        let msg = if host.err.is_empty() { "unreachable" } else { &host.err };
        fb.text(&f.small, ix, cy + 56, 1, DIM, msg);
        return None;
    }
    Some((ix, iw, cy))
}

// --------------------------------------------------------------------------
// Memory page
// --------------------------------------------------------------------------

fn mem_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host, series: &[f64]) {
    let (ix, iw, mut cy) = match begin_panel(fb, f, x, y, w, h, host) {
        Some(v) => v,
        None => return,
    };
    cy = mem_block(fb, f, ix, cy, iw, &host.mem);
    cy += 12;
    fb.graph(ix, cy, iw as usize, 96, series, level_color(host.mem.frac()), GRID, TRACK, BORDER);
    cy += 96 + 16;
    fb.text(&f.small, ix, cy, 1, DIM, "TOP MEMORY CONSUMERS");
    cy += 24;
    let maxrss = host.procs.first().map(|p| p.rss_kb).unwrap_or(1).max(1);
    for p in &host.procs {
        cy = consumer_row(fb, f, ix, cy, iw, &p.name, human_kb(p.rss_kb), p.rss_kb as f64 / maxrss as f64, TEXT);
        if cy > y + h as isize - 30 {
            break;
        }
    }
}

fn mem_block(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, m: &Mem) -> isize {
    let frac = m.frac();
    let clr = level_color(frac);
    fb.text(&f.big, x, y, 1, TEXT, &format!("{:.1} / {:.1} GB", gb(m.used_kb()), gb(m.total_kb)));
    let pct = format!("{:.0}%", frac * 100.0);
    fb.text(&f.big, x + w - Fb::text_w(&f.big, 1, &pct), y, 1, clr, &pct);
    let by = y + 40;
    fb.bar(x, by, w as usize, 30, frac, clr, TRACK, BORDER);
    let foot = format!(
        "free {:.1}   cache {:.1}   swap {:.1}/{:.1} GB",
        gb(m.free_kb), gb(m.buffers_kb + m.cached_kb),
        gb(m.swap_total_kb.saturating_sub(m.swap_free_kb)), gb(m.swap_total_kb),
    );
    fb.text(&f.small, x, by + 38, 1, DIM, &foot);
    by + 58
}

fn consumer_row(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, name: &str, value: String, frac: f64, clr: Color) -> isize {
    fb.text(&f.small, x, y, 1, clr, name);
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, &value), y, 1, TEXT, &value);
    fb.rect(x, y + 18, w as usize, 2, TRACK);
    let bw = ((w as f64) * frac.clamp(0.0, 1.0)) as usize;
    if bw > 0 {
        fb.rect(x, y + 18, bw, 2, clr);
    }
    y + 26
}

// --------------------------------------------------------------------------
// CPU page
// --------------------------------------------------------------------------

fn cpu_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host, series: &[f64]) {
    let (ix, iw, mut cy) = match begin_panel(fb, f, x, y, w, h, host) {
        Some(v) => v,
        None => return,
    };
    let c = &host.cpu;
    let frac = c.overall;
    let clr = level_color(frac);
    fb.text(&f.big, ix, cy, 1, TEXT, &format!("{:.0}% busy", frac * 100.0));
    let info = format!("{} cores", c.cores);
    fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, &info), cy + 8, 1, DIM, &info);
    cy += 40;
    fb.bar(ix, cy, iw as usize, 26, frac, clr, TRACK, BORDER);
    cy += 34;
    fb.text(&f.small, ix, cy, 1, DIM, &format!("load  {:.2}  {:.2}  {:.2}", c.load[0], c.load[1], c.load[2]));
    cy += 24;
    fb.graph(ix, cy, iw as usize, 92, series, clr, GRID, TRACK, BORDER);
    cy += 92 + 16;
    fb.text(&f.small, ix, cy, 1, DIM, "PER-CORE");
    cy += 22;

    let cores = &c.per_core;
    let cols = if cores.len() > 16 { 4 } else { 2 };
    let cell_w = iw / cols as isize;
    let row_h = 22isize;
    for (i, &v) in cores.iter().enumerate() {
        let col = (i % cols) as isize;
        let row = (i / cols) as isize;
        let cx = ix + col * cell_w;
        let cyy = cy + row * row_h;
        if cyy > y + h as isize - 24 {
            break;
        }
        fb.text(&f.small, cx, cyy, 1, DIM, &format!("c{:<2}", i));
        let bx = cx + 34;
        let bw = (cell_w - 86).max(20) as usize;
        fb.bar(bx, cyy, bw, 14, v, level_color(v), TRACK, BORDER);
        let pct = format!("{:>3.0}%", v * 100.0);
        fb.text(&f.small, bx + bw as isize + 6, cyy, 1, TEXT, &pct);
    }
}

// --------------------------------------------------------------------------
// Temperature page
// --------------------------------------------------------------------------

fn temp_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host, series: &[f64]) {
    let (ix, iw, mut cy) = match begin_panel(fb, f, x, y, w, h, host) {
        Some(v) => v,
        None => return,
    };
    // GPU temp first (highlighted) if present.
    for g in &host.gpus {
        cy = temp_row(fb, f, ix, cy, iw, &format!("GPU {}", g.idx), g.temp_c, GPU_CLR);
    }
    if host.temps.is_empty() && host.gpus.is_empty() {
        fb.text(&f.small, ix, cy + 4, 1, DIM, "no sensors exposed");
        return;
    }
    for t in host.temps.iter().take(8) {
        cy = temp_row(fb, f, ix, cy, iw, &t.label, t.celsius, DIM);
    }

    // Fans / pump — 0 RPM is flagged red as a likely failure.
    if !host.fans.is_empty() {
        cy += 10;
        fb.text(&f.small, ix, cy, 1, DIM, "FANS / PUMP (rpm)");
        cy += 24;
        for fan in &host.fans {
            let clr = if fan.rpm == 0 { RED } else { TEXT };
            fb.text(&f.small, ix, cy, 1, clr, &fan.label);
            let v = if fan.rpm == 0 { "STOPPED".to_string() } else { format!("{} rpm", fan.rpm) };
            fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, &v), cy, 1, clr, &v);
            cy += 22;
        }
    } else {
        cy += 10;
        fb.text(&f.small, ix, cy, 1, DIM, "no fan/pump tach exposed (Super I/O driver?)");
        cy += 22;
    }

    cy += 10;
    fb.text(&f.small, ix, cy, 1, DIM, "HOTTEST (history)");
    cy += 22;
    let gh = ((y + h as isize - 26) - cy).clamp(40, 110) as usize;
    fb.graph(ix, cy, iw as usize, gh, series, RED, GRID, TRACK, BORDER);
}

fn temp_row(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, label: &str, c: f64, label_clr: Color) -> isize {
    fb.text(&f.small, x, y, 1, label_clr, label);
    let val = fmt_temp(c);
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, &val), y, 1, TEXT, &val);
    let bw = (w / 2) as usize;
    fb.bar(x + w - bw as isize - 70, y + 1, bw, 12, temp_frac(c), temp_color(c), TRACK, BORDER);
    y + 26
}

// --------------------------------------------------------------------------
// Disk page
// --------------------------------------------------------------------------

fn disk_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host) {
    let (ix, iw, mut cy) = match begin_panel(fb, f, x, y, w, h, host) {
        Some(v) => v,
        None => return,
    };
    if host.disks.is_empty() {
        fb.text(&f.small, ix, cy + 4, 1, DIM, "no storage data");
        return;
    }
    for d in &host.disks {
        let frac = d.frac();
        let clr = level_color(frac);
        fb.text(&f.small, ix, cy, 1, TEXT, &d.name);
        let val = format!("{} / {}  {:.0}%", human_bytes(d.used), human_bytes(d.total), frac * 100.0);
        fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, &val), cy, 1, DIM, &val);
        cy += 20;
        fb.bar(ix, cy, iw as usize, 18, frac, clr, TRACK, BORDER);
        cy += 30;
        if cy > y + h as isize - 30 {
            break;
        }
    }
}

// --------------------------------------------------------------------------
// AI workload page (single wide panel)
// --------------------------------------------------------------------------

fn ai_panel(fb: &mut Fb, f: &Fonts, ix: isize, iy: isize, iw: isize, _ih: isize, host: &Host, hist: &History) {
    let dot = if host.ok { GREEN } else { RED };
    fb.rect(ix, iy + 6, 16, 16, dot);
    fb.text(&f.big, ix + 28, iy, 1, TEXT, &format!("{}  ·  GPU workload", host.label));
    let mut cy = iy + 48;
    if !host.ok {
        fb.text(&f.small, ix, cy + 16, 2, RED, "OFFLINE");
        return;
    }

    // Model badge.
    let model = if host.model.is_empty() { "unknown".to_string() } else { host.model.clone() };
    fb.text(&f.small, ix, cy, 1, DIM, "MODEL");
    fb.text(&f.big, ix + 90, cy - 6, 1, GPU_CLR, &model);
    cy += 44;

    // GPU summary + bars.
    if let Some(g) = host.gpus.first() {
        cy = gpu_detail(fb, f, ix, cy, iw, g);
        // Two history graphs: VRAM and utilization.
        let gw = ((iw - 30) / 2) as usize;
        fb.text(&f.small, ix, cy, 1, DIM, "VRAM %");
        fb.text(&f.small, ix + gw as isize + 30, cy, 1, DIM, "UTIL %");
        cy += 20;
        fb.graph(ix, cy, gw, 110, &hist.gpu_mem.slice(), GPU_CLR, GRID, TRACK, BORDER);
        fb.graph(ix + gw as isize + 30, cy, gw, 110, &hist.gpu_util.slice(), ACCENT, GRID, TRACK, BORDER);
        cy += 110 + 18;
    }

    // Two columns: containers + workload command on the left, GPU procs right.
    let colw = (iw - 30) / 2;
    let mut ly = cy;
    fb.text(&f.small, ix, ly, 1, DIM, "CONTAINERS");
    ly += 22;
    for c in &host.containers {
        fb.text(&f.small, ix, ly, 1, TEXT, &c.name);
        fb.text(&f.small, ix + 240, ly, 1, DIM, &format!("{}  {}", c.image, c.status));
        ly += 22;
    }
    if host.containers.is_empty() {
        fb.text(&f.small, ix, ly, 1, DIM, "none");
        ly += 22;
    }
    ly += 6;
    fb.text(&f.small, ix, ly, 1, DIM, "RUNNING COMMAND");
    ly += 22;
    for w in &host.workload {
        fb.text(&f.small, ix, ly, 1, TEXT, w);
        ly += 20;
    }

    let rx = ix + colw + 30;
    let mut ry = cy;
    fb.text(&f.small, rx, ry, 1, DIM, "GPU PROCESSES");
    ry += 22;
    for gp in &host.gpu_procs {
        fb.text(&f.small, rx, ry, 1, GPU_CLR, &gp.name);
        let v = format!("{:.1}G", gp.mem_mb as f64 / 1024.0);
        fb.text(&f.small, rx + colw - Fb::text_w(&f.small, 1, &v), ry, 1, TEXT, &v);
        ry += 22;
    }
}

fn gpu_detail(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, g: &Gpu) -> isize {
    let frac = g.frac();
    let clr = level_color(frac);
    fb.text(&f.small, x, y, 1, GPU_CLR, &format!("GPU {}  {}", g.idx, g.name));
    let stat = format!("{:.0}% util   {}   {:.0} W", g.util, fmt_temp(g.temp_c), g.power_w);
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, &stat), y, 1, TEXT, &stat);
    let cy = y + 24;
    fb.bar(x, cy, w as usize, 30, frac, clr, TRACK, BORDER);
    let lbl = format!("{:.1} / {:.1} GB VRAM", g.used_mb as f64 / 1024.0, g.total_mb as f64 / 1024.0);
    fb.text(&f.small, x, cy + 38, 1, DIM, &lbl);
    cy + 60
}

// --------------------------------------------------------------------------
// Log / kernel error page
// --------------------------------------------------------------------------

fn log_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, host: &Host) {
    let (ix, _iw, mut cy) = match begin_panel(fb, f, x, y, w, h, host) {
        Some(v) => v,
        None => return,
    };
    fb.text(&f.small, ix, cy, 1, DIM, "RECENT ERRORS (journal, this boot)");
    cy += 26;
    if host.logs.is_empty() {
        fb.text(&f.small, ix, cy, 1, GREEN, "no recent errors");
        return;
    }
    for line in &host.logs {
        fb.text(&f.small, ix, cy, 1, rgb(230, 180, 180), line);
        cy += 20;
        if cy > y + h as isize - 24 {
            break;
        }
    }
}
