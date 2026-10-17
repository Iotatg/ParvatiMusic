//! Per-command help texts, verbatim key registry.
//! (format_help_text). Keys kept verbatim, including the four entries
//! the registers without a leading slash (autoleave/bcast/broadcast/gcast).

/// format_help_text: exact text for `cmd`, empty when unknown.
pub fn format_help_text(cmd: &str) -> String {
    let t = match cmd {
        "/ac" => r####"<i>Show all active voice chat sessions.</i>

<u>Usage:</u>
<b>/active</b> or <b>/ac</b> — List active chats

<b>📊 Information Shown:</b>
• Total active voice chats
• Playback status (playing/paused/muted)
• Now playing track and queue size

<b>🔒 Restrictions:</b>
• <b>Sudo users</b> only

<b>💡 Use Case:</b>
Monitor bot usage and identify issues."####,
        "/active" => r####"<i>Show all active voice chat sessions.</i>

<u>Usage:</u>
<b>/active</b> or <b>/ac</b> — List active chats

<b>📊 Information Shown:</b>
• Total active voice chats
• Playback status (playing/paused/muted)
• Now playing track and queue size

<b>🔒 Restrictions:</b>
• <b>Sudo users</b> only

<b>💡 Use Case:</b>
Monitor bot usage and identify issues."####,
        "/activevc" => r####"<i>Show all active voice chat sessions.</i>

<u>Usage:</u>
<b>/active</b> or <b>/ac</b> — List active chats

<b>📊 Information Shown:</b>
• Total active voice chats
• Playback status (playing/paused/muted)
• Now playing track and queue size

<b>🔒 Restrictions:</b>
• <b>Sudo users</b> only

<b>💡 Use Case:</b>
Monitor bot usage and identify issues."####,
        "/activevoice" => r####"<i>Show all active voice chat sessions.</i>

<u>Usage:</u>
<b>/active</b> or <b>/ac</b> — List active chats

<b>📊 Information Shown:</b>
• Total active voice chats
• Playback status (playing/paused/muted)
• Now playing track and queue size

<b>🔒 Restrictions:</b>
• <b>Sudo users</b> only

<b>💡 Use Case:</b>
Monitor bot usage and identify issues."####,
        "/addauth" => r####"<i>Grant permission to a regular user to control playback and other admin-level features without making them a Telegram admin.</i>

<u>Usage:</u>
<b>/addauth [reply to user]</b> — Add a user by replying to their message.  
<b>/addauth &lt;user_id / username&gt;</b> — Add a user directly by ID or @username.

<b>⚙️ Notes:</b>
• Only <b>chat admins</b> can use this command.  
• Auth users can control playback with commands like <code>/pause</code>, <code>/resume</code>, <code>/skip</code>, <code>/seek</code>, <code>/mute</code>, etc.  
• 🤖 Bots cannot be added as auth users.  
• 🔢 You can have up to <b>%d</b> auth users per chat.  
• 👑 The <b>Bot Owner</b>, <b>Assistant</b>, and all <b>Sudoers</b> are <b>already authorized by default</b> — they do not appear in the list and cannot be removed.

For related commands, see <code>/delauth</code> and <code>/authlist</code>."####,
        "/addtoplaylist" => r####"<i>Add a track to one of your playlists.</i>

<u>Usage:</u>
<b>/addtoplaylist [playlist id] [song name or URL]</b>"####,
        "/adminmode" => r####"<i>Control who can use admin-level music commands in this chat.</i>

<u>Usage:</u>
<b>/adminmode [admin|adminauth|everyone]</b> — Set admin command access

<b>⚙️ Options:</b>
• <b>admin</b> — Only chat admins can use admin commands
• <b>adminauth</b> — Chat admins + authorized users can use admin commands (default)
• <b>everyone</b> — Everyone can use admin commands"####,
        "/askip" => r####"<i>Skip the current track and immediately play the next recommended autoplay track.</i>

<u>Usage:</u>
<b>/askip</b> — Skip current track to next autoplay track

<b>⚙️ Behavior:</b>
• Automatically picks a non-repeating recommendation
• Skips tracks already played in the current session
• Seamlessly continues playback

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this"####,
        "/authlist" => r####"<u>Usage:</u>
<b>/authlist</b> - <i>Displays all users currently authorized to control playback in this chat.</i>

<b>⚙️ Notes:</b>
• Anyone in the chat can use this command.  
• Shows only manually added auth users — the Owner, Assistant, and Sudoers are not listed but are always authorized."####,
        "/autoplay" => r####"<i>Automatically play recommended songs when the queue ends.</i>

<u>Usage:</u>
<b>/autoplay</b> — Open the autoplay control panel

<b>⚙️ Behavior:</b>
• When enabled, once the queue finishes the bot keeps playing similar recommended tracks from YouTube.
• Recommendations are based on the last played song and skip tracks already in the queue.
• Autoplay is tied to the current session and resets once playback stops.

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this"####,
        "/bash" => r####"<i>Execute shell commands on server.</i>

<u>Usage:</u>
<b>/sh [command]</b> — Run shell command

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>⚠️ Warning:</b>
Direct system access - extremely powerful."####,
        "/bug" => r####"<i>Report a bug, issue, or unexpected behavior directly to the bot developers.</i>

<u>Usage:</u>
<b>/bug &lt;description&gt;</b> — Send a bug report with a short explanation.
<b>Reply + /bug</b> — Report a specific message or media as a bug.

<b>🧠 Details:</b>
When used, the bot forwards your report (and the replied message, if any) to the <b>owner</b> and <b>logger</b> chats.
Flood protection is applied — you can send one report every <b>5 minutes</b> per chat.

<b>⚠️ Note:</b>
Reports are used for debugging only. Spamming this command may restrict your access."####,
        "/channelplay" => r####"<i>Configure linked channel for channel play mode.</i>

<u>Usage:</u>
<b>/channelplay [channel_id]</b> — Set linked channel

<b>⚙️ Behavior:</b>
• Links a channel to current group
• All <code>c*</code> commands affect linked channel
• Channel must be accessible by bot

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> can configure

<b>💡 Examples:</b>
<code>/setcplay -1001234567890</code>

<b>⚠️ Notes:</b>
• Get channel ID using forward + @userinfobot
• Bot must be admin in linked channel
• Use <code>/cplay</code> after setup"####,
        "/cleanmode" => r####"<i>Enable timed cleanup for command/service messages in this chat.</i>

<u>Usage:</u>
<b>/cleanmode [enable|disable]</b> — Set clean mode status

<b>⚙️ Behavior:</b>
• <b>enable</b> — Bot replies and command messages are auto-deleted after a short delay
• <b>disable</b> — Keep messages in chat (default)"####,
        "/cmddelete" => r####"<i>Toggle automatic deletion of bot commands in this chat.</i>

<u>Usage:</u>
<b>/cmddelete [enable|disable]</b> — Set command deletion status

<b>⚙️ Options:</b>
• <b>enable</b> — Commands will be deleted after being handled
• <b>disable</b> — Commands will remain in the chat (default)"####,
        "/commanddelete" => r####"<i>Toggle automatic deletion of bot commands in this chat.</i>

<u>Usage:</u>
<b>/cmddelete [enable|disable]</b> — Set command deletion status

<b>⚙️ Options:</b>
• <b>enable</b> — Commands will be deleted after being handled
• <b>disable</b> — Commands will remain in the chat (default)"####,
        "/cplay" => r####"<i>Play in linked channel's voice chat.</i>

<u>Usage:</u>
<b>/cplay [query]</b> — Play in linked channel

<b>⚙️ Setup Required:</b>
First use <code>/setcplay [channel_id]</code>

<b>⚠️ Note:</b>
All c* commands work the same as regular commands but affect the linked channel."####,
        "/createplaylist" => r####"<i>Create a new personal playlist.</i>

<u>Usage:</u>
<b>/createplaylist [name]</b> — Create a playlist with the given name

<b>💡 Notes:</b>
• Up to 10 playlists per user
• Names longer than 40 characters are trimmed"####,
        "/cvplay" => r####""####,
        "/delauth" => r####"<i>Revoke permission from a user who was previously authorized to control playback.</i>

<u>Usage:</u>
<b>/delauth [reply to user]</b> — Remove by replying to their message.  
<b>/delauth &lt;user_id / username&gt; </b>— Remove by ID or @username.

<b>⚙️ Notes:</b>
• Only <b>chat admins</b> can use this command.  
• Use this to revoke access from misbehaving users.  
• To check who’s currently authorized, use <code>/authlist</code>."####,
        "/deleteplaylist" => r####"<i>Delete one of your playlists.</i>

<u>Usage:</u>
<b>/deleteplaylist [playlist id]</b>"####,
        "/end" => r####"<i>Stop playback and leave the voice chat.</i>

<u>Usage:</u>
<b>/stop</b> or <b>/end</b> — Stop playback

<b>⚙️ Behavior:</b>
• Stops current track
• Clears queue
• Assistant leaves voice chat
•
<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>⚠️ Note:</b>
This action cannot be undone. Use <code>/pause</code> for temporary stops."####,
        "/ev" => r####"<i>Execute Go code dynamically (eval mode).</i>

<u>Usage:</u>
<b>/eval [code]</b> — Run Go code

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>⚠️ Warning:</b>
Powerful command - use with caution."####,
        "/eval" => r####"<i>Execute Go code dynamically (eval mode).</i>

<u>Usage:</u>
<b>/eval [code]</b> — Run Go code

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>⚠️ Warning:</b>
Powerful command - use with caution."####,
        "/fcplay" => r####""####,
        "/fplay" => r####"<i>Force play a song, skipping the current queue.</i>

<u>Usage:</u>
<b>/fplay [query/URL]</b> — Force play immediately
<b>/fplay [reply to audio/video]</b> — Force play replied media

<b>🎵 Behavior:</b>
• Stops current playback
• Clears queue
• Starts playing immediately

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Example:</b>
<code>/fplay urgent announcement track</code>

<b>⚠️ Note:</b>
This command is useful for urgent playback needs but will disrupt the current queue."####,
        "/fvplay" => r####"<i>Force play video content, skipping queue.</i>

<u>Usage:</u>
<b>/fvplay [query/URL]</b> — Force play video immediately

<b>🔒 Restrictions:</b>
• Admin/auth only command

<b>💡 Use Case:</b>
Immediate video playback when something urgent needs to be shown."####,
        "/help" => r####"ℹ️ <b>Help Command</b>
<i>Displays general bot help or detailed information about a specific command.</i>

<u>Usage:</u>
<code>/help</code> — Show the main help menu.  
<code>/help &lt;command&gt;</code> — Show help for a specific command.

For more info, visit our <a href="%s">Support Chat</a>."####,
        "/json" => r####"<i>Get JSON representation of message/user/chat.</i>

<u>Usage:</u>
<b>/json</b> — Current message JSON
<b>/json -s</b> — Sender JSON
<b>/json -c</b> — Chat JSON
<b>/json -m</b> — Media JSON
<b>/json [reply] -f</b> — File JSON

<b>💡 Use Case:</b>
Debugging and development."####,
        "/jump" => r####"<i>Jump to a specific position in the track.</i>

<u>Usage:</u>
<b>/jump [seconds]</b> — Jump to exact position

<b>⚙️ Features:</b>
• Absolute position seeking
• Precise time control
• 10-second buffer from end

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Examples:</b>
<code>/jump 90</code> — Jump to 1:30
<code>/jump 0</code> — Jump to start (same as /replay)

<b>⚠️ Notes:</b>
• Position must be within track duration - 10 seconds
• More precise than <code>/seek</code> and <code>/seekback</code>"####,
        "/logs" => r####"<i>Send current bot logs file.</i>

<u>Usage:</u>
<b>/logs</b> — Send current log file

<b>🔒 Restrictions:</b>
• <b>Sudo users only</b>"####,
        "/loop" => r####"<i>Set loop count for the current track.</i>

<u>Usage:</u>
<b>/loop</b> — Show current loop count
<b>/loop [count]</b> — Set loop count (0-10)

<b>⚙️ Behavior:</b>
• 0 = No loop (play once)
• 1-10 = Repeat track that many times
• Loop counter decrements after each playback

<b>💡 Examples:</b>
<code>/loop 0</code> — Disable loop
<code>/loop 3</code> — Loop current track 3 times
<code>/loop 10</code> — Loop current track 10 times

<b>⚠️ Notes:</b>
• Maximum loop count: 10
• Loop affects only current track
• After loops complete, plays next in queue
• If the track is forcefully skipped using <code>/skip</code>, the loop will stop and reset automatically"####,
        "/maint" => r####"<i>Toggle maintenance mode.</i>

<u>Usage:</u>
<b>/maintenance</b> — Show current status
<b>/maintenance on [reason]</b> — Enable maintenance
<b>/maintenance off</b> — Disable maintenance

<b>⚙️ Behavior When Active:</b>
• Stops all active rooms
• Blocks non-owner/sudo commands
• Shows maintenance message to users

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>💡 Examples:</b>
<code>/maintenance on Server upgrade</code>
<code>/maintenance off</code>

<b>⚠️ Notes:</b>
• Owner and sudoers can still use bot
• All rooms are destroyed when enabled
• Users see maintenance message with reason"####,
        "/maintenance" => r####"<i>Toggle maintenance mode.</i>

<u>Usage:</u>
<b>/maintenance</b> — Show current status
<b>/maintenance on [reason]</b> — Enable maintenance
<b>/maintenance off</b> — Disable maintenance

<b>⚙️ Behavior When Active:</b>
• Stops all active rooms
• Blocks non-owner/sudo commands
• Shows maintenance message to users

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>💡 Examples:</b>
<code>/maintenance on Server upgrade</code>
<code>/maintenance off</code>

<b>⚠️ Notes:</b>
• Owner and sudoers can still use bot
• All rooms are destroyed when enabled
• Users see maintenance message with reason"####,
        "/mute" => r####"<i>Mute the audio output in voice chat.</i>

<u>Usage:</u>
<b>/mute</b> — Mute indefinitely

<b>⚙️ Features:</b>
• Audio continues playing (progress tracked)

<b>💡 Examples:</b>
<code>/mute</code> — Mute until manual unmute

<b>⚠️ Notes:</b>
• Track continues playing in background
• Use <code>/unmute</code> to restore audio"####,
        "/myplaylists" => r####"<i>List all of your saved playlists.</i>

<u>Usage:</u>
<b>/myplaylists</b>"####,
        "/nothumb" => r####"<i>Toggle thumbnail/artwork display in playback messages.</i>

<u>Usage:</u>
<b>/nothumb</b> — Show current status
<b>/nothumb [enable|disable]</b> — Change setting

<b>⚙️ Behavior:</b>
• <b>Disabled (default):</b> Shows track artwork/thumbnail
• <b>Enabled:</b> Hides artwork, text-only messages

<b>💡 Examples:</b>
<code>/nothumb enable</code> — Disable thumbnails
<code>/nothumb disable</code> — Enable thumbnails

<b>⚠️ Note:</b>
This setting affects all future playback messages in this chat."####,
        "/pause" => r####"<i>Pause the current playback.</i>

<u>Usage:</u>
<b>/pause</b> — Pause playback

<b>⚙️ Features:</b>
• Manual pause/resume control

<b>💡 Examples:</b>
<code>/pause</code> — Pause indefinitely
"####,
        "/ping" => r####"<i>Check bot responsiveness and system stats.</i>

<u>Usage:</u>
<b>/ping</b> — Get bot status

<b>📊 Information Shown:</b>
• Response latency (ms)
• Uptime
• RAM usage
• CPU usage
• Disk usage

<b>💡 Use Case:</b>
Check if bot is responsive and view system health."####,
        "/play" => r####"<i>Play a song in the voice chat from YouTube, Spotify, or other sources.</i>

<u>Usage:</u>
<b>/play [query/URL]</b> — Search and play a song
<b>/play [reply to audio/video]</b> — Play replied media

<b>🎵 Supported Sources:</b>
• YouTube (videos, playlists)
• Spotify (tracks, albums, playlists)
• SoundCloud
• Direct audio/video links

<b>⚙️ Features:</b>
• Queue support - adds to end if already playing
• Auto-join voice chat if not present
• Duration limit check
• Multiple track support (playlists)

<b>💡 Examples:</b>
<code>/play never gonna give you up</code>
<code>/play https://youtu.be/dQw4w9WgXcQ</code>

<b>⚠️ Notes:</b>
• Bot must have proper permissions in voice chat
• Tracks exceeding duration limit will be skipped
• Use <code>/queue</code> to view upcoming tracks
• Use <code>/fplay</code> to force play (skip queue)"####,
        "/playforce" => r####"<i>Force play a song, skipping the current queue.</i>

<u>Usage:</u>
<b>/fplay [query/URL]</b> — Force play immediately
<b>/fplay [reply to audio/video]</b> — Force play replied media

<b>🎵 Behavior:</b>
• Stops current playback
• Clears queue
• Starts playing immediately

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Example:</b>
<code>/fplay urgent announcement track</code>

<b>⚠️ Note:</b>
This command is useful for urgent playback needs but will disrupt the current queue."####,
        "/playlistinfo" => r####"<i>Show the tracks saved in a playlist.</i>

<u>Usage:</u>
<b>/playlistinfo [playlist id]</b>"####,
        "/playmode" => r####"<i>Control who can use the /play command in this chat.</i>

<u>Usage:</u>
<b>/playmode [enable|disable]</b> — Set play mode restriction

<b>⚙️ Options:</b>
• <b>enable</b> — Only admins and authorized users can play
• <b>disable</b> — Everyone can play (default)"####,
        "/position" => r####"<i>Show current playback position and track info.</i>

<u>Usage:</u>
<b>/position</b> — Show position

<b>📊 Information Displayed:</b>
• Current track title
• Current position (MM:SS)
• Total duration (MM:SS)
• Playback speed (if not 1.0x)

<b>💡 Use Case:</b>
Quick position check without full queue display."####,
        "/privacy" => r####"<i>Show the bot's privacy policy.</i>"####,
        "/reload" => r####"<i>Reload admin cache and refresh voice chat state.</i>

<u>Usage:</u>
<b>/reload</b> — Refresh all cached data

<b>🔄 What Gets Reloaded:</b>
• Chat admin list
• Voice chat status
• Assistant presence status
• Assistant ban status

<b>🔒 Flood Protection:</b>
• Regular users: 5 minute cooldown
• Admins: 2 minute cooldown

<b>💡 When to Use:</b>
• After promoting/demoting admins
• Voice chat issues
• Permission problems
• Bot behaving incorrectly

<b>⚠️ Notes:</b>
• May reset room state if admin permissions required
"####,
        "/removefromplaylist" => r####"<i>Remove a track from one of your playlists.</i>

<u>Usage:</u>
<b>/removefromplaylist [playlist id] [song number or url]</b>"####,
        "/replay" => r####"<i>Restart the current track from the beginning.</i>

<u>Usage:</u>
<b>/replay</b> — Restart current track

<b>⚙️ Behavior:</b>
• Resets position to 0:00
• Maintains speed setting
• Continues playback immediately

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this
"####,
        "/repo" => r####"<i>Show bot source code repository and developer info.</i>"####,
        "/restart" => r####"<i>Restart the bot process.</i>

<u>Usage:</u>
<b>/restart</b> — Restart bot

<b>⚙️ Behavior:</b>
• Stops all active rooms
• Notifies all active chats
• Restarts bot process
• Clears download cache

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>⚠️ Warning:</b>
All playback will be interrupted. Bot will be offline for a few seconds."####,
        "/resume" => r####"<i>Resume the paused playback.</i>

<u>Usage:</u>
<b>/resume</b> — Resume playback from pause

<b>⚙️ Behavior:</b>
• Continues from last paused position
• Cancels auto-resume timer if active

<b>⚠️ Notes:</b>
• Can only resume if currently paused
• Position is preserved during pause
• Speed settings remain active after resume"####,
        "/seek" => r####"<i>Seek forward in the currently playing track.</i>

<u>Usage:</u>
<b>/seek [seconds]</b> — Skip forward by specified seconds

<b>⚙️ Features:</b>
• Jump ahead in current track
• Position tracking updated
• Cannot seek past track end (10s buffer)

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Examples:</b>
<code>/seek 30</code> — Skip forward 30 seconds
<code>/seek 120</code> — Skip forward 2 minutes

<b>⚠️ Notes:</b>
• Minimum: any positive value
• Maximum: track_duration - current_position - 10 seconds"####,
        "/seekback" => r####"<i>Seek backward in the currently playing track.</i>

<u>Usage:</u>
<b>/seekback [seconds]</b> — Go back by specified seconds

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Examples:</b>
<code>/seekback 15</code> — Go back 15 seconds
<code>/seekback 60</code> — Go back 1 minute
"####,
        "/setcplay" => r####"<i>Configure linked channel for channel play mode.</i>

<u>Usage:</u>
<b>/channelplay [channel_id]</b> — Set linked channel

<b>⚙️ Behavior:</b>
• Links a channel to current group
• All <code>c*</code> commands affect linked channel
• Channel must be accessible by bot

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> can configure

<b>💡 Examples:</b>
<code>/setcplay -1001234567890</code>

<b>⚠️ Notes:</b>
• Get channel ID using forward + @userinfobot
• Bot must be admin in linked channel
• Use <code>/cplay</code> after setup"####,
        "/sh" => r####"<i>Execute shell commands on server.</i>

<u>Usage:</u>
<b>/sh [command]</b> — Run shell command

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>⚠️ Warning:</b>
Direct system access - extremely powerful."####,
        "/shell" => r####"<i>Execute shell commands on server.</i>

<u>Usage:</u>
<b>/sh [command]</b> — Run shell command

<b>🔒 Restrictions:</b>
• <b>Owner only</b> command

<b>⚠️ Warning:</b>
Direct system access - extremely powerful."####,
        "/shuffle" => r####"<i>Toggle shuffle mode for the queue.</i>

<u>Usage:</u>
<b>/shuffle</b> — Show current shuffle state
<b>/shuffle on</b> — Enable shuffle
<b>/shuffle off</b> — Disable shuffle

<b>⚙️ Behavior:</b>
• Randomly reorders queue when enabled
• Affects track selection order
• Can be toggled at any time

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Examples:</b>
<code>/shuffle on</code> — Enable shuffle mode
<code>/shuffle off</code> — Disable shuffle mode

<b>⚠️ Note:</b>
Shuffle only affects queue order, not currently playing track."####,
        "/skip" => r####"<i>Skip the currently playing track and play the next in queue.</i>

<u>Usage:</u>
<b>/skip</b> — Skip current track

<b>⚙️ Behavior:</b>
• Downloads next track in queue
• Starts playback automatically
• If queue is empty and loop is 0, stops playback

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>⚠️ Notes:</b>
• Cannot be undone
• If no tracks in queue, playback stops
• Loop count affects skip behavior"####,
        "/speed" => r####"<i>Control playback speed (tempo).</i>

<u>Usage:</u>
<b>/speed</b> — Show current speed
<b>/speed [multiplier]</b> — Set speed (0.5-4.0x)
<b>/speed normal</b> or <b>/speed reset</b> — Reset to 1.0x

<b>⚙️ Features:</b>
• Range: 0.50x to 4.00x
• Pitch preservation
• Real-time adjustment

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>💡 Examples:</b>
<code>/speed 1.5</code> — Play 1.5x faster
<code>/speed 0.75</code> — Play slower (0.75x)
<code>/speed normal</code> — Reset to normal speed

<b>⚠️ Notes:</b>
• Speed affects duration calculations
• Suffix 'x' is optional: <code>1.5</code> = <code>1.5x</code>"####,
        "/stats" => r####"<i>View detailed bot statistics.</i>

<u>Usage:</u>
<b>/stats</b> — Show statistics

<b>📊 Information Shown:</b>
• System stats (OS, CPU, RAM, disk)
• Go runtime stats (memory, GC)
• Server resources
• Served chats count
• Served users count

<b>🔒 Restrictions:</b>
• <b>Sudo users</b> only"####,
        "/stop" => r####"<i>Stop playback and leave the voice chat.</i>

<u>Usage:</u>
<b>/stop</b> or <b>/end</b> — Stop playback

<b>⚙️ Behavior:</b>
• Stops current track
• Clears queue
• Assistant leaves voice chat
•
<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this

<b>⚠️ Note:</b>
This action cannot be undone. Use <code>/pause</code> for temporary stops."####,
        "/unmute" => r####"<i>Unmute the audio output in voice chat.</i>

<u>Usage:</u>
<b>/unmute</b> — Restore audio

<b>⚙️ Behavior:</b>
• Restores audio immediately
• Cancels auto-unmute timer if active
• Shows current playback info"####,
        "/vclogger" => r####"<i>Toggle voice chat join/leave alerts in your group.</i>

<u>Usage:</u>
<b>/vclogger</b> — Check current VC logger status
<b>/vclogger on</b> — Enable voice chat alerts
<b>/vclogger off</b> — Disable voice chat alerts
<b>/vcstatus</b> — View detailed VC logger and monitor metrics

<b>⚙️ Behavior:</b>
• Tracks participants joining and leaving the voice chat
• Automatically cleans up alert messages after 5 seconds
• Displays user's name, ID, username, and total VC count

<b>🔒 Restrictions:</b>
• Only <b>chat admins</b> or <b>authorized users</b> can use this"####,
        "/vplay" => r####"<i>Play video content in voice chat (video mode).</i>

<u>Usage:</u>
<b>/vplay [query/URL]</b> — Play video
<b>/vplay [reply to video]</b> — Play replied video

<b>📹 Features:</b>
• Full video playback support
• Audio + Video streaming
• Same queue system as audio

<b>⚠️ Notes:</b>
• Requires video streaming permissions
• Use <code>/fvplay</code> for force video play"####,
        "autoleave" => r####"<i>Automatically makes the assistant leave inactive or unnecessary chats every 10 minutes.</i>

<u>Usage:</u>
<b>/autoleave </b>— Shows current auto-leave status (enabled/disabled).  
<b>/autoleave enable</b> — Enable auto-leave mode.  
<b>/autoleave disable</b> — Disable auto-leave mode.

<b>🧠 Details:</b>
Once enabled, the bot checks all joined groups/channels every <b>10 minutes</b> and leaves up to <b>%d chats per cycle</b> that are not in the active room  list.

<b>⚠️ Restrictions:</b>
This command can only be used by <b>owners</b> or <b>sudo users</b>."####,
        "bcast" => r####"<i>Broadcast a message to all served chats and users.</i>

<u>Usage:</u>
<b>/broadcast [flags] [text] </b> — Broadcast text message.
<b>/broadcast [flags] [reply to message]</b> — Broadcast the replied message.
<b>/broadcast -cancel</b> — Cancel ongoing broadcast.

<blockquote>
<b>📋 Flags:</b>
• <code>--nochat</code> — Exclude groups from broadcast
• <code>--nouser</code> — Exclude users from broadcast
• <code>--copy</code> — Remove forwarded tag, when broadcasting a replied message (copy mode)
• <code>--limit [n]</code> — Limit total messages sent (default: 0 = no limit)
• <code>--delay [seconds]</code> — Delay between messages (default: 1.5s)
• <code>--pin</code> — Pin the message (silent)
• <code>--pinloud</code> — Pin the message (with notification)

• <code>-cancel</code> - Cancel a ongoing broadcast.
</blockquote>
<blockquote>
<b>📌 Examples:</b>
/broadcast -nochat -delay 2 Important announcement
/broadcast -copy -nochat -pin [reply to message]
/broadcast -limit 10 -delay 3 Limited broadcast
</blockquote>
<b>⚠️ Notes:</b>
• Only the <b>owner</b> can use this command
• After every 30 messages, there's an automatic 7.5s pause
• You can cancel ongoing broadcasts using the inline button or <code>/broadcast -cancel</code>
• Only one broadcast can run at a time"####,
        "broadcast" => r####"<i>Broadcast a message to all served chats and users.</i>

<u>Usage:</u>
<b>/broadcast [flags] [text] </b> — Broadcast text message.
<b>/broadcast [flags] [reply to message]</b> — Broadcast the replied message.
<b>/broadcast -cancel</b> — Cancel ongoing broadcast.

<blockquote>
<b>📋 Flags:</b>
• <code>--nochat</code> — Exclude groups from broadcast
• <code>--nouser</code> — Exclude users from broadcast
• <code>--copy</code> — Remove forwarded tag, when broadcasting a replied message (copy mode)
• <code>--limit [n]</code> — Limit total messages sent (default: 0 = no limit)
• <code>--delay [seconds]</code> — Delay between messages (default: 1.5s)
• <code>--pin</code> — Pin the message (silent)
• <code>--pinloud</code> — Pin the message (with notification)

• <code>-cancel</code> - Cancel a ongoing broadcast.
</blockquote>
<blockquote>
<b>📌 Examples:</b>
/broadcast -nochat -delay 2 Important announcement
/broadcast -copy -nochat -pin [reply to message]
/broadcast -limit 10 -delay 3 Limited broadcast
</blockquote>
<b>⚠️ Notes:</b>
• Only the <b>owner</b> can use this command
• After every 30 messages, there's an automatic 7.5s pause
• You can cancel ongoing broadcasts using the inline button or <code>/broadcast -cancel</code>
• Only one broadcast can run at a time"####,
        "gcast" => r####"<i>Broadcast a message to all served chats and users.</i>

<u>Usage:</u>
<b>/broadcast [flags] [text] </b> — Broadcast text message.
<b>/broadcast [flags] [reply to message]</b> — Broadcast the replied message.
<b>/broadcast -cancel</b> — Cancel ongoing broadcast.

<blockquote>
<b>📋 Flags:</b>
• <code>--nochat</code> — Exclude groups from broadcast
• <code>--nouser</code> — Exclude users from broadcast
• <code>--copy</code> — Remove forwarded tag, when broadcasting a replied message (copy mode)
• <code>--limit [n]</code> — Limit total messages sent (default: 0 = no limit)
• <code>--delay [seconds]</code> — Delay between messages (default: 1.5s)
• <code>--pin</code> — Pin the message (silent)
• <code>--pinloud</code> — Pin the message (with notification)

• <code>-cancel</code> - Cancel a ongoing broadcast.
</blockquote>
<blockquote>
<b>📌 Examples:</b>
/broadcast -nochat -delay 2 Important announcement
/broadcast -copy -nochat -pin [reply to message]
/broadcast -limit 10 -delay 3 Limited broadcast
</blockquote>
<b>⚠️ Notes:</b>
• Only the <b>owner</b> can use this command
• After every 30 messages, there's an automatic 7.5s pause
• You can cancel ongoing broadcasts using the inline button or <code>/broadcast -cancel</code>
• Only one broadcast can run at a time"####,
        _ => "",
    };
    t.to_string()
}
