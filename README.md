<p align="center">
  <img src="https://capsule-render.vercel.app/api?type=waving&color=gradient&customColorList=6,11,19,26&height=220&section=header&text=ParvatiMusic&fontSize=60&fontColor=ffffff&animation=fadeIn&fontAlignY=38&desc=Next-Generation%20Telegram%20Voice%20Chat%20Music%20Engine&descAlignY=60&descAlign=50" width="100%"/>
</p>

<p align="center">
  <img src="https://readme-typing-svg.demolab.com?font=Fira+Code&weight=600&size=20&pause=1000&color=00E5FF&center=true&vCenter=true&width=640&lines=High-Performance+Rust+Streaming+Engine;Native+Ferogram+MTProto+Client;Ultra-Low+Latency+Voice+Chat+Audio;Live+Message+Editing+%2B+Download+CANCEL;Multi-Language+YAML+Locales;Developed+%26+Maintained+by+Iota+coder" alt="Typing SVG" />
</p>

<p align="center">
  <a href="https://github.com/Iotatg/ParvatiMusic/stargazers">
    <img src="https://img.shields.io/badge/GitHub-Stars-FFD700?style=for-the-badge&logo=github&logoColor=white" alt="GitHub Stars">
  </a>
  <a href="https://github.com/Iotatg/ParvatiMusic/fork">
    <img src="https://img.shields.io/badge/GitHub-Forks-00C853?style=for-the-badge&logo=github&logoColor=white" alt="GitHub Forks">
  </a>
  <a href="https://www.rust-lang.org/">
    <img src="https://img.shields.io/badge/Rust-2024-000000?style=for-the-badge&logo=rust&logoColor=white" alt="Rust Edition">
  </a>
  <a href="https://github.com/ankit-chaubey/ferogram">
    <img src="https://img.shields.io/badge/MTProto-Native-2CA5E0?style=for-the-badge&logo=telegram&logoColor=white" alt="MTProto">
  </a>
  <a href="https://github.com/Iotatg/ParvatiMusic/blob/main/LICENSE">
    <img src="https://img.shields.io/badge/License-MIT-blue?style=for-the-badge&logo=opensourceinitiative&logoColor=white" alt="License">
  </a>
  <a href="https://t.me/im_iota">
    <img src="https://img.shields.io/badge/Telegram-Support-26A69A?style=for-the-badge&logo=telegram&logoColor=white" alt="Support Chat">
  </a>
</p>

---

## ✨ Features & Highlights

- ⚡ **Ultra-Low Latency Streaming:** A pure Rust core wired straight into `ntgcalls`, delivering crystal-clear voice-chat playback with zero runtime overhead.
- 🧵 **Single Static Binary:** The whole engine compiles into one `parvati-bot` executable — no interpreter, no garbage collector, no stutters.
- 📡 **Native MTProto Client:** A from-scratch MTProto stack (vendored **Ferogram**) instead of the Bot API, so every status message is **edited in place** through the full search → download → progress → now-playing chain.
- 🎵 **Multi-Platform Provider:** Stream resolution for YouTube, Spotify metadata matching, Telegram audio files, live streams, and direct HTTP audio URLs.
- 🛑 **Real Download Cancellation:** Live progress editing plus a working `/cancel` that aborts the transfer mid-stream through the download backend.
- 🎛️ **Full Playback Controls:** Complete command suite — seek, volume, mute, loop, shuffle, autoplay, force-play variants, and queue management.
- 🌐 **Multi-Language Engine:** YAML localization packs (English, Hindi, Arabic, Turkish) hot-reloadable via `/reload`.
- 🛡️ **Admin & Security:** Granular sudo management, per-chat user authorization, anti-flood protection, and owner-only maintenance flows.

---

## 🚀 Quick Deploy

