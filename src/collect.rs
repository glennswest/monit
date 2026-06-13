//! Gather stats. The local host (pve) is read from /proc, /sys/class/hwmon,
//! pvesm and journalctl; the remote GPU host (ai) is read over a single SSH
//! call that emits a delimited blob we parse into sections.
//!
//! Each collection is self-contained: CPU utilization is measured by sampling
//! /proc/stat twice with a short sleep, so no cross-cycle state is needed.

use std::fs;
use std::process::Command;
use std::time::Duration;

const SAMPLE_MS: u64 = 250;

#[derive(Default, Clone)]
pub struct Mem {
    pub total_kb: u64,
    pub avail_kb: u64,
    pub free_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

impl Mem {
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.avail_kb)
    }
    pub fn frac(&self) -> f64 {
        if self.total_kb == 0 { 0.0 } else { self.used_kb() as f64 / self.total_kb as f64 }
    }
}

#[derive(Default, Clone)]
pub struct Cpu {
    pub overall: f64,
    pub per_core: Vec<f64>,
    pub load: [f64; 3],
    pub cores: usize,
}

#[derive(Clone)]
pub struct Proc {
    pub name: String,
    pub rss_kb: u64,
}

#[derive(Clone)]
pub struct Temp {
    pub label: String,
    pub celsius: f64,
}

#[derive(Clone)]
pub struct Fan {
    pub label: String,
    pub rpm: u64,
}

#[derive(Clone)]
pub struct Disk {
    pub name: String,
    pub used: u64,  // bytes
    pub total: u64, // bytes
}

impl Disk {
    pub fn frac(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.used as f64 / self.total as f64 }
    }
}

#[derive(Clone)]
pub struct Container {
    pub name: String,
    pub image: String,
    pub status: String,
}

#[derive(Clone)]
pub struct Gpu {
    pub idx: u32,
    pub name: String,
    pub used_mb: u64,
    pub total_mb: u64,
    pub util: u32,
    pub temp_c: f64,
    pub power_w: f64,
}

impl Gpu {
    pub fn frac(&self) -> f64 {
        if self.total_mb == 0 { 0.0 } else { self.used_mb as f64 / self.total_mb as f64 }
    }
}

#[derive(Clone)]
pub struct GpuProc {
    pub name: String,
    pub mem_mb: u64,
}

#[derive(Clone)]
pub struct Host {
    pub label: String,
    pub ok: bool,
    pub err: String,
    pub mem: Mem,
    pub cpu: Cpu,
    pub procs: Vec<Proc>,
    pub temps: Vec<Temp>,
    pub fans: Vec<Fan>,
    pub disks: Vec<Disk>,
    pub logs: Vec<String>,
    pub containers: Vec<Container>,
    pub workload: Vec<String>, // GPU process command lines
    pub model: String,         // derived model hint
    pub gpus: Vec<Gpu>,
    pub gpu_procs: Vec<GpuProc>,
}

impl Host {
    fn new(label: &str) -> Host {
        Host {
            label: label.to_string(),
            ok: false,
            err: String::new(),
            mem: Mem::default(),
            cpu: Cpu::default(),
            procs: vec![],
            temps: vec![],
            fans: vec![],
            disks: vec![],
            logs: vec![],
            containers: vec![],
            workload: vec![],
            model: String::new(),
            gpus: vec![],
            gpu_procs: vec![],
        }
    }
    /// Hottest sensor temperature in °C (0 if none).
    pub fn max_temp(&self) -> f64 {
        let s = self.temps.iter().map(|t| t.celsius).fold(0.0, f64::max);
        let g = self.gpus.iter().map(|g| g.temp_c).fold(0.0, f64::max);
        s.max(g)
    }
}

fn parse_meminfo(text: &str) -> Mem {
    let mut m = Mem::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "MemTotal:" => m.total_kb = val,
            "MemAvailable:" => m.avail_kb = val,
            "MemFree:" => m.free_kb = val,
            "Buffers:" => m.buffers_kb = val,
            "Cached:" => m.cached_kb = val,
            "SwapTotal:" => m.swap_total_kb = val,
            "SwapFree:" => m.swap_free_kb = val,
            _ => {}
        }
    }
    m
}

