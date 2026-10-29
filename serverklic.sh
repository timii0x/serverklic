#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ACTION="${1:-help}"
PORT=5000
shift || true
while (($#)); do
  case "$1" in
    --port) PORT="${2:?Manjka port}"; shift 2 ;;
    *) echo "Neznan argument: $1" >&2; exit 1 ;;
  esac
done
[[ "$PORT" =~ ^[0-9]{4,5}$ ]] && ((10#$PORT >= 1024 && 10#$PORT <= 65535)) || { echo 'Port mora biti 1024–65535.'; exit 1; }
PORT=$((10#$PORT))
admin() { if [[ "$EUID" == 0 ]]; then "$@"; else sudo "$@"; fi; }
case "$ACTION" in
  install)
    [[ "$(uname -s)" == Linux ]] || { echo 'Namestitev storitve je namenjena Linux strežniku s systemd.'; exit 1; }
    command -v systemctl >/dev/null || { echo 'Potreben je systemd.'; exit 1; }
    BIN="$ROOT/serverklic-server"
    if [[ ! -x "$BIN" ]]; then
      if command -v apt-get >/dev/null; then
        admin apt-get update
        admin apt-get install -y build-essential curl ca-certificates
      else
        echo 'Namesti C prevajalnik, curl in Rust, nato ponovi. Samodejne odvisnosti so podprte na Debian/Ubuntu.'
      fi
      if ! command -v cargo >/dev/null; then
        if [[ -f "${HOME}/.cargo/env" ]]; then source "${HOME}/.cargo/env"; fi
      fi
      if ! command -v cargo >/dev/null; then
        INSTALLER="$(mktemp)"
        curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs -o "$INSTALLER"
        sh "$INSTALLER" -y --profile minimal --default-toolchain 1.92.0
        rm -f "$INSTALLER"
        source "${HOME}/.cargo/env"
      fi
      cargo build --manifest-path "$ROOT/Cargo.toml" --release --locked -p serverklic-server
      BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/serverklic-server"
    fi
    admin install -d /opt/serverklic /etc/serverklic
    admin install -m 755 "$BIN" /opt/serverklic/serverklic-server
    if ! id serverklic >/dev/null 2>&1; then admin useradd --system --home-dir /var/lib/serverklic --shell /usr/sbin/nologin serverklic; fi
    admin install -d -o serverklic -g serverklic -m 750 /var/lib/serverklic
    UNIT="$(mktemp)"
    cat > "$UNIT" <<'UNIT'
[Unit]
Description=ServerKlic voice server on UDP %i
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
User=serverklic
Group=serverklic
EnvironmentFile=-/etc/serverklic/%i.env
ExecStart=/opt/serverklic/serverklic-server --bind 0.0.0.0:%i --data-dir /var/lib/serverklic/%i
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/serverklic
UMask=0077
LimitNOFILE=4096
[Install]
WantedBy=multi-user.target
UNIT
    admin install -m 644 "$UNIT" /etc/systemd/system/serverklic@.service
    rm -f "$UNIT"
    admin systemctl daemon-reload
    echo "Nameščeno. Zagon: $0 start --port $PORT"
    echo "Za internet odpri UDP $PORT na požarnem zidu / usmerjevalniku."
    ;;
  start) admin systemctl enable --now "serverklic@$PORT" ;;
  stop) admin systemctl disable --now "serverklic@$PORT" ;;
  restart) admin systemctl restart "serverklic@$PORT" ;;
  status) systemctl --no-pager status "serverklic@$PORT" ;;
  logs) admin journalctl -u "serverklic@$PORT" -n 100 --no-pager ;;
  run)
    if [[ -x "$ROOT/serverklic-server" ]]; then BIN="$ROOT/serverklic-server";
    else BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/serverklic-server"; fi
    [[ -x "$BIN" ]] || { echo 'Najprej prevedi: cargo build --release -p serverklic-server'; exit 1; }
    exec "$BIN" --bind "0.0.0.0:$PORT" --data-dir "$ROOT/server-data/$PORT"
    ;;
  *) echo 'Uporaba: ./serverklic.sh {install|start|stop|restart|status|logs|run} [--port 5000]' ;;
esac
