#!/usr/bin/env sh
# Deploy monit to a host over SSH.
#
#   ./deploy/deploy.sh root@pve-host          # build (if needed) + install + restart
#   SKIP_BUILD=1 ./deploy/deploy.sh root@pve  # use the existing binary
#
# Installs the static binary to /usr/local/bin/monit, the systemd unit, and (on
# first install only) the config template, then restarts the service. The
# config at /etc/monit/monit.conf is never overwritten if it already exists.
set -eu

HOST="${1:?usage: deploy.sh user@host}"
TARGET=x86_64-unknown-linux-musl
BIN="target/$TARGET/release/monit"
DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$DIR"

if [ "${SKIP_BUILD:-0}" != "1" ]; then
  # macOS needs the musl cross-linker; a native x86_64 Linux host does not.
  if command -v x86_64-linux-musl-gcc >/dev/null 2>&1; then
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=x86_64-linux-musl-gcc
    export CC_x86_64_unknown_linux_musl=x86_64-linux-musl-gcc
  fi
  echo ">> building $BIN"
  cargo build --release --target "$TARGET"
fi

[ -f "$BIN" ] || { echo "missing $BIN (build first)"; exit 1; }

echo ">> copying binary to $HOST"
scp "$BIN" "$HOST:/tmp/monit.new"
scp deploy/monit.service "$HOST:/tmp/monit.service"
scp deploy/monit.conf.example "$HOST:/tmp/monit.conf.example"

echo ">> installing on $HOST"
ssh "$HOST" 'sh -s' <<'REMOTE'
set -eu
install -m 0755 /tmp/monit.new /usr/local/bin/monit
install -d -m 0755 /etc/monit
[ -f /etc/monit/monit.conf ] || install -m 0644 /tmp/monit.conf.example /etc/monit/monit.conf
install -m 0644 /tmp/monit.service /etc/systemd/system/monit.service
rm -f /tmp/monit.new /tmp/monit.service /tmp/monit.conf.example
systemctl daemon-reload
systemctl enable monit.service >/dev/null 2>&1 || true
systemctl restart monit.service
sleep 1
systemctl --no-pager --lines=5 status monit.service || true
REMOTE

echo ">> done. API: curl -s http://${HOST#*@}:9090/healthz"
