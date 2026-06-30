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
- Current: **0.9.0** (pre-1.0; defined in `Cargo.toml`).

## Layout of the code
- `src/config.rs` — config-file + env loader (keeps infra out of the source).
- `src/font.rs` — PSF1/PSF2 loader (Terminus fonts embedded from `assets/`).
- `src/fb.rs` — framebuffer surface, draw primitives (rect/text/bar/graph), VT.
- `src/collect.rs` — local `/proc`+`/sys`+`pvesm`+`journalctl`+RAPL/cpufreq and
  remote SSH blob collection & parsing (mem/cpu/temp/fan/disk/docker/logs/gpu,
  per-process GPU via `nvidia-smi pmon`).
- `src/history.rs` — ring buffers feeding the graphs.
- `src/api.rs` — REST server (background thread) + declarative page model for
  app-pushed pages (TTL store, optional bearer token).
- `src/governor.rs` — opt-in closed-loop thermal governor (background thread):
  pump full + dynamic radiator-fan curve + CPU temp-band via intel_pstate.
- `src/ui.rs` — page enum + per-page rendering (Mem/Cpu/Temp/Disk/Gpu/Ai/Logs)
  + app-pushed widget rendering; `Screen` rotation type.
- `src/main.rs` — config, signal handling, API startup, page rotation, loop.

## Work plan
- [x] Framebuffer renderer + collectors + memory dashboard (v0.1.0).
- [x] Multi-page rotation + graphs: CPU, temps, disk, AI workload, logs (v0.2.0).
- [x] Fan/pump RPM on temp page; °C/°F option; always-on thermal banner (v0.2.1).
- [x] Move site-specific values to a config file for a public repo (v0.3.0).
- [x] Deep GPU page (clocks/power/throttle/per-process SM), RAPL CPU package
      power + AIO verdict, REST API for app-pushed pages, drop empty Logs page
      from rotation (v0.4.0).

## REST API (app-pushed pages)
- `POST /api/v1/pages` — upsert a page `{id, title, ttl_secs?, widgets[]}`.
  Widget types: `heading`, `text`, `bar`, `graph`, `table`. Colors are names
  (green/yellow/red/accent/gpu/power/dim/text) or `#rrggbb`.
- `GET /api/v1/pages` — list; `GET /api/v1/pages/{id}` — echo; `DELETE` — remove.
- `GET /healthz` — liveness (no auth). `ttl_secs:0` = never expire; default 60s.
- `GET/POST /api/v1/power` — RAPL CPU package power control (gated by
  `api_control`, default on). POST `{limit_w}`, `{scale}`, or `{restore:true}`.
  Used for UPS-overload throttling; clamps to the domain min/max.
- Bind via `api_bind` (default `0.0.0.0:9090`, `off` disables); optional
  `api_token` bearer.

## Build note (macOS cross)
- On the dev mac, the host linker can't emit a Linux static binary. A musl
  cross-linker is installed (`x86_64-linux-musl-gcc`, Homebrew). Build with:
  `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc \`
  `CC_x86_64_unknown_linux_musl=x86_64-linux-musl-gcc \`
  `cargo build --release --target x86_64-unknown-linux-musl`
  (On a native x86_64 Linux build host, plain `cargo build --release --target
  x86_64-unknown-linux-musl` works as before.)

## Notes / gotchas
- ioctl request arg type differs by libc (c_int on musl) — cast `KDSETMODE as _`.
- Pixels packed `0x00RRGGBB`; on LE 32bpp XRGB that lands as B,G,R,X — correct.
- Service `Conflicts=getty@tty1.service` to own the console display.
- Embedded Terminus font lacks `°`; use plain ` C`/` F`.
- Fan/pump RPM needs the Super I/O driver (e.g. `nct6775`); persist via
  `/etc/modules-load.d/`.
