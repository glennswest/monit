//! Dashboard rendering: a rotating set of pages, each drawn onto the
//! framebuffer. Most pages are two panels (pve | ai); the AI workload page is
//! a single full-width panel.

use crate::api::Widget;
use crate::collect::{Gpu, Host, Mem};
use crate::fb::{rgb, Color, Fb};
use crate::font::Font;
use crate::history::History;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Display temperatures in Fahrenheit (set from MONIT_TEMP_UNIT). Sensor values
/// and color thresholds stay in Celsius internally; only the label converts.
pub static FAHRENHEIT: AtomicBool = AtomicBool::new(false);

/// Overscan inset in pixels (set from MONIT_OVERSCAN). Many HDMI panels crop the
/// edges; the overview insets all drawing by this much on every side so nothing
/// falls off the visible area.
pub static OVERSCAN: AtomicUsize = AtomicUsize::new(0);

/// Explicit drawable viewport for the overview (set from MONIT_VIEW_*). When a
/// panel shows only part of the framebuffer (e.g. it scales the left region to
/// full width), point the overview at exactly the visible rectangle. A zero
/// width or height means "use the full framebuffer" (minus OVERSCAN).
pub static VIEW_X: AtomicUsize = AtomicUsize::new(0);
pub static VIEW_Y: AtomicUsize = AtomicUsize::new(0);
pub static VIEW_W: AtomicUsize = AtomicUsize::new(0);
pub static VIEW_H: AtomicUsize = AtomicUsize::new(0);

/// Resolve the overview's drawable rectangle: an explicit viewport if set,
/// otherwise the full framebuffer inset by OVERSCAN.
fn overview_rect(fbw: usize, fbh: usize) -> (isize, isize, usize, usize) {
    let (vw, vh) = (VIEW_W.load(Ordering::Relaxed), VIEW_H.load(Ordering::Relaxed));
    if vw > 0 || vh > 0 {
        let x = VIEW_X.load(Ordering::Relaxed).min(fbw.saturating_sub(1));
        let y = VIEW_Y.load(Ordering::Relaxed).min(fbh.saturating_sub(1));
        let w = if vw > 0 { vw.min(fbw - x) } else { fbw - x };
        let h = if vh > 0 { vh.min(fbh - y) } else { fbh - y };
        (x as isize, y as isize, w.max(200), h.max(200))
    } else {
        let ov = OVERSCAN.load(Ordering::Relaxed);
        let w = fbw.saturating_sub(2 * ov).max(200);
        let h = fbh.saturating_sub(2 * ov).max(200);
        (ov as isize, ov as isize, w, h)
    }
}

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

// Overview is the only page in the default rotation; the others are retained
// (still rendered by `render`) so they can be re-enabled without rewriting them.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Overview,
    Mem,
    Cpu,
    Temp,
    Disk,
    Gpu,
    Ai,
    Logs,
}

