//! Optional closed-loop thermal governor (opt-in via `thermal_control`).
//!
//! Ported from the standalone adaptive-thermal.sh into monit so there's one
//! service. Every `interval` seconds it:
//!   * forces the AIO pump PWM to full (cooling must never be throttled),
//!   * drives the radiator-fan PWM on a temperature curve (dynamic / quieter),
//!   * holds a CPU temperature band by steering intel_pstate `max_perf_pct`
//!     (auto-throttle if cooling fails, ramp back to full when cool).
//!
//! The hardware PROCHOT at Tjmax (~100 °C) remains the ultimate backstop if this
//! thread ever dies, so run monit with `Restart=always`.

use std::fs;
use std::thread;
use std::time::Duration;

const PCT: &str = "/sys/devices/system/cpu/intel_pstate/max_perf_pct";

#[derive(Clone)]
pub struct GovConfig {
    pub enabled: bool,
    pub cpu_govern: bool,
    // CPU temperature band (°C).
    pub t_ok: i32,    // below this: ramp performance up
    pub t_high: i32,  // at/above: step down
    pub t_crit: i32,  // at/above: cut hard
    pub t_emerg: i32, // at/above: slam to minimum
    pub perf_min: i32,
    // Fan control. PWM channel names as exposed by the Super I/O hwmon
    // (e.g. "pwm3"). Empty disables that piece.
    pub pump_pwm: String, // forced to full
    pub fan_pwm: String,  // dynamic curve
    pub fan_temp_lo: i32, // curve: at/below -> duty_lo
    pub fan_temp_hi: i32, // curve: at/above -> duty_hi
    pub fan_duty_lo: i32, // percent 0..100
    pub fan_duty_hi: i32, // percent 0..100
    // CPU-load boost: when busy% >= load_hi, add load_boost points to the
    // temperature-derived fan duty (spin up before the heat arrives).
    pub load_hi: i32,    // percent 0..100; 0 disables the boost
    pub load_boost: i32, // percent points added to fan duty when busy
    pub interval_s: u64,
}

fn read_i32(path: &str) -> Option<i32> {
    fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok())
}

fn write_i32(path: &str, val: i32) {
    let _ = fs::write(path, val.to_string());
}

