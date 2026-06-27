# Changelog

## [Unreleased]

## [v0.7.0] — 2026-06-27

### Added
- **Overview** is now the sole default view (rotation dropped), full-screen with
  no title bar/banner. Top half: **CPU** (left) and **GPU** (right) with big
  USAGE % and TEMP numbers, a memory/VRAM bar, a **LIMITING** line, and a boxed
  **COOLING** verdict that flags a stopped AIO pump (`PUMP STOPPED!`) or GPU
  `THERMAL THROTTLE!`.
- **LCARS history panel** on the bottom half — black ground, rounded header bar,
  two **device sections** (CPU/GPU), each with **PERF % / TEMP** (utilization +
  white temp line) and **MEMORY / VRAM %** graphs over time, colored by device
  (CPU = ice blue, GPU = amber). New `Fb::fill_round` and `Fb::graph_multi`.
- **Closed-loop thermal governor** (`governor.rs`, opt-in via `thermal_control`)
  — folds the standalone adaptive-thermal.sh into monit. Forces the AIO pump to
  full, drives the radiator fan on a temperature curve (`gov_temp_lo/hi`,
  `gov_duty_lo/hi`), and holds a CPU temperature band via intel_pstate
  `max_perf_pct` (`gov_t_*`, `gov_perf_min`). PROCHOT (~100 °C) is the hardware
  backstop; `monit.service` runs `Restart=always` and `modprobe nct6775`.
- **Viewport config** (`view_x`/`view_y`/`view_w`/`view_h`) and `overscan` so the
  overview fits panels that crop their edges or display only part of the
  framebuffer.
- **Fan config** (`fan_labels`, `pump_fan`) for boards whose Super I/O exposes no
  fan labels (e.g. nct6798); the COOLING verdict watches the configured pump
  channel for a 0-rpm stall.
- CPU **LIMITING** line distinguishes stock-TDP protection from an imposed RAPL
  throttle (`pkg_max_w`) or a governor pstate cap (`Power.perf_pct` →
  `THROTTLED perf N%`).

### Fixed
- Documented `video=HDMI-A-1:1920x1080@60e` as the host-side fix for a
  framebuffer that won't scan out after cold-booting with no panel attached
  (forces the display pipe up at init regardless of attach state).

## [v0.4.1] — 2026-06-27

### Fixed
- **Blank/DPMS panel stayed dark.** On startup `Fb::open` now issues an
  `FBIOBLANK` / `FB_BLANK_UNBLANK` ioctl to wake the display before taking the
  console into graphics mode. If the panel had DPMS-powered-down (e.g. while the
  service was down or the framebuffer was missing), the CRTC scanout was off, so
  writes to `/dev/fb0` never reached the screen and the monitor sat "trying to
  sync." Unblanking re-enables scanout; it's a no-op when already awake.

## [v0.4.0] — 2026-06-14

### Added
- **GPU page** (dedicated, full-width) focused on what the accelerators are
  actually doing: per-GPU SM-utilization headline, VRAM / power-vs-cap / temp
  meters, SM & memory clocks, PCIe link (gen×width), performance state, and an
  explicit **throttle status** (decoded from `clocks_throttle_reasons.active` —
  SW/HW power cap, SW/HW thermal, power brake). Four history graphs (SM util,
  VRAM, power-% of cap, temperature) plus a per-process **SM% / VRAM** table.
- **Per-process GPU utilization** via `nvidia-smi pmon` (SM/enc/dec per PID),
  merged with `--query-compute-apps` memory by PID.
- Expanded `nvidia-smi --query-gpu` set: memory-controller utilization, SM/mem
  clocks, power limit, fan %, memory temperature, pstate, PCIe gen/width, and
  active throttle reasons. Falls back gracefully on older `nvidia-smi`.
- **CPU package power (RAPL)** + average/max core frequency on the local host
  (Intel `intel-rapl` and AMD via the same powercap framework), sampled around
  the existing `/proc/stat` window. Shown on the CPU page.
- **AIO / CPU-thermal verdict** on the Temperatures page: package power vs cap,
  clock, pump RPM, and a heuristic that distinguishes "the AIO is dissipating
  real heat" (high sustained draw held at a safe temperature) from "we're just
  holding power low" (low draw → low temp regardless of cooling). Flags a
  stopped pump.
