//! monit — framebuffer memory/CPU/temp/disk/GPU dashboard for the g8 cluster.
//!
//! Runs as a systemd service on pve.g8.lo. Reads the local host from /proc,
//! the remote GPU host (ai.g8.lo) over SSH, and paints a rotating set of pages
//! to /dev/fb0. Config via environment:
//!   MONIT_AI_HOST    ssh target for the GPU host   (default root@ai.g8.lo)
//!   MONIT_PVE_LABEL  panel label for the local host (default pve.g8.lo)
//!   MONIT_AI_LABEL   panel label for the GPU host   (default ai.g8.lo)
//!   MONIT_REFRESH    data refresh seconds           (default 2)
//!   MONIT_PAGE_SECS  seconds per page before rotate (default 8)
//!   MONIT_TOP        rows of top consumers per host (default 8)

mod collect;
mod fb;
mod font;
mod history;
mod ui;

use fb::Fb;
use font::Font;
use history::History;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use ui::{Fonts, Page};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Local wall-clock "YYYY-MM-DD HH:MM:SS" via libc, honoring TZ.
#[allow(deprecated)]
fn clock_string() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec
        )
    }
}

fn main() {
    let ai_host = env_or("MONIT_AI_HOST", "root@ai.g8.lo");
    let pve_label = env_or("MONIT_PVE_LABEL", "pve.g8.lo");
    let ai_label = env_or("MONIT_AI_LABEL", "ai.g8.lo");
    let refresh: u64 = env_or("MONIT_REFRESH", "2").parse().unwrap_or(2);
    let page_secs: u64 = env_or("MONIT_PAGE_SECS", "8").parse().unwrap_or(8);
    let top: usize = env_or("MONIT_TOP", "8").parse().unwrap_or(8);
    let unit = env_or("MONIT_TEMP_UNIT", "C");
    ui::FAHRENHEIT.store(unit.eq_ignore_ascii_case("F"), std::sync::atomic::Ordering::Relaxed);

    let handler = on_signal as *const () as usize;
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGHUP, handler);
    }

    let fonts = Fonts {
        small: Font::parse(include_bytes!("../assets/Lat15-Terminus16.psf")),
        big: Font::parse(include_bytes!("../assets/Lat15-Terminus32x16.psf")),
    };

    let mut fb = match Fb::open() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("monit: cannot open framebuffer: {e}");
            std::process::exit(1);
        }
    };

    let mut hist = History::default();
    let cycles_per_page = (page_secs / refresh.max(1)).max(1);
    let mut tick: u64 = 0;

    while !STOP.load(Ordering::SeqCst) {
        let pve = collect::local(&pve_label, top);
        let ai = collect::remote(&ai_label, &ai_host, top);
        hist.record(&pve, &ai);

        let page = Page::ALL[((tick / cycles_per_page) % Page::ALL.len() as u64) as usize];
        ui::render(&mut fb, &fonts, page, &pve, &ai, &hist, &clock_string());
        fb.present();
        tick += 1;

        for _ in 0..(refresh * 10).max(1) {
            if STOP.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fb.restore();
}
