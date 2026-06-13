# CLAUDE.md — monit

Framebuffer memory dashboard for the **g8** cluster. Single static Rust binary,
runs as a systemd service on **pve.g8.lo**, draws to `/dev/fb0` (the attached
HDMI display). Reads pve locally from `/proc`; reads **ai.g8.lo** (GPU host)
over SSH (`nvidia-smi` + meminfo + ps).

## Topology / constants
- pve.g8.lo = 192.168.8.129 — Proxmox (Debian 13), 192 GB RAM, Intel iGPU fb0
  (1920×1080, 32bpp XRGB, stride 7680). Service host.
- ai.g8.lo = 192.168.8.140 — GPU host (RTX 5060 Ti, 16 GB), 64 GB RAM. Read-only.
- dev.g8.lo = VMID 110 on pve — **build host** (Fedora 43, cargo + musl target).
- root SSH works to all three; pve→ai key auth confirmed working.
- **Do not change anything on pve/ai beyond installing this service.**

## Build & deploy
- Build on dev: `ssh root@dev.g8.lo 'cd /root/monit-build && cargo build --release --target x86_64-unknown-linux-musl'`
- Source is synced to `dev.g8.lo:/root/monit-build` via rsync (exclude target/.git).
- Binary: `target/x86_64-unknown-linux-musl/release/monit` (static, ~530K).
- Deploy to pve: `/usr/local/bin/monit` + `/etc/systemd/system/monit.service`.

## Version
- Current: **0.2.0** (pre-1.0; defined in `Cargo.toml`).

## Layout of the code
- `src/font.rs` — PSF1/PSF2 loader (Terminus fonts embedded from `assets/`).
- `src/fb.rs` — framebuffer surface, draw primitives (rect/text/bar/graph), VT.
- `src/collect.rs` — local `/proc`+`/sys`+`pvesm`+`journalctl` and remote SSH
  blob collection & parsing (mem/cpu/temp/fan/disk/docker/logs/gpu).
- `src/history.rs` — ring buffers feeding the graphs.
- `src/ui.rs` — page enum + per-page rendering (Mem/Cpu/Temp/Disk/Ai/Logs).
- `src/main.rs` — config, signal handling, page rotation, refresh loop.

## Work plan
- [x] Probe access + display (fb0, not web — user has an LCD on pve).
- [x] Framebuffer renderer + collectors + memory dashboard (v0.1.0).
- [x] Multi-page rotation + graphs: CPU, temps, disk, AI workload, logs (v0.2.0).
- [x] Fan/pump RPM on temp page; °C/°F option.
- [x] Deploy + enable systemd service on pve.g8.lo; verified on screen.
- [ ] (Pending user OK) Load `nct6775` on pve to expose pump/fan tach.

## Important runtime finding (2026-06-13)
- pve CPU package is running at **100 °C (crit)** at light load. No fan/pump
  tach is exposed (only `coretemp` loaded; board = ASRock B760M PG Riptide,
  Nuvoton Super I/O needs `nct6775`). The AIO pump is on the CPU_FAN header and
  is the prime suspect for not circulating coolant. Needs physical check.

## Notes / gotchas
- ioctl request arg type differs by libc (c_int on musl) — cast `KDSETMODE as _`.
- Pixels packed `0x00RRGGBB`; on LE 32bpp XRGB that lands as B,G,R,X — correct.
- Service `Conflicts=getty@tty1.service` to own the console display.
- Embedded Terminus font lacks `°`; use plain ` C`/` F`.
