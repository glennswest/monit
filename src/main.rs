//! monit — framebuffer memory dashboard for the g8 cluster.
//!
//! Runs as a systemd service on pve.g8.lo. Reads the local host's memory from
//! /proc, the remote GPU host (ai.g8.lo) over SSH, and paints a live dashboard
//! to /dev/fb0. Config via environment:
//!   MONIT_AI_HOST   ssh target for the GPU host   (default root@ai.g8.lo)
//!   MONIT_PVE_LABEL panel label for the local host (default pve.g8.lo)
//!   MONIT_AI_LABEL  panel label for the GPU host   (default ai.g8.lo)
//!   MONIT_REFRESH   refresh seconds                (default 2)
//!   MONIT_TOP       rows of top consumers per host (default 8)

mod collect;
mod fb;
mod font;
mod ui;

use fb::Fb;
use font::Font;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use ui::Fonts;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Local wall-clock "YYYY-MM-DD HH:MM:SS" via libc, honoring TZ.
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
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}

fn main() {
    let ai_host = env_or("MONIT_AI_HOST", "root@ai.g8.lo");
    let pve_label = env_or("MONIT_PVE_LABEL", "pve.g8.lo");
    let ai_label = env_or("MONIT_AI_LABEL", "ai.g8.lo");
    let refresh: u64 = env_or("MONIT_REFRESH", "2").parse().unwrap_or(2);
    let top: usize = env_or("MONIT_TOP", "8").parse().unwrap_or(8);

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

    while !STOP.load(Ordering::SeqCst) {
        let pve = collect::local(&pve_label, top);
        let ai = collect::remote(&ai_label, &ai_host, top);
        ui::render(&mut fb, &fonts, &pve, &ai, &clock_string());
        fb.present();

        // Sleep in small slices so a signal stops us promptly.
        for _ in 0..(refresh * 10).max(1) {
            if STOP.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fb.restore();
}