impl Page {
    #[allow(dead_code)]
    pub const ALL: [Page; 8] =
        [Page::Overview, Page::Mem, Page::Cpu, Page::Temp, Page::Disk, Page::Gpu, Page::Ai, Page::Logs];
    fn title(self) -> &'static str {
        match self {
            Page::Overview => "OVERVIEW",
            Page::Mem => "MEMORY",
            Page::Cpu => "CPU",
            Page::Temp => "TEMPERATURES",
            Page::Disk => "DISK",
            Page::Gpu => "GPU",
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

/// One slot in the rotation: a built-in page or an app-pushed page (by id).
#[derive(Clone)]
pub enum Screen {
    Builtin(Page),
    Pushed(String),
}

pub fn render(
    fb: &mut Fb,
    f: &Fonts,
    screens: &[Screen],
    idx: usize,
    pve: &Host,
    ai: &Host,
    hist: &History,
    clock: &str,
    store: &crate::api::Store,
) {
    fb.clear(BG);
    let w = fb.w as isize;
    let margin = 40isize;

    let screen = screens.get(idx).cloned().unwrap_or(Screen::Builtin(Page::Overview));

    // The Overview owns the whole screen — no title bar, dots, or banner. Inset
    // by the overscan margin so the panel's cropped edges don't eat content.
    if matches!(&screen, Screen::Builtin(Page::Overview)) {
        let (x, y, ww, hh) = overview_rect(fb.w, fb.h);
        overview_page(fb, f, x, y, ww, hh, pve, ai, hist, clock);
        return;
    }

    let page_title = match &screen {
        Screen::Builtin(p) => p.title().to_string(),
        Screen::Pushed(id) => store
            .lock()
            .unwrap()
            .get(id)
            .map(|s| if s.page.title.is_empty() { s.page.id.clone() } else { s.page.title.clone() })
            .unwrap_or_else(|| "APP PAGE".to_string()),
    };

    // Title bar: app + page name, page dots, clock.
    let title = format!("g8 monitor   {}", page_title.to_uppercase());
    fb.text(&f.big, margin, 22, 1, ACCENT, &title);
    let cw = Fb::text_w(&f.small, 2, clock);
    fb.text(&f.small, w - margin - cw, 8, 2, DIM, clock);

    // Always-on thermal banner (right side, second line): pve CPU + ai GPU.
    let mut bx = w - margin;
    let ai_t = ai.gpus.first().map(|g| g.temp_c).unwrap_or_else(|| ai.max_temp());
    if ai.ok && ai_t > 0.0 {
        let s = format!("ai GPU {}", fmt_temp(ai_t));
        bx -= Fb::text_w(&f.big, 1, &s);
        fb.text(&f.big, bx, 44, 1, temp_color(ai_t), &s);
        bx -= 40;
    }
    let pve_t = pve.max_temp();
    if pve.ok && pve_t > 0.0 {
        let s = format!("pve CPU {}", fmt_temp(pve_t));
        bx -= Fb::text_w(&f.big, 1, &s);
        fb.text(&f.big, bx, 44, 1, temp_color(pve_t), &s);
    }
    // Page dots under the title text — one per screen, spacing adapts to count.
    let n = screens.len().max(1);
    let avail = (w - 2 * margin) as usize;
    let step = (30usize).min(avail / n).max(8);
    let dotw = (step.saturating_sub(8)).max(6);
    let dy = 64isize;
    if screens.len() > 1 {
        for i in 0..screens.len() {
            let c = if i == idx { ACCENT } else { GRID };
            fb.rect(margin + (i * step) as isize, dy, dotw, 6, c);
        }
    }
    fb.rect(margin, 78, (w - 2 * margin) as usize, 2, BORDER);

    let top = 100isize;
    let gap = 30isize;
    let h = (fb.h as isize - top - margin) as usize;
    let pw = ((w - 2 * margin - gap) / 2) as usize;
    let lx = margin;
    let rx = margin + pw as isize + gap;

    let page = match screen {
        Screen::Builtin(p) => p,
        Screen::Pushed(id) => {
            let fw = (w - 2 * margin) as usize;
            panel_bg(fb, lx, top, fw, h);
            pushed_panel(fb, f, lx + 26, top + 26, fw as isize - 52, h as isize - 52, &id, store);
            return;
        }
    };

    match page {
        Page::Overview => {} // handled above (full-screen, no chrome)
        Page::Gpu => {
            let fw = (w - 2 * margin) as usize;
            panel_bg(fb, lx, top, fw, h);
            gpu_page(fb, f, lx + 26, top + 26, fw as isize - 52, h as isize - 52, ai, hist);
        }
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
    if host.power.known() {
        let mut s = format!("pkg {:.0} W", host.power.pkg_w);
        if host.power.pkg_limit_w > 0.0 {
            s.push_str(&format!(" / {:.0} W cap", host.power.pkg_limit_w));
        }
        if host.power.freq_mhz > 0 {
            s.push_str(&format!("   {:.2} GHz", host.power.freq_mhz as f64 / 1000.0));
        }
        fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, &s), cy, 1, DIM, &s);
    }
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
    // AIO / CPU-thermal verdict (local host only — it's where RAPL power reads).
    if host.power.known() {
        cy = aio_block(fb, f, ix, cy, iw, host);
        cy += 8;
    }
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

/// AIO / CPU-thermal panel: package power vs cap, average clock, pump RPM, and
/// a verdict that separates "the cooler is removing real heat" from "we're just
/// holding power/clocks down". Returns the new y cursor.
fn aio_block(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, host: &crate::collect::Host) -> isize {
    let p = &host.power;
    let t = host.max_temp();
    let pump = host.fans.iter().find(|fan| fan.is_pump)
        .or_else(|| host.fans.iter().find(|fan| fan.label.to_lowercase().contains("pump")));
    let pump_rpm = pump.map(|f| f.rpm);
    let frac = p.frac(); // draw / cap (0 if no cap known)

    fb.text(&f.small, x, y, 1, DIM, "AIO / CPU THERMAL");
    // Package power figure on the right of the caption.
    let pwr = if p.pkg_limit_w > 0.0 {
        format!("{:.0} / {:.0} W  ({:.0}% cap)", p.pkg_w, p.pkg_limit_w, frac * 100.0)
    } else {
        format!("{:.0} W", p.pkg_w)
    };
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, &pwr), y, 1, TEXT, &pwr);
    let mut cy = y + 22;
    // Power-draw bar (vs cap when known, else vs a 250 W reference scale).
    let bar_frac = if p.pkg_limit_w > 0.0 { frac } else { (p.pkg_w / 250.0).clamp(0.0, 1.0) };
    fb.bar(x, cy, w as usize, 16, bar_frac, POWER_CLR, TRACK, BORDER);
    cy += 24;
    // Clock + pump line.
    let mut info = format!("clock {:.2} GHz", p.freq_mhz as f64 / 1000.0);
    if p.freq_max_mhz > 0 {
        info.push_str(&format!(" / {:.2} max", p.freq_max_mhz as f64 / 1000.0));
    }
    match pump_rpm {
        Some(0) => info.push_str("    pump STOPPED"),
        Some(r) => info.push_str(&format!("    pump {} rpm", r)),
        None => {}
    }
    fb.text(&f.small, x, cy, 1, DIM, &info);
    cy += 22;
    // Verdict.
    let (clr, verdict) = aio_verdict(p, t, pump_rpm);
    fb.text(&f.small, x, cy, 1, clr, &verdict);
    cy + 22
}