fn parse_stat(text: &str) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if !line.starts_with("cpu") {
            continue;
        }
        let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
        if nums.len() < 4 {
            continue;
        }
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
        let total: u64 = nums.iter().sum();
        out.push((total.saturating_sub(idle), total));
    }
    out
}

fn cpu_from_samples(s1: &[(u64, u64)], s2: &[(u64, u64)], load: [f64; 3]) -> Cpu {
    let mut usages = Vec::new();
    for i in 0..s1.len().min(s2.len()) {
        let db = s2[i].0.saturating_sub(s1[i].0) as f64;
        let dt = s2[i].1.saturating_sub(s1[i].1) as f64;
        usages.push(if dt > 0.0 { (db / dt).clamp(0.0, 1.0) } else { 0.0 });
    }
    let overall = usages.first().copied().unwrap_or(0.0);
    let per_core: Vec<f64> = usages.into_iter().skip(1).collect();
    let cores = per_core.len();
    Cpu { overall, per_core, load, cores }
}

fn parse_loadavg(text: &str) -> [f64; 3] {
    let mut it = text.split_whitespace();
    [
        it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
        it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
        it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0),
    ]
}

fn parse_temp_line(line: &str) -> Option<Temp> {
    let mut f = line.split('|');
    let name = f.next()?.trim();
    let label = f.next().unwrap_or("").trim();
    let milli: f64 = f.next()?.trim().parse().ok()?;
    let celsius = milli / 1000.0;
    if celsius <= 0.0 || celsius > 150.0 {
        return None;
    }
    let disp = if label.is_empty() { name.to_string() } else { format!("{name} {label}") };
    Some(Temp { label: clip(&disp, 26), celsius })
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn tidy_name(name: &str) -> String {
    let n = name.trim();
    let base = n.rsplit('/').next().unwrap_or(n);
    clip(base, 28)
}

/// Heuristic: derive a model name from GPU command lines + container images.
fn derive_model(cmds: &[String], containers: &[Container]) -> String {
    let mut hay = cmds.join(" ").to_lowercase();
    for c in containers {
        hay.push(' ');
        hay.push_str(&c.image.to_lowercase());
        hay.push(' ');
        hay.push_str(&c.name.to_lowercase());
    }
    let families = [
        ("qwen", "Qwen"), ("llama", "Llama"), ("mixtral", "Mixtral"),
        ("mistral", "Mistral"), ("deepseek", "DeepSeek"), ("gemma", "Gemma"),
        ("phi", "Phi"), ("falcon", "Falcon"), ("yi", "Yi"), ("gpt", "GPT"),
    ];
    let sizes = ["72b", "70b", "34b", "32b", "14b", "13b", "8b", "7b", "4b", "3b", "2b", "1b"];
    let fam = families.iter().find(|(k, _)| hay.contains(k)).map(|(_, v)| *v);
    let size = sizes.iter().find(|s| hay.contains(**s));
    match (fam, size) {
        (Some(f), Some(s)) => format!("{f} {}", s.to_uppercase()),
        (Some(f), None) => f.to_string(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Local (pve) collection
// ---------------------------------------------------------------------------

pub fn local(label: &str, top: usize) -> Host {
    let mut h = Host::new(label);
    h.mem = parse_meminfo(&fs::read_to_string("/proc/meminfo").unwrap_or_default());

    let s1 = parse_stat(&fs::read_to_string("/proc/stat").unwrap_or_default());
    std::thread::sleep(Duration::from_millis(SAMPLE_MS));
    let s2 = parse_stat(&fs::read_to_string("/proc/stat").unwrap_or_default());
    let load = parse_loadavg(&fs::read_to_string("/proc/loadavg").unwrap_or_default());
    h.cpu = cpu_from_samples(&s1, &s2, load);

    h.procs = local_procs();
    h.procs.sort_by(|a, b| b.rss_kb.cmp(&a.rss_kb));
    h.procs.truncate(top);

    h.temps = local_temps();
    h.fans = local_fans();
    h.disks = local_disks();
    h.logs = local_logs();

    h.ok = h.mem.total_kb > 0;
    h
}

fn local_procs() -> Vec<Proc> {
    let mut out = Vec::new();
    let dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return out,
    };
    for ent in dir.flatten() {
        let pid = match ent.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => p,
            None => continue,
        };
        let status = match fs::read_to_string(format!("/proc/{pid}/status")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut name = String::new();
        let mut rss_kb = 0u64;
        for line in status.lines() {
            if let Some(v) = line.strip_prefix("Name:") {
                name = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("VmRSS:") {
                rss_kb = v.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0);
            }
        }
        if rss_kb == 0 {
            continue;
        }
        if name.starts_with("kvm") || name.starts_with("qemu") {
            if let Ok(cmd) = fs::read_to_string(format!("/proc/{pid}/cmdline")) {
                name = qemu_label(&cmd).unwrap_or(name);
            }
        }
        out.push(Proc { name: tidy_name(&name), rss_kb });
    }
    out
}

fn qemu_label(cmdline: &str) -> Option<String> {
    let args: Vec<&str> = cmdline.split('\0').collect();
    let mut id = String::new();
    let mut gname = String::new();
    for i in 0..args.len() {
        if args[i] == "-id" && i + 1 < args.len() {
            id = args[i + 1].to_string();
        }
        if args[i] == "-name" && i + 1 < args.len() {
            let raw = args[i + 1].strip_prefix("guest=").unwrap_or(args[i + 1]);
            gname = raw.split(',').next().unwrap_or(raw).to_string();
        }
    }
    match (id.is_empty(), gname.is_empty()) {
        (false, false) => Some(format!("VM {id} {gname}")),
        (false, true) => Some(format!("VM {id}")),
        (true, false) => Some(format!("VM {gname}")),
        _ => None,
    }
}

fn local_temps() -> Vec<Temp> {
    let mut out = Vec::new();
    let dir = match fs::read_dir("/sys/class/hwmon") {
        Ok(d) => d,
        Err(_) => return out,
    };
    for ent in dir.flatten() {
        let base = ent.path();
        let name = fs::read_to_string(base.join("name")).unwrap_or_default().trim().to_string();
        let inner = match fs::read_dir(&base) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for f in inner.flatten() {
            let fname = f.file_name().to_string_lossy().to_string();
            if !(fname.starts_with("temp") && fname.ends_with("_input")) {
                continue;
            }
            let stem = fname.trim_end_matches("_input");
            let milli = fs::read_to_string(f.path()).ok().and_then(|s| s.trim().parse::<f64>().ok());
            let label = fs::read_to_string(base.join(format!("{stem}_label")))
                .unwrap_or_default().trim().to_string();
            if let Some(milli) = milli {
                if let Some(t) = parse_temp_line(&format!("{name}|{label}|{milli}")) {
                    out.push(t);
                }
            }
        }
    }
    sort_temps(&mut out);
    out
}

fn local_fans() -> Vec<Fan> {
    let mut out = Vec::new();
    let dir = match fs::read_dir("/sys/class/hwmon") {
        Ok(d) => d,
        Err(_) => return out,
    };
    for ent in dir.flatten() {
        let base = ent.path();
        let name = fs::read_to_string(base.join("name")).unwrap_or_default().trim().to_string();
        let inner = match fs::read_dir(&base) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for f in inner.flatten() {
            let fname = f.file_name().to_string_lossy().to_string();
            if !(fname.starts_with("fan") && fname.ends_with("_input")) {
                continue;
            }
            let stem = fname.trim_end_matches("_input");
            let rpm = fs::read_to_string(f.path()).ok().and_then(|s| s.trim().parse::<u64>().ok());
            let label = fs::read_to_string(base.join(format!("{stem}_label")))
                .unwrap_or_default().trim().to_string();
            if let Some(rpm) = rpm {
                let lbl = if label.is_empty() { format!("{name} {stem}") } else { format!("{name} {label}") };
                out.push(Fan { label: clip(&lbl, 24), rpm });
            }
        }
    }
    out
}

fn sort_temps(v: &mut Vec<Temp>) {
    v.sort_by(|a, b| b.celsius.partial_cmp(&a.celsius).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(10);
}

/// pve storages via `pvesm status` (more meaningful than raw df here).
fn local_disks() -> Vec<Disk> {
    let mut out = Vec::new();
    let res = Command::new("pvesm").arg("status").output();
    if let Ok(o) = res {
        let text = String::from_utf8_lossy(&o.stdout);
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            // Name Type Status Total Used Available %
            if f.len() >= 6 && f[2] == "active" {
                let total_kib: u64 = f[3].parse().unwrap_or(0);
                let used_kib: u64 = f[4].parse().unwrap_or(0);
                if total_kib > 0 {
                    out.push(Disk { name: clip(f[0], 22), used: used_kib * 1024, total: total_kib * 1024 });
                }
            }
        }
    }
    out.sort_by(|a, b| b.frac().partial_cmp(&a.frac()).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(9);
    out
}

fn local_logs() -> Vec<String> {
    let res = Command::new("journalctl")
        .args(["-p", "err", "-b", "--no-pager", "-n", "10", "-o", "short"])
        .output();
    let mut out = Vec::new();
    if let Ok(o) = res {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            if !line.trim().is_empty() {
                out.push(clip(line, 86));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Remote (ai) collection over SSH
// ---------------------------------------------------------------------------

const REMOTE_SCRIPT: &str = "\
cat /proc/meminfo
echo '@@PROCS@@'
ps -eo rss=,comm= --sort=-rss 2>/dev/null | head -n 10
echo '@@STAT1@@'
cat /proc/stat
sleep 0.25
echo '@@STAT2@@'
cat /proc/stat
echo '@@LOAD@@'
cat /proc/loadavg
echo '@@TEMP@@'
for h in /sys/class/hwmon/hwmon*; do n=$(cat $h/name 2>/dev/null); for t in $h/temp*_input; do [ -e \"$t\" ] || continue; lf=${t%_input}_label; l=$(cat $lf 2>/dev/null); v=$(cat $t 2>/dev/null); echo \"$n|$l|$v\"; done; done
echo '@@FAN@@'
for h in /sys/class/hwmon/hwmon*; do n=$(cat $h/name 2>/dev/null); for fch in $h/fan*_input; do [ -e \"$fch\" ] || continue; lf=${fch%_input}_label; l=$(cat $lf 2>/dev/null); v=$(cat $fch 2>/dev/null); echo \"$n|$l|$v\"; done; done
echo '@@DISK@@'
df -B1 --output=target,size,used -x tmpfs -x devtmpfs -x overlay 2>/dev/null | tail -n +2
echo '@@DOCKER@@'
docker ps --format '{{.Names}}|{{.Image}}|{{.Status}}' 2>/dev/null
echo '@@LOG@@'
journalctl -p err -b --no-pager -n 10 -o short 2>/dev/null
echo '@@GPU@@'
nvidia-smi --query-gpu=index,name,memory.used,memory.total,utilization.gpu,temperature.gpu,power.draw --format=csv,noheader,nounits 2>/dev/null
echo '@@GPUPROCS@@'
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>/dev/null
echo '@@GPUCMD@@'
for pid in $(nvidia-smi --query-compute-apps=pid --format=csv,noheader,nounits 2>/dev/null); do tr '\\0' ' ' < /proc/$pid/cmdline 2>/dev/null | cut -c1-180; echo; done";

pub fn remote(label: &str, ssh_host: &str, top: usize) -> Host {
    let mut h = Host::new(label);
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "ConnectTimeout=6",
            ssh_host,
            REMOTE_SCRIPT,
        ])
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => {
            h.err = format!("ssh spawn: {e}");
            return h;
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        h.err = err.lines().last().unwrap_or("ssh failed").to_string();
        return h;
    }
    parse_remote(&mut h, &String::from_utf8_lossy(&out.stdout), top);
    h.ok = h.mem.total_kb > 0;
    if !h.ok && h.err.is_empty() {
        h.err = "no data".to_string();
    }
    h.model = derive_model(&h.workload, &h.containers);
    h
}

fn parse_remote(h: &mut Host, text: &str, top: usize) {
    let mut sec = "MEM";
    let mut meminfo = String::new();
    let mut stat1 = String::new();
    let mut stat2 = String::new();
    let mut load = [0.0; 3];
    for line in text.lines() {
        match line.trim() {
            "@@PROCS@@" => { sec = "PROCS"; continue; }
            "@@STAT1@@" => { sec = "STAT1"; continue; }
            "@@STAT2@@" => { sec = "STAT2"; continue; }
            "@@LOAD@@" => { sec = "LOAD"; continue; }
            "@@TEMP@@" => { sec = "TEMP"; continue; }
            "@@FAN@@" => { sec = "FAN"; continue; }
            "@@DISK@@" => { sec = "DISK"; continue; }
            "@@DOCKER@@" => { sec = "DOCKER"; continue; }
            "@@LOG@@" => { sec = "LOG"; continue; }
            "@@GPU@@" => { sec = "GPU"; continue; }
            "@@GPUPROCS@@" => { sec = "GPUPROCS"; continue; }
            "@@GPUCMD@@" => { sec = "GPUCMD"; continue; }
            _ => {}
        }
        match sec {
            "MEM" => { meminfo.push_str(line); meminfo.push('\n'); }
            "PROCS" => {
                let mut it = line.split_whitespace();
                if let (Some(rss), Some(comm)) = (it.next(), it.next()) {
                    if let Ok(rss_kb) = rss.parse::<u64>() {
                        h.procs.push(Proc { name: tidy_name(comm), rss_kb });
                    }
                }
            }
            "STAT1" => { stat1.push_str(line); stat1.push('\n'); }
            "STAT2" => { stat2.push_str(line); stat2.push('\n'); }
            "LOAD" => { load = parse_loadavg(line); }
            "TEMP" => { if let Some(t) = parse_temp_line(line) { h.temps.push(t); } }
            "FAN" => {
                let f: Vec<&str> = line.split('|').collect();
                if f.len() >= 3 {
                    let name = f[0].trim();
                    let label = f[1].trim();
                    if let Ok(rpm) = f[2].trim().parse::<u64>() {
                        let lbl = if label.is_empty() { name.to_string() } else { format!("{name} {label}") };
                        h.fans.push(Fan { label: clip(&lbl, 24), rpm });
                    }
                }
            }
            "DISK" => {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() >= 3 {
                    let total: u64 = f[1].parse().unwrap_or(0);
                    let used: u64 = f[2].parse().unwrap_or(0);
                    if total > 0 {
                        h.disks.push(Disk { name: clip(f[0], 22), used, total });
                    }
                }
            }
            "DOCKER" => {
                let f: Vec<&str> = line.split('|').collect();
                if f.len() >= 3 && !f[0].is_empty() {
                    h.containers.push(Container {
                        name: clip(f[0], 22),
                        image: clip(f[1], 26),
                        status: clip(f[2], 20),
                    });
                }
            }
            "LOG" => { if !line.trim().is_empty() { h.logs.push(clip(line, 86)); } }
            "GPU" => {
                let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if f.len() >= 7 {
                    h.gpus.push(Gpu {
                        idx: f[0].parse().unwrap_or(0),
                        name: f[1].to_string(),
                        used_mb: f[2].parse().unwrap_or(0),
                        total_mb: f[3].parse().unwrap_or(0),
                        util: f[4].parse().unwrap_or(0),
                        temp_c: f[5].parse().unwrap_or(0.0),
                        power_w: f[6].parse().unwrap_or(0.0),
                    });
                }
            }
            "GPUPROCS" => {
                let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if f.len() >= 3 {
                    h.gpu_procs.push(GpuProc { name: tidy_name(f[1]), mem_mb: f[2].parse().unwrap_or(0) });
                }
            }
            "GPUCMD" => { if !line.trim().is_empty() { h.workload.push(clip(line.trim(), 120)); } }
            _ => {}
        }
    }
    h.mem = parse_meminfo(&meminfo);
    h.cpu = cpu_from_samples(&parse_stat(&stat1), &parse_stat(&stat2), load);
    h.procs.truncate(top);
    sort_temps(&mut h.temps);
    h.gpu_procs.sort_by(|a, b| b.mem_mb.cmp(&a.mem_mb));
    h.gpu_procs.truncate(top);
}
