# Changelog

## [Unreleased]

### 2026-06-13
- **feat:** Initial framebuffer memory dashboard (`monit`). Renders directly to
  `/dev/fb0` on pve.g8.lo — no X/web. Shows RAM usage + top consumers for the
  local host (pve) and the GPU host (ai.g8.lo, over SSH), plus per-GPU memory
  and GPU process usage via `nvidia-smi`.
- **feat:** Embedded Terminus PSF fonts (8×16 and 16×32) with a minimal
  PSF1/PSF2 loader and integer-scaled bitmap text renderer.
- **feat:** Takes the active VT into `KD_GRAPHICS` mode while running and
  restores text mode on exit, so the console cursor never bleeds through.
- **feat:** systemd unit (`deploy/monit.service`) that conflicts with
  `getty@tty1` to own the attached display.
- **chore:** Builds as a static `x86_64-unknown-linux-musl` binary on dev.g8.lo.