/// Heuristic verdict from package power, temperature and pump RPM.
fn aio_verdict(p: &crate::collect::Power, t: f64, pump_rpm: Option<u64>) -> (Color, String) {
    if pump_rpm == Some(0) {
        return (RED, "PUMP STOPPED — AIO not circulating coolant".to_string());
    }
    let frac = p.frac();
    let has_cap = p.pkg_limit_w > 0.0;
    // High sustained draw held at a safe temperature ⇒ the cooler is working.
    if p.pkg_w >= 90.0 && t > 0.0 && t < 85.0 && (!has_cap || frac >= 0.6) {
        return (GREEN, format!("AIO COOLING — dissipating {:.0} W, held at {}", p.pkg_w, fmt_temp(t)));
    }
    // Low draw ⇒ temps are low because there's little heat, not because of cooling.
    if (has_cap && frac < 0.35) || (!has_cap && p.pkg_w < 60.0) {
        return (YELLOW, format!("POWER-LIMITED — only {:.0} W draw; low heat, cooling not stressed", p.pkg_w));
    }
    (ACCENT, format!("nominal — {:.0} W at {}", p.pkg_w, fmt_temp(t)))
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
// Overview page — CPU (pve) | GPU (ai) side by side. Big usage/temp numbers,
// usage+temp graphs, a memory bar, an "are we limiting/throttling" line, and a
// prominent cooling-failure status (stopped pump / thermal throttle).
// --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Cpu,
    Gpu,
}

// LCARS (Star-Trek computer) palette for the history panel.
const BLACK: Color = rgb(0, 0, 0);
const LC_ORANGE: Color = rgb(255, 153, 0);
const LC_TAN: Color = rgb(255, 204, 153);
const LC_LILAC: Color = rgb(204, 153, 204);
const LC_PEACH: Color = rgb(255, 170, 128);
// Two device colors used across BOTH graphs so one legend covers everything.
const LC_CPU: Color = rgb(120, 180, 255); // ice blue
const LC_GPU: Color = rgb(255, 150, 40); // amber

/// Full-screen overview: top half is CPU | GPU live stats; bottom half is an
/// LCARS-styled history panel with THERMAL and PERFORMANCE graphs over time.
fn overview_page(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, pve: &Host, ai: &Host, hist: &History, clock: &str) {
    let cx = x + w as isize / 2; // divider center
    let colpad = 26isize;
    let half_w = (w as isize / 2 - colpad - colpad / 2).max(100);
    let lx = x + colpad;
    let rx = cx + colpad / 2;

    // Top: live stats. Bottom: LCARS history graphs.
    let split = y + 356.min(h as isize * 7 / 10);
    let stats_h = (split - y - 8).max(120);

    draw_half(fb, f, lx, y + 8, half_w as usize, stats_h as usize, pve, Role::Cpu);
    draw_half(fb, f, rx, y + 8, half_w as usize, stats_h as usize, ai, Role::Gpu);
    fb.rect(cx - 1, y + 16, 2, (stats_h - 16).max(20) as usize, BORDER);

    let by = split + 8;
    let bottom_margin = 30isize; // keep the panel off the very bottom edge
    let panel_h = (y + h as isize - by - bottom_margin).max(80) as usize;
    lcars_panel(fb, f, x, by, w, panel_h, hist, clock);
}

