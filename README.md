# monit — g8 framebuffer memory dashboard

A tiny single-binary Rust service that paints a live memory dashboard directly
onto the HDMI display attached to **pve.g8.lo** — no X server, no web browser,
just the Linux framebuffer (`/dev/fb0`).

It shows, side by side:

- **pve.g8.lo** (local) — total/used RAM, free/cache/swap, and the top memory
  consumers (Proxmox VMs are labelled by VMID + guest name).
- **ai.g8.lo** (remote, over SSH) — total/used RAM, top consumers, plus the
  **GPU**: per-card VRAM usage/utilization and the top GPU processes from
  `nvidia-smi`.

The bar colors shift green → yellow → red as usage climbs.

## How it works

- pve memory is read straight from `/proc/meminfo` and `/proc/[pid]/status`.
- ai is polled with a single SSH call (`root@ai.g8.lo`) that returns meminfo,
  `ps` top consumers, and `nvidia-smi` output, which `monit` parses.
- The dashboard is drawn into a back buffer and flushed to `/dev/fb0`.
- While running, the active VT is switched to `KD_GRAPHICS` so the kernel
  console stops drawing text/cursor over the dashboard; text mode is restored
  on exit.

## Configuration (environment)

| Var | Default | Meaning |
|-----|---------|---------|
| `MONIT_AI_HOST` | `root@ai.g8.lo` | SSH target for the GPU host |
| `MONIT_PVE_LABEL` | `pve.g8.lo` | Panel title for the local host |
| `MONIT_AI_LABEL` | `ai.g8.lo` | Panel title for the GPU host |
| `MONIT_REFRESH` | `2` | Refresh interval, seconds |
| `MONIT_TOP` | `8` | Rows of top consumers per host |

## Build

Rust is not installed on pve, so build the static binary on **dev.g8.lo** and
copy it over:

```sh
# from the project root (synced to dev.g8.lo:/root/monit-build)
cargo build --release --target x86_64-unknown-linux-musl
```

This produces a fully static `target/x86_64-unknown-linux-musl/release/monit`
that runs on pve (Debian 13) with no dependencies.

## Deploy (on pve.g8.lo)

```sh
install -m 0755 monit /usr/local/bin/monit
install -m 0644 monit.service /etc/systemd/system/monit.service
systemctl daemon-reload
systemctl enable --now monit.service
```

The service `Conflicts=getty@tty1.service`, so enabling it hands the console
display to the dashboard. To get the login prompt back:

```sh
systemctl stop monit.service
systemctl start getty@tty1.service
```

## Notes

- Requires the prerequisite that `root@pve.g8.lo` can SSH to `root@ai.g8.lo`
  (key-based, non-interactive). `monit` uses `BatchMode=yes` and does not
  persist host keys.
- Nothing is installed or changed on ai.g8.lo; it is only read remotely.
