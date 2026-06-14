# CLAUDE.md — monit

Framebuffer system dashboard. Single static Rust binary, runs as a systemd
service on a Proxmox/KVM hypervisor host, draws to `/dev/fb0` (an attached
display). Reads the local host from `/proc` + `/sys`; reads a remote GPU host
over SSH (`nvidia-smi` + meminfo + ps + docker).

All site-specific values (hostnames, SSH target) live in a runtime config file
(`/etc/monit/monit.conf`, see `deploy/monit.conf.example`) — **never hardcode
real hostnames or IPs in the source or committed docs.** The repo is public.

## Roles (configure via /etc/monit/monit.conf, not in git)
- **Local host** — the hypervisor running the service; has the framebuffer
  (`/dev/fb0`, typically 1920×1080 32bpp XRGB) and an attached screen.
- **Remote GPU host** — read-only over SSH (`ai_host`); needs `nvidia-smi`.
- **Build host** — any x86_64 Linux with the Rust musl target + `musl-gcc`.
- The service host's root must be able to SSH non-interactively to the GPU host.

## Build & deploy
- Build static: `cargo build --release --target x86_64-unknown-linux-musl`.
- Binary: `target/x86_64-unknown-linux-musl/release/monit` (static, ~530K).
- Deploy: `/usr/local/bin/monit`, `/etc/monit/monit.conf`,
  `/etc/systemd/system/monit.service`. See README.

## Version
- Current: **0.3.0** (pre-1.0; defined in `Cargo.toml`).

## Layout of the code
- `src/config.rs` — config-file + env loader (keeps infra out of the source).
- `src/font.rs` — PSF1/PSF2 loader (Terminus fonts embedded from `assets/`).
- `src/fb.rs` — framebuffer surface, draw primitives (rect/text/bar/graph), VT.
- `src/collect.rs` — local `/proc`+`/sys`+`pvesm`+`journalctl` and remote SSH
  blob collection & parsing (mem/cpu/temp/fan/disk/docker/logs/gpu).
- `src/history.rs` — ring buffers feeding the graphs.
- `src/ui.rs` — page enum + per-page rendering (Mem/Cpu/Temp/Disk/Ai/Logs).
- `src/main.rs` — config, signal handling, page rotation, refresh loop.

## Work plan
- [x] Framebuffer renderer + collectors + memory dashboard (v0.1.0).
- [x] Multi-page rotation + graphs: CPU, temps, disk, AI workload, logs (v0.2.0).
- [x] Fan/pump RPM on temp page; °C/°F option; always-on thermal banner (v0.2.1).
- [x] Move site-specific values to a config file for a public repo (v0.3.0).

## Notes / gotchas
- ioctl request arg type differs by libc (c_int on musl) — cast `KDSETMODE as _`.
- Pixels packed `0x00RRGGBB`; on LE 32bpp XRGB that lands as B,G,R,X — correct.
- Service `Conflicts=getty@tty1.service` to own the console display.
- Embedded Terminus font lacks `°`; use plain ` C`/` F`.
- Fan/pump RPM needs the Super I/O driver (e.g. `nct6775`); persist via
  `/etc/modules-load.d/`.