/// LCARS history panel: header bar, a sidebar that doubles as the CPU/GPU color
/// legend, and two stacked graphs (THERMAL °C, PERFORMANCE %) over time. CPU is
/// one color and GPU another across both graphs, so the sidebar legend says all.
fn lcars_panel(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, hist: &History, clock: &str) {
    let wi = w as isize;
    fb.rect(x, y, w, h, BLACK);

    let head_h = 34isize;
    let gap = 10isize;
    let side_w = 92isize;

    // Header bar (rounded), title left + clock right, in LCARS orange.
    fb.fill_round(x, y, w, head_h as usize, head_h as usize / 2, LC_ORANGE);
    fb.text(&f.big, x + 26, y + 1, 1, BLACK, "LCARS // SENSOR HISTORY");
    fb.text(&f.small, x + wi - Fb::text_w(&f.small, 1, clock) - 24, y + 9, 1, BLACK, clock);

    let body_y = y + head_h + gap;
    let body_h = (y + h as isize - body_y).max(40);

    // Sidebar: top two blocks ARE the legend (CPU / GPU colors); rest is LCARS
    // filler for the look.
    let blocks = [(LC_CPU, "CPU"), (LC_GPU, "GPU"), (LC_LILAC, "SYS"), (LC_PEACH, "47")];
    let n = blocks.len() as isize;
    let sgap = 8isize;
    let bh = (body_h - (n - 1) * sgap) / n;
    for (i, (c, lbl)) in blocks.iter().enumerate() {
        let yb = body_y + i as isize * (bh + sgap);
        fb.fill_round(x, yb, side_w as usize, bh.max(8) as usize, 14, *c);
        fb.text(&f.small, x + 16, yb + bh - 22, 1, BLACK, lbl);
    }

    // Two stacked graphs to the right of the sidebar. CPU=ice, GPU=amber in both.
    let gx = x + side_w + gap;
    let gw = (x + wi - gx).max(40) as usize;
    let gh = ((body_h - gap) / 2).max(40) as usize;

    lcars_graph(
        fb, f, gx, body_y, gw, gh, "THERMAL  C",
        &[(hist.pve_temp.slice(), LC_CPU), (hist.gpu_temp.slice(), LC_GPU)],
    );
    lcars_graph(
        fb, f, gx, body_y + gh as isize + gap, gw, gh, "PERFORMANCE  %",
        &[(hist.pve_cpu.slice(), LC_CPU), (hist.gpu_util.slice(), LC_GPU)],
    );
}

/// One LCARS graph: a colored title tab, the plotted history below, and a bright
/// base rail.
fn lcars_graph(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, h: usize, title: &str, series: &[(Vec<f64>, Color)]) {
    // Title tab (rounded) top-left.
    let tab_w = Fb::text_w(&f.small, 1, title) + 30;
    fb.fill_round(x, y, tab_w as usize, 22, 11, LC_TAN);
    fb.text(&f.small, x + 15, y + 3, 1, BLACK, title);
    // History plot (newest at right) on a near-black track with a dim rail.
    let gy = y + 28;
    let gh = (h as isize - 28 - 6).max(20) as usize;
    fb.graph_multi(x, gy, w, gh, series, rgb(10, 12, 16), rgb(70, 45, 12));
    // Bright LCARS base rail.
    fb.fill_round(x, y + h as isize - 4, w, 4, 2, LC_ORANGE);
}

