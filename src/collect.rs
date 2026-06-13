//! Gather memory stats. The local host (pve) is read straight from /proc; the
//! remote GPU host (ai) is read over a single SSH call that emits a delimited
//! blob we parse into sections.

use std::fs;
use std::process::Command;

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
        if self.total_kb == 0 {
            0.0
        } else {
            self.used_kb() as f64 / self.total_kb as f64
        }
    }
}

#[derive(Clone)]
pub struct Proc {
    pub name: String,
    pub rss_kb: u64,
}

#[derive(Clone)]
pub struct Gpu {
    pub idx: u32,
    pub name: String,
    pub used_mb: u64,
    pub total_mb: u64,
    pub util: u32,
}

impl Gpu {
    pub fn frac(&self) -> f64 {
        if self.total_mb == 0 {
            0.0
        } else {
            self.used_mb as f64 / self.total_mb as f64
        }
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
    pub procs: Vec<Proc>,
    pub gpus: Vec<Gpu>,
    pub gpu_procs: Vec<GpuProc>,
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

/// Shorten a qemu/long command name for display.
fn tidy_name(name: &str) -> String {
    let n = name.trim();
    let base = n.rsplit('/').next().unwrap_or(n);
    base.chars().take(28).collect()
}

/// Collect the local (pve) host directly from /proc.
pub fn local(label: &str, top: usize) -> Host {
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mem = parse_meminfo(&meminfo);
    let mut procs = local_procs();
    procs.sort_by(|a, b| b.rss_kb.cmp(&a.rss_kb));
    procs.truncate(top);
    Host {
        label: label.to_string(),
        ok: !meminfo.is_empty(),
        err: String::new(),
        mem,
        procs,
        gpus: vec![],
        gpu_procs: vec![],
    }
}

/// Walk /proc/[pid] collecting RSS, naming qemu guests by their Proxmox VMID.
fn local_procs() -> Vec<Proc> {
    let mut out = Vec::new();
    let dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return out,
    };
    for ent in dir.flatten() {
        let fname = ent.file_name();
        let pid = match fname.to_str().and_then(|s| s.parse::<u32>().ok()) {
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
        // Annotate qemu guests with their VMID / guest name from cmdline.
        if name.starts_with("kvm") || name.starts_with("qemu") {
            if let Ok(cmd) = fs::read_to_string(format!("/proc/{pid}/cmdline")) {
                let args: Vec<&str> = cmd.split('\0').collect();
                let mut id = String::new();
                let mut gname = String::new();
                for i in 0..args.len() {
                    if args[i] == "-id" && i + 1 < args.len() {
                        id = args[i + 1].to_string();
                    }
                    if args[i] == "-name" && i + 1 < args.len() {
                        let raw = args[i + 1];
                        let raw = raw.strip_prefix("guest=").unwrap_or(raw);
                        gname = raw.split(',').next().unwrap_or(raw).to_string();
                    }
                }
                name = match (id.is_empty(), gname.is_empty()) {
                    (false, false) => format!("VM {id} {gname}"),
                    (false, true) => format!("VM {id}"),
                    (true, false) => format!("VM {gname}"),
                    _ => name,
                };
            }
        }
        out.push(Proc { name: tidy_name(&name), rss_kb });
    }
    out
}

/// One SSH call collecting meminfo, top processes, and nvidia-smi output.
const REMOTE_SCRIPT: &str = "\
cat /proc/meminfo; echo '@@PROCS@@'; \
ps -eo rss=,comm= --sort=-rss 2>/dev/null | head -n 10; echo '@@GPU@@'; \
nvidia-smi --query-gpu=index,name,memory.used,memory.total,utilization.gpu --format=csv,noheader,nounits 2>/dev/null; \
echo '@@GPUPROCS@@'; \
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits 2>/dev/null";

/// Collect a remote host over SSH (host = "root@ai.g8.lo").
pub fn remote(label: &str, ssh_host: &str, top: usize) -> Host {
    let mut h = Host {
        label: label.to_string(),
        ok: false,
        err: String::new(),
        mem: Mem::default(),
        procs: vec![],
        gpus: vec![],
        gpu_procs: vec![],
    };
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
    let text = String::from_utf8_lossy(&out.stdout);
    parse_remote(&mut h, &text, top);
    h.ok = h.mem.total_kb > 0;
    if !h.ok && h.err.is_empty() {
        h.err = "no data".to_string();
    }
    h
}

fn parse_remote(h: &mut Host, text: &str, top: usize) {
    let mut section = 0; // 0 meminfo, 1 procs, 2 gpu, 3 gpuprocs
    let mut meminfo = String::new();
    for line in text.lines() {
        match line.trim() {
            "@@PROCS@@" => { section = 1; continue; }
            "@@GPU@@" => { section = 2; continue; }
            "@@GPUPROCS@@" => { section = 3; continue; }
            _ => {}
        }
        match section {
            0 => { meminfo.push_str(line); meminfo.push('\n'); }
            1 => {
                let mut it = line.split_whitespace();
                if let (Some(rss), Some(comm)) = (it.next(), it.next()) {
                    if let Ok(rss_kb) = rss.parse::<u64>() {
                        h.procs.push(Proc { name: tidy_name(comm), rss_kb });
                    }
                }
            }
            2 => {
                let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if f.len() >= 5 {
                    h.gpus.push(Gpu {
                        idx: f[0].parse().unwrap_or(0),
                        name: f[1].to_string(),
                        used_mb: f[2].parse().unwrap_or(0),
                        total_mb: f[3].parse().unwrap_or(0),
                        util: f[4].parse().unwrap_or(0),
                    });
                }
            }
            3 => {
                let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if f.len() >= 3 {
                    h.gpu_procs.push(GpuProc {
                        name: tidy_name(f[1]),
                        mem_mb: f[2].parse().unwrap_or(0),
                    });
                }
            }
            _ => {}
        }
    }
    h.mem = parse_meminfo(&meminfo);
    h.procs.truncate(top);
    h.gpu_procs.sort_by(|a, b| b.mem_mb.cmp(&a.mem_mb));
    h.gpu_procs.truncate(top);
}
