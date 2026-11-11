#!/usr/bin/env bash
# Start everything the bot needs on one box: Bot API bridge, downloader, bot,
# optional tunnel.
#
#   ./tools/run.sh            bot + PO provider + downloader
#   ./tools/run.sh --tunnel   …and a public HTTPS URL for the Mini App
#
# COOKIES_FILE=/path/cookies.txt ./tools/run.sh   # when YouTube blocks the host IP
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

[ -f .env ] || { echo "missing .env (copy .env.example)"; exit 1; }
[ -x target/release/parvati-bot ] || { echo "build first: cargo build --release -p parvati_bot"; exit 1; }

mkdir -p downloads logs

# YouTube hands out storyboards only unless a PO token is supplied; the
# bgutil provider (tools/pot-server.md) answers on :4416.
if [ -d /root/bgutil/server/build ] && ! pgrep -f "bgutil/server/build/main.js" >/dev/null; then
  echo "→ pot provider :4416"
  ( cd /root/bgutil/server && setsid node build/main.js --port 4416 \
      > "$ROOT/logs/pot.log" 2>&1 < /dev/null & )
fi

# Bot API bridge: roster, avatars, sticker packs and premium emoji.
if ! pgrep -f "tools/tgapi.py" >/dev/null; then
  echo "→ tgapi :8098"
  setsid python3 tools/tgapi.py > logs/tgapi.log 2>&1 < /dev/null &
fi

if ! pgrep -f "tools/downloader.py" >/dev/null; then
  echo "→ downloader :8099"
  COOKIES_FILE="${COOKIES_FILE:-}" setsid python3 tools/downloader.py \
    > logs/downloader.log 2>&1 < /dev/null &
fi

echo "→ bot"
setsid ./target/release/parvati-bot > logs/bot.log 2>&1 < /dev/null &

sleep 3
port="$(grep -E '^MINIAPP_PORT=' .env | cut -d= -f2)"
port="${port:-8080}"

if [ "${1:-}" = "--tunnel" ]; then
  CF="$(command -v cloudflared || true)"
  if [ -z "$CF" ] && [ -x /root/cloudflared ]; then CF=/root/cloudflared; fi
  if [ -n "$CF" ]; then
    setsid "$CF" tunnel --url "http://localhost:$port" --no-autoupdate \
      > logs/tunnel.log 2>&1 < /dev/null &
    sleep 10
    url="$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' logs/tunnel.log | head -1)"
    if [ -n "$url" ]; then
      echo "→ mini app: $url"
      # Point the group's menu button at the fresh tunnel so "Open Player"
      # always lands on the running instance.
      tok="$(grep -m1 '^TOKEN=' .env | cut -d= -f2-)"
      curl -s -o /dev/null -X POST "https://api.telegram.org/bot$tok/setChatMenuButton" \
        -H 'content-type: application/json' \
        -d "{\"menu_button\":{\"type\":\"web_app\",\"text\":\"Open Player\",\"web_app\":{\"url\":\"$url\"}}}"
      echo "→ menu button → $url"
    fi
  else
    echo "cloudflared not installed — skipping tunnel"
  fi
fi

echo "→ mini app: http://localhost:$port"
echo "→ logs: logs/bot.log"