/// One side of the overview (CPU or GPU): status dot + role tag, big USAGE and
/// TEMP numbers, a memory/VRAM bar, a limit line, and a boxed cooling verdict.
fn draw_half(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: usize, _h: usize, host: &Host, role: Role) {
    let iw = w as isize;
    let dot = if host.ok { GREEN } else { RED };
    fb.rect(x, y + 6, 16, 16, dot);
    let rl = if role == Role::Cpu { "CPU" } else { "GPU" };
    fb.text(&f.big, x + 28, y, 1, DIM, rl);
    let mut cy = y + 50;

    if !host.ok {
        fb.text(&f.big, x, cy + 10, 2, RED, "OFFLINE");
        let msg = if host.err.is_empty() { "unreachable" } else { &host.err };
        fb.text(&f.small, x, cy + 70, 1, DIM, msg);
        return;
    }
    let gpu = host.gpus.first();
    if role == Role::Gpu && gpu.is_none() {
        fb.text(&f.big, x, cy + 10, 1, DIM, "no GPU reported");
        return;
    }

    let (usage, temp, mem_used_gb, mem_total_gb, mem_frac, mem_label) = match role {
        Role::Cpu => (
            host.cpu.overall, host.max_temp(),
            gb(host.mem.used_kb()), gb(host.mem.total_kb), host.mem.frac(), "MEMORY",
        ),
        Role::Gpu => {
            let g = gpu.unwrap();
            (g.util as f64 / 100.0, g.temp_c, g.used_mb as f64 / 1024.0, g.total_mb as f64 / 1024.0, g.frac(), "VRAM")
        }
    };

    // Big USAGE (left) and TEMP (right of this half).
    let thalf = iw / 2;
    fb.text(&f.small, x, cy, 1, DIM, "USAGE");
    fb.text(&f.small, x + thalf, cy, 1, DIM, "TEMP");
    fb.text(&f.big, x, cy + 20, 3, level_color(usage), &format!("{:.0}%", usage * 100.0));
    fb.text(&f.big, x + thalf, cy + 20, 3, temp_color(temp), &fmt_temp(temp));
    cy += 20 + 96 + 22;

    // Memory / VRAM bar.
    fb.text(&f.small, x, cy, 1, DIM, mem_label);
    let memval = format!("{:.1} / {:.1} GB   {:.0}%", mem_used_gb, mem_total_gb, mem_frac * 100.0);
    fb.text(&f.small, x + iw - Fb::text_w(&f.small, 1, &memval), cy, 1, TEXT, &memval);
    cy += 22;
    fb.bar(x, cy, iw as usize, 28, mem_frac, level_color(mem_frac), TRACK, BORDER);
    cy += 48;

    // Limit / throttle line.
    let (limit_clr, limit_text) = match role {
        Role::Cpu => cpu_limit(host),
        Role::Gpu => gpu_limit(gpu.unwrap()),
    };
    fb.text(&f.small, x, cy, 1, DIM, "LIMITING");
    fb.text(&f.small, x + iw - Fb::text_w(&f.small, 1, &limit_text), cy, 1, limit_clr, &limit_text);
    cy += 32;

    // Cooling verdict — boxed and colored so a failure is unmissable.
    let (cool_clr, cool_text) = match role {
        Role::Cpu => cpu_cooling(host),
        Role::Gpu => gpu_cooling(gpu.unwrap()),
    };
    let bh = 44usize;
    fb.frame(x, cy, iw as usize, bh, cool_clr);
    fb.text(&f.small, x + 12, cy + 16, 1, DIM, "COOLING");
    let cv = Fb::text_w(&f.big, 1, &cool_text);
    fb.text(&f.big, x + iw - cv - 12, cy + 6, 1, cool_clr, &cool_text);
}

/// CPU power-cap status from RAPL. Only flags when the long-term cap has been
/// lowered below the hardware max (an imposed throttle); a cap sitting at the
/// stock TDP is just normal protection and is shown calmly.
fn cpu_limit(host: &Host) -> (Color, String) {
    let p = &host.power;
    // Strongest signal first: the thermal governor capping CPU performance
    // (intel_pstate max_perf_pct < 100) — the CPU is being actively throttled.
    if p.perf_pct > 0 && p.perf_pct < 100 {
        return (YELLOW, format!("THROTTLED perf {}% (thermal gov)", p.perf_pct));
    }
    if !p.known() || p.pkg_limit_w <= 0.0 {
        return (DIM, "RAPL n/a".to_string());
    }
    // Imposed RAPL throttle: cap held meaningfully below the hardware max.
    if p.pkg_max_w > 0.0 && p.pkg_limit_w < p.pkg_max_w * 0.98 {
        return (YELLOW, format!("THROTTLED {:.0}/{:.0} W", p.pkg_limit_w, p.pkg_max_w));
    }
    // Stock TDP — protection only, not limiting performance.
    (DIM, format!("TDP {:.0} W · {:.0} W draw", p.pkg_limit_w, p.pkg_w))
}

/// GPU throttle status from nvidia-smi clocks_throttle_reasons.
fn gpu_limit(g: &Gpu) -> (Color, String) {
    let pbase = if g.power_limit_w > 0.0 {
        format!("{:.0}/{:.0} W", g.power_w, g.power_limit_w)
    } else {
        format!("{:.0} W", g.power_w)
    };
    if g.throttle.is_empty() {
        return (DIM, format!("{} · not throttled", pbase));
    }
    let why = g.throttle.join(", ");
    if why.to_lowercase().contains("thermal") {
        (RED, format!("THERMAL CAP · {}", why))
    } else {
        (YELLOW, format!("POWER CAP · {}", why))
    }
}

