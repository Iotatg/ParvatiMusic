#!/usr/bin/env bash
# Tiny supervisor for a dev box: keeps the sidecars and the bot alive while
# you iterate. Not for production — use systemd (docs/miniapp.md) there.
#
#   ./tools/supervise.sh          keep tgapi + downloader + bot up
#   ./tools/supervise.sh --tunnel …and a fresh public URL when the tunnel drops
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p logs

want_tunnel=0
[ "${1:-}" = "--tunnel" ] && want_tunnel=1

CF="$(command -v cloudflared || true)"
if [ -z "$CF" ] && [ -x /root/cloudflared ]; then CF=/root/cloudflared; fi

port="$(grep -E '^MINIAPP_PORT=' .env 2>/dev/null | cut -d= -f2)"
port="${port:-8080}"

set_menu() {
  local url="$1" tok
  tok="$(grep -m1 '^TOKEN=' .env | cut -d= -f2-)"
  [ -n "$tok" ] || return 0
  curl -s -o /dev/null -X POST "https://api.telegram.org/bot$tok/setChatMenuButton" \
    -H 'content-type: application/json' \
    -d "{\"menu_button\":{\"type\":\"web_app\",\"text\":\"Open Player\",\"web_app\":{\"url\":\"$url\"}}}"
  echo "menu → $url"
}

echo "supervisor up (tunnel=$want_tunnel)"
while true; do
  if ! pgrep -f "[t]ools/tgapi.py" >/dev/null; then
    setsid python3 tools/tgapi.py >> logs/tgapi.log 2>&1 < /dev/null &
    echo "$(date +%H:%M:%S) started tgapi"
  fi

  if ! pgrep -f "[t]ools/downloader.py" >/dev/null; then
    # The bot reaches this sidecar as the fallback Iota backend, so it has to
    # accept the Iota key as well as its own local one.
    iota_key="$(grep -m1 '^IOTA_API_KEY=' .env 2>/dev/null | cut -d= -f2-)"
    COOKIES_FILE="${COOKIES_FILE:-tools/cookies.txt}" API_KEY_ALT="$iota_key" \
      setsid python3 tools/downloader.py \
      >> logs/downloader.log 2>&1 < /dev/null &
    echo "$(date +%H:%M:%S) started downloader"
  fi

  if ! pgrep -x parvati-bot >/dev/null; then
    setsid ./target/release/parvati-bot >> logs/bot.log 2>&1 < /dev/null &
    echo "$(date +%H:%M:%S) started bot"
  fi

  if [ "$want_tunnel" = 1 ] && [ -n "$CF" ] && ! pgrep -f "[c]loudflared tunnel" >/dev/null; then
    setsid "$CF" tunnel --url "http://localhost:$port" --no-autoupdate \
      >> logs/tunnel.log 2>&1 < /dev/null &
    sleep 10
    url="$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' logs/tunnel.log | tail -1)"
    [ -n "$url" ] && set_menu "$url"
  fi

  sleep 5
done