<details>
  <summary><b>🖥️ [Click to Expand] VPS / Linux Server Build Guide</b></summary>
  <br>

  ### 📋 Prerequisites
  ```bash
  sudo apt-get update && sudo apt-get upgrade -y
  sudo apt-get install -y git build-essential pkg-config libssl-dev ffmpeg
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```

  ### 🚀 Step-by-Step Setup
  ```bash
  # 1. Clone the repository
  git clone https://github.com/Iotatg/ParvatiMusic.git
  cd ParvatiMusic

  # 2. Configure environment
  nano bot/.env   # API_ID, API_HASH, TOKEN, OWNER_ID, STRING_SESSION, etc.

  # 3. Compile and run
  cargo build --release -p parvati_bot
  ./target/release/parvati-bot
  ```

  ### 🔄 Running 24/7 in Background
  **Method 1: Using Tmux (Recommended)**
  ```bash
  tmux new -s parvatimusic
  ./target/release/parvati-bot
  # Press Ctrl+B then D to detach. Re-attach anytime with: tmux a -t parvatimusic
  ```

  **Method 2: Using Systemd Service**
  ```bash
  sudo nano /etc/systemd/system/parvatimusic.service
  ```
  Paste the following unit configuration:
  ```ini
  [Unit]
  Description=ParvatiMusic Rust Telegram Bot
  After=network.target

  [Service]
  Type=simple
  User=ubuntu
  WorkingDirectory=/home/ubuntu/ParvatiMusic
  ExecStart=/home/ubuntu/ParvatiMusic/target/release/parvati-bot
  Restart=always
  RestartSec=5

  [Install]
  WantedBy=multi-user.target
  ```
  Enable and start the service:
  ```bash
  sudo systemctl daemon-reload
  sudo systemctl enable parvatimusic
  sudo systemctl start parvatimusic
  sudo systemctl status parvatimusic
  ```
</details>

---

## ⚙️ Configuration Variables

