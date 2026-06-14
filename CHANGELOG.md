# Changelog

## [Unreleased]

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
