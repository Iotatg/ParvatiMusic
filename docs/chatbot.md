# AI Chatbot

Every instance ships with a chat persona. It answers in DMs and in groups, keeps
a short memory per chat, shows a "typing…" indicator, and never floods a group.

## When it answers

| Context | Behaviour |
| :--- | :--- |
| DM | every non-command message |
| Group, default (`on`) | only an `@username` tag, a mention entity, a reply to one of its messages, or its own name in the text |
| Group, `/chatbot off` | fully muted — even a name or a tag gets nothing until `/chatbot on` |
| Group, `/chatbot` with no argument | prints the current state instead of toggling |
| Ignored | commands (`/…`, `.…`), messages from other bots, its own messages |

Being an admin does **not** make it answer everything: admin status only decides
what Telegram *delivers* to the bot, never what the bot *replies* to.

## Name triggers

A name counts wherever it stands on its own, so `parvati`, `Parvati, gaana laga`,
`arre parvati!` and `parvati.` all hit — matching is on word boundaries, not spaces.
`parvatimusic` does not.

Triggers are built from two sources:

1. the configured display name, plus each word of it;
2. the readable parts of the `@username`.

The second source matters because display names are often written in styled
letters (`𝐀‌ᴅᴠᴀɪᴛ 𝘅 ϻ‌υsɪᴄ` contains no plain latin letters at all), so someone
typing `parvati` would never match the name. `ParvatiMusicBot` → `parvati`.
Generic halves (`music`, `bot`, `official`, `the`, `team`, `ai`, `x`) are dropped,
and a trailing `bot` is stripped. Triggers are rebuilt as soon as the bot learns
its own username at login.

`CHATBOT_TRIGGER=parvati,adi` overrides the whole list.

## Commands

| Command | Effect |
| :--- | :--- |
| `/chatbot on` | allow replies in this chat (still trigger-only) |
| `/chatbot off` | mute the chatbot in this chat |
| `/chatbot` | show the current state |
| `/resetbot` | forget this chat's conversation memory |
| `/gchatbot on\|off` | owner only: master switch for every chat |

## Providers

Tried in order, first one with a key wins:

1. `AI_API_KEY` + `AI_BASE_URL` + `AI_MODEL`
2. `APINEX_API_KEY` + `APINEX_API_URL` + `APINEX_MODEL` (`APINEX_FALLBACK_MODEL` retried on failure)
3. `GROQ_API_KEY` + `GROQ_MODEL`
4. `OPENROUTER_API_KEY` + `OPENROUTER_MODEL`
5. `OPENAI_API_KEY` + `OPENAI_MODEL`

All five speak the OpenAI chat-completions shape, so any compatible gateway works.
Without a key the bot still runs — it just replies with a short "no key" notice.

## Persona

The character sheet lives in the binary and is filled from `BOT_NAME`,
`BOT_AGE`, `BOT_FROM`, `BOT_DOB`, `OWNER_NAME`, `OWNER_USERNAME`. To rewrite it
without touching code, point `CHATBOT_PERSONA_FILE` at a text file, or set
`CHATBOT_PERSONA` inline.

Three tiers are applied automatically: the owner gets the softest tone, someone
with three or more turns becomes familiar, everyone else stays a stranger.

## Group privacy (the usual "why is it silent" answer)

Telegram hides plain group messages from bots unless the bot is an admin **or**
privacy mode is disabled. So in a group where it is neither, `parvati kaisi hai`
never reaches the process at all — the gate is never even asked.

Fix either way:

- add the bot as an **admin** in that group, or
- BotFather → `/setprivacy` → pick the bot → **Disable**.

The bot prints a one-time hint when `/chatbot on` is used while it is still
restricted.

## Environment

| Variable | Default | Meaning |
| :--- | :--- | :--- |
| `CHATBOT_ENABLED` | `true` | master switch at boot |
| `BOT_NAME` | bot's Telegram first name | persona name and trigger source |
| `BOT_AGE` | `19` | persona age |
| `BOT_FROM` | `Vrindavan` | persona hometown |
| `BOT_DOB` | `8 August` | persona birthday |
| `OWNER_NAME` | — | owner shown in the persona |
| `OWNER_USERNAME` | — | owner's @handle |
| `CHATBOT_TRIGGER` | — | comma-separated trigger words, overrides the derived list |
| `CHATBOT_PERSONA` | — | inline persona override |
| `CHATBOT_PERSONA_FILE` | — | path to a persona override file |