/// CPU cooling verdict — a stopped pump is an outright failure.
fn cpu_cooling(host: &Host) -> (Color, String) {
    let t = host.max_temp();
    let pump = host.fans.iter().find(|fan| fan.is_pump)
        .or_else(|| host.fans.iter().find(|fan| fan.label.to_lowercase().contains("pump")));
    if let Some(p) = pump {
        if p.rpm == 0 {
            return (RED, "PUMP STOPPED!".to_string());
        }
        if t >= 90.0 {
            return (RED, format!("HOT {} · pump {}rpm", fmt_temp(t), p.rpm));
        }
        if t >= 80.0 {
            return (YELLOW, format!("WARM · pump {}rpm", p.rpm));
        }
        return (GREEN, format!("OK · pump {}rpm", p.rpm));
    }
    if t >= 90.0 {
        (RED, format!("HOT {}", fmt_temp(t)))
    } else if t >= 80.0 {
        (YELLOW, "WARM".to_string())
    } else if t > 0.0 {
        (GREEN, "OK".to_string())
    } else {
        (DIM, "no tach".to_string())
    }
}

/// GPU cooling verdict — a thermal throttle means cooling can't keep up.
fn gpu_cooling(g: &Gpu) -> (Color, String) {
    let thermal = g.throttle.join(",").to_lowercase().contains("thermal");
    if thermal {
        return (RED, "THERMAL THROTTLE!".to_string());
    }
    let faninfo = if g.fan_pct > 0 { format!(" · fan {}%", g.fan_pct) } else { String::new() };
    if g.temp_c >= 87.0 {
        (RED, format!("HOT {}", fmt_temp(g.temp_c)))
    } else if g.temp_c >= 80.0 {
        (YELLOW, format!("WARM{}", faninfo))
    } else if g.temp_c > 0.0 {
        (GREEN, format!("OK{}", faninfo))
    } else {
        (DIM, "n/a".to_string())
    }
}

// --------------------------------------------------------------------------
// GPU page (single wide panel) — what the accelerators are actually doing.
// --------------------------------------------------------------------------

const POWER_CLR: Color = rgb(255, 170, 60);

fn gpu_page(fb: &mut Fb, f: &Fonts, ix: isize, iy: isize, iw: isize, ih: isize, host: &Host, hist: &History) {
    let dot = if host.ok { GREEN } else { RED };
    fb.rect(ix, iy + 6, 16, 16, dot);
    fb.text(&f.big, ix + 28, iy, 1, TEXT, &format!("{}  ·  GPU compute", host.label));
    let mut cy = iy + 48;
    if !host.ok {
        fb.text(&f.small, ix, cy + 16, 2, RED, "OFFLINE");
        let msg = if host.err.is_empty() { "unreachable" } else { &host.err };
        fb.text(&f.small, ix, cy + 56, 1, DIM, msg);
        return;
    }
    if host.gpus.is_empty() {
        fb.text(&f.small, ix, cy + 8, 1, DIM, "no NVIDIA GPU detected (nvidia-smi returned nothing)");
        return;
    }

    // Detailed block for the primary one or two GPUs.
    for g in host.gpus.iter().take(2) {
        cy = gpu_block(fb, f, ix, cy, iw, g);
        cy += 10;
    }
    // Any GPUs beyond the first two get a one-line compact summary.
    for g in host.gpus.iter().skip(2) {
        let s = format!(
            "GPU {}  {:>3}% util  {:>3}% vram  {:.0}W  {}",
            g.idx, g.util, (g.frac() * 100.0) as u32, g.power_w, fmt_temp(g.temp_c)
        );
        fb.text(&f.small, ix, cy, 1, DIM, &s);
        cy += 20;
    }

    // History graphs for the primary GPU: utilization, VRAM, power, temp.
    cy += 6;
    let cols = 4isize;
    let ggap = 24isize;
    let gw = ((iw - ggap * (cols - 1)) / cols) as usize;
    let labels = ["SM UTIL %", "VRAM %", "POWER % CAP", "TEMP"];
    let series = [
        hist.gpu_util.slice(),
        hist.gpu_mem.slice(),
        hist.gpu_power.slice(),
        hist.gpu_temp.slice(),
    ];
    let clrs = [ACCENT, GPU_CLR, POWER_CLR, RED];
    for i in 0..cols {
        let gx = ix + i * (gw as isize + ggap);
        fb.text(&f.small, gx, cy, 1, DIM, labels[i as usize]);
    }
    cy += 20;
    let gh = 130usize;
    for i in 0..cols {
        let gx = ix + i * (gw as isize + ggap);
        fb.graph(gx, cy, gw, gh, &series[i as usize], clrs[i as usize], GRID, TRACK, BORDER);
    }
    cy += gh as isize + 22;

    // Per-process compute table: SM utilization and resident VRAM.
    fb.text(&f.small, ix, cy, 1, DIM, "COMPUTE PROCESSES");
    let hdr = "SM%      VRAM";
    fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, hdr), cy, 1, DIM, hdr);
    cy += 24;
    if host.gpu_procs.is_empty() {
        fb.text(&f.small, ix, cy, 1, DIM, "idle — no compute processes");
        return;
    }
    for gp in &host.gpu_procs {
        if cy > iy + ih - 24 {
            break;
        }
        let label = if host.gpus.len() > 1 {
            format!("g{} {} [{}]", gp.gpu_idx, gp.name, gp.pid)
        } else {
            format!("{} [{}]", gp.name, gp.pid)
        };
        fb.text(&f.small, ix, cy, 1, GPU_CLR, &label);
        // SM bar + values, right-aligned.
        let mem = if gp.mem_mb > 0 { format!("{:.1}G", gp.mem_mb as f64 / 1024.0) } else { "—".to_string() };
        let val = format!("{:>3}%   {:>6}", gp.sm, mem);
        fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, &val), cy, 1, TEXT, &val);
        let bw = 160usize;
        fb.bar(ix + iw - bw as isize - 230, cy + 1, bw, 12, gp.sm as f64 / 100.0, level_color(gp.sm as f64 / 100.0), TRACK, BORDER);
        cy += 22;
    }
}