/// Directory of the first nct67xx (or compatible) Super I/O hwmon, if present.
fn nct_dir() -> Option<String> {
    for ent in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let p = ent.path();
        if fs::read_to_string(p.join("name")).unwrap_or_default().trim().starts_with("nct67") {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

/// Cumulative (busy, total) CPU jiffies from the `cpu` line of /proc/stat.
fn cpu_jiffies() -> Option<(u64, u64)> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?; // aggregate "cpu  ..." line
    let vals: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if vals.len() < 4 {
        return None;
    }
    let total: u64 = vals.iter().sum();
    let idle = vals[3] + vals.get(4).copied().unwrap_or(0); // idle + iowait
    Some((total.saturating_sub(idle), total))
}

/// Busy CPU percent (0..100) between two /proc/stat snapshots.
fn cpu_busy_pct(prev: (u64, u64), now: (u64, u64)) -> i32 {
    let dt = now.1.saturating_sub(prev.1);
    if dt == 0 {
        return 0;
    }
    let db = now.0.saturating_sub(prev.0);
    (db.saturating_mul(100) / dt) as i32
}

/// Hottest coretemp core in whole °C (0 if no coretemp sensors).
fn hottest_core() -> i32 {
    let mut max = 0;
    let Ok(dir) = fs::read_dir("/sys/class/hwmon") else { return 0 };
    for ent in dir.flatten() {
        let p = ent.path();
        if fs::read_to_string(p.join("name")).unwrap_or_default().trim() != "coretemp" {
            continue;
        }
        let Ok(inner) = fs::read_dir(&p) else { continue };
        for f in inner.flatten() {
            let n = f.file_name().to_string_lossy().to_string();
            if n.starts_with("temp") && n.ends_with("_input") {
                if let Some(v) = read_i32(&f.path().to_string_lossy()) {
                    max = max.max(v / 1000);
                }
            }
        }
    }
    max
}

/// Linear fan-curve duty (0..255) for a temperature.
fn fan_duty(cfg: &GovConfig, t: i32) -> i32 {
    let (tlo, thi) = (cfg.fan_temp_lo, cfg.fan_temp_hi.max(cfg.fan_temp_lo + 1));
    let (dlo, dhi) = (cfg.fan_duty_lo, cfg.fan_duty_hi);
    let pct = if t <= tlo {
        dlo
    } else if t >= thi {
        dhi
    } else {
        dlo + (dhi - dlo) * (t - tlo) / (thi - tlo)
    };
    pct.clamp(0, 100) * 255 / 100
}

/// Final radiator-fan duty (0..255) for a temperature and CPU busy%.
///
/// Below `load_hi` (or with the boost disabled) it's just the temperature
/// curve. At/above `load_hi` the fan gets an immediate `load_boost`-point notch
/// on top of the curve, then ramps the rest of the way to FULL (255) as load
/// climbs to 100% — a pinned CPU is always driven to maximum cooling, audibly,
/// before the heat reaches the cores.
fn fan_target(cfg: &GovConfig, t: i32, busy: i32) -> i32 {
    let curve = fan_duty(cfg, t);
    if cfg.load_hi <= 0 || busy < cfg.load_hi {
        return curve;
    }
    let span = (100 - cfg.load_hi).max(1);
    let frac = (busy - cfg.load_hi).clamp(0, span);
    let notch = cfg.load_boost.max(0) * 255 / 100; // points -> PWM units
    let base = (curve + notch).min(255);
    (base + (255 - base) * frac / span).min(255)
}

/// Spawn the governor thread if enabled. No-op otherwise.
pub fn serve(cfg: GovConfig) {
    if !cfg.enabled {
        return;
    }
    thread::spawn(move || gov_loop(cfg));
}

fn gov_loop(cfg: GovConfig) {
    let nct = nct_dir();
    let mut cur = read_i32(PCT).unwrap_or(100);
    let mut prev_cpu = cpu_jiffies();
    eprintln!(
        "monit: thermal governor active (nct={}, pstate={}%, pump={}, fan={})",
        nct.is_some(), cur, cfg.pump_pwm, cfg.fan_pwm
    );
    loop {
        let t = hottest_core();

        // CPU busy% across this interval (0 until we have two samples).
        let busy = match (prev_cpu, cpu_jiffies()) {
            (Some(p), Some(n)) => {
                prev_cpu = Some(n);
                cpu_busy_pct(p, n)
            }
            (_, n) => {
                prev_cpu = n;
                0
            }
        };

        if let Some(ref h) = nct {
            // Pump always full — never throttle the coolant flow.
            if !cfg.pump_pwm.is_empty() {
                write_i32(&format!("{h}/{}_enable", cfg.pump_pwm), 1);
                write_i32(&format!("{h}/{}", cfg.pump_pwm), 255);
            }
            // Radiator fan: temperature curve, ramped hard toward full when the
            // CPU is busy (see fan_target). Re-assert enable+duty every tick so
            // the Super-I/O chip can't drift the header back to its own control.
            if !cfg.fan_pwm.is_empty() && t > 0 {
                let duty = fan_target(&cfg, t, busy);
                write_i32(&format!("{h}/{}_enable", cfg.fan_pwm), 1);
                write_i32(&format!("{h}/{}", cfg.fan_pwm), duty);
            }
        }

        // CPU band via intel_pstate max_perf_pct.
        if cfg.cpu_govern && t > 0 {
            let mut new = cur;
            if t >= cfg.t_emerg {
                new = cfg.perf_min;
            } else if t >= cfg.t_crit {
                new = cur - 15;
            } else if t >= cfg.t_high {
                new = cur - 7;
            } else if t < cfg.t_ok {
                new = cur + 3;
            }
            new = new.clamp(cfg.perf_min, 100);
            if new != cur {
                write_i32(PCT, new);
                eprintln!("monit: thermal governor temp={t}C max_perf_pct {cur} -> {new}");
                cur = new;
            }
        }

        thread::sleep(Duration::from_secs(cfg.interval_s.max(1)));
    }
}
