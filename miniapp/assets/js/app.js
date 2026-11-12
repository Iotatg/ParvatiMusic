/* ============================================================
   Parvati Music — Mini App client
   Talks to the bot's mini-app API (/api/*). Falls back to a
   local demo room when opened without a backend.
   ============================================================ */
(() => {
  'use strict';

  const TOKEN = new URLSearchParams(location.search).get('token') || '';
  const withToken = (p) => (TOKEN ? p + (p.includes('?') ? '&' : '?') + 'token=' + encodeURIComponent(TOKEN) : p);
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];

  const tg = window.Telegram?.WebApp;
  if (tg) {
    try {
      tg.ready();
      tg.expand();
      tg.setHeaderColor?.('#000000');
      tg.setBackgroundColor?.('#000000');
      tg.disableVerticalSwipes?.();
    } catch {}
  }
  /* Telegram signs `initData` with the bot token; the server verifies that
     HMAC, so this is the only thing that proves who is opening the app. */
  const INIT_DATA = tg?.initData || '';
  if (INIT_DATA) {
    // <img>, <video> and EventSource cannot set request headers, so the same
    // credential is stored in a cookie they send automatically.
    document.cookie = 'parvati_auth=' + encodeURIComponent(INIT_DATA) + '; path=/; SameSite=Lax; max-age=86400';
  }
  const withAuth = (p) => {
    let u = withToken(p);
    if (INIT_DATA) u += (u.includes('?') ? '&' : '?') + 'init=' + encodeURIComponent(INIT_DATA);
    return u;
  };
  const haptic = (type = 'light') => {
    if (state.settings && state.settings.haptics === false) return;
    try {
      if (type === 'select') tg?.HapticFeedback?.selectionChanged?.();
      else tg?.HapticFeedback?.impactOccurred?.(type);
    } catch {}
  };

  /* ────────────────────────── viewer ────────────────────────── */
  const qsParam = new URLSearchParams(location.search);
  const tgUser = tg?.initDataUnsafe?.user;
  let viewer = tgUser
    ? {
        id: String(tgUser.id),
        name: [tgUser.first_name, tgUser.last_name].filter(Boolean).join(' '),
        username: tgUser.username || '',
        photo: tgUser.photo_url || '',
      }
    : qsParam.get('uid')
      ? { id: qsParam.get('uid'), name: qsParam.get('name') || 'you', username: qsParam.get('un') || '', photo: '' }
      : null;

  /* ────────────────────────── demo fallback ────────────────────────── */
  const IMG = ['khat', 'stage', 'neon', 'crowd', 'dj', 'guitar'].map(n => `assets/img/cover-${n}.jpg`);
  const DEMO = {
    room: { id: -1001, title: 'chingu gang', members: 2, online: 1, live: true },
    session: { live: true, started_at: 0, duration: 0, ended_at: 0, ended_hm: '', ended_by: '', tracks: 0, last_track: '', cleared: false },
    me: {
      id: '42', name: '– ѕ т α ℓ к є я –', username: 'stalker', photo: 'assets/img/av-me.jpg',
      level: 4, levelPct: 62, toNext: '1h 0m', listening: '4h 59m', songs: 125, liked: 12,
    },
    quote: "I can't hate anyone, I just know where not to return.",
    playback: {
      track: { id: 't1', title: 'Navjot Ahuja - Khat (Official Audio)', artist: 'Navjot Ahuja', duration: 297, art: IMG[0], requester: '– ѕ т α ℓ к є я –', source: 'YouTube' },
      position: 43, paused: false, loop: 0, shuffle: false,
      queue: [
        { id: 't2', title: 'Khat (Lyrics) | Tere Liye', artist: 'Navjot Ahuja', duration: 297, art: IMG[1], requester: 'stalker', source: 'YouTube' },
        { id: 't3', title: 'Khat (Lyrics)', artist: 'Navjot Ahuja', duration: 297, art: IMG[2], requester: 'stalker', source: 'YouTube' },
        { id: 't4', title: 'Until I Found You', artist: 'Stephen Sanchez', duration: 181, art: IMG[3], requester: 'chingu', source: 'Spotify' },
      ],
    },
    members: [
      { id: '42', name: '– ѕ т α ℓ к є я –', username: 'stalker', photo: 'assets/img/av-me.jpg', role: 'owner', tags: [{ k: 'owner', label: 'OWNER' }], vc: true, host: true },
      { id: '77', name: 'stalker', username: 'stalker', photo: 'assets/img/av-2.jpg', role: 'member', tags: [{ k: 'member', label: 'MEMBER' }] },
    ],
    messages: [
      { id: 'm1', mid: 1, from: '42', name: 'you', mine: true, kind: 'text', text: 'YO', ts: '16:32' },
      { id: 'm3', mid: 3, from: '77', name: 'stalker', kind: 'text', text: 'stickers tool!', ts: '16:34' },
      { id: 'm5', mid: 5, from: '42', name: 'you', mine: true, kind: 'text', text: 'yesss', ts: '16:52' },
    ],
  };

  const state = structuredClone(DEMO);
  state.recent = [];
  state.favorites = [];
  state.sleep = 0;
  state.settings = {};
  /// Chat the viewer is looking at (one per group) and the switcher list.
  state.chat = { id: '', title: 'chingu gang', members: 0, live: true };
  state.chats = [];
  const BLANK_ME = { name: 'You', username: 'you', photo: '', level: 1, levelPct: 0, toNext: '0m', listening: '0m', songs: 0, liked: 0 };
  let online = false;
  let localPos = 0;
  let lastSync = 0;
  let dragging = false;
  let activeTab = 'song';
  let replyTarget = null;
  let unread = 0;
  let chatSig = '';
  /// A long-press or a swipe consumes the click that follows it, so the
  /// tap-to-reply / double-tap-to-react handler must ignore that one event.
  let suppressUntil = 0;
  let suppressGridClick = false;
  /// Shared by the tap / double-tap / long-press / swipe handlers so a swipe
  /// or a long-press never also fires the plain tap-to-reply action.
  let lastTap = { id: '', at: 0 };
  /// Everything social that is not part of the room snapshot: the open
  /// profile, the message in the action menu, the sticker being previewed,
  /// the message being forwarded, and this viewer's saved sticker list.
  const social = {
    profile: null,
    msgMenu: null,
    sticker: null,
    forward: null,
    savedStickers: [],
  };
  /// 'vc' = only people currently in the voice chat (the room's own chat),
  /// 'all' = every mirrored group message.
  let chatScope = 'vc';
  const dm = { open: false, mode: 'list', peer: null, threads: [], messages: [] };
  const tgsCache = new Map();
  /// Per-section render signatures. The app used to repaint every section on
  /// every server tick (1.5 s), which is what made it feel laggy; a section is
  /// now only rebuilt when its own data actually changed.
  const lastSig = { members: '', queue: '', song: '', session: '', chat: '', profile: '' };
  let errSeenAt = 0;

  /* ────────────────────────── api ────────────────────────── */
  async function api(path, body) {
    const headers = { 'content-type': 'application/json' };
    if (TOKEN) headers['x-miniapp-token'] = TOKEN;
    // Header values must stay ASCII: initData contains '&' and '='.
    if (INIT_DATA) headers['x-telegram-init-data'] = encodeURIComponent(INIT_DATA);
    const res = await fetch(withAuth(path), body === undefined ? { headers } : {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      let msg = `${res.status}`;
      try { const j = await res.json(); if (j.error) msg = j.error; if (j.hint) msg = j.hint; } catch {}
      throw new Error(msg);
    }
    return res.json();
  }

  function merge(next) {
    if (!next) return;
    if (next.room) Object.assign(state.room, next.room);
    if (next.me) state.me = { ...BLANK_ME, ...next.me };
    if (next.playback) {
      Object.assign(state.playback, next.playback);
      if ('position' in next.playback) { localPos = next.playback.position; lastSync = performance.now(); }
    }
    if (next.members) state.members = next.members;
    if (next.chat) state.chat = { ...state.chat, ...next.chat };
    if (Array.isArray(next.chats)) {
      // Delete permissions are resolved by /api/chats when the switcher opens.
      // A cheaper state frame must not wipe them and make the buttons blink
      // away while the sheet is open.
      const known = new Map((state.chats || []).map(c => [String(c.id), c]));
      state.chats = next.chats.map(c => {
        const old = known.get(String(c.id));
        return old && old.can_delete && !c.can_delete
          ? { ...c, can_delete: true } : c;
      });
    }
    if (next.messages) {
      state.messages = next.messages;
      chatSig = sigOf(next.messages);
    }
    if (next.session) state.session = { ...(state.session || {}), ...next.session };
    if (typeof next.quote === 'string') state.quote = next.quote;
    if (Array.isArray(next.recent)) state.recent = next.recent;
    if (Array.isArray(next.favorites)) state.favorites = next.favorites;
    if (typeof next.sleep === 'number') state.sleep = next.sleep;
    if (next.settings) state.settings = { ...(state.settings || {}), ...next.settings };
  }

  const sigOf = (list) => {
    if (!list || !list.length) return '0';
    const last = list[list.length - 1];
    return `${list.length}|${last.id}|${last.mid || 0}|${last.media || ''}|${last.media_kind || ''}|${last.reacted || ''}`;
  };

  async function boot() {
    try {
      const s = await api('/api/state');
      merge(s);
      online = true;
      if (!viewer) viewer = { id: String(state.me.id || ''), name: state.me.name || 'you', username: state.me.username || '', photo: state.me.photo || '' };
      connectEvents();
    } catch (e) {
      online = false;
      // The server refused the credential: never fall back to a demo room,
      // that is exactly the "anyone with the link can use it" hole.
      const msg = String((e && e.message) || '');
      if (msg === 'unauthorized' || /Telegram|member|allowed/i.test(msg)) {
        showGate(msg);
        return;
      }
      if (!viewer) viewer = { id: '42', name: 'you', username: 'you', photo: '' };
      setInterval(() => { if (!state.playback.paused && state.playback.track) localPos += 1; }, 1000);
    }
    loadSavedStickers();
    renderAll();
  }

  /** Full-screen block shown when Telegram verification fails. */
  function showGate(reason) {
    if (document.querySelector('.auth-gate')) return;
    const el = document.createElement('div');
    el.className = 'auth-gate';
    el.innerHTML = `
      <div class="auth-card">
        <div class="auth-icon">🔒</div>
        <h2>Verification required</h2>
        <p>${esc(reason || 'Open this app from Telegram.')}</p>
        <p class="auth-sub">This room only opens for Telegram accounts that belong to it. A shared link alone is not enough.</p>
      </div>`;
    document.body.appendChild(el);
  }

  function connectEvents() {
    try {
      const es = new EventSource(withAuth('/api/events'));
      es.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data);
          if (msg.type === 'state') { merge(msg.data); queueRender(); }
          else if (msg.type === 'messages') {
            // A frame can arrive for the chat the viewer just left; applying it
            // would paint another group's messages into this one.
            if (msg.chat && state.chat.id && msg.chat !== state.chat.id) return;
            const sig = sigOf(msg.data);
            if (sig === chatSig) return;
            const grew = msg.data.length > state.messages.length;
            state.messages = msg.data;
            chatSig = sig;
            renderChat(true);
            if (grew) {
              if (activeTab !== 'chat') { unread += 1; paintUnread(); }
              else markRead();
            }
          }
        } catch {}
      };
      es.onerror = () => { es.close(); setTimeout(connectEvents, 3000); };
    } catch {}
  }

  /* ────────────────────────── utils ────────────────────────── */
  const esc = (s = '') => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const fmt = (sec) => {
    sec = Math.max(0, Math.round(sec || 0));
    return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`;
  };
  const clamp = (n, a, b) => Math.min(b, Math.max(a, n));

  /* ── generated fallbacks (bot rooms often have no artwork/avatars) ── */
  const hueOf = (str) => { let h = 0; for (const c of String(str)) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
  const avatarURI = (name = '?') => {
    const h = hueOf(name);
    // Prefer a plain letter: decorated names (𝐈ᴏᴛᴀ) render blank in some fonts.
    // Stay inside the BMP so a broken surrogate in a name can never make
    // encodeURIComponent throw.
    const raw = String(name || '').trim();
    const ascii = raw.match(/[A-Za-z0-9]/);
    let initial = '?';
    if (ascii) {
      initial = ascii[0];
    } else if (raw) {
      const cp = raw.codePointAt(0);
      if (cp && cp <= 0xffff && !(cp >= 0xd800 && cp <= 0xdfff)) {
        initial = String.fromCharCode(cp);
      }
    }
    initial = initial.toUpperCase();
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${h},58%,52%)"/><stop offset="1" stop-color="hsl(${(h + 46) % 360},64%,28%)"/></linearGradient></defs><rect width="96" height="96" fill="url(#g)"/><text x="48" y="62" font-family="Poppins,Arial" font-size="38" font-weight="600" fill="rgba(255,255,255,.94)" text-anchor="middle">${initial.replace(/[<>&]/g, '')}</text></svg>`;
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  };
  const coverURI = (title = '') => {
    const h = hueOf(title);
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="240"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${h},48%,30%)"/><stop offset=".55" stop-color="hsl(${(h + 30) % 360},44%,14%)"/><stop offset="1" stop-color="#050506"/></linearGradient></defs><rect width="240" height="240" fill="url(#g)"/><circle cx="188" cy="54" r="46" fill="hsl(${(h + 60) % 360},70%,58%)" opacity=".22"/><g fill="none" stroke="rgba(255,255,255,.5)" stroke-width="5" stroke-linecap="round"><path d="M104 158V92l52-11v62"/><circle cx="92" cy="160" r="13"/><circle cx="144" cy="147" r="13"/></g></svg>`;
    return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  };
  const pic = (src, name) => src || avatarURI(name);
  let fbWired = false;
  // Delegated: images are inserted all the time (roster, chat, stickers), so
  // one capture-phase listener handles every load failure instead of binding
  // per element at boot.
  function wireImgFallbacks() {
    if (fbWired) return;
    fbWired = true;
    document.addEventListener('error', (ev) => {
      const el = ev.target;
      if (!el || el.tagName !== 'IMG' || el.dataset.dead) return;
      el.dataset.dead = '1';
      const isCover = el.id === 'artImg' || el.id === 'miniArt'
        || el.classList.contains('q-thumb') || !!el.closest('.result');
      if (isCover) {
        el.src = coverURI(el.alt || el.dataset.name || '');
      } else if (el.classList.contains('emoji')) {
        el.replaceWith(document.createTextNode(el.dataset.ch || '🙂'));
      } else {
        el.src = avatarURI(el.dataset.name || el.alt || '?');
      }
    }, true);
  }

  function toast(text) {
    const el = $('#toast');
    el.textContent = text;
    el.classList.add('is-on');
    clearTimeout(toast._t);
    toast._t = setTimeout(() => el.classList.remove('is-on'), 1900);
  }

  /* ────────────────────────── tags ────────────────────────── */
  const memberById = (id) => state.members.find(m => String(m.id) === String(id));
  const myTags = () => memberById(viewer?.id)?.tags || [{ k: 'member', label: 'MEMBER' }];
  const amAdmin = () => (memberById(viewer?.id)?.tags || [])
    .some(t => t.k === 'owner' || t.k === 'admin' || t.k === 'host');
  const tagHTML = (tags, max = 3) => (tags || [])
    .slice(0, max)
    .map(t => `<span class="tag ${esc(t.k)}">${esc(t.label)}</span>`)
    .join('');

  /* ────────────────────────── render ────────────────────────── */
  function renderAll(force = false) {
    // A hidden tab costs nothing to update: skip the DOM work entirely and
    // paint once when the app comes back to the foreground.
    if (document.hidden) { renderDirty = true; return; }
    renderDirty = false;
    renderRoom();
    renderChatHeader();
    renderSong(force);
    renderMembers(force);
    renderQueue(force);
    renderProfile(force);
    renderChat(force);
    renderMini();
    wireImgFallbacks();
  }

  /// Several SSE frames can land inside one animation frame (a track change
  /// also touches members and the queue). Coalesce them into a single paint.
  let renderQueued = false;
  let renderDirty = false;
  function queueRender(force = false) {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => {
      renderQueued = false;
      renderAll(force);
    });
  }

  function renderRoom() {
    $('#roomName').textContent = state.room.title;
  }

  /// Header of the chat tab: which group this chat belongs to, whether its
  /// voice chat is live, and how many unread messages the *other* chats hold.
  function renderChatHeader() {
    const c = state.chat || {};
    const name = c.title || state.room?.title || 'Chat';
    const nameEl = $('#chatName');
    if (nameEl) nameEl.textContent = name;
    const dot = $('#csDot');
    if (dot) dot.classList.toggle('is-on', !!c.live);
    const list = state.chats || [];
    const other = list.reduce((n, x) =>
      n + (String(x.id) === String(c.id) ? 0 : Math.max(0, +x.unread || 0)), 0);
    const badge = $('#chatBadge');
    if (badge) {
      badge.hidden = other <= 0;
      badge.textContent = other > 99 ? '99+' : String(other);
    }
    const sw = $('#chatSwitch');
    if (sw) sw.title = `${list.length || 1} chat${(list.length || 1) === 1 ? '' : 's'} · tap to switch`;
    // Only an admin (or the owner) can clear a chat: hide the action rather
    // than letting a member tap it and get a 403.
    const clearRow = $('#chatClearRow');
    if (clearRow) clearRow.style.display = c.can_clear ? '' : 'none';
    // The sheet stays mounted; repaint it in place while it is open.
    const sheet = $('#chatsSheet');
    if (sheet && sheet.classList.contains('is-open')) renderChatRows();
    paintUnread();
  }

  /* ───────────────────── session (live / ended) ───────────────────── */
  const isLive = () => state.room?.live !== false;
  /// Is the chat on screen the one that belongs to the playing room? Every
  /// other group has its own chat, and those stay open (and unfiltered) no
  /// matter what happens in this room's voice session.
  const viewingLiveRoom = () => !state.chat?.id || String(state.chat.id) === String(state.room?.id);
  const chatLive = () => (viewingLiveRoom() ? isLive() : true);
  const session = () => state.session || {};

  const fmtSpan = (sec) => {
    sec = Math.max(0, Math.round(sec || 0));
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    if (h) return `${h}h ${m}m`;
    if (m) return `${m}m ${String(sec % 60).padStart(2, '0')}s`;
    return `${sec}s`;
  };

  // One function owns every "the call is over" surface: the pill, the song
  // tab card, the members strip, the composer hint and the chat empty state.
  function renderSession() {
    const s = session();
    const live = isLive();
    const ended = !live && (s.ended_at || 0) > 0;
    $('#app').dataset.ended = ended ? 'true' : 'false';

    $('#liveBadge').classList.toggle('is-off', !live);
    $('#liveText').textContent = live ? 'LIVE' : 'ENDED';

    const total = state.room.members || state.members.length;
    const inVoice = live ? (state.room.online || state.members.filter(m => m.vc).length) : 0;
    const vcCount = $('#vcCount');
    if (vcCount) vcCount.textContent = String(inVoice);
    $('#roomSub').textContent = live
      ? `${total} members · ${inVoice} in voice chat`
      : `${total} members · voice chat ended`;

    const strip = $('#membersStrip');
    if (strip) {
      strip.hidden = !ended;
      if (ended) {
        $('#membersStripText').textContent = `Voice chat ended${s.ended_hm ? ` at ${s.ended_hm}` : ''} — left the call · chat cleared`;
      }
    }

    if (ended) {
      $('#endedSub').textContent = s.ended_by
        ? `Ended by ${s.ended_by}${s.ended_hm ? ` at ${s.ended_hm}` : ''}`
        : 'Voice chat ended';
      const cells = [
        ['DURATION', fmtSpan(s.duration)],
        ['TRACKS', String(s.tracks || 0)],
        ['IN VOICE', '0'],
        ['CHAT', s.cleared ? 'cleared' : 'kept'],
      ];
      $('#endedGrid').innerHTML = cells
        .map(([k, v]) => `<div class="ended-cell"><span>${esc(k)}</span><b>${esc(v)}</b></div>`)
        .join('');
      const last = $('#endedLast');
      last.textContent = s.last_track ? `Last played · ${s.last_track}` : '';
      last.hidden = !s.last_track;
    }

    const input = $('#chatInput');
    const open = chatLive();
    if (input) input.dataset.ph = open ? 'Message the room…' : 'Voice chat ended — chat is cleared';
    // No posting into a group whose voice chat is over.
    if (input) input.contentEditable = open ? 'true' : 'false';
    const pill = document.querySelector('.composer-pill');
    if (pill) pill.classList.toggle('is-locked', !open);
  }

  function currentTrack() { return state.playback.track; }
  /// The track the user just picked, still downloading on the server. Painting
  /// it right away is what makes a tap feel instant.
  const pendingTrack = () => state.playback.pending || null;
  const displayTrack = () => currentTrack() || pendingTrack();

  function renderSong(force = false) {
    const t = currentTrack();
    const p = pendingTrack();
    const shown = t || p;
    const sig = shown
      ? [shown.id, shown.title, shown.art, t ? 1 : 0, p ? 1 : 0,
         state.playback.paused ? 1 : 0, state.playback.loop, shown.requester, shown.source,
         shown.video ? 1 : 0, state.playback.liked ? 1 : 0, state.sleep].join('|')
      : `none|${state.sleep}|${state.playback.liked ? 1 : 0}`;
    if (!force && sig === lastSig.song) { paintSleep(); paintFav(); paintProgress(); return; }
    lastSig.song = sig;

    const app = $('#app');
    app.dataset.playing = t ? 'true' : 'false';
    $('#screen').dataset.playing = app.dataset.playing;
    renderSession();
    if (!shown) { $('#bgArt').style.backgroundImage = ''; paintVideo(null); paintFav(); paintSleep(); return; }

    $('#quote').textContent = state.quote || '';
    $('#quote').style.display = state.quote ? '' : 'none';
    $('#artImg').src = shown.art || coverURI(shown.title);
    const src = (shown.source || 'YouTube').toLowerCase();
    const tag = src.includes('spot') ? 'SP' : src.includes('telegram') ? 'TG' : src.includes('deezer') ? 'DZ' : 'YT';
    $('#artBadge').innerHTML = shown.video
      ? `<b>VIDEO</b><i>720P</i>`
      : `<b>${tag}</b><i>192K AUDIO</i>`;
    $('#trackTitle').textContent = shown.title;
    $('#trackSub').textContent = !t && p
      ? `STARTING · DOWNLOADING ${shown.video ? '720P VIDEO' : 'AUDIO'} FROM YOUTUBE`
      : shown.requester
        ? `ADDED BY · ${shown.requester.toUpperCase()}`
        : shown.video
          ? '720P VIDEO · PLAYING IN VOICE CHAT'
          : 'PLAYING IN VOICE CHAT';
    $('#timeTotal').textContent = fmt(shown.duration);
    $('#bgArt').style.backgroundImage = `url("${shown.art}")`;
    $('#reqAvatar img').src = pic(state.me.photo, state.me.name);
    $('#btnPlay').querySelector('use').setAttribute('href', state.playback.paused ? '#i-play' : '#i-pause');
    $('#btnLoop').classList.toggle('is-on', state.playback.loop > 0);
    paintVideo(shown);
    paintAudio(shown);
    paintFav();
    paintSleep();
    paintProgress();
  }

  /// Video tracks play their picture here while the voice chat carries the
  /// sound, so the element stays muted (no echo) and follows the room clock.
  let videoBound = '';
  let videoRetry = 0;
  function paintVideo(shown) {
    const v = $('#artVideo');
    if (!v) return;
    if (!v.dataset.wired) {
      v.dataset.wired = '1';
      // The 720p mp4 is written after the DASH merge finishes, so the first
      // request can be a 404. Retry quietly until the file lands.
      v.addEventListener('error', () => {
        if (!videoBound || videoRetry >= 10 || !v.getAttribute('src')) return;
        videoRetry += 1;
        const id = videoBound;
        setTimeout(() => {
          if (videoBound !== id) return;
          v.src = withToken(`/api/media?id=${encodeURIComponent(id)}&kind=video`);
          try { v.load(); } catch {}
          syncVideo();
        }, 1500);
      });
      // Only reveal the picture once there is a frame — until then the cover
      // art underneath stays visible instead of a black rectangle.
      v.addEventListener('loadeddata', () => {
        if (videoBound) { v.classList.add('is-on'); syncVideo(); }
      });
    }
    const on = !!(shown && shown.video && currentTrack());
    if (!on) {
      if (videoBound) {
        videoBound = '';
        videoRetry = 0;
        try { v.pause(); } catch {}
        v.removeAttribute('src');
        try { v.load(); } catch {}
      }
      v.classList.remove('is-on');
      return;
    }
    if (videoBound !== shown.id) {
      videoBound = shown.id;
      videoRetry = 0;
      v.src = withToken(`/api/media?id=${encodeURIComponent(shown.id)}&kind=video`);
      try { v.load(); } catch {}
    }
    if (v.readyState >= 2) v.classList.add('is-on');
    syncVideo();
  }

  /// Nudge the element back onto the room's position. A 1.5 s window keeps
  /// this from fighting the decoder every tick.
  function syncVideo() {
    const v = $('#artVideo');
    if (!v || !v.classList.contains('is-on') || !videoBound) return;
    const want = localPos;
    if (Math.abs((v.currentTime || 0) - want) > 1.5) {
      try { v.currentTime = want; } catch {}
    }
    if (state.playback.paused) {
      if (!v.paused) { try { v.pause(); } catch {} }
    } else if (v.paused) {
      v.play?.().catch(() => {});
    }
  }

  /// Audio tracks: the file streams into this element so "Play audio" is
  /// audible. It stays muted while you are in the voice chat (the call is
  /// already playing it — two copies would echo), and unmuted otherwise.
  let audioBound = '';
  let audioRetry = 0;
  function paintAudio(shown) {
    const a = $('#artAudio');
    if (!a) return;
    if (!a.dataset.wired) {
      a.dataset.wired = '1';
      a.addEventListener('error', () => {
        if (!audioBound || audioRetry >= 12 || !a.getAttribute('src')) return;
        audioRetry += 1;
        const id = audioBound;
        setTimeout(() => {
          if (audioBound !== id) return;
          a.src = withToken(`/api/media?id=${encodeURIComponent(id)}&kind=audio`);
          try { a.load(); } catch {}
          syncAudio();
        }, 1500);
      });
      a.addEventListener('loadeddata', syncAudio);
    }
    const on = !!(shown && !shown.video && currentTrack());
    if (!on) {
      if (audioBound) {
        audioBound = '';
        audioRetry = 0;
        try { a.pause(); } catch {}
        a.removeAttribute('src');
        try { a.load(); } catch {}
      }
      return;
    }
    if (audioBound !== shown.id) {
      audioBound = shown.id;
      audioRetry = 0;
      a.src = withToken(`/api/media?id=${encodeURIComponent(shown.id)}&kind=audio`);
      try { a.load(); } catch {}
    }
    a.muted = !!(state.me && state.me.vc);
    a.volume = a.muted ? 0 : 1;
    syncAudio();
  }

  function syncAudio() {
    const a = $('#artAudio');
    if (!a || !audioBound) return;
    const want = localPos;
    if (Math.abs((a.currentTime || 0) - want) > 1.5) {
      try { a.currentTime = want; } catch {}
    }
    if (state.playback.paused) {
      if (!a.paused) { try { a.pause(); } catch {} }
    } else if (a.paused) {
      a.play?.().catch(() => {});
    }
  }

  function paintFav() {
    const btn = $('#btnFav');
    if (!btn) return;
    const on = !!state.playback.liked;
    btn.classList.toggle('is-on', on);
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
    btn.querySelector('use').setAttribute('href', on ? '#i-heart-fill' : '#i-heart');
    $('#favLabel').textContent = on ? 'Liked' : 'Like';
  }

  function paintSleep() {
    const btn = $('#btnSleep');
    if (!btn) return;
    const s = state.sleep || 0;
    btn.dataset.timer = s > 0 ? '1' : '0';
    $('#sleepLabel').textContent = s > 0 ? `Sleep ${fmt(s)}` : 'Sleep';
  }

  function paintProgress() {
    const t = currentTrack();
    if (!t) return;
    const pct = clamp((localPos / t.duration) * 100, 0, 100);
    $('#seekFill').style.width = `${pct}%`;
    $('#seekKnob').style.left = `${pct}%`;
    $('#timeNow').textContent = fmt(localPos);
    $('#miniFill').style.width = `${pct}%`;
    const nowPos = $('#qNowPos');
    if (nowPos) nowPos.textContent = fmt(localPos);
    if (videoBound) syncVideo();
    if (audioBound) syncAudio();
  }

  function renderMini() {
    const t = displayTrack();
    if (!t) return;
    $('#miniArt').src = t.art || coverURI(t.title);
    $('#miniTitle').textContent = t.title;
    $('#miniIcon').querySelector('use').setAttribute('href', state.playback.paused ? '#i-play' : '#i-pause');
    paintProgress();
  }

  function renderMembers(force = false) {
    const total = state.room.members || state.members.length;
    const live = isLive();
    const inVoice = live ? state.members.filter(m => m.vc) : [];
    const rest = state.members.filter(m => !inVoice.includes(m));
    const sig = `${total}|${live}|${inVoice.length}|` + state.members
      .map(m => `${m.id}:${m.vc ? 1 : 0}:${m.role}:${m.name}:${m.photo}:${m.username}:${(m.tags || []).map(t => t.k).join('.')}`)
      .join(',');
    if (!force && sig === lastSig.members) return;
    lastSig.members = sig;

    $('#membersLabel').textContent = `MEMBERS · ${total} · ${inVoice.length} IN VOICE`;
    const rank = { owner: 0, host: 1, admin: 2, member: 3 };
    const sort = (list) => [...list].sort((a, b) =>
      (rank[a.role] ?? 9) - (rank[b.role] ?? 9) || (b.vc ? 1 : 0) - (a.vc ? 1 : 0));

    const row = (m) => {
      const mine = String(m.id) === String(viewer?.id);
      const custom = m.custom && m.custom !== m.tags?.[0]?.label ? m.custom : '';
      return `
      <div class="member" data-id="${esc(m.id)}" data-role="${esc(m.role)}">
        <div class="av-wrap">
          <img class="avatar" src="${esc(pic(m.photo, m.name))}" data-name="${esc(m.name)}" alt="" loading="lazy" decoding="async" />
          <span class="ring"></span>
          ${m.vc ? `<span class="vc-dot"><svg class="ic"><use href="#i-mic"/></svg></span>` : ''}
        </div>
        <div class="member-body">
          <div class="member-name">
            <span class="m-name">${esc(m.name)}</span>
            ${tagHTML(m.tags)}
            ${mine ? '<span class="tag me">you</span>' : ''}
          </div>
          <div class="member-sub">
            ${m.username ? `<span>@${esc(m.username)}</span>` : '<span>no username</span>'}
            ${custom ? `<span class="dot">·</span><span class="custom">${esc(custom)}</span>` : ''}
            ${m.vc ? `<span class="dot">·</span><span class="member-flag"><svg class="ic"><use href="#i-mic"/></svg>in voice</span>` : ''}
          </div>
        </div>
        ${mine || m.bot ? '' : `<button class="member-invite" data-invite="${esc(m.id)}" aria-label="Invite">
          <svg class="ic"><use href="#i-plane"/></svg></button>`}
        ${mine || m.bot ? '' : `<button class="promote-btn" data-menu="${esc(m.id)}">•••</button>`}
      </div>`;
    };

    let html = '';
    if (inVoice.length) {
      html += `<div class="member-group"><b class="g-live">● In voice chat</b><span>${inVoice.length}</span><span class="g-line"></span></div>`;
      html += sort(inVoice).map(row).join('');
    }
    html += `<div class="member-group"><b>Group members</b><span>${rest.length}</span><span class="g-line"></span></div>`;
    html += sort(rest).map(row).join('');
    if (!rest.length) {
      html += `<div class="member-more">Everyone in the group is already in the voice chat.</div>`;
    } else if (total > state.members.length) {
      html += `<div class="member-more">${total - state.members.length} more in the group — they appear here once Telegram returns them.</div>`;
    }
    $('#memberList').innerHTML = html;
  }

  /// "Recently played" strip under the queue: the last tracks this room
  /// played, so a song can be replayed without searching for it again.
  function recentRows(cur) {
    const list = (state.recent || []).filter(r => !cur || r.id !== cur.id).slice(0, 12);
    if (!list.length) return '';
    const rows = list.map((r) => `
      <div class="recent-item" data-recent="${esc(r.id)}">
        <img class="q-thumb" src="${esc(r.art || coverURI(r.title))}" alt="" loading="lazy" decoding="async" />
        <div class="q-body">
          <div class="q-title">${esc(r.title)}</div>
          <div class="q-sub">${r.video ? `<b class="k-flag">VIDEO 720P</b> · ` : ''}${esc(r.requester || 'played earlier')} · ${fmt(r.duration)}</div>
        </div>
        <button class="q-btn" data-requeue="${esc(r.id)}" aria-label="Add to queue"><svg class="ic"><use href="#i-plus"/></svg></button>
      </div>`).join('');
    return `<div class="recent-head"><span>Recently played</span><span>${list.length}</span></div>${rows}`;
  }

  function renderQueue(force = false) {
    const q = state.playback.queue || [];
    const t = currentTrack();
    const p = pendingTrack();
    const sig = `${t ? t.id : '-'}${t && t.video ? 'v' : ''}|${p ? p.id : '-'}|`
      + q.map(x => x.id + (x.video ? 'v' : '')).join(',')
      + `|${(state.recent || []).map(r => r.id).join(',')}`;
    if (!force && sig === lastSig.queue) return;
    lastSig.queue = sig;
    $('#queueLabel').textContent = t ? `PLAYING · ${q.length} UP NEXT` : `UP NEXT · ${q.length}`;
    $('#queueEmpty').classList.toggle('is-on', !q.length && !t && !p && !(state.recent || []).length);
    const clearBtn = $('#btnClearQueue');
    if (clearBtn) clearBtn.hidden = !q.length;
    const now = (t || p) ? `
      <div class="q-now${t ? '' : ' is-loading'}">
        <img class="q-thumb" src="${esc((t || p).art || coverURI((t || p).title))}" alt="${esc((t || p).title)}" />
        <div class="q-body">
          <div class="q-title">${esc((t || p).title)}</div>
          <div class="q-sub">${t
            ? `${(t || p).video ? `<b class="k-flag">VIDEO 720P</b> · ` : ''}${esc(t.requester || '')} · <span id="qNowPos">${fmt(localPos)}</span> / ${fmt(t.duration)}`
            : `Starting · downloading ${(t || p).video ? '720p video' : 'audio'}…`}</div>
        </div>
        <div class="eq"><i></i><i></i><i></i><i></i></div>
      </div>` : '';
    $('#queueList').innerHTML = now + q.map((x, i) => `
      <div class="q-item">
        <span class="q-idx">${String(i + 1).padStart(2, '0')}</span>
        <img class="q-thumb" src="${esc(x.art || coverURI(x.title))}" alt="${esc(x.title)}" loading="lazy" decoding="async" />
        <div class="q-body" data-play="${i}">
          <div class="q-title">${esc(x.title)}</div>
          <div class="q-sub">${x.video ? `<b class="k-flag">VIDEO 720P</b> · ` : ''}${esc(x.requester || '')} · ${fmt(x.duration)} · ${esc(x.source || '')}</div>
        </div>
        <div class="q-actions">
          <button class="q-btn" data-up="${i}" ${i === 0 ? 'disabled' : ''} aria-label="Move up"><svg class="ic"><use href="#i-up"/></svg></button>
          <button class="q-btn" data-down="${i}" ${i === q.length - 1 ? 'disabled' : ''} aria-label="Move down"><svg class="ic"><use href="#i-down"/></svg></button>
          <button class="q-btn" data-remove="${i}" aria-label="Remove"><svg class="ic"><use href="#i-x"/></svg></button>
        </div>
      </div>`).join('') + recentRows(t);
  }

  function renderProfile(force = false) {
    const me = state.me;
    const known = memberById(viewer?.id) || me;
    const s = session();
    const live = isLive();
    const tracks = (live ? s.tracks : (s.ended_at ? s.tracks : 0)) || 0;
    const secs = live
      ? Math.max(0, Math.floor(Date.now() / 1000) - (s.started_at || Math.floor(Date.now() / 1000)))
      : (s.duration || 0);
    // The "time in this call" line only needs a refresh every half minute.
    const sig = `${known?.name}|${known?.username}|${me.name}|${tracks}|${live}|${Math.floor(secs / 30)}|${(known?.tags || []).map(t => t.k).join('.')}`;
    if (!force && sig === lastSig.profile) return;
    lastSig.profile = sig;

    $('#meAvatar').src = pic(known.photo || me.photo, me.name);
    $('#meName').textContent = known.name || me.name;
    $('#meHandle').textContent = known.username ? `@${known.username}` : (me.username ? `@${me.username}` : '');
    $('#meTag').innerHTML = tagHTML(known.tags, 2);
    // Real numbers: the profile is about this voice chat, not a fake streak.
    const lvl = Math.min(10, 1 + Math.floor(tracks / 5));
    const span = fmtSpan(secs);
    $('#levelNum').textContent = lvl;
    $('#levelTitle').textContent = `Level ${lvl}`;
    $('#levelSub').textContent = live
      ? `${span} in this voice chat · ${tracks} track${tracks === 1 ? '' : 's'} played`
      : (s.ended_at ? `Last session · ${span} · ${tracks} track${tracks === 1 ? '' : 's'}` : 'No session yet');
    $('#levelFill').style.width = `${Math.min(100, (tracks % 5) * 20)}%`;
    $('#statListen').textContent = span;
    $('#statSongs').textContent = tracks;
  }

  /* ────────────────────────── chat ────────────────────────── */
  const EMOJI = [
    '😀', '😃', '😄', '😁', '😆', '😅', '😂', '🤣', '🥲', '😊', '😇', '🙂', '🙃', '😉', '😌', '😍',
    '🥰', '😘', '😗', '😙', '😚', '😋', '😛', '😝', '😜', '🤪', '🤨', '🧐', '🤓', '😎', '🥸', '🤩',
    '🥳', '😏', '😒', '😞', '😔', '😟', '😕', '🙁', '😣', '😖', '😫', '😩', '🥺', '😢', '😭', '😤',
    '😠', '😡', '🤬', '🤯', '😳', '🥵', '🥶', '😱', '😨', '😰', '😥', '😓', '🤗', '🤔', '🫡', '🤭',
    '🫢', '🫣', '🤫', '🤥', '😶', '😐', '😑', '😬', '🫠', '🙄', '😯', '😦', '😧', '😮', '😲', '🥱',
    '😴', '🤤', '😪', '😵', '😵‍💫', '🤐', '🥴', '🤢', '🤮', '🤧', '😷', '🤒', '🤕', '🤑', '🤠', '😈',
    '👿', '👹', '👺', '🤡', '💩', '👻', '💀', '☠️', '👽', '🤖', '🎃', '😺', '😸', '😹', '😻', '😼',
    '❤️', '🧡', '💛', '💚', '💙', '💜', '🖤', '🤍', '🤎', '💔', '❣️', '💕', '💞', '💓', '💗', '💖',
    '💘', '💝', '💟', '✨', '⭐', '🌟', '💫', '⚡', '🔥', '💥', '💯', '🎉', '🎊', '🎈', '🎁', '🏆',
    '🎧', '🎵', '🎶', '🎤', '🎸', '🥁', '🎹', '🎺', '🎷', '🪕', '💿', '📀', '🎬', '📸', '🌙', '☀️',
  ];

  /* sticker + premium-emoji packs (public sets the bot can read) */
  const STICKER_PACKS = ['EmojiPack', 'Twemoji', 'HotCherry', 'EmojiFun'];
  const PREMIUM_PACKS = ['AnimatedEmoji', 'DuckEmoji', 'PepeEmoji'];
  const packCache = {};
  let pane = 'emoji';
  let packName = STICKER_PACKS[0];

  const JUMBO = /^(?:[\p{Extended_Pictographic}\u200d\uFE0F\s]{1,8})$/u;

  function renderText(text, entities) {
    if (!text) return '';
    const list = (entities || []).filter(e => e && e.type && e.length > 0);
    if (!list.length) return esc(text);

    const opens = new Map();
    const closes = new Map();
    const customs = [];
    for (const e of list) {
      const s = e.offset, en = e.offset + e.length;
      if (s < 0 || en > text.length || s >= en) continue;
      if (e.type === 'custom_emoji') { customs.push(e); continue; }
      const pair = tagPair(e, text);
      if (!pair) continue;
      if (!opens.has(s)) opens.set(s, []);
      opens.get(s).push(pair);
      if (!closes.has(en)) closes.set(en, []);
      closes.get(en).push(pair);
    }

    let out = '';
    let i = 0;
    while (i < text.length) {
      if (closes.has(i)) for (const [, c] of closes.get(i).reverse()) out += c;
      if (opens.has(i)) for (const [o] of opens.get(i)) out += o;
      const ce = customs.find(e => e.offset === i);
      if (ce) {
        const ch = text.slice(ce.offset, ce.offset + ce.length);
        // Mirrored messages carry the document id in `extra`; resolve it
        // through the proxy so premium emoji look the same for everyone.
        const src = ce.src || (ce.extra ? `/api/emoji/${ce.extra}` : '');
        if (src) out += `<img class="emoji" src="${esc(src)}" alt="${esc(ch)}" data-ch="${esc(ch)}" />`;
        else out += esc(ch);
        i += ce.length;
        continue;
      }
      out += esc(text[i]);
      i += 1;
    }
    if (closes.has(text.length)) for (const [, c] of closes.get(text.length).reverse()) out += c;
    return out;
  }

  function tagPair(e, text) {
    const s = text.slice(e.offset, e.offset + e.length);
    switch (e.type) {
      case 'bold': return ['<b>', '</b>'];
      case 'italic': return ['<i>', '</i>'];
      case 'underline': return ['<u>', '</u>'];
      case 'strikethrough': return ['<s>', '</s>'];
      case 'code': return ['<code>', '</code>'];
      case 'pre': return ['<pre>', '</pre>'];
      case 'spoiler': return ['<span class="spoiler">', '</span>'];
      case 'blockquote': return ['<span style="border-left:2px solid rgba(255,255,255,.25);padding-left:6px;display:inline-block">', '</span>'];
      case 'text_link': return [`<a href="${esc(e.extra || '#')}" target="_blank" rel="noreferrer">`, '</a>'];
      case 'url': return [`<a href="${esc(s)}" target="_blank" rel="noreferrer">`, '</a>'];
      case 'mention': return ['<span style="color:#6fb6ff">', '</span>'];
      case 'hashtag': return ['<span style="color:#6fb6ff">', '</span>'];
      default: return null;
    }
  }

  const vcIds = () => {
    const explicit = state.room && state.room.vc_ids;
    if (Array.isArray(explicit) && explicit.length) return new Set(explicit.map(String));
    return new Set(state.members.filter(m => m.vc).map(m => String(m.id)));
  };

  /// The room chat only shows people who are in the voice chat (plus you and
  /// bot notices). "Everyone" brings back the whole mirrored group chat.
  /// When Telegram has not told us who is in the call yet (`vc_known` false)
  /// filtering would hide everybody, so nothing is filtered.
  function visibleMessages() {
    const all = state.messages || [];
    if (chatScope !== 'vc' || !isLive() || !viewingLiveRoom() || state.room.vc_known === false) return all;
    const ids = vcIds();
    if (!ids.size) return all;
    return all.filter(m => m.mine || !m.from || m.from === 'bot'
      || String(m.from) === String(viewer?.id) || ids.has(String(m.from)));
  }

  /// When "In voice" hides messages, say so instead of looking like the chat
  /// lost them — one tap brings the whole group chat back.
  function hiddenBanner() {
    if (chatScope !== 'vc' || !isLive() || !viewingLiveRoom()) return '';
    const hidden = (state.messages || []).length - visibleMessages().length;
    if (hidden <= 0) return '';
    return `<div class="scope-hint" data-scope-all="1">
      <svg class="ic"><use href="#i-users"/></svg>
      <span>${hidden} message${hidden === 1 ? '' : 's'} from members outside the call</span>
      <b>Show all</b>
    </div>`;
  }

  function stickerHTML(m) {
    // The little + is the "Add to Stickers" affordance. It is pinned to the
    // bubble (hence position:relative here) and its click is handled before
    // the generic tap-to-reply gesture in bind().
    const save = `<button class="sticker-save" data-sticker-save="${esc(m.id)}" aria-label="Add to Stickers"><svg class="ic"><use href="#i-plus"/></svg></button>`;
    const wrap = (inner) => `<div class="bubble sticker" style="position:relative">${save}${inner}</div>`;
    if (!m.media) {
      return wrap(`<span style="font-size:58px;line-height:1">${esc(m.emoji || '🎵')}</span>`);
    }
    const kind = m.media_kind || 'image';
    if (kind === 'video') {
      return wrap(`<video class="sticker-vid" src="${esc(m.media)}"
        autoplay loop muted playsinline webkit-playsinline></video>`);
    }
    if (kind === 'tgs') {
      return wrap(`<div class="tgs-box" data-tgs="${esc(m.media)}"
        data-emoji="${esc(m.emoji || '🎵')}"></div>`);
    }
    return wrap(`<img src="${esc(m.media)}" alt="${esc(m.emoji || '')}"
      data-emoji="${esc(m.emoji || '')}" draggable="false" />`);
  }

  /// Telegram's animated stickers are gzipped Lottie JSON (`.tgs`). Unpack in
  /// the browser and hand the data to lottie-web; if anything is unavailable,
  /// fall back to the sticker's emoji so the bubble is never blank.
  ///
  /// lottie-web is 165 KB and only animated stickers need it, so the library
  /// is fetched on first use instead of on every app start.
  let lottieLoad = null;
  function loadLottie() {
    if (window.lottie) return Promise.resolve(window.lottie);
    if (!lottieLoad) {
      lottieLoad = new Promise((resolve, reject) => {
        const s = document.createElement('script');
        s.src = 'assets/vendor/lottie_light.min.js';
        s.onload = () => (window.lottie ? resolve(window.lottie) : reject(new Error('lottie missing')));
        s.onerror = () => reject(new Error('lottie failed'));
        document.head.appendChild(s);
      }).catch(() => null);
    }
    return lottieLoad;
  }

  async function hydrateTgs(root) {
    for (const el of root.querySelectorAll('[data-tgs]')) {
      if (el.dataset.done) continue;
      el.dataset.done = '1';
      const url = el.dataset.tgs;
      try {
        let anim = tgsCache.get(url);
        if (!anim) {
          const buf = await (await fetch(url)).arrayBuffer();
          const stream = new Response(buf).body.pipeThrough(new DecompressionStream('gzip'));
          anim = await new Response(stream).json();
          tgsCache.set(url, anim);
        }
        const lottie = await loadLottie();
        if (!lottie) throw new Error('lottie missing');
        lottie.loadAnimation({
          container: el, renderer: 'svg', loop: true, autoplay: true, animationData: anim,
        });
      } catch {
        el.innerHTML = `<span style="font-size:58px;line-height:1;display:grid;place-items:center;height:100%">${esc(el.dataset.emoji || '🎵')}</span>`;
      }
    }
  }

  function renderChat(force = false) {
    const box = $('#msgs');
    const list = visibleMessages();
    const sig = `${state.chat.id}|${list.length}|${sigOf(list)}|${chatScope}|${chatLive() ? 1 : 0}|${vcIds().size}`;
    if (!force && sig === lastSig.chat) return;
    lastSig.chat = sig;
    const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 90;
    const rows = list.map(m => {
      const mine = m.mine || String(m.from) === String(viewer?.id);
      const tags = m.tags || [];
      const reply = m.reply
        ? `<div class="reply"><b>${esc(m.reply.name || '')}</b><span>${esc(m.reply.text || '')}</span></div>`
        : '';
      let inner;
      if (m.kind === 'voice') {
        inner = `<div class="bubble voice">
            <span class="voice-play"><svg class="ic"><use href="#i-play"/></svg></span>
            <span class="wave">${waveBars(m.id)}</span>
            <span class="voice-dur">0:0${m.duration || 5}</span>
          </div>`;
      } else if (m.kind === 'photo') {
        inner = m.media
          ? `<div class="bubble media"><img src="${esc(m.media)}" alt="" draggable="false" /></div>`
          : `<div class="bubble media"><img src="${coverURI(m.name || 'photo')}" alt="" draggable="false" /></div>`;
      } else if (m.kind === 'sticker') {
        inner = stickerHTML(m);
      } else {
        const body = renderText(m.text || '', m.entities);
        const jumbo = (m.text || '').length <= 12 && JUMBO.test(m.text || '');
        inner = `<div class="bubble${jumbo ? ' jumbo' : ''}">${reply}${body}</div>`;
      }
      const reacted = m.reacted ? `<div class="reacts"><span class="react-chip">${esc(m.reacted)} 1</span></div>` : '';
      const head = mine
        ? ''
        : `<div class="msg-head"><b>${esc(m.name || 'member')}</b>${tagHTML(tags, 2)}<span class="stamp">${esc(m.ts || '')}</span></div>`;
      return `<div class="msg ${mine ? 'out' : 'in'}" data-id="${esc(m.id)}" data-mid="${m.mid || 0}" data-from="${esc(m.from || '')}">
          <span class="swipe-hint"><svg class="ic"><use href="#i-reply"/></svg></span>
          ${mine ? '' : `<img class="msg-av" src="${esc(pic(m.photo, m.name))}" data-name="${esc(m.name || '?')}" alt="" loading="lazy" decoding="async" draggable="false" />`}
          <div class="msg-col">
            ${head}
            ${inner}
            ${mine ? `<span class="stamp" style="font-size:7.5px;color:rgba(255,255,255,.26);padding:0 4px">${esc(m.ts || '')}</span>` : ''}
            ${reacted}
          </div>
        </div>`;
    }).join('');

    if (!state.messages.length) {
      const sm = session();
      const ended = !isLive() && viewingLiveRoom() && (sm.ended_at || 0) > 0;
      const others = (state.chats || []).filter(c => String(c.id) !== String(state.chat.id)).length;
      const otherLine = others
        ? `<span style="display:block;margin-top:6px">${others} other group chat${others === 1 ? '' : 's'} · tap the group name above to switch</span>`
        : '';
      box.innerHTML = ended
        ? `<div class="chat-empty">
             <div class="chat-empty-ic"><svg class="ic"><use href="#i-mic-off"/></svg></div>
             <b>Voice chat ended</b>
             <span>${esc(sm.ended_by ? `Ended by ${sm.ended_by}` : 'Everyone left the call')}${sm.ended_hm ? ` · ${esc(sm.ended_hm)}` : ''}<br/>Chat cleared${sm.tracks ? ` · ${sm.tracks} track${sm.tracks > 1 ? 's' : ''} played` : ''}</span>
             ${otherLine}
           </div>`
        : `<div class="chat-empty">
             <div class="chat-empty-ic"><svg class="ic"><use href="#i-chat"/></svg></div>
             <b>No messages yet</b>
             <span>${viewingLiveRoom() ? 'Say something in the group and it shows up here.' : `“${esc(state.chat.title || 'This group')}” has no messages yet.`}</span>
             ${otherLine}
           </div>`;
      renderChat._first = true;
      return;
    }

    if (!list.length) {
      // There are group messages, but nobody in the call has spoken yet.
      box.innerHTML = `<div class="sys"><svg class="ic"><use href="#i-lock"/></svg> Room chat · members in voice only</div>
        ${hiddenBanner()}
        <div class="scope-note">
          <b>No messages from the voice chat yet</b>
          Only the ${vcIds().size} member${vcIds().size === 1 ? '' : 's'} in the call can post here.
          Tap <b style="display:inline;color:#9ccbff">Everyone</b> above to see the whole group chat.
        </div>`;
      renderChat._first = true;
      return;
    }

    box.innerHTML = `<div class="sys"><svg class="ic"><use href="#i-lock"/></svg> Messages are synced with the Telegram group</div>`
      + hiddenBanner()
      + rows;
    if (nearBottom || renderChat._first === undefined) box.scrollTop = box.scrollHeight;
    wireImgFallbacks();
    hydrateTgs(box);
    renderChat._first = true;
  }

  function waveBars(id) {
    let seed = 0;
    for (const ch of String(id)) seed = (seed * 31 + ch.charCodeAt(0)) % 9973;
    return Array.from({ length: 26 }, (_, i) => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      const h = 4 + Math.round((seed / 2147483648) * 16);
      return `<i style="height:${h}px" class="${i < 9 ? 'on' : ''}"></i>`;
    }).join('');
  }

  function paintUnread() {
    const dot = $('#chatDot');
    // The header badge counts unread messages in the *other* group chats; the
    // tab dot has to agree with it instead of counting only local SSE frames.
    const other = (state.chats || []).reduce((n, x) =>
      n + (String(x.id) === String(state.chat?.id) ? 0 : Math.max(0, +x.unread || 0)), 0);
    dot.classList.toggle('is-on', (unread > 0 || other > 0) && activeTab !== 'chat');
  }

  /// Tell the server this viewer has seen the chat. Throttled: it fires on
  /// every incoming message while the chat tab is open.
  let readAt = 0;
  function markRead() {
    if (!online || !state.chat?.id) return;
    const now = Date.now();
    if (now - readAt < 1500) return;
    readAt = now;
    api('/api/chat/read', {}).catch(() => {});
  }

  /* ─────────────────── group chats (one per group) ─────────────────── */

  /// Telegram's own confirmation sheet when it is there, a plain one otherwise.
  function askConfirm(title, message, label = 'Delete') {
    return new Promise((resolve) => {
      let done = false;
      const finish = (v) => { if (!done) { done = true; resolve(v); } };
      try {
        if (tg?.showPopup) {
          tg.showPopup({
            title,
            message,
            buttons: [
              { id: 'cancel', type: 'cancel' },
              { id: 'ok', type: 'destructive', text: label },
            ],
          }, (id) => finish(id === 'ok'));
          return;
        }
      } catch {}
      finish(window.confirm(`${title}\n\n${message}`));
    });
  }

  function renderChatRows() {
    const box = $('#chatRows');
    if (!box) return;
    const list = state.chats || [];
    if (!list.length) {
      box.innerHTML = '<div class="prof-none">No chats yet. As soon as the bot is in a group, its chat shows up here.</div>';
      return;
    }
    box.innerHTML = list.map((c) => {
      const on = String(c.id) === String(state.chat.id);
      const unread = Math.max(0, +c.unread || 0);
      const sub = c.last_text
        ? `${c.last_ts ? `${c.last_ts} · ` : ''}${c.last_text}`
        : (c.live ? 'Voice chat live · no messages yet' : 'No messages yet');
      return `<div class="chat-row ${on ? 'is-on' : ''}" data-chat="${esc(c.id)}" role="button" tabindex="0">
          <span class="cr-ic"><svg class="ic"><use href="#i-users"/></svg></span>
          <span class="cr-body"><b>${esc(c.title || `Chat ${c.id}`)}</b><span>${esc(sub)}</span></span>
          ${c.live ? '<span class="cr-live"></span>' : ''}
          ${unread ? `<span class="cr-unread">${unread > 99 ? '99+' : unread}</span>` : ''}
          ${c.can_delete ? `<button class="cr-del" data-del-chat="${esc(c.id)}" aria-label="Delete chat">
            <svg class="ic"><use href="#i-trash"/></svg></button>` : ''}
        </div>`;
    }).join('');
  }

  async function openChats() {
    haptic('light');
    renderChatRows();
    openSocial('#chatsSheet');
    // Permissions ("can I delete this chat?") are only resolved on open, not on
    // every state tick — that would mean a bridge call per chat per second.
    await refreshChats(true);
  }

  async function refreshChats(silent = false) {
    if (!online) return;
    try {
      const r = await api('/api/chats');
      if (Array.isArray(r.chats)) state.chats = r.chats;
      renderChatRows();
      renderChatHeader();
      if (!silent) toast(`${state.chats.length} chat${state.chats.length === 1 ? '' : 's'}`);
    } catch (e) {
      if (!silent) toast(`Could not refresh: ${e.message}`);
    }
  }

  async function switchChat(id) {
    id = String(id || '');
    if (!id) return;
    if (id === String(state.chat.id)) { closeSocial(); return; }
    try {
      await api('/api/chat/select', { id });
      // Pull the new chat's own timeline; the SSE frame for it would arrive on
      // the next tick anyway, but this makes the switch instant.
      const s = await api('/api/state');
      merge(s);
      chatSig = sigOf(state.messages || []);
      lastSig.chat = '';
      closeSocial();
      renderAll(true);
      scrollChat();
      toast(`Switched to ${state.chat.title || 'chat'}`);
    } catch (e) {
      toast(`Could not switch: ${e.message}`);
    }
  }

  async function deleteChat(id) {
    id = String(id || '');
    if (!id) return;
    const row = (state.chats || []).find(c => String(c.id) === id) || {};
    const title = row.title || `Chat ${id}`;
    const home = !!row.active;
    const ok = await askConfirm(
      home ? 'Clear this chat' : 'Delete chat',
      home
        ? `Delete every mirrored message of “${title}” in the app? The Telegram group itself is not touched.`
        : `Remove “${title}” from the app? Its mirrored messages are deleted; the Telegram group is not touched.`,
      home ? 'Clear' : 'Delete',
    );
    if (!ok) return;
    try {
      const r = await api('/api/chat/forget', { id });
      if (Array.isArray(r.chats)) state.chats = r.chats;
      if (String(state.chat.id) === id) {
        // The chat being viewed was cleared or removed: reload the view.
        const s = await api('/api/state');
        merge(s);
        chatSig = sigOf(state.messages || []);
        lastSig.chat = '';
        renderAll(true);
      } else {
        renderChatRows();
        renderChatHeader();
      }
      toast(r.cleared ? 'Chat cleared' : 'Chat deleted');
    } catch (e) {
      toast(`Delete failed: ${e.message}`);
    }
  }

  async function clearChatNow() {
    const id = String(state.chat.id || '');
    if (!id) return;
    const ok = await askConfirm(
      'Clear this chat',
      `Delete every mirrored message of “${state.chat.title || 'this chat'}” in the app? The Telegram group itself is not touched.`,
      'Clear',
    );
    if (!ok) return;
    try {
      await api('/api/chat/clear', {});
      state.messages = [];
      chatSig = '0';
      lastSig.chat = '';
      renderChat(true);
      renderChatRows();
      toast('Chat cleared');
    } catch (e) {
      toast(`Could not clear: ${e.message}`);
    }
  }

  /* ────────────────────────── tabs ────────────────────────── */
  function switchTab(tab) {
    closeSocial();
    activeTab = tab;
    $('#app').dataset.tab = tab;
    $('#screen').dataset.tab = tab;
    $$('.tab').forEach(b => b.classList.toggle('is-active', b.dataset.tab === tab));
    $$('.view').forEach(v => v.classList.toggle('is-active', v.dataset.view === tab));
    if (tab !== 'chat') $('#stickerPanel').classList.remove('is-on');
    if (tab === 'chat') {
      unread = 0;
      paintUnread();
      markRead();
      const box = $('#msgs');
      requestAnimationFrame(() => { box.scrollTop = box.scrollHeight; });
    }
    haptic('light');
  }

  /* ────────────────────────── composer ────────────────────────── */
  const editor = () => $('#chatInput');

  function composerText() {
    return (editor().innerText || '').replace(/\u200B/g, '').trim();
  }

  function readComposer() {
    let text = '';
    let off = 0;
    const entities = [];
    const walk = (node) => {
      node.childNodes.forEach((n) => {
        if (n.nodeType === 3) {
          const v = n.nodeValue.replace(/\u200B/g, '');
          text += v;
          off += v.length;
        } else if (n.nodeType === 1) {
          if (n.classList.contains('pe')) {
            const ch = n.dataset.emoji || '😀';
            text += ch;
            entities.push({
              type: 'custom_emoji', offset: off, length: ch.length,
              custom_emoji_id: n.dataset.id, src: n.dataset.src,
            });
            off += ch.length;
          } else if (n.tagName === 'BR') {
            text += '\n'; off += 1;
          } else if (n.tagName === 'DIV' || n.tagName === 'P') {
            if (text && !text.endsWith('\n')) { text += '\n'; off += 1; }
            walk(n);
          } else {
            walk(n);
          }
        }
      });
    };
    walk(editor());
    return { text: text.replace(/\n$/, ''), entities };
  }

  function placeCaretEnd() {
    const r = document.createRange();
    r.selectNodeContents(editor());
    r.collapse(false);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(r);
  }

  function insertNode(node) {
    const el = editor();
    el.focus();
    const sel = window.getSelection();
    if (!sel || !sel.rangeCount || !el.contains(sel.anchorNode)) {
      el.appendChild(node);
      placeCaretEnd();
    } else {
      const r = sel.getRangeAt(0);
      r.deleteContents();
      r.insertNode(node);
      r.setStartAfter(node);
      r.collapse(true);
      sel.removeAllRanges();
      sel.addRange(r);
    }
    updateSendState();
  }

  function insertEmoji(ch) {
    insertNode(document.createTextNode(ch));
  }

  function insertPremium(s) {
    const span = document.createElement('span');
    span.className = 'pe';
    span.contentEditable = 'false';
    span.dataset.id = s.custom_emoji_id || '';
    span.dataset.emoji = s.emoji || '😀';
    span.dataset.src = s.thumb || '';
    span.style.backgroundImage = `url("${s.thumb}")`;
    span.textContent = s.emoji || '';
    insertNode(span);
    insertNode(document.createTextNode('\u200B'));
  }

  function updateSendState() {
    const has = composerText().length > 0 || !!editor().querySelector('.pe');
    $('#btnSend').classList.toggle('is-send', has);
    $('#sendIcon').querySelector('use').setAttribute('href', has ? '#i-send' : '#i-mic');
  }

  function clearComposer() {
    editor().innerHTML = '';
    updateSendState();
  }

  async function sendText() {
    if (!chatLive()) { toast('Voice chat ended — chat is cleared'); return; }
    const { text, entities } = readComposer();
    if (!text.trim()) return;
    const reply = replyTarget;
    clearComposer();
    cancelReply();
    const local = {
      id: `l${Date.now()}`,
      mid: 0,
      from: viewer.id,
      name: viewer.name,
      photo: viewer.photo,
      tags: myTags(),
      mine: true,
      kind: 'text',
      text,
      entities,
      ts: nowStamp(),
      reply: reply ? { name: reply.name, text: reply.text } : null,
    };
    state.messages.push(local);
    chatSig = sigOf(state.messages);
    renderChat();
    scrollChat();
    haptic('light');
    try {
      const res = await api('/api/chat/send', {
        kind: 'text', text, entities,
        replyTo: reply?.mid || 0,
        senderId: viewer.id, senderName: viewer.name, tags: myTags(),
      });
      if (res.message) {
        const i = state.messages.findIndex(m => m.id === local.id);
        if (i >= 0) state.messages[i] = { ...res.message, mine: true };
        chatSig = sigOf(state.messages);
        renderChat();
      }
    } catch (e) {
      const i = state.messages.findIndex(m => m.id === local.id);
      if (i >= 0) state.messages.splice(i, 1);
      renderChat();
      editor().textContent = text;
      updateSendState();
      toast(`Send failed: ${e.message}`);
    }
  }

  async function sendSticker(s, thumb) {
    if (!chatLive()) { toast('Voice chat ended — chat is cleared'); return; }
    // Use the sticker document itself, not its static thumbnail: the Bot API
    // proxy answers with WebP / WebM / gzip `.tgs`, and media_kind tells the
    // bubble which renderer to use.
    // A sticker out of a public pack carries file_id; one mirrored from the
    // group only has cached bytes under a mediaKey. Saved stickers keep both
    // fields so they can be resent from the Saved tab unchanged.
    const kind = s.media_kind || (s.video ? 'video' : s.animated ? 'tgs' : 'image');
    const media = s.file_id
      ? `/api/tgimg?fid=${encodeURIComponent(s.file_id)}`
      : (thumb || s.thumb || s.media);
    const local = {
      id: `l${Date.now()}`, mid: 0, from: viewer.id, name: viewer.name, photo: viewer.photo,
      tags: myTags(), mine: true, kind: 'sticker', text: '', emoji: s.emoji,
      media, media_kind: kind, ts: nowStamp(),
    };
    state.messages.push(local);
    chatSig = sigOf(state.messages);
    renderChat();
    scrollChat();
    haptic('medium');
    $('#stickerPanel').classList.remove('is-on');
    try {
      const res = await api('/api/chat/send', {
        kind: 'sticker', sticker: s.file_id || '', thumb: thumb || s.thumb || s.media,
        emoji: s.emoji, media, media_kind: kind, mediaKey: s.mediaKey || '',
        senderId: viewer.id, senderName: viewer.name, tags: myTags(),
      });
      if (res.message) {
        const i = state.messages.findIndex(m => m.id === local.id);
        if (i >= 0) state.messages[i] = { ...res.message, media, media_kind: kind, mine: true };
        chatSig = sigOf(state.messages);
        renderChat();
      }
    } catch (e) {
      toast(`Sticker failed: ${e.message}`);
    }
  }

  async function react(m, emoji = '❤️') {
    if (!m || !m.mid) return;
    m.reacted = emoji;
    renderChat();
    haptic('medium');
    try { await api('/api/chat/react', { mid: m.mid, emoji }); } catch {}
  }

  function scrollChat() {
    const box = $('#msgs');
    requestAnimationFrame(() => { box.scrollTop = box.scrollHeight; });
  }

  function nowStamp() {
    const d = new Date();
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  }

  /* ────────────────────────── panels ────────────────────────── */
  function openPanel(which) {
    const panel = $('#stickerPanel');
    const open = panel.classList.contains('is-on');
    if (!open) {
      panel.classList.add('is-on');
      $('#btnSticker').classList.add('is-on');
      editor().blur();
      renderPane();
      scrollChat();
    } else if (which && pane !== which) {
      pane = which;
      renderPane();
    } else {
      panel.classList.remove('is-on');
      $('#btnSticker').classList.remove('is-on');
    }
  }

  async function renderPane() {
    $$('.sticker-tab').forEach(t => t.classList.toggle('is-on', t.dataset.pane === pane));
    const packRow = $('#stickerPacks');
    const grid = $('#stickerGrid');

    // "Saved" is the viewer's own "Add to Stickers" list. It never touches
    // Telegram, so it opens instantly and also works when the bridge is busy.
    if (pane === 'saved') {
      packRow.hidden = true;
      grid.classList.add('packs');
      grid.dataset.set = '';
      const list = social.savedStickers;
      if (!list.length) {
        grid.innerHTML = `<div class="saved-empty">
            <svg class="ic"><use href="#i-bookmark"/></svg>
            <b>No saved stickers yet</b>
            <span>Tap the small + on any sticker in the chat to keep it here.</span>
          </div>`;
        return;
      }
      grid.innerHTML = list.map((s, i) => {
        const kind = s.media_kind || 'image';
        const inner = kind === 'video'
          ? `<video src="${esc(s.media || '')}" autoplay loop muted playsinline webkit-playsinline></video>`
          : (kind === 'tgs' || !s.media)
            ? `<span class="sd-emoji">${esc(s.emoji || '🎵')}</span>`
            : `<img src="${esc(s.media)}" loading="lazy" alt="${esc(s.emoji || '')}" />`;
        return `<button class="sticker-cell" data-saved="${i}" title="${esc(s.emoji || '')}">${inner}</button>`;
      }).join('');
      return;
    }

    const packs = pane === 'premium' ? PREMIUM_PACKS : pane === 'sticker' ? STICKER_PACKS : null;

    if (!packs) {
      packRow.hidden = true;
      grid.classList.remove('packs');
      grid.innerHTML = EMOJI.map(e => `<button class="sticker-cell emoji-cell" data-emoji="${esc(e)}">${e}</button>`).join('');
      return;
    }

    packRow.hidden = false;
    if (!packs.includes(packName)) packName = packs[0];
    packRow.innerHTML = packs.map(p =>
      `<button class="pack-chip ${p === packName ? 'is-on' : ''}" data-pack="${esc(p)}">${esc(p)}</button>`).join('');
    grid.classList.add('packs');
    grid.innerHTML = '<div class="sticker-cell loading"></div>'.repeat(10);

    let set;
    try {
      set = packCache[packName] || (packCache[packName] = await api(`/api/stickers?set=${encodeURIComponent(packName)}`));
    } catch {
      grid.innerHTML = `<div class="no-results" style="grid-column:1/-1">Couldn't load ${esc(packName)}</div>`;
      return;
    }
    if (!set.stickers?.length) {
      grid.innerHTML = '<div class="no-results" style="grid-column:1/-1">Empty pack</div>';
      return;
    }
    grid.innerHTML = set.stickers.map((s, i) => pane === 'premium'
      ? `<button class="sticker-cell emoji-cell" data-prem="${i}" title="${esc(s.emoji)}"><img src="${esc(s.thumb)}" loading="lazy" alt="${esc(s.emoji)}" /></button>`
      : `<button class="sticker-cell" data-sticker="${i}" title="${esc(s.emoji)}"><img src="${esc(s.thumb)}" loading="lazy" alt="${esc(s.emoji)}" /></button>`
    ).join('');
    grid.dataset.set = packName;
  }

  /* ────────────────────────── search ────────────────────────── */
  const TRENDING = ['khat', 'arijit singh', 'lofi beats', 'navjot ahuja', 'punjabi hits', '@stalker'];

  function openSearch() {
    $('#searchOverlay').classList.add('is-open');
    renderTrending();
    setTimeout(() => $('#searchInput').focus(), 240);
    tg?.BackButton?.show?.();
    tg?.BackButton?.onClick?.(closeSearch);
  }
  function closeSearch() {
    $('#searchOverlay').classList.remove('is-open');
    $('#searchInput').blur();
    tg?.BackButton?.hide?.();
  }

  function renderTrending() {
    $('#trendChips').innerHTML = TRENDING.map(t => `<button class="chip" data-q="${esc(t)}">${esc(t)}</button>`).join('');
    renderHist();
  }

  /* ── recent searches (device-local) ── */
  const HIST_KEY = 'pm-searches';
  function getHist() {
    try { return JSON.parse(localStorage.getItem(HIST_KEY) || '[]').slice(0, 8); } catch { return []; }
  }
  function pushHist(q) {
    q = (q || '').trim();
    if (q.length < 2) return;
    const list = [q, ...getHist().filter(x => x.toLowerCase() !== q.toLowerCase())].slice(0, 8);
    try { localStorage.setItem(HIST_KEY, JSON.stringify(list)); } catch {}
  }
  function renderHist() {
    const list = getHist();
    const block = $('#histBlock');
    if (!block) return;
    block.hidden = !list.length;
    $('#histChips').innerHTML = list.map(t => `<button class="chip" data-q="${esc(t)}">${esc(t)}</button>`).join('');
  }

  let searchTimer;
  function onSearchInput(e) {
    const q = e.target.value.trim();
    $('#searchClear').classList.toggle('is-on', !!q);
    clearTimeout(searchTimer);
    if (!q) { $('#results').innerHTML = ''; $('#searchBody').querySelector('.trend').style.display = ''; return; }
    $('#searchBody').querySelector('.trend').style.display = 'none';
    $('#results').innerHTML = '<div class="skeleton"></div><div class="skeleton"></div><div class="skeleton"></div>';
    searchTimer = setTimeout(() => doSearch(q), 220);
  }

  async function doSearch(q) {
    let items = [];
    try {
      const res = await api(`/api/search?q=${encodeURIComponent(q)}`);
      items = res.results || [];
    } catch {
      items = demoSearch(q);
    }
    if ($('#searchInput').value.trim() !== q) return;
    if (items.length) pushHist(q);
    renderHist();
    if (!items.length) { $('#results').innerHTML = `<div class="no-results">Nothing found for “${esc(q)}”</div>`; return; }
    $('#results').innerHTML = items.map((t, i) => `
      <div class="result" data-id="${esc(t.id)}" data-idx="${i}" style="animation-delay:${i * 28}ms">
        <img src="${esc(t.art || coverURI(t.title))}" alt="${esc(t.title)}" loading="lazy" decoding="async" />
        <div class="result-body">
          <div class="result-title">${esc(t.title)}</div>
          <div class="result-sub">${esc(t.source || 'YouTube')} · ${fmt(t.duration)}</div>
        </div>
        <div class="result-actions">
          <button class="ghost-circle" data-like="${esc(t.id)}" aria-label="Like"><svg class="ic"><use href="#i-heart"/></svg></button>
          <button class="ghost-circle" data-add="${esc(t.id)}" aria-label="Add"><svg class="ic"><use href="#i-plus"/></svg></button>
        </div>
      </div>`).join('');
    $('#results').dataset.items = JSON.stringify(items);
  }

  function demoSearch(q) {
    const pool = [
      { id: 't1', title: 'Navjot Ahuja - Khat (Official Audio)', duration: 297, art: IMG[0], source: 'YouTube' },
      { id: 't2', title: 'Navjot Ahuja - Khat (Lyrics) | Tere Liye', duration: 297, art: IMG[1], source: 'YouTube' },
      { id: 't3', title: 'Navjot Ahuja - Khat (Lyrics)', duration: 297, art: IMG[2], source: 'YouTube' },
      { id: 't5', title: 'Khat - Slowed + Reverb', duration: 322, art: IMG[3], source: 'YouTube' },
      { id: 't6', title: 'Khat (Live at Mumbai)', duration: 341, art: IMG[4], source: 'YouTube' },
      { id: 't7', title: 'Tere Liye - Acoustic', duration: 254, art: IMG[5], source: 'Spotify' },
    ];
    const k = q.toLowerCase();
    return pool.filter(t => t.title.toLowerCase().includes(k) || k.includes('khat'));
  }

  /* ────────────────────────── player ────────────────────────── */
  /// One path for "start this track": search results, queue rows and the
  /// recently-played strip all go through here, so the optimistic paint and
  /// the error toast stay identical everywhere.
  /// `kind` is 'audio' (192 kbps MP3, sound only) or 'video' (real 720p mp4,
  /// picture in the voice chat and on screen).
  async function playTrack(id, kind = 'audio') {
    if (!id) return;
    const video = kind === 'video';
    const known = (state.playback.queue || []).find(t => t.id === id)
      || (state.recent || []).find(t => t.id === id);
    if (known) {
      state.playback.pending = { ...known, video, kind: video ? 'video' : 'audio' };
      renderSong(true);
      renderQueue(true);
    }
    haptic('medium');
    try {
      const s = await api('/api/play', { id, kind: video ? 'video' : 'audio' });
      if (s) merge(s);
    } catch (err) {
      state.playback.pending = null;
      renderSong(true);
      toast(`Could not play: ${err.message}`);
      return;
    }
    renderAll(true);
    switchTab('song');
  }

  /* ── audio / video picker ── */
  let pickTrack = null;
  function openPick(t) {
    if (!t) return;
    pickTrack = t;
    $('#pickArt').src = t.art || coverURI(t.title);
    $('#pickArt').alt = t.title || '';
    $('#pickTitle').textContent = t.title || 'Unknown track';
    $('#pickSub').textContent = `${t.source || 'YouTube'} · ${fmt(t.duration)}`;
    $('#pickSheet').classList.add('is-open');
    $('#scrim').classList.add('is-on');
    haptic('light');
  }
  function closePick() {
    $('#pickSheet').classList.remove('is-open');
    $('#scrim').classList.remove('is-on');
  }

  async function togglePlay() {
    if (!currentTrack()) return;
    state.playback.paused = !state.playback.paused;
    haptic('medium');
    renderSong(); renderMini();
    try { await api('/api/toggle', {}); } catch {}
  }

  async function skip() {
    if (!currentTrack()) return;
    haptic('medium');
    try {
      const s = await api('/api/skip', {});
      if (s) merge(s);
    } catch {
      const q = state.playback.queue;
      if (q.length) {
        state.playback.track = q.shift();
        localPos = 0;
      }
    }
    renderAll();
  }

  async function seekTo(pos) {
    localPos = clamp(pos, 0, currentTrack()?.duration || 0);
    paintProgress();
    try { await api('/api/seek', { position: Math.round(localPos) }); } catch {}
  }

  function bindSeek() {
    const el = $('#seek');
    const ratio = (e) => {
      const r = el.getBoundingClientRect();
      return clamp((e.clientX - r.left) / r.width, 0, 1);
    };
    let wasPaused = false;
    el.addEventListener('pointerdown', (e) => {
      if (!currentTrack()) return;
      dragging = true; wasPaused = state.playback.paused;
      el.classList.add('is-drag');
      el.setPointerCapture(e.pointerId);
      localPos = ratio(e) * currentTrack().duration;
      paintProgress();
    });
    el.addEventListener('pointermove', (e) => {
      if (!dragging) return;
      localPos = ratio(e) * currentTrack().duration;
      paintProgress();
    });
    const end = (e) => {
      if (!dragging) return;
      dragging = false;
      el.classList.remove('is-drag');
      seekTo(ratio(e) * currentTrack().duration);
      if (wasPaused) state.playback.paused = true;
    };
    el.addEventListener('pointerup', end);
    el.addEventListener('pointercancel', end);
  }

  /* ────────────────────────── reply ────────────────────────── */
  function setReply(m) {
    if (!m) return;
    const snippet = m.kind === 'sticker' ? 'Sticker'
      : m.kind === 'photo' ? 'Photo'
      : m.kind === 'voice' ? 'Voice message'
      : (m.text || '').slice(0, 90);
    replyTarget = { mid: m.mid || 0, name: m.name, text: snippet };
    $('#replyTo').textContent = m.name;
    $('#replyText').textContent = snippet;
    $('#replyBar').classList.add('is-on');
    editor().focus();
  }
  function cancelReply() {
    replyTarget = null;
    $('#replyBar').classList.remove('is-on');
  }

  /* ────────────────────────── member menu ────────────────────────── */
  let ctxMember = null;
  function openCtx(id, anchor) {
    const m = memberById(id);
    if (!m) return;
    ctxMember = m;
    $('#ctxName').textContent = m.username ? `@${m.username}` : m.name;
    const isAdmin = m.role === 'admin';
    $('#ctxPromote').querySelector('span').textContent = isAdmin ? 'Dismiss as admin' : 'Promote to admin';
    const dmRow = $('#ctxDm');
    if (dmRow) dmRow.hidden = !!m.bot || String(m.id) === String(viewer?.id);
    const menu = $('#ctxMenu');
    menu.classList.add('is-open');
    $('#scrim').classList.add('is-on');
    const screen = $('#screen').getBoundingClientRect();
    const r = anchor.getBoundingClientRect();
    const w = 232;
    const left = clamp(r.left - screen.left - 8, 12, screen.width - w - 12);
    let top = r.bottom - screen.top + 6;
    const h = menu.offsetHeight || 176;
    if (top + h > screen.height - 16) top = Math.max(12, r.top - screen.top - h - 6);
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  }
  function closeCtx() {
    $('#ctxMenu').classList.remove('is-open');
    $('#scrim').classList.remove('is-on');
  }

  async function memberAction(act) {
    if (!ctxMember) return;
    const m = ctxMember;
    closeCtx();
    haptic('medium');
    if (act === 'profile') {
      openProfile(m.id);
      return;
    }
    if (act === 'dm') {
      dm.open = true;
      $('#dmSheet').classList.add('is-open');
      openDmThread(m.id);
      return;
    }
    const label = m.username ? `@${m.username}` : m.name;
    try {
      await api('/api/members', { id: m.id, action: act });
      if (act === 'promote') toast(`${label} promoted to admin`);
      else if (act === 'demote') toast(`${label} is a member again`);
      else if (act === 'kick') toast(`${label} kicked`);
      else if (act === 'ban') toast(`${label} banned`);
      const s = await api('/api/state');
      merge(s);
      renderAll();
    } catch (e) {
      toast(`${act} failed: ${e.message}`);
    }
  }

  /* ────────────────────────── invite / share ────────────────────────── */
  function openTelegram(url) {
    if (!url) { toast('Link unavailable'); return; }
    try {
      if (tg?.openTelegramLink) { tg.openTelegramLink(url); return; }
    } catch {}
    window.open(url, '_blank', 'noopener');
  }

  function copyText(text) {
    if (!text) { toast('Nothing to copy'); return; }
    navigator.clipboard?.writeText(text).then(
      () => toast('Copied'),
      () => toast('Copy failed'),
    );
  }

  // `https://t.me/share/url?url=…&text=Join my music room` — the share sheet
  // Telegram itself shows, with the bot's mini-app link inside.
  function openInvite(target) {
    const r = state.room || {};
    const link = r.app_link || '';
    const bot = r.bot || '';
    const card = $('#inviteCard');
    if (card) {
      card.innerHTML = `
        <div class="ic-room">${esc(r.title || 'Music room')}</div>
        <span class="ic-sub">${bot ? `@${esc(bot)} · ` : ''}${state.members.length} members${target ? ` · invite ${esc(target)}` : ''}</span>
        <div class="ic-link"><span>${esc(link || 'Link unavailable')}</span>
          <button data-copy="${esc(link)}">Copy</button></div>`;
    }
    const rows = [
      { k: 'share', label: target ? `Invite ${target}` : 'Share via Telegram', hint: bot ? `Opens t.me/${bot} inside Telegram` : 'Send the room link', ic: '#i-plane' },
      { k: 'copy', label: 'Copy room link', hint: link || 'link unavailable', ic: '#i-lock' },
      { k: 'group', label: 'Add bot to a group', hint: bot ? `t.me/${bot}?startgroup` : '', ic: '#i-plus' },
      { k: 'invite', label: 'Copy group invite', hint: r.invite || 'no invite link', ic: '#i-users' },
    ];
    $('#inviteRows').innerHTML = rows.map(x => `
      <div class="sheet-row" data-inv="${x.k}">
        <span class="invite-row-ic"><svg class="ic"><use href="${x.ic}"/></svg></span>
        <div style="flex:1;min-width:0">
          <div class="lbl">${esc(x.label)}</div>
          <div class="hint" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${esc(x.hint)}</div>
        </div>
        <svg class="ic" style="width:12px;height:12px;color:#55565c"><use href="#i-chevron"/></svg>
      </div>`).join('');
    $('#inviteSheet').classList.add('is-open');
    $('#scrim').classList.add('is-on');
  }

  function closeInvite() {
    $('#inviteSheet').classList.remove('is-open');
  }

  /* ────────────────────────── private chat ────────────────────────── */
  // A profile card can open a private chat with somebody who is not in the
  // visible roster yet; keep their details so the DM header is not blank.
  const dmPeer = () => memberById(dm.peer)
    || (social.profile && String(social.profile.id) === String(dm.peer) ? social.profile : null);

  async function openDm() {
    dm.open = true;
    dm.mode = 'list';
    dm.peer = null;
    $('#dmSheet').classList.add('is-open');
    renderDm();
    await loadDmThreads();
    renderDm();
  }

  function closeDm() {
    dm.open = false;
    dm.peer = null;
    dm.mode = 'list';
    $('#dmSheet').classList.remove('is-open');
  }

  async function loadDmThreads() {
    if (!viewer?.id) return;
    try {
      const r = await api(`/api/dm?me=${encodeURIComponent(viewer.id)}`);
      dm.threads = r.threads || [];
    } catch {
      dm.threads = [];
    }
  }

  async function openDmThread(peer) {
    dm.mode = 'thread';
    dm.peer = String(peer);
    dm.messages = [];
    renderDm();
    try {
      const r = await api(`/api/dm?me=${encodeURIComponent(viewer.id)}&peer=${encodeURIComponent(peer)}`);
      dm.messages = r.messages || [];
    } catch {
      dm.messages = [];
    }
    renderDm();
    const body = $('#dmBody');
    requestAnimationFrame(() => { body.scrollTop = body.scrollHeight; });
  }

  function renderDm() {
    const body = $('#dmBody');
    const title = $('#dmTitle');
    const sub = $('#dmSub');
    const peer = dmPeer();
    $('#dmComposer').hidden = dm.mode !== 'thread';
    $('#dmNew').style.visibility = dm.mode === 'thread' ? 'hidden' : 'visible';

    if (dm.mode === 'thread') {
      title.textContent = peer?.name || 'Private chat';
      sub.textContent = `Private · only you and ${peer?.name || 'this member'}`;
      if (!dm.messages.length) {
        body.innerHTML = `<div class="dm-empty">
          <svg class="ic"><use href="#i-lock"/></svg>
          <b>This conversation is private</b>
          <span>Nothing here is posted to the group or shown in the voice chat.</span>
        </div>`;
        return;
      }
      body.innerHTML = dm.messages.map(m => {
        const out = String(m.from) === String(viewer?.id);
        return `<div class="dm-msg ${out ? 'out' : 'in'}">
          <div class="dm-bubble">${esc(m.text || '')}<span class="dm-stamp">${esc(m.ts || '')}</span></div>
        </div>`;
      }).join('');
      body.scrollTop = body.scrollHeight;
      return;
    }

    if (dm.mode === 'pick') {
      title.textContent = 'New private chat';
      sub.textContent = 'Pick one member — nobody else can read it';
      const people = state.members
        .filter(m => String(m.id) !== String(viewer?.id) && !m.bot)
        .sort((a, b) => (b.vc ? 1 : 0) - (a.vc ? 1 : 0));
      if (!people.length) {
        body.innerHTML = `<div class="dm-empty"><b>No members to pick</b><span>The roster is still loading.</span></div>`;
        return;
      }
      body.innerHTML = people.map(m => `
        <div class="dm-pick" data-pick="${esc(m.id)}">
          <img class="avatar" src="${esc(pic(m.photo, m.name))}" data-name="${esc(m.name)}" alt="" loading="lazy" decoding="async" />
          <div class="dm-pick-body">
            <b>${esc(m.name)} ${m.vc ? '<span class="dm-lock">in voice</span>' : ''}</b>
            <span>${m.username ? '@' + esc(m.username) : 'no username'}</span>
          </div>
          <svg class="ic" style="width:12px;height:12px;color:#55565c"><use href="#i-chevron"/></svg>
        </div>`).join('');
      return;
    }

    title.textContent = 'Private chats';
    sub.textContent = 'Only you and the person you pick';
    if (!dm.threads.length) {
      body.innerHTML = `<div class="dm-empty">
        <svg class="ic"><use href="#i-lock"/></svg>
        <b>No private chats yet</b>
        <span>Start one with any member. It never appears in the group chat or the voice room.</span>
        <button class="primary-pill sm" id="dmStart" style="margin-top:14px">
          <svg class="ic ic-13"><use href="#i-plus"/></svg> New private chat
        </button>
      </div>`;
      return;
    }
    body.innerHTML = dm.threads.map(t => `
      <div class="dm-thread" data-thread="${esc(t.id)}">
        <img class="avatar" src="${esc(pic(t.photo, t.name))}" data-name="${esc(t.name)}" alt="" />
        <div class="dm-thread-body">
          <div class="dm-thread-top"><b>${esc(t.name)}</b><span class="dm-lock">private</span><i>${esc(t.ts || '')}</i></div>
          <div class="dm-thread-last">${esc(t.last || '')}</div>
        </div>
      </div>`).join('');
  }

  async function sendDm() {
    const input = $('#dmInput');
    const text = (input.innerText || '').replace(/\u200B/g, '').trim();
    if (!text || !dm.peer) return;
    const peer = dm.peer;
    input.innerHTML = '';
    const local = { id: `dl${Date.now()}`, from: viewer.id, to: peer, text, ts: nowStamp() };
    dm.messages.push(local);
    renderDm();
    try {
      const r = await api('/api/dm/send', { me: viewer.id, peer, text });
      if (r.message) {
        const i = dm.messages.findIndex(m => m.id === local.id);
        if (i >= 0) dm.messages[i] = r.message;
      }
      loadDmThreads();
    } catch (e) {
      toast(`Private message failed: ${e.message}`);
    }
  }

  /* ────────────────────────── settings ────────────────────────── */
  const SETTINGS = [
    { key: 'autoplay', label: 'Autoplay', hint: 'Keep playing similar songs', on: true },
    { key: 'loop', label: 'Loop queue', hint: 'Restart when the queue ends', on: false },
    { key: 'haptics', label: 'Haptics', hint: 'Vibrate on controls', on: true },
    { key: 'notify', label: 'Now playing alerts', hint: 'Post a card in the chat', on: true },
  ];

  function renderSettings() {
    const toggles = SETTINGS.map(s => {
      const on = typeof state.settings?.[s.key] === 'boolean' ? state.settings[s.key] : s.on;
      return `
      <div class="sheet-row">
        <div style="flex:1">
          <div class="lbl">${s.label}</div>
          <div class="hint">${s.hint}</div>
        </div>
        <button class="switch ${on ? 'is-on' : ''}" data-setting="${s.key}" aria-label="${s.label}"></button>
      </div>`;
    }).join('');
    // Ending the call is a real action (stops the stream, leaves the group
    // call, clears the chat), so it sits at the bottom, in red, only while
    // there is something to end.
    const invite = `<div class="sheet-row" data-invite-room="1">
        <span class="invite-row-ic"><svg class="ic"><use href="#i-plane"/></svg></span>
        <div style="flex:1">
          <div class="lbl">Invite &amp; share</div>
          <div class="hint">Send the room link with the bot username</div>
        </div>
        <svg class="ic" style="width:12px;height:12px;color:#55565c"><use href="#i-chevron"/></svg>
      </div>`;
    const end = isLive()
      ? `<div class="sheet-row danger-row">
           <div style="flex:1">
             <div class="lbl">End voice chat</div>
             <div class="hint">Stops playback, leaves the call and clears the chat</div>
           </div>
           <button class="end-btn" id="btnEndSession">End</button>
         </div>`
      : `<div class="sheet-row">
           <div style="flex:1">
             <div class="lbl">Voice chat</div>
             <div class="hint">No active session — send /play in the group</div>
           </div>
           <span class="tag member">ended</span>
         </div>`;
    $('#settingsRows').innerHTML = invite + toggles + end;
  }

  async function endVoiceChat() {
    if (!isLive()) { toast('No active voice chat'); return; }
    try {
      const r = await api('/api/session/end', { id: viewer?.id, name: viewer?.name });
      state.members = state.members.map(m => ({ ...m, vc: false }));
      state.playback = { ...state.playback, track: null, playing: false, paused: true, position: 0, queue: [] };
      state.messages = [];
      chatSig = '0';
      merge({ room: { live: false, online: 0 }, session: r.session });
      closeSheet();
      renderAll();
      toast('Voice chat ended · left the call · chat cleared');
    } catch (e) {
      toast(`Could not end: ${e.message}`);
    }
  }

  function openSheet() {
    // Re-render first: the end-session row depends on the live state.
    renderSettings();
    $('#settingsSheet').classList.add('is-open');
    $('#scrim').classList.add('is-on');
  }
  function closeSheet() {
    $('#settingsSheet').classList.remove('is-open');
    $('#scrim').classList.remove('is-on');
  }

  /* ─────────────────── social: shared overlays ─────────────────── */
  const SOCIAL_SHEETS = ['#profileSheet', '#msgMenu', '#stickerSheet', '#forwardSheet', '#chatsSheet'];

  function closeSocial() {
    SOCIAL_SHEETS.forEach(s => { const el = $(s); if (el) el.classList.remove('is-open'); });
    $('#scrim').classList.remove('is-on');
  }

  function openSocial(sel) {
    // Only one overlay at a time. Closing the others clears the shared scrim,
    // which is put straight back under the freshly opened sheet.
    closeCtx(); closeSheet(); closeInvite(); closePick(); closeSocial();
    $(sel).classList.add('is-open');
    $('#scrim').classList.add('is-on');
  }

  async function loadSavedStickers() {
    if (!online || !viewer?.id) return;
    try {
      const r = await api(`/api/stickers/saved?me=${encodeURIComponent(viewer.id)}`);
      social.savedStickers = r.stickers || [];
      if (pane === 'saved') renderPane();
    } catch { social.savedStickers = []; }
  }

  /* ─────────────────── user profile card ─────────────────── */
  async function openProfile(id) {
    id = String(id || '');
    if (!id || id === 'bot') return;
    closeCtx();
    haptic('light');
    let u = null;
    if (online) {
      try { u = await api(`/api/user?id=${encodeURIComponent(id)}`); } catch {}
    }
    // Bridge offline? Still show everything the roster already knows.
    u = u || memberById(id) || { id, name: 'member', tags: [{ k: 'member', label: 'MEMBER' }] };
    social.profile = { ...u, id, photo: u.photo || `/api/avatar/${id}` };
    paintProfile(social.profile);
    openSocial('#profileSheet');
  }

  function paintProfile(u) {
    const isMe = String(u.id) === String(viewer?.id);
    const av = $('#profAv');
    av.src = u.photo_big || u.photo || avatarURI(u.name);
    av.dataset.name = u.name || '?';
    $('#profName').textContent = u.name || 'member';
    $('#profHandle').textContent = u.username ? `@${u.username}` : `id ${u.id}`;
    $('#profTags').innerHTML = tagHTML(u.tags || [{ k: 'member', label: 'MEMBER' }], 3);
    const bio = (u.bio || '').trim();
    $('#profBio').hidden = !bio;
    $('#profBio').textContent = bio;
    const stats = [
      { v: u.messages ?? '–', l: 'Messages here' },
      { v: u.in_voice ? 'In call' : 'Away', l: 'Voice chat' },
      { v: u.last_seen || '–', l: 'Last message' },
    ];
    $('#profStats').innerHTML = stats.map(s =>
      `<div class="prof-stat"><b>${esc(String(s.v))}</b><span>${esc(s.l)}</span></div>`).join('');

    const quick = (act) => $(`#profileSheet [data-prof="${act}"]`);
    const dm = quick('dm'), mention = quick('mention'), tg = quick('tg');
    if (dm) dm.hidden = isMe || !!u.bot;
    if (mention) mention.hidden = isMe;
    if (tg) tg.hidden = !u.username;

    const rows = [];
    if (u.username) rows.push({ ic: '#i-at', label: `@${u.username}`, hint: 'Username · tap to copy', copy: `@${u.username}` });
    rows.push({ ic: '#i-info', label: `User ID · ${u.id}`, hint: 'Telegram account id · tap to copy', copy: String(u.id) });
    if (u.premium) rows.push({ ic: '#i-info', label: 'Telegram Premium', hint: 'This account has Premium' });
    if (u.username) rows.push({ ic: '#i-external', label: 'Open in Telegram', hint: `t.me/${u.username}`, act: 'tg' });
    if (!isMe && !u.bot) rows.push({ ic: '#i-lock', label: 'Send private message', hint: 'Only the two of you can read it', act: 'dm' });
    $('#profRows').innerHTML = rows.length
      ? rows.map(r => {
          const attr = r.copy ? `data-prof-copy="${esc(r.copy)}"`
            : (r.act ? `data-prof="${esc(r.act)}"` : '');
          return `<div class="sheet-row" ${attr}>
            <span class="invite-row-ic"><svg class="ic"><use href="${r.ic}"/></svg></span>
            <div style="flex:1;min-width:0">
              <div class="lbl">${esc(r.label)}</div>
              <div class="hint">${esc(r.hint)}</div>
            </div>
            <svg class="ic" style="width:12px;height:12px;color:#55565c"><use href="#i-chevron"/></svg>
          </div>`;
        }).join('')
      : '<div class="prof-none">Only public Telegram details are shown.</div>';
  }

  function profileAction(act) {
    const u = social.profile;
    if (!u) return;
    if (act === 'dm') {
      closeSocial();
      dm.open = true;
      $('#dmSheet').classList.add('is-open');
      openDmThread(String(u.id));
      haptic('light');
      return;
    }
    if (act === 'mention') {
      closeSocial();
      switchTab('chat');
      const name = u.username ? `@${u.username}` : (u.name || 'member');
      insertNode(document.createTextNode(`${name} `));
      editor().focus();
      haptic('light');
      return;
    }
    if (act === 'tg') {
      if (u.username) openTelegram(`https://t.me/${u.username}`);
      else toast('No public username');
    }
  }

  /* ─────────────────── stickers: save / resend ─────────────────── */
  // Turn a chat message into a portable sticker record. Mirrored stickers
  // point at /api/tgmedia/<key> (cached MTProto bytes), pack stickers at
  // /api/tgimg?fid=<file_id>. Keep both so resending always works.
  function stickerFromMessage(m) {
    const media = m.media || '';
    let fileId = '', mediaKey = '';
    const mTg = media.match(/\/api\/tgmedia\/([A-Za-z0-9_.\-]+)/);
    const mFid = media.match(/[?&]fid=([^&]+)/);
    if (mTg) mediaKey = mTg[1];
    if (mFid) { try { fileId = decodeURIComponent(mFid[1]); } catch { fileId = mFid[1]; } }
    return {
      id: `sv_${m.id || m.mid || Date.now()}`,
      emoji: m.emoji || '🎵',
      media,
      media_kind: m.media_kind || 'image',
      file_id: fileId,
      mediaKey,
      thumb: m.sticker || m.media || '',
      set: m.set || '',
    };
  }

  function openStickerDetail(stk, fromSaved = false) {
    if (!stk) return;
    social.sticker = { ...stk, fromSaved };
    const kind = stk.media_kind || 'image';
    const prev = $('#sdPreview');
    if (kind === 'video' && stk.media) {
      prev.innerHTML = `<video src="${esc(stk.media)}" autoplay loop muted playsinline webkit-playsinline></video>`;
    } else if (kind === 'tgs' || !stk.media) {
      prev.innerHTML = `<span class="sd-emoji">${esc(stk.emoji || '🎵')}</span>`;
    } else {
      prev.innerHTML = `<img src="${esc(stk.media)}" alt="${esc(stk.emoji || '')}" />`;
    }
    $('#sdTitle').textContent = stk.emoji ? `Sticker ${stk.emoji}` : 'Sticker';
    $('#sdSub').textContent = fromSaved
      ? 'Saved in your personal stickers — send it in one tap.'
      : 'Save it to your stickers and send it in one tap.';
    $('#sdSave').hidden = !!fromSaved;
    $('#sdRemove').hidden = !fromSaved;
    openSocial('#stickerSheet');
    haptic('light');
  }

  async function saveStickerQuick(m) {
    social.sticker = stickerFromMessage(m);
    await saveStickerNow();
  }

  async function saveStickerNow() {
    const stk = social.sticker;
    if (!stk || !viewer?.id) return;
    try {
      const r = await api('/api/stickers/saved', {
        me: viewer.id, action: 'add',
        sticker: {
          id: stk.id, emoji: stk.emoji, media: stk.media,
          media_kind: stk.media_kind, file_id: stk.file_id || '',
          mediaKey: stk.mediaKey || '', thumb: stk.thumb || stk.media, set: stk.set || '',
        },
      });
      social.savedStickers = r.stickers || social.savedStickers;
      closeSocial();
      toast('Added to Stickers');
      if (pane === 'saved') renderPane();
    } catch (e) { toast(`Could not save: ${e.message}`); }
  }

  async function removeStickerNow() {
    const stk = social.sticker;
    if (!stk || !viewer?.id) return;
    try {
      const r = await api('/api/stickers/saved', { me: viewer.id, action: 'remove', id: stk.id });
      social.savedStickers = r.stickers || [];
      closeSocial();
      toast('Removed from Saved');
      if (pane === 'saved') renderPane();
    } catch (e) { toast(`Could not remove: ${e.message}`); }
  }

  /* ─────────────────── message action menu ─────────────────── */
  function msgPreview(m) {
    if (!m) return '';
    return m.kind === 'sticker' ? `Sticker ${m.emoji || ''}`.trim()
      : m.kind === 'photo' ? 'Photo'
      : m.kind === 'voice' ? 'Voice message'
      : (m.text || '').slice(0, 160) || 'Message';
  }

  function openMsgMenu(m) {
    if (!m) return;
    social.msgMenu = m;
    const own = !!m.mine || String(m.from) === String(viewer?.id);
    $('#actPreview').innerHTML = `<b>${esc(own ? 'You' : (m.name || 'member'))}</b>${esc(msgPreview(m))}`;
    const acts = [
      { k: 'reply', label: 'Reply', ic: '#i-reply' },
      { k: 'copy', label: 'Copy text', ic: '#i-copy', show: !!m.text },
      { k: 'forward', label: 'Forward', ic: '#i-forward' },
      { k: 'saveSticker', label: 'Add to Stickers', ic: '#i-bookmark', show: m.kind === 'sticker' },
      { k: 'react', label: 'React ❤️', ic: '#i-heart', show: !!m.mid },
      { k: 'profile', label: 'View profile', ic: '#i-user', show: !own && !!m.from && m.from !== 'bot' },
      // Local-only bubbles (mid 0) never reached Telegram, so they are removed
      // from the mirror only; the server handles both cases on the same route.
      { k: 'delete', label: 'Delete', ic: '#i-trash', danger: true,
        show: online && (own || amAdmin()) && (!!m.mid || !!m.id) },
    ].filter(a => a.show !== false);
    $('#actGrid').innerHTML = acts.map(a => `
      <button class="act-btn ${a.danger ? 'danger' : ''}" data-msg-act="${a.k}">
        <svg class="ic"><use href="${a.ic}"/></svg><span>${esc(a.label)}</span>
      </button>`).join('');
    openSocial('#msgMenu');
    haptic('medium');
  }

  function msgAction(act) {
    const m = social.msgMenu;
    if (!m) return;
    closeSocial();
    switch (act) {
      case 'reply': setReply(m); break;
      case 'copy': copyText(m.text || ''); break;
      case 'forward': openForward(m); break;
      case 'saveSticker': saveStickerQuick(m); break;
      case 'react': react(m, '❤️'); break;
      case 'profile': openProfile(m.from); break;
      case 'delete': deleteMsg(m); break;
    }
  }

  async function deleteMsg(m) {
    const run = async () => {
      try {
        await api('/api/chat/delete', { mid: m.mid || 0, id: m.id });
        state.messages = state.messages.filter(x => String(x.id) !== String(m.id));
        chatSig = sigOf(state.messages);
        renderChat(true);
        toast('Message deleted');
      } catch (e) { toast(`Delete failed: ${e.message}`); }
    };
    const ok = await askConfirm(
      'Delete message',
      m.mid ? 'Delete this message in the group and the mirrored chat?' : 'Delete this message from the chat?',
    );
    if (ok) run();
  }

  /* ─────────────────── forward to a private chat ─────────────────── */
  function openForward(m) {
    if (!m) return;
    social.forward = m;
    $('#forwardSub').textContent = `“${(msgPreview(m) || '').slice(0, 60)}” — a private copy, never posted to the group.`;
    const people = state.members
      .filter(x => String(x.id) !== String(viewer?.id) && !x.bot)
      .sort((a, b) => (b.vc ? 1 : 0) - (a.vc ? 1 : 0));
    $('#forwardList').innerHTML = people.length
      ? people.map(p => `<button class="forward-row" data-forward="${esc(p.id)}">
          <img class="avatar" src="${esc(pic(p.photo, p.name))}" data-name="${esc(p.name)}" alt="" />
          <span><b>${esc(p.name)}</b><span>${p.username ? '@' + esc(p.username) : 'private chat'}</span></span>
          <svg class="ic" style="width:12px;height:12px;color:#55565c"><use href="#i-chevron"/></svg>
        </button>`).join('')
      : '<div class="prof-none">No members to forward to yet.</div>';
    openSocial('#forwardSheet');
  }

  async function forwardTo(peer) {
    const m = social.forward;
    if (!m || !peer || !viewer?.id) return;
    closeSocial();
    const head = m.name && !m.mine ? `Forwarded from ${m.name}:\n` : '';
    const text = `${head}${msgPreview(m)}`.slice(0, 1000);
    try {
      await api('/api/dm/send', { me: viewer.id, peer: String(peer), text });
      toast('Forwarded privately');
    } catch (e) { toast(`Forward failed: ${e.message}`); }
  }

  /* ─────────────────── jump to a replied message ─────────────────── */
  function jumpToMessage(m) {
    if (!m?.reply?.mid) { toast('Original message not linked'); return; }
    const target = state.messages.find(x => x.mid && String(x.mid) === String(m.reply.mid));
    if (!target) { toast('Original message is not in this view'); return; }
    const el = $$('#msgs .msg').find(x => x.dataset.id === String(target.id));
    if (!el) { toast('Switch to Everyone to see that message'); return; }
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    el.classList.remove('is-highlight');
    void el.offsetWidth;
    el.classList.add('is-highlight');
    setTimeout(() => el.classList.remove('is-highlight'), 1200);
  }

  /* ─────────────────── chat gestures ─────────────────── */
  // Telegram-like: horizontal swipe → reply, long-press → action menu,
  // plain tap → reply (existing), double-tap → ❤️ (existing). Vertical
  // scrolling is left to the browser via `touch-action: pan-y` on .msg.
  function wireChatGestures() {
    const box = $('#msgs');
    let g = null;

    const settle = (el) => {
      if (!el) return;
      el.classList.remove('is-swiping');
      el.style.transform = '';
      const hint = el.querySelector('.swipe-hint');
      if (hint) hint.style.opacity = '';
    };

    box.addEventListener('pointerdown', (e) => {
      const el = e.target.closest('.msg');
      if (!el || e.button === 2) return;
      // Interactive children keep their own behaviour (profile, +, links).
      if (e.target.closest('a, .sticker-save, .react-chip, .reply, .msg-av, .msg-head b')) return;
      const m = state.messages.find(x => String(x.id) === el.dataset.id);
      if (!m) return;
      g = { id: e.pointerId, el, m, x0: e.clientX, y0: e.clientY, dx: 0, active: false, timer: null };
      g.timer = setTimeout(() => {
        if (!g || g.active) return;
        const cur = g;
        g = null;
        clearTimeout(cur.timer);
        suppressUntil = Date.now() + 500;   // swallow the click that follows
        lastTap = { id: '', at: 0 };
        haptic('medium');
        openMsgMenu(cur.m);
      }, 470);
    });

    box.addEventListener('pointermove', (e) => {
      if (!g || e.pointerId !== g.id) return;
      const dx = e.clientX - g.x0;
      const dy = e.clientY - g.y0;
      if (!g.active) {
        if (Math.abs(dx) < 12) return;
        if (Math.abs(dx) < Math.abs(dy) * 1.2) {   // vertical scroll — let it be
          clearTimeout(g.timer); g = null; return;
        }
        g.active = true;
        clearTimeout(g.timer);
        try { g.el.setPointerCapture(e.pointerId); } catch {}
        g.el.classList.add('is-swiping');
        const hint = g.el.querySelector('.swipe-hint');
        if (hint) hint.style.opacity = '1';
      }
      g.dx = clamp(dx, 0, 84);
      g.el.style.transform = `translateX(${g.dx}px)`;
    });

    const end = (e) => {
      if (!g || e.pointerId !== g.id) return;
      clearTimeout(g.timer);
      const cur = g;
      g = null;
      if (!cur.active) return;                     // plain tap → click handler
      const fired = cur.dx > 54;
      cur.el.classList.remove('is-swiping');
      cur.el.classList.add('is-settling');
      cur.el.style.transform = '';
      const hint = cur.el.querySelector('.swipe-hint');
      if (hint) hint.style.opacity = '';
      setTimeout(() => cur.el && cur.el.classList.remove('is-settling'), 280);
      suppressUntil = Date.now() + 450;           // swallow the trailing click
      lastTap = { id: '', at: 0 };
      if (fired) { haptic('light'); setReply(cur.m); }
    };
    box.addEventListener('pointerup', end);
    box.addEventListener('pointercancel', (e) => {
      if (!g || e.pointerId !== g.id) return;
      clearTimeout(g.timer);
      settle(g.el);
      g = null;
    });
    box.addEventListener('contextmenu', (e) => {
      if (e.target.closest('.msg')) e.preventDefault();
    });
    // A native HTML5 image drag cancels the pointer stream (pointercancel),
    // which used to make swipe-to-reply silently fail on sticker/photo
    // bubbles while working on text bubbles. Suppress it on chat media.
    box.addEventListener('dragstart', (e) => {
      if (e.target.closest('.msg')) e.preventDefault();
    }, true);
  }

  /* ────────────────────────── wiring ────────────────────────── */
  function bind() {
    $$('.tab').forEach(b => b.addEventListener('click', () => switchTab(b.dataset.tab)));
    $('#btnSearch').addEventListener('click', openSearch);
    $('#searchClose').addEventListener('click', closeSearch);
    $('#searchClear').addEventListener('click', () => {
      $('#searchInput').value = '';
      onSearchInput({ target: $('#searchInput') });
      $('#searchInput').focus();
    });
    $('#searchInput').addEventListener('input', onSearchInput);
    $('#trendChips').addEventListener('click', (e) => {
      const chip = e.target.closest('[data-q]');
      if (!chip) return;
      $('#searchInput').value = chip.dataset.q;
      onSearchInput({ target: $('#searchInput') });
    });
    $('#histChips').addEventListener('click', (e) => {
      const chip = e.target.closest('[data-q]');
      if (!chip) return;
      $('#searchInput').value = chip.dataset.q;
      onSearchInput({ target: $('#searchInput') });
    });
    $('#results').addEventListener('click', async (e) => {
      const like = e.target.closest('[data-like]');
      const add = e.target.closest('[data-add]');
      const row = e.target.closest('.result');
      const items = JSON.parse($('#results').dataset.items || '[]');
      // Select by index, not by id: the id only has to identify the track on
      // the server, the row the user actually tapped is what must play.
      const rowTrack = row ? items[+row.dataset.idx] : null;
      if (like) {
        const id = like.dataset.like;
        const on = !like.classList.contains('liked');
        like.classList.toggle('liked', on);
        like.querySelector('use').setAttribute('href', on ? '#i-heart-fill' : '#i-heart');
        haptic('light');
        toast(on ? 'Saved to favourites' : 'Removed from favourites');
        try { await api('/api/fav', { id, on }); } catch {}
        return;
      }
      if (!add) {
        if (!rowTrack) return;
        // Audio or video? The sheet answers that, and only then do we start.
        openPick(rowTrack);
        return;
      }
      const track = items.find(t => String(t.id) === add.dataset.add);
      if (!track) return;
      add.classList.add('added');
      add.querySelector('use').setAttribute('href', '#i-check');
      toast('Added to queue');
      haptic('light');
      try {
        const s = await api('/api/queue/add', { id: track.id, kind: 'audio' });
        if (s) { merge(s); renderAll(); }
      } catch { state.playback.queue.push(track); renderQueue(); }
    });

    // ── audio / video picker
    $('#pickSheet').addEventListener('click', (e) => {
      const b = e.target.closest('[data-kind]');
      if (!b || !pickTrack) return;
      const t = pickTrack;
      closePick();
      closeSearch();
      playTrack(t.id, b.dataset.kind);
    });
    $('#pickQueue').addEventListener('click', async () => {
      if (!pickTrack) return;
      const t = pickTrack;
      closePick();
      toast('Added to queue');
      haptic('light');
      try {
        const s = await api('/api/queue/add', { id: t.id, kind: 'audio' });
        if (s) { merge(s); renderAll(); }
      } catch (err) { toast(`Could not add: ${err.message}`); }
    });
    $('#pickFav').addEventListener('click', async () => {
      if (!pickTrack) return;
      const t = pickTrack;
      closePick();
      haptic('light');
      toast('Saved to favourites');
      try { await api('/api/fav', { id: t.id, on: true }); } catch {}
    });

    $('#btnPlay').addEventListener('click', togglePlay);
    $('#miniPlay').addEventListener('click', togglePlay);
    $('#btnNext').addEventListener('click', skip);
    $('#btnLoop').addEventListener('click', async () => {
      state.playback.loop = (state.playback.loop + 1) % 3;
      $('#btnLoop').classList.toggle('is-on', state.playback.loop > 0);
      haptic('light');
      toast(state.playback.loop === 0 ? 'Loop off' : state.playback.loop === 1 ? 'Loop queue' : 'Loop one');
      try { await api('/api/loop', { mode: state.playback.loop }); } catch {}
    });
    $('#btnAddCurrent').addEventListener('click', () => toast('Already in queue'));

    // ── like + sleep timer
    $('#btnFav').addEventListener('click', async () => {
      const t = currentTrack();
      if (!t) { toast('Nothing playing'); return; }
      const on = !state.playback.liked;
      state.playback.liked = on;
      paintFav();
      haptic('light');
      try {
        const r = await api('/api/fav', { id: t.id });
        if (r && typeof r.liked === 'boolean') { state.playback.liked = r.liked; paintFav(); }
      } catch (err) { toast(`Could not save: ${err.message}`); }
    });
    $('#btnSleep').addEventListener('click', () => {
      const grid = $('#sleepGrid');
      grid.querySelectorAll('.sleep-opt').forEach(b => {
        const m = +b.dataset.min;
        b.classList.toggle('is-on', m > 0 && Math.abs(state.sleep - m * 60) < 60);
        b.classList.toggle('is-off', m === 0);
      });
      $('#sleepSheet').classList.add('is-open');
      $('#scrim').classList.add('is-on');
    });
    $('#sleepGrid').addEventListener('click', async (e) => {
      const opt = e.target.closest('[data-min]');
      if (!opt) return;
      const minutes = +opt.dataset.min;
      $('#sleepSheet').classList.remove('is-open');
      $('#scrim').classList.remove('is-on');
      haptic('light');
      try {
        const r = await api('/api/sleep', { minutes });
        state.sleep = r?.sleep ?? minutes * 60;
        paintSleep();
        toast(minutes ? `Sleep timer · ${minutes} min` : 'Sleep timer off');
      } catch (err) { toast(`Timer failed: ${err.message}`); }
    });
    $('#btnRestart').addEventListener('click', () => openSearch());

    // ── profile settings: lite mode + local cache
    const liteOn = () => document.documentElement.classList.contains('fx-lite');
    function paintLite() {
      const b = $('#liteToggle');
      if (!b) return;
      b.classList.toggle('is-on', liteOn());
      b.setAttribute('aria-checked', liteOn() ? 'true' : 'false');
    }
    paintLite();
    $('#liteToggle').addEventListener('click', () => {
      const on = !liteOn();
      document.documentElement.classList.toggle('fx-lite', on);
      try { localStorage.setItem('pm-lite', on ? '1' : '0'); } catch {}
      paintLite();
      haptic('light');
      toast(on ? 'Lite mode on — blur off' : 'Lite mode off');
    });
    const clearCache = () => {
      tgsCache.clear();
      try {
        localStorage.removeItem('pm-covers');
        localStorage.removeItem('pm-searches');
      } catch {}
      // Drop HTTP-cached stickers/avatars by reloading with a fresh query.
      toast('Cache cleared · reloading');
      setTimeout(() => {
        const u = new URL(location.href);
        u.searchParams.set('r', String(Date.now() % 100000));
        location.replace(u.toString());
      }, 500);
    };
    $('#btnClearCache').addEventListener('click', clearCache);
    $('#btnClearCache').addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); clearCache(); }
    });
    const botLink = () => (state.room?.bot ? `https://t.me/${state.room.bot}` : 'https://t.me/');
    $('#btnOpenBot').addEventListener('click', () => {
      if (tg?.openTelegramLink) tg.openTelegramLink(botLink());
      else toast('Open the bot in Telegram');
    });
    $('#btnTelegram').addEventListener('click', () => {
      if (tg?.openTelegramLink) tg.openTelegramLink(botLink());
      else toast('Open in Telegram');
    });
    bindSeek();

    // ── composer
    $('#btnSend').addEventListener('click', () => {
      if (composerText()) sendText();
      else toast('Hold to record a voice note');
    });
    editor().addEventListener('input', updateSendState);
    editor().addEventListener('focus', () => {
      $('#stickerPanel').classList.remove('is-on');
      $('#btnSticker').classList.remove('is-on');
      setTimeout(scrollChat, 240);
    });
    editor().addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendText(); }
    });
    editor().addEventListener('paste', (e) => {
      e.preventDefault();
      const text = (e.clipboardData || window.clipboardData).getData('text');
      insertNode(document.createTextNode(text));
    });

    $('#btnSticker').addEventListener('click', () => openPanel());
    $$('.sticker-tab').forEach(t => t.addEventListener('click', () => {
      pane = t.dataset.pane;
      renderPane();
      if (pane === 'saved') loadSavedStickers();
    }));
    $('#stickerPacks').addEventListener('click', (e) => {
      const chip = e.target.closest('[data-pack]');
      if (!chip) return;
      packName = chip.dataset.pack;
      renderPane();
    });
    $('#stickerGrid').addEventListener('click', async (e) => {
      // A long-press on a Saved cell opens the remove sheet and consumes the
      // click that follows it.
      if (suppressGridClick) { suppressGridClick = false; return; }
      const em = e.target.closest('[data-emoji]');
      if (em) { insertEmoji(em.dataset.emoji); haptic('light'); return; }
      const sv = e.target.closest('[data-saved]');
      if (sv) {
        const s = social.savedStickers[+sv.dataset.saved];
        if (s) sendSticker(s, s.thumb || s.media);
        return;
      }
      const cell = e.target.closest('[data-sticker]');
      const prem = e.target.closest('[data-prem]');
      const set = packCache[$('#stickerGrid').dataset.set];
      if (!set) return;
      if (cell) { sendSticker(set.stickers[+cell.dataset.sticker]); return; }
      if (prem) { insertPremium(set.stickers[+prem.dataset.prem]); haptic('light'); }
    });
    let savedHold = null;
    const clearSavedHold = () => { if (savedHold) { clearTimeout(savedHold); savedHold = null; } };
    $('#stickerGrid').addEventListener('pointerdown', (e) => {
      const c = e.target.closest('[data-saved]');
      if (!c) return;
      clearSavedHold();
      savedHold = setTimeout(() => {
        savedHold = null;
        suppressGridClick = true;
        const s = social.savedStickers[+c.dataset.saved];
        if (s) openStickerDetail(s, true);
      }, 480);
    });
    ['pointerup', 'pointercancel', 'pointerleave'].forEach(ev =>
      $('#stickerGrid').addEventListener(ev, clearSavedHold));

    $('#replyCancel').addEventListener('click', cancelReply);

    // Tap targets, resolved before the plain tap-to-reply / double-tap-to-❤️
    // path. A swipe or long-press raises suppressUntil so its trailing click
    // never becomes a reply.
    $('#msgs').addEventListener('click', (e) => {
      if (Date.now() < suppressUntil) return;
      const el = e.target.closest('.msg');
      if (!el) return;
      const m = state.messages.find(x => String(x.id) === el.dataset.id);
      if (!m) return;
      if (e.target.closest('.msg-av, .msg-head b')) { openProfile(m.from); return; }
      if (e.target.closest('.reply')) { jumpToMessage(m); return; }
      if (e.target.closest('.react-chip')) { openMsgMenu(m); return; }
      if (e.target.closest('.sticker-save, .bubble.sticker')) {
        openStickerDetail(stickerFromMessage(m));
        return;
      }
      if (e.target.closest('a')) return;         // links stay clickable
      const now = Date.now();
      if (lastTap.id === m.id && now - lastTap.at < 340) {
        react(m, '❤️');
        lastTap = { id: '', at: 0 };
        return;
      }
      lastTap = { id: m.id, at: now };
      setTimeout(() => {
        if (lastTap.id !== m.id || Date.now() < suppressUntil) return;
        setReply(m);
      }, 350);
    });
    wireChatGestures();

    // ── members
    $('#memberList').addEventListener('click', (e) => {
      const inv = e.target.closest('[data-invite]');
      if (inv) {
        const m = memberById(inv.dataset.invite);
        openInvite(m?.name);
        return;
      }
      const menu = e.target.closest('[data-menu]');
      if (menu) { openCtx(menu.dataset.menu, menu); return; }
      const row = e.target.closest('.member');
      if (row) openProfile(row.dataset.id);   // tap a member → profile card
    });
    $('#ctxMenu').addEventListener('click', (e) => {
      const b = e.target.closest('[data-act]');
      if (b) memberAction(b.dataset.act);
    });

    // ── social v3: profile / message actions / sticker detail / forward
    $('#profileSheet').addEventListener('click', (e) => {
      if (e.target.closest('#profClose')) { closeSocial(); return; }
      const cp = e.target.closest('[data-prof-copy]');
      if (cp) { copyText(cp.dataset.profCopy); return; }
      const q = e.target.closest('[data-prof]');
      if (q) profileAction(q.dataset.prof);
    });
    $('#msgMenu').addEventListener('click', (e) => {
      const r = e.target.closest('[data-react]');
      if (r) { const m = social.msgMenu; closeSocial(); if (m) react(m, r.dataset.react); return; }
      const a = e.target.closest('[data-msg-act]');
      if (a) msgAction(a.dataset.msgAct);
    });
    $('#stickerSheet').addEventListener('click', (e) => {
      const b = e.target.closest('[data-sd]');
      if (!b) return;
      if (b.dataset.sd === 'save') saveStickerNow();
      else if (b.dataset.sd === 'send') {
        const s = social.sticker; closeSocial();
        if (s) sendSticker(s, s.thumb || s.media);
      } else if (b.dataset.sd === 'remove') removeStickerNow();
    });
    $('#forwardList').addEventListener('click', (e) => {
      const r = e.target.closest('[data-forward]');
      if (r) forwardTo(r.dataset.forward);
    });

    $('#scrim').addEventListener('click', () => { closeCtx(); closeSheet(); closeInvite(); closePick(); closeSocial(); });
    $('#btnInvite').addEventListener('click', () => openInvite());
    $('#inviteRows').addEventListener('click', (e) => {
      const row = e.target.closest('[data-inv]');
      if (!row) return;
      const r = state.room || {};
      const act = row.dataset.inv;
      if (act === 'share') openTelegram(r.share);
      else if (act === 'copy') copyText(r.app_link);
      else if (act === 'group') openTelegram(r.add_to_group);
      else if (act === 'invite') copyText(r.invite);
      closeInvite();
      $('#scrim').classList.remove('is-on');
    });
    $('#inviteCard').addEventListener('click', (e) => {
      const b = e.target.closest('[data-copy]');
      if (b) copyText(b.dataset.copy);
    });

    // ── chat scope + private chats
    $('#chatSwitch').addEventListener('click', openChats);
    $('#chatsRefresh').addEventListener('click', () => refreshChats());
    $('#chatClearRow').addEventListener('click', clearChatNow);
    $('#chatRows').addEventListener('click', (e) => {
      const del = e.target.closest('[data-del-chat]');
      if (del) { e.stopPropagation(); deleteChat(del.dataset.delChat); return; }
      const row = e.target.closest('[data-chat]');
      if (row) switchChat(row.dataset.chat);
    });
    $('#chatSeg').addEventListener('click', (e) => {
      const b = e.target.closest('[data-scope]');
      if (!b) return;
      chatScope = b.dataset.scope;
      $$('#chatSeg .seg-btn').forEach(x => x.classList.toggle('is-on', x === b));
      renderChat();
      haptic('light');
    });
    $('#btnPrivate').addEventListener('click', openDm);
    $('#msgs').addEventListener('click', (e) => {
      if (!e.target.closest('[data-scope-all]')) return;
      chatScope = 'all';
      $$('#chatSeg .seg-btn').forEach(x => x.classList.toggle('is-on', x.dataset.scope === 'all'));
      renderChat(true);
      haptic('light');
    });
    $('#dmBack').addEventListener('click', () => {
      if (dm.mode === 'list') closeDm();
      else { dm.mode = 'list'; dm.peer = null; renderDm(); loadDmThreads().then(renderDm); }
    });
    $('#dmNew').addEventListener('click', () => { dm.mode = 'pick'; renderDm(); });
    $('#dmSend').addEventListener('click', sendDm);
    $('#dmInput').addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendDm(); }
    });
    $('#dmBody').addEventListener('click', (e) => {
      if (e.target.closest('#dmStart')) { dm.mode = 'pick'; renderDm(); return; }
      const th = e.target.closest('[data-thread]');
      if (th) { openDmThread(th.dataset.thread); return; }
      const pick = e.target.closest('[data-pick]');
      if (pick) { openDmThread(pick.dataset.pick); }
    });

    // ── queue
    $('#queueList').addEventListener('click', async (e) => {
      const play = e.target.closest('[data-play]');
      if (play) {
        const t = state.playback.queue[+play.dataset.play];
        // Same question everywhere: audio or video?
        if (t) openPick(t);
        return;
      }
      const up = e.target.closest('[data-up]');
      const down = e.target.closest('[data-down]');
      if (up || down) {
        const from = +(up ? up.dataset.up : down.dataset.down);
        const to = up ? from - 1 : from + 1;
        if (to < 0 || to >= state.playback.queue.length) return;
        const q = state.playback.queue.slice();
        const [row] = q.splice(from, 1);
        q.splice(to, 0, row);
        state.playback.queue = q;
        renderQueue(true);
        try { await api('/api/queue/move', { from, to }); } catch { toast('Move failed'); }
        return;
      }
      const rm = e.target.closest('[data-remove]');
      if (rm) {
        const idx = +rm.dataset.remove;
        state.playback.queue.splice(idx, 1);
        renderQueue(true);
        toast('Removed from queue');
        try { await api('/api/queue/remove', { index: idx }); } catch {}
        return;
      }
      const rq = e.target.closest('[data-requeue]');
      if (rq) {
        try { await api('/api/queue/add', { id: rq.dataset.requeue }); toast('Added to queue'); }
        catch (err) { toast(`Could not add: ${err.message}`); }
        return;
      }
      const rec = e.target.closest('[data-recent]');
      if (rec) {
        const r = (state.recent || []).find(x => String(x.id) === rec.dataset.recent);
        if (r) openPick(r);
      }
    });
    $('#btnClearQueue').addEventListener('click', async () => {
      state.playback.queue = [];
      renderQueue();
      toast('Queue cleared');
      try { await api('/api/queue/clear', {}); } catch {}
    });

    // ── settings
    $('#btnSettings').addEventListener('click', openSheet);
    $('#btnMore').addEventListener('click', openSheet);
    $('#settingsRows').addEventListener('click', (e) => {
      if (e.target.closest('[data-invite-room]')) { closeSheet(); openInvite(); return; }
      if (e.target.closest('#btnEndSession')) { endVoiceChat(); return; }
      const sw = e.target.closest('[data-setting]');
      if (!sw) return;
      sw.classList.toggle('is-on');
      const s = SETTINGS.find(x => x.key === sw.dataset.setting);
      if (s) s.on = sw.classList.contains('is-on');
      if (s) state.settings[s.key] = s.on;
      haptic('light');
      api('/api/settings', { [sw.dataset.setting]: s.on }).then(r => { if (r) merge(r); }).catch(() => {});
    });

    // mini player -> song tab
    $('#mini').addEventListener('click', (e) => {
      if (e.target.closest('#miniPlay')) return;
      switchTab('song');
    });

    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') { closeSearch(); closeCtx(); closeSheet(); closeInvite(); closeDm(); closePick(); closeSocial(); }
    });
  }

  /* ────────────────────────── ticker ────────────────────────── */
  let sleepShown = -1;
  function tick() {
    const t = currentTrack();
    if (t && !state.playback.paused && !dragging) {
      localPos += 0.25;
      if (localPos >= t.duration) {
        localPos = t.duration;
        if (!online) skip();
      }
    }
    if (t) paintProgress();
    // Count the sleep timer down locally; the server sends the authoritative
    // value on every state frame.
    if (state.sleep > 0) {
      state.sleep = Math.max(0, state.sleep - 0.25);
      const s = Math.ceil(state.sleep);
      if (s !== sleepShown) { sleepShown = s; paintSleep(); }
    }
  }

  /* ────────────────────────── go ────────────────────────── */
  renderSettings();
  bind();
  boot();
  setInterval(tick, 250);
  setInterval(() => { if (online) api('/api/state').then(merge).then(() => queueRender()).catch(() => {}); }, 8000);

  // Background tabs: pause CSS animations and stop painting; on return do one
  // catch-up render and refresh state, so nothing looks stale.
  document.addEventListener('visibilitychange', () => {
    const hidden = document.hidden;
    document.documentElement.classList.toggle('app-hidden', hidden);
    if (!hidden) {
      if (renderDirty) renderAll();
      if (online) api('/api/state').then(merge).then(() => renderAll()).catch(() => {});
    }
  });
})();
