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
  curl -s -m 10 -o /dev/null -X POST "https://api.telegram.org/bot$tok/setChatMenuButton" \
    -H 'content-type: application/json' \
    -d "{\"menu_button\":{\"type\":\"web_app\",\"text\":\"Open Player\",\"web_app\":{\"url\":\"$url\"}}}"
  echo "menu → $url"
}

sync_menu() {
  # A quick tunnel gets a brand new hostname on every restart. Push the menu
  # button only when the URL actually changes, and re-push it after a
  # supervisor restart so a stale hostname can never be left behind.
  if [ -n "$tunnel_url" ] && [ "$tunnel_url" != "$menu_url" ]; then
    set_menu "$tunnel_url"
    menu_url="$tunnel_url"
  fi
}

echo "supervisor up (tunnel=$want_tunnel)"
tunnel_url=""
menu_url=""
tunnel_fail=0

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

  # A quick tunnel can stay "running" while its edge connection is dead, so a
  # process check alone is not enough. Probe the public URL and recycle the
  # tunnel after three consecutive failures (that is what a user sees as
  # "cloudflare down").
  if [ "$want_tunnel" = 1 ] && [ -n "$CF" ]; then
    if pgrep -f "[c]loudflared tunnel" >/dev/null; then
      # Every reconnect gives the quick tunnel a NEW hostname. Follow the
      # newest line in the log: probing a stale hostname used to make us kill a
      # healthy tunnel over and over and rotate the public URL every 15 s.
      latest="$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' logs/tunnel.log | tail -1)"
      if [ -n "$latest" ] && [ "$latest" != "$tunnel_url" ]; then
        tunnel_url="$latest"
        tunnel_fail=0
      fi
      if [ -n "$tunnel_url" ]; then
        code="$(curl -s -m 8 -o /dev/null -w '%{http_code}' "$tunnel_url/" || echo 000)"
        case "$code" in
          200|401)
            tunnel_fail=0
            sync_menu
            ;;
          *)
            tunnel_fail=$((tunnel_fail + 1))
            echo "$(date +%H:%M:%S) tunnel probe=$code fail=$tunnel_fail url=$tunnel_url"
            if [ "$tunnel_fail" -ge 3 ]; then
              pkill -f "[c]loudflared tunnel" >/dev/null 2>&1 || true
              sleep 2
              tunnel_url=""
              menu_url=""
              tunnel_fail=0
            fi
            ;;
        esac
      fi
    else
      # Start from the byte offset we already know, so a hostname from an old
      # tunnel can never be mistaken for this one while cloudflared warms up.
      before="$(wc -l < logs/tunnel.log | tr -d ' ')"
      setsid "$CF" tunnel --url "http://localhost:$port" --no-autoupdate \
        >> logs/tunnel.log 2>&1 < /dev/null &
      echo "$(date +%H:%M:%S) started tunnel"
      sleep 12
      fresh="$(tail -n +$((before + 1)) logs/tunnel.log \
        | grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' | tail -1)"
      if [ -n "$fresh" ]; then
        tunnel_url="$fresh"
        sync_menu
        tunnel_fail=0
      fi
    fi
  fi

  sleep 5
done