<details open>
  <summary><b>🔑 Environment Variables Reference Table</b></summary>
  <br>

  | Variable | Required | Default | Description |
  | :--- | :---: | :---: | :--- |
  | `API_ID` | **Yes** | — | Telegram API ID from [my.telegram.org](https://my.telegram.org) |
  | `API_HASH` | **Yes** | — | Telegram API Hash from [my.telegram.org](https://my.telegram.org) |
  | `TOKEN` | **Yes** | — | Telegram Bot Token from [@BotFather](https://t.me/BotFather) |
  | `OWNER_ID` | **Yes** | — | Telegram User ID of the bot owner |
  | `STRING_SESSIONS` | **Yes** | — | Assistant user session string(s), comma-separated |
  | `IOTA_API_URL` | No | `http://127.0.0.1:8000` | Download backend API endpoint |
  | `IOTA_API_KEY` | No | — | Download backend API key |
  | `DURATION_LIMIT` | No | `9000` | Max track duration in seconds |
  | `QUEUE_LIMIT` | No | `500` | Max tracks allowed in queue per chat |
  | `DEFAULT_LANG` | No | `en` | Default language code (`en`, `hi`, `ar`, `tr`) |
  | `LOGGER_ID` | No | `0` | Telegram Chat ID for logs and error reports |
  | `SUPPORT_CHAT` | No | `https://t.me/+Q493A_VBAUpkMmRl` | Link to your Telegram support group |
  | `SUPPORT_CHANNEL` | No | `https://t.me/im_iota` | Link to your Telegram announcement channel |
  | `DEV_URL` | No | — | Developer profile link shown on repo cards |
  | `COOKIES_LINK` | No | — | Remote URL to YouTube cookies file |
  | `START_IMG_URL` | No | — | Media shown on `/start` greeting |
  | `APINEX_API_KEY` | No | — | LLM key for the AI chatbot (see [docs/chatbot.md](docs/chatbot.md)) |
  | `APINEX_MODEL` | No | `free/gemini-3.8-flash` | Chatbot model |
  | `CHATBOT_ENABLED` | No | `true` | Master switch for the chatbot |
  | `BOT_NAME` | No | Telegram first name | Persona name and name-trigger source |
  | `BOT_AGE` / `BOT_FROM` / `BOT_DOB` | No | `19` / `Vrindavan` / `8 August` | Persona identity |
  | `OWNER_NAME` / `OWNER_USERNAME` | No | — | Owner shown in the persona |
  | `CHATBOT_TRIGGER` | No | — | Extra comma-separated trigger words |
  | `PORT` | No | `8000` | HTTP health/metrics server port |
</details>

---

## 🤖 AI Chatbot

<details>
  <summary><b>💬 [Click to Expand] Persona, triggers and commands</b></summary>
  <br>

  A built-in persona answers in DMs and in groups. In groups it speaks **only**
  when called: its name in the text, an `@username` tag, a mention picked from the
  member list, or a reply to one of its messages. `/chatbot on` merely allows
  replies in a chat — it never turns the bot into a reply-to-everything bot, admin
  or not.

  | Command | Effect |
  | :--- | :--- |
  | `/chatbot on` | allow replies here (still trigger-only) |
  | `/chatbot off` | mute the chatbot here |
  | `/chatbot` | show the current state |
  | `/resetbot` | forget this chat's memory |
  | `/gchatbot on\|off` | owner only: master switch |

  > **Silent in a group?** Telegram withholds plain messages from bots that are
  > neither admin nor privacy-disabled. Make the bot an admin, or run
  > BotFather → `/setprivacy` → **Disable**.

  Full reference: [docs/chatbot.md](docs/chatbot.md)
</details>

---

## 🎮 Bot Commands Reference

<details>
  <summary><b>🎵 [Click to Expand] Playback & Stream Commands</b></summary>
  <br>

  | Command | Description |
  | :--- | :--- |
  | `/play <query/url>` | Streams requested song into group voice chat |
  | `/vplay <query/url>` | Streams track with video rendering enabled |
  | `/playforce <query/url>` | Force plays track as owner/sudo privileges |
  | `/skip` | Skips current track to the next item in queue |
  | `/pause` | Pauses active playback stream |
  | `/resume` | Resumes paused audio stream |
  | `/stop` | Stops playback and clears the entire playlist queue |
  | `/seek <seconds>` | Seeks playback to a specific timestamp |
  | `/speed <1.0-3.0>` | Adjusts voice-chat playback speed |
  | `/mute` / `/unmute` | Mutes or unmutes the stream audio |
  | `/queue` | Displays the current playlist and upcoming tracks |
  | `/replay` | Replays the current playing song from beginning |
  | `/cancel` | Aborts the active track download mid-transfer |
</details>

<details>
  <summary><b>👥 [Click to Expand] Group Admin & Settings Commands</b></summary>
  <br>

  | Command | Description |
  | :--- | :--- |
  | `/auth <user>` | Authorizes non-admin user to use playback commands |
  | `/unauth <user>` | Revokes bot access authorization from a user |
  | `/autoplay` | Toggles continuous auto-play when queue empties |
  | `/loop` | Cycles loop mode (off / single / queue) |
  | `/shuffle` | Shuffles the current playlist queue |
  | `/lang` | Opens the interactive language selector panel |
  | `/settings` | Opens the interactive visual settings control panel |
  | `/reload` | Refreshes admin caches and locale packs live |
  | `/askip` | Personal skip that only advances your own request |
</details>

<details>
  <summary><b>🛡️ [Click to Expand] Bot Owner & Sudo Commands</b></summary>
  <br>

  | Command | Description |
  | :--- | :--- |
  | `/addsudo <user>` | Adds a new user to sudoers list |
  | `/delsudo <user>` | Removes a user from sudoers list |
  | `/sudolist` | Shows list of all current bot sudoers |
  | `/stats` | Shows server performance, memory, and uptime metrics |
  | `/broadcast <msg>` | Broadcasts announcements to all served chats |
  | `/purge <n>` | Deletes last n messages from the current chat |
  | `/ping` | Live latency and response-time benchmark |
</details>

---

## 📁 Project Architecture

```
ParvatiMusic/
├── mtproto/              # From-scratch MTProto protocol core (RPC, crypto, transport)
├── ntg/                  # ntgcalls FFI bindings (voice-chat media engine)
├── bot/                  # Bot logic: commands, modules, locales, state
│   ├── src/ferogram_bot.rs   # Sync Bot trait bridged onto Ferogram (Tokio runtime)
│   ├── src/modules/          # Command modules (play, skip, admin, settings, sticker, ...)
│   └── locales/              # YAML multi-language packs (en / hi / ar / tr)
├── vendor/ferogram/      # Vendored MTProto client library
└── Cargo.toml            # Cargo workspace definition
```

---

## 📜 License

Distributed under the **MIT License**. See the [`LICENSE`](./LICENSE) file for full details.

<p align="center">
  <img src="https://capsule-render.vercel.app/api?type=waving&color=gradient&customColorList=6,11,19,26&height=100&section=footer" width="100%"/>
</p>

<p align="center">
  <b>Developed & Maintained with ❤️ by <a href="https://github.com/Iotatg">Iota coder</a></b>
</p>