/// A detailed single-GPU block: utilization headline + util/VRAM/power bars and
/// a clocks/fan/throttle status line. Returns the new y cursor.
fn gpu_block(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, g: &Gpu) -> isize {
    // Header: id + name on the left; pstate + PCIe link on the right.
    let head = format!("GPU {}  {}", g.idx, g.name);
    fb.text(&f.small, x, y, 1, GPU_CLR, &head);
    let mut right = String::new();
    if !g.pstate.is_empty() { right.push_str(&g.pstate); }
    if g.pcie_gen > 0 { right.push_str(&format!("  PCIe {}x{}", g.pcie_gen, g.pcie_width)); }
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, &right), y, 1, DIM, &right);
    let mut cy = y + 24;

    // Utilization headline + clocks.
    let uclr = level_color(g.util as f64 / 100.0);
    fb.text(&f.big, x, cy, 1, uclr, &format!("{:>3}% SM", g.util));
    let clk = format!("sm {} / mem {} MHz   mem-ctl {}%", g.sm_clock, g.mem_clock, g.mem_util);
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, &clk), cy + 8, 1, DIM, &clk);
    cy += 38;
    fb.bar(x, cy, w as usize, 22, g.util as f64 / 100.0, uclr, TRACK, BORDER);
    cy += 30;

    // Three side-by-side meters: VRAM, power, temperature.
    let gap = 24isize;
    let cw = (w - 2 * gap) / 3;
    meter(fb, f, x, cy, cw, "VRAM", g.frac(), level_color(g.frac()),
          &format!("{:.1}/{:.1}G", g.used_mb as f64 / 1024.0, g.total_mb as f64 / 1024.0));
    let plabel = if g.power_limit_w > 0.0 {
        format!("{:.0}/{:.0}W", g.power_w, g.power_limit_w)
    } else {
        format!("{:.0}W", g.power_w)
    };
    meter(fb, f, x + cw + gap, cy, cw, "POWER", g.power_frac(), POWER_CLR, &plabel);
    let tlabel = if g.mem_temp_c > 0.0 {
        format!("{} mem {}", fmt_temp(g.temp_c), fmt_temp(g.mem_temp_c))
    } else {
        fmt_temp(g.temp_c)
    };
    meter(fb, f, x + 2 * (cw + gap), cy, cw, "TEMP", temp_frac(g.temp_c), temp_color(g.temp_c), &tlabel);
    cy += 56;

    // Throttle / fan status line.
    let fan = if g.fan_pct > 0 { format!("fan {}%", g.fan_pct) } else { String::new() };
    if g.throttled() {
        let s = format!("THROTTLING: {}   {}", g.throttle.join(", "), fan);
        fb.text(&f.small, x, cy, 1, RED, s.trim());
    } else {
        let s = format!("no throttling   {}", fan);
        fb.text(&f.small, x, cy, 1, GREEN, s.trim());
    }
    cy + 22
}