- **REST API** (`api.rs`) so apps can push their own pages into the rotation: a
  declarative page (title + widgets: heading / text / bar / graph / table) is
  `POST`ed to `/api/v1/pages` and rendered with monit's primitives. `GET` lists
  and echoes pages, `DELETE` removes them, `/healthz` for liveness. Pages expire
  on a TTL (re-POST to refresh). Optional bearer-token auth; bind address
  configurable (`api_bind`, default `0.0.0.0:9090`; `off` disables). Untrusted
  input is bounded (body/widget/series size caps). New deps: `serde`,
  `serde_json`.
- Config keys `api_bind` / `api_token` (env: `MONIT_API_BIND` / `MONIT_API_TOKEN`).
- **Power-control endpoint** `/api/v1/power` (gated by `api_control`, default on):
  `GET` reports live draw + cap + bounds; `POST` caps the CPU package power via
  RAPL — `{"limit_w":80}`, `{"scale":0.5}` (halve the current cap), or
  `{"restore":true}` (back to the cap captured at startup). Built for UPS-overload
  events; monit runs as root so it applies the cap directly. New caps clamp to the
  domain's min/max. Config `api_control` (env `MONIT_API_CONTROL`).

### Changed
- Page rotation now spans built-in **and** live app-pushed pages; the page-dot
  row adapts its spacing to the number of pages.
- The **Logs** page is dropped from the rotation when both hosts report no
  recent journal errors, so it no longer wastes a rotation slot.
- GPU processes are sorted by SM utilization, then resident VRAM.

## [v0.3.0] — 2026-06-13

### Added
- Runtime config file (`/etc/monit/monit.conf`, override via `MONIT_CONFIG`)
  holding all site-specific values (ssh target, labels, intervals, temp unit).
  `deploy/monit.conf.example` ships as a template. Environment variables still
  override the file.

### Changed
- Removed hardcoded hostnames from the source, systemd unit, and docs so the
  repository can be public. Local-host label now defaults to the system
  hostname; remote label defaults to the host part of `ai_host`.

## [v0.2.1] — 2026-06-13

### Added
- Always-on thermal banner in the title bar of **every** page: pve CPU temp and
  ai GPU temp, color-coded (green/amber/red). Keeps the most safety-critical
  reading visible regardless of which page is showing.

## [v0.2.0] — 2026-06-13

### Added
- Rotating multi-page dashboard. Pages: **Memory**, **CPU**, **Temperatures**,
  **Disk**, **AI workload**, **Kernel/log errors**. Rotation interval is
  `MONIT_PAGE_SECS` (default 8s); a dot row in the title shows the active page.
- Time-series **graphs** (area charts) with per-metric history ring buffers:
  memory %, CPU %, hottest temperature, and GPU VRAM % / utilization %.
- **CPU page**: overall busy %, load average, core count, history graph, and a
  per-core utilization grid.
- **Temperature page**: per-sensor bars (coretemp, nvme, GPU…) colored by temp,
  hottest-temp history graph, and a **Fans / Pump (rpm)** block that flags any
  0-rpm channel as `STOPPED` in red.
- **Disk page**: pve Proxmox storages (via `pvesm status`) and ai filesystems
  (via `df`) with usage bars.
- **AI workload page**: running Docker containers, a derived **model** badge
  (e.g. "Qwen 7B" from the training command / image), GPU VRAM/util/temp/power,
  GPU VRAM & util history graphs, the running command line, and GPU processes.
- **Kernel/log errors page**: recent `journalctl -p err` lines per host.
- `MONIT_TEMP_UNIT=C|F` to display temperatures in Celsius or Fahrenheit.

### Changed
- Remote SSH collector extended to a single delimited blob carrying meminfo,
  top procs, two `/proc/stat` samples, loadavg, hwmon temps + fans, `df`,
  `docker ps`, `journalctl`, and `nvidia-smi` (now including temp + power).
- Dropped the `°` glyph (absent from the embedded Terminus font) in favor of a
  plain ` C` / ` F` suffix.

## [v0.1.0] — 2026-06-13

### Added
- Initial framebuffer memory dashboard (`monit`). Renders directly to
  `/dev/fb0` on the hypervisor host — no X/web. RAM usage + top consumers for
  the local host and a remote GPU host (over SSH), plus per-GPU memory and GPU
  process usage via `nvidia-smi`.
- Embedded Terminus PSF fonts (8×16, 16×32) with a minimal PSF1/PSF2 loader and
  integer-scaled bitmap text renderer.
- VT `KD_GRAPHICS` takeover while running; text mode restored on exit.
- systemd unit conflicting with `getty@tty1` to own the attached display.
- Static `x86_64-unknown-linux-musl` build.
