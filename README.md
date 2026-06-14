# monit — framebuffer cluster dashboard

A tiny single-binary Rust service that paints a live, rotating system dashboard
directly onto a display attached to a Linux host — no X server, no web browser,
just the Linux framebuffer (`/dev/fb0`). Built for a Proxmox/KVM hypervisor with
an attached screen, monitoring itself plus a remote GPU host over SSH.

It covers a **local host** and a **remote GPU host** (read over SSH), rotating
through pages every `page_secs`:

1. **Memory** — total/used RAM, free/cache/swap, usage history graph, and top
   consumers (KVM/QEMU guests are labelled by VMID + guest name).
2. **CPU** — overall busy %, load average, history graph, per-core grid.
3. **Temperatures** — per-sensor bars (coretemp/nvme/GPU), hottest-temp history,
   and a **Fans / Pump (rpm)** block that flags any 0-rpm channel as `STOPPED`.
4. **Disk** — Proxmox storages (`pvesm`) on the local host and filesystems
   (`df`) on the remote host.
5. **AI workload** — running Docker containers, a derived model badge (e.g.
   "Qwen 7B" inferred from the training command / image), GPU VRAM/util/temp/
   power with history graphs, the running command, and GPU processes.
6. **Kernel / log errors** — recent `journalctl -p err` lines per host.

An always-on thermal banner (local CPU + remote GPU temperature) sits in the
title bar of every page. Bar and graph colors shift green → yellow → red as
usage/temperature climbs.

## How it works

- Local memory/CPU/temps/fans are read straight from `/proc` and
  `/sys/class/hwmon`; storages from `pvesm`; errors from `journalctl`.
- The remote host is polled with a single SSH call that returns meminfo, top
  processes, two `/proc/stat` samples, loadavg, hwmon temps/fans, `df`,
  `docker ps`, `journalctl`, and `nvidia-smi` output, which `monit` parses.
- The dashboard is drawn into a back buffer and flushed to `/dev/fb0`.
- While running, the active VT is switched to `KD_GRAPHICS` so the kernel
  console stops drawing over the dashboard; text mode is restored on exit.

## Configuration

Settings live in a config file (default `/etc/monit/monit.conf`, override with
`MONIT_CONFIG`). Copy `deploy/monit.conf.example` and edit. Keeping site values
here means no hostnames are baked into the binary or committed to the repo.

| Key | Env override | Default | Meaning |
|-----|--------------|---------|---------|
| `ai_host` | `MONIT_AI_HOST` | `root@gpu-host.local` | SSH target for the remote GPU host |
| `pve_label` | `MONIT_PVE_LABEL` | system hostname | Panel title for the local host |
| `ai_label` | `MONIT_AI_LABEL` | host part of `ai_host` | Panel title for the remote host |
| `refresh` | `MONIT_REFRESH` | `2` | Data refresh interval, seconds |
| `page_secs` | `MONIT_PAGE_SECS` | `8` | Seconds per page before rotating |
| `top` | `MONIT_TOP` | `8` | Rows of top consumers per host |
| `temp_unit` | `MONIT_TEMP_UNIT` | `C` | Temperature unit: `C` or `F` |

## Build

The binary is dependency-free; build a fully static musl binary so it runs on
any glibc/musl Linux without runtime deps:

```sh
cargo build --release --target x86_64-unknown-linux-musl
```

Output: `target/x86_64-unknown-linux-musl/release/monit`.

## Deploy

```sh
install -m 0755 monit              /usr/local/bin/monit
install -d -m 0755                 /etc/monit
install -m 0644 monit.conf.example /etc/monit/monit.conf   # then edit
install -m 0644 monit.service      /etc/systemd/system/monit.service
systemctl daemon-reload
systemctl enable --now monit.service
```

The service `Conflicts=getty@tty1.service`, so enabling it hands the console
display to the dashboard. To get the login prompt back:

```sh
systemctl stop monit.service
systemctl start getty@tty1.service
```

## Requirements

- A Linux framebuffer at `/dev/fb0` (no X/Wayland needed) and a 32bpp mode.
- The host running monit must be able to SSH to `ai_host` non-interactively
  (key-based root login; `BatchMode=yes` is used and host keys are not
  persisted).
- For fan/pump RPM, the motherboard's Super I/O driver must be loaded (e.g.
  `nct6775`); add it to `/etc/modules-load.d/` to persist across reboots.
- `nvidia-smi` on the remote host for GPU metrics.