/// A small labelled meter: caption, bar, and a value string underneath.
fn meter(fb: &mut Fb, f: &Fonts, x: isize, y: isize, w: isize, cap: &str, frac: f64, clr: Color, val: &str) {
    fb.text(&f.small, x, y, 1, DIM, cap);
    fb.text(&f.small, x + w - Fb::text_w(&f.small, 1, val), y, 1, TEXT, val);
    fb.bar(x, y + 20, w as usize, 18, frac, clr, TRACK, BORDER);
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

// --------------------------------------------------------------------------
// App-pushed pages (declarative widgets over the REST API)
// --------------------------------------------------------------------------

/// Map a widget color name (or "#rrggbb") to a palette color.
fn parse_color(spec: &Option<String>, default: Color) -> Color {
    let s = match spec {
        Some(s) => s.trim(),
        None => return default,
    };
    match s.to_ascii_lowercase().as_str() {
        "green" | "ok" => GREEN,
        "yellow" | "warn" => YELLOW,
        "red" | "crit" | "error" => RED,
        "accent" | "blue" => ACCENT,
        "gpu" | "purple" => GPU_CLR,
        "power" | "orange" => POWER_CLR,
        "dim" | "gray" | "grey" => DIM,
        "text" | "white" => TEXT,
        _ => parse_hex(s).unwrap_or(default),
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(rgb(r, g, b))
}

/// Render an app-pushed page: iterate its declared widgets top-to-bottom,
/// clipping to the panel. Unknown/oversized content is bounded by the API.
fn pushed_panel(fb: &mut Fb, f: &Fonts, ix: isize, iy: isize, iw: isize, ih: isize, id: &str, store: &crate::api::Store) {
    let guard = store.lock().unwrap();
    let stored = match guard.get(id) {
        Some(s) => s,
        None => {
            fb.text(&f.small, ix, iy + 8, 1, DIM, "page expired");
            return;
        }
    };
    let page = &stored.page;

    // Header: title + source id.
    let head = if page.title.is_empty() { page.id.clone() } else { page.title.clone() };
    fb.rect(ix, iy + 6, 16, 16, ACCENT);
    fb.text(&f.big, ix + 28, iy, 1, TEXT, &head);
    let src = format!("app · {}", page.id);
    fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, &src), iy + 8, 1, DIM, &src);
    let mut cy = iy + 48;
    let bottom = iy + ih;

    for wdg in &page.widgets {
        if cy > bottom - 24 {
            break;
        }
        match wdg {
            Widget::Heading { text, color } => {
                cy += 6;
                fb.text(&f.small, ix, cy, 1, parse_color(color, ACCENT), &text.to_uppercase());
                cy += 20;
                fb.rect(ix, cy, iw as usize, 1, BORDER);
                cy += 12;
            }
            Widget::Text { label, value, color } => {
                let clr = parse_color(color, TEXT);
                match label {
                    Some(l) => {
                        fb.text(&f.small, ix, cy, 1, DIM, l);
                        fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, value), cy, 1, clr, value);
                    }
                    None => {
                        fb.text(&f.small, ix, cy, 1, clr, value);
                    }
                }
                cy += 22;
            }
            Widget::Bar { label, frac, value, color } => {
                let frac = frac.clamp(0.0, 1.0);
                let clr = parse_color(color, level_color(frac));
                fb.text(&f.small, ix, cy, 1, DIM, label);
                if let Some(v) = value {
                    fb.text(&f.small, ix + iw - Fb::text_w(&f.small, 1, v), cy, 1, TEXT, v);
                }
                cy += 20;
                fb.bar(ix, cy, iw as usize, 18, frac, clr, TRACK, BORDER);
                cy += 28;
            }
            Widget::Graph { label, series, color, max } => {
                if let Some(l) = label {
                    fb.text(&f.small, ix, cy, 1, DIM, l);
                    cy += 20;
                }
                let maxv = max
                    .filter(|m| *m > 0.0)
                    .unwrap_or_else(|| series.iter().cloned().fold(0.0_f64, f64::max).max(1e-9));
                let norm: Vec<f64> = series.iter().map(|v| (v / maxv).clamp(0.0, 1.0)).collect();
                let gh = ((bottom - cy - 8).clamp(60, 140)) as usize;
                fb.graph(ix, cy, iw as usize, gh, &norm, parse_color(color, ACCENT), GRID, TRACK, BORDER);
                cy += gh as isize + 16;
            }
            Widget::Table { columns, rows } => {
                let ncol = columns.len().max(rows.iter().map(|r| r.len()).max().unwrap_or(0)).max(1);
                let colw = iw / ncol as isize;
                if !columns.is_empty() {
                    for (c, name) in columns.iter().enumerate() {
                        fb.text(&f.small, ix + c as isize * colw, cy, 1, DIM, name);
                    }
                    cy += 22;
                }
                for row in rows {
                    if cy > bottom - 20 {
                        break;
                    }
                    for (c, cell) in row.iter().enumerate() {
                        fb.text(&f.small, ix + c as isize * colw, cy, 1, TEXT, cell);
                    }
                    cy += 20;
                }
                cy += 8;
            }
        }
    }
}
