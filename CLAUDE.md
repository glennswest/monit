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
- Current: **0.1.0** (pre-1.0; defined in `Cargo.toml`).

## Layout of the code
- `src/font.rs` — PSF1/PSF2 loader (Terminus fonts embedded from `assets/`).
- `src/fb.rs` — framebuffer surface, draw primitives, VT KD_GRAPHICS handling.
- `src/collect.rs` — local `/proc` + remote SSH stat collection & parsing.
- `src/ui.rs` — two-panel dashboard layout/rendering.
- `src/main.rs` — config, signal handling, refresh loop.

## Work plan
- [x] Probe access + display (fb0, not web — user has an LCD on pve).
- [x] Implement framebuffer renderer + collectors + dashboard.
- [x] Build static musl binary on dev.g8.lo.
- [ ] Deploy + enable systemd service on pve.g8.lo; verify on the screen.
- [ ] Tune layout/readability after seeing it on the panel.

## Notes / gotchas
- ioctl request arg type differs by libc (c_int on musl) — cast `KDSETMODE as _`.
- Pixels packed `0x00RRGGBB`; on LE 32bpp XRGB that lands as B,G,R,X — correct.
- Service `Conflicts=getty@tty1.service` to own the console display.
