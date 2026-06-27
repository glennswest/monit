//! monit — framebuffer memory/CPU/temp/disk/GPU dashboard for the g8 cluster.
//!
//! Runs as a systemd service on the hypervisor host. Reads the local host from
//! /proc, the remote GPU host over SSH, and paints a rotating set of pages to
//! /dev/fb0. Site-specific values come from a config file (see config.rs);
//! defaults are generic so the source carries no infrastructure details.

mod api;
mod collect;
mod config;
mod fb;
mod font;
mod history;
mod ui;

use config::Config;
use fb::Fb;
use font::Font;
use history::History;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use ui::{Fonts, Page, Screen};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
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
    let cfg = Config::load();
    let ai_host = cfg.string("ai_host", "MONIT_AI_HOST", "root@gpu-host.local");
    let pve_label = cfg.opt("pve_label", "MONIT_PVE_LABEL").unwrap_or_else(config::hostname);
    let ai_label = cfg.opt("ai_label", "MONIT_AI_LABEL").unwrap_or_else(|| config::host_part(&ai_host));
    let refresh: u64 = cfg.parse("refresh", "MONIT_REFRESH", 2);
    let page_secs: u64 = cfg.parse("page_secs", "MONIT_PAGE_SECS", 8);
    let top: usize = cfg.parse("top", "MONIT_TOP", 8);
    let unit = cfg.string("temp_unit", "MONIT_TEMP_UNIT", "C");
    ui::FAHRENHEIT.store(unit.eq_ignore_ascii_case("F"), std::sync::atomic::Ordering::Relaxed);
    ui::OVERSCAN.store(cfg.parse("overscan", "MONIT_OVERSCAN", 0), Ordering::Relaxed);
    ui::VIEW_X.store(cfg.parse("view_x", "MONIT_VIEW_X", 0), Ordering::Relaxed);
    ui::VIEW_Y.store(cfg.parse("view_y", "MONIT_VIEW_Y", 0), Ordering::Relaxed);
    ui::VIEW_W.store(cfg.parse("view_w", "MONIT_VIEW_W", 0), Ordering::Relaxed);
    ui::VIEW_H.store(cfg.parse("view_h", "MONIT_VIEW_H", 0), Ordering::Relaxed);

    // Fan naming: friendly labels for unlabeled hwmon channels + which is the
    // pump (watched for stalls). E.g. fan_labels="fan3=Pump,fan7=Rad" pump_fan=fan3.
    collect::set_fan_cfg(collect::FanCfg::parse(
        &cfg.opt("fan_labels", "MONIT_FAN_LABELS").unwrap_or_default(),
        &cfg.opt("pump_fan", "MONIT_PUMP_FAN").unwrap_or_default(),
    ));

    // REST API for app-pushed pages + power control. Empty/"off" bind disables.
    let store = api::new_store();
    let allow_control = cfg.parse("api_control", "MONIT_API_CONTROL", true);
    let orig_cap_uw = collect::power_cap().map(|c| c.cur_uw);
    api::serve(
        api::ApiConfig {
            bind: cfg.string("api_bind", "MONIT_API_BIND", "0.0.0.0:9090"),
            token: cfg.opt("api_token", "MONIT_API_TOKEN"),
            allow_control,
            orig_cap_uw,
        },
        store.clone(),
    );

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

        // Single consolidated Overview pane (CPU + GPU usage/mem/temp + limit &
        // cooling status). App-pushed pages, if any, join the rotation after it.
        let mut screens: Vec<Screen> = vec![Screen::Builtin(Page::Overview)];
        for id in api::active_ids(&store, Instant::now()) {
            screens.push(Screen::Pushed(id));
        }
        let idx = ((tick / cycles_per_page) % screens.len() as u64) as usize;
        ui::render(&mut fb, &fonts, &screens, idx, &pve, &ai, &hist, &clock_string(), &store);
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
