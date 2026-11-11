#!/usr/bin/env node
/**
 * Parvati Music — Mini App dev server
 *
 * Zero-dependency HTTP server that serves the mini app and the exact JSON
 * API the Rust bot exposes (`bot/src/webapp.rs`). Use it to preview the UI
 * without Telegram, or as a reverse proxy in front of a running bot:
 *
 *   node server.mjs                      # demo room, in-memory
 *   node server.mjs --port 8080
 *   node server.mjs --proxy http://127.0.0.1:8000   # talk to the real bot
 */
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const arg = (k, d) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : d; };
const PORT = Number(arg('--port', process.env.PORT || 8787));
const PROXY = arg('--proxy', process.env.BOT_API || '');
const TOKEN = arg('--token', process.env.MINIAPP_TOKEN || '');

/* ───────────────────────── demo room ───────────────────────── */
const IMG = ['khat', 'stage', 'neon', 'crowd', 'dj', 'guitar'].map(n => `assets/img/cover-${n}.jpg`);
const T = (id, title, dur, art, src = 'YouTube', req = 'stalker') =>
  ({ id, title, artist: title.split(' - ')[0], duration: dur, art, requester: req, source: src });

const CATALOG = [
  T('yt:khat', 'Navjot Ahuja - Khat (Official Audio)', 297, IMG[0], 'YouTube', '– ѕ т α ℓ к є я –'),
  T('yt:khat-lyr', 'Navjot Ahuja - Khat (Lyrics) | Tere Liye', 297, IMG[1]),
  T('yt:khat-2', 'Navjot Ahuja - Khat (Lyrics)', 297, IMG[2]),
  T('yt:khat-slowed', 'Khat - Slowed + Reverb', 322, IMG[3]),
  T('yt:khat-live', 'Khat (Live at Mumbai)', 341, IMG[4]),
  T('sp:tere-liye', 'Tere Liye - Acoustic', 254, IMG[5], 'Spotify'),
  T('yt:until', 'Stephen Sanchez - Until I Found You', 181, IMG[3], 'YouTube'),
  T('yt:kesariya', 'Kesariya - Brahmastra', 268, IMG[4], 'YouTube'),
  T('yt:lofi', 'lofi hip hop radio - beats to relax/study to', 3600, IMG[2], 'YouTube'),
  T('yt:apna', 'Apna Bana Le - Bhediya', 261, IMG[1], 'YouTube'),
  T('yt:rahim', 'Rahim - Arijit Singh', 289, IMG[0], 'YouTube'),
  T('yt:tum-hi', 'Tum Hi Ho - Aashiqui 2', 262, IMG[5], 'YouTube'),
  T('sp:night', 'Night Changes - One Direction', 226, IMG[3], 'Spotify'),
  T('yt:shape', 'Ed Sheeran - Shape of You', 233, IMG[4], 'YouTube'),
  T('yt:believer', 'Imagine Dragons - Believer', 204, IMG[2], 'YouTube'),
  T('yt:blinding', 'The Weeknd - Blinding Lights', 200, IMG[1], 'YouTube'),
];

const state = {
  room: { id: -1001789, title: 'chingu gang', members: 2, online: 1, live: true },
  me: {
    id: '42', name: '– ѕ т α ℓ к є я –', username: 'stalker', photo: 'assets/img/av-me.jpg',
    level: 4, levelPct: 62, toNext: '1h 0m', listening: '4h 59m', songs: 125, liked: 12,
  },
  quote: "I can't hate anyone, I just know where not to return.",
  playback: {
    track: CATALOG[0],
    position: 43,
    paused: false,
    loop: 0,
    shuffle: false,
    queue: [CATALOG[1], CATALOG[2], CATALOG[6]],
  },
  members: [
    { id: '42', name: '– ѕ т α ℓ к є я –', username: 'stalker', photo: 'assets/img/av-me.jpg', role: 'owner', me: true },
    { id: '77', name: 'stalker', username: 'stalker', photo: 'assets/img/av-2.jpg', role: 'member', me: false },
  ],
  messages: [
    { id: 'm1', from: '42', name: 'you', mine: true, kind: 'text', text: 'YO', ts: '16:32' },
    { id: 'm2', from: '42', name: 'you', mine: true, kind: 'voice', duration: 5, ts: '16:33' },
    { id: 'm3', from: '77', name: 'stalker', mine: false, kind: 'text', text: 'stickers tool!', ts: '16:34' },
    { id: 'm4', from: '77', name: 'stalker', mine: false, kind: 'photo', art: IMG[3], ts: '16:50' },
    { id: 'm5', from: '42', name: 'you', mine: true, kind: 'text', text: 'yesss', ts: '16:52' },
    { id: 'm6', from: '42', name: 'you', mine: true, kind: 'text', text: 'yes', ts: '21:27', reply: { name: 'stalker', text: 'stickers tool!' } },
  ],
};

const settings = { autoplay: true, loop: false, haptics: true, notify: true };

/* ───────────────────────── sse ───────────────────────── */
const clients = new Set();
const broadcast = (type, data) => {
  const frame = `data: ${JSON.stringify({ type, data })}\n\n`;
  for (const res of clients) { try { res.write(frame); } catch {} }
};

/* ───────────────────────── playback loop ───────────────────────── */
setInterval(() => {
  const p = state.playback;
  if (!p.track || p.paused) return;
  p.position += 1;
  if (p.position >= p.track.duration) {
    if (p.loop === 2) { p.position = 0; }
    else {
      const next = p.queue.shift();
      if (next) { p.track = next; p.position = 0; }
      else if (p.loop === 1 && state.messages.length) { p.position = 0; }
      else { p.paused = true; p.position = p.track.duration; }
    }
    broadcast('state', state);
  }
}, 1000);

setInterval(() => broadcast('state', state), 1500);

/* ───────────────────────── http helpers ───────────────────────── */
const MIME = {
  '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8', '.json': 'application/json; charset=utf-8',
  '.jpg': 'image/jpeg', '.png': 'image/png', '.svg': 'image/svg+xml',
  '.woff2': 'font/woff2', '.mp4': 'video/mp4', '.ico': 'image/x-icon',
};

const json = (res, code, body) => {
  const buf = Buffer.from(JSON.stringify(body));
  res.writeHead(code, { 'content-type': 'application/json; charset=utf-8', 'content-length': buf.length, 'cache-control': 'no-store' });
  res.end(buf);
};

const readBody = (req) => new Promise((resolve) => {
  let raw = '';
  req.on('data', c => { raw += c; if (raw.length > 1e6) req.destroy(); });
  req.on('end', () => { try { resolve(JSON.parse(raw || '{}')); } catch { resolve({}); } });
});

const search = (q) => {
  const k = q.toLowerCase().trim();
  if (!k) return [];
  return CATALOG
    .map(t => ({ t, score: (t.title.toLowerCase().includes(k) ? 10 : 0) + (t.artist.toLowerCase().includes(k) ? 4 : 0) }))
    .filter(x => x.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, 12)
    .map(x => x.t);
};

const stamp = () => {
  const d = new Date();
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
};

/* ───────────────────────── api ───────────────────────── */
async function handleApi(req, res, url) {
  if (TOKEN) {
    const given = url.searchParams.get('token') || req.headers['x-miniapp-token'];
    if (given !== TOKEN) return json(res, 401, { error: 'unauthorized' });
  }
  if (PROXY) return proxy(req, res, url);

  const p = url.pathname;

  if (req.method === 'GET' && p === '/api/state') return json(res, 200, state);

  if (req.method === 'GET' && p === '/api/search') {
    return json(res, 200, { results: search(url.searchParams.get('q') || '') });
  }

  if (req.method === 'GET' && p === '/api/events') {
    res.writeHead(200, {
      'content-type': 'text/event-stream', 'cache-control': 'no-cache',
      connection: 'keep-alive', 'x-accel-buffering': 'no',
    });
    res.write(`data: ${JSON.stringify({ type: 'state', data: state })}\n\n`);
    clients.add(res);
    const ping = setInterval(() => { try { res.write(': ping\n\n'); } catch {} }, 20000);
    req.on('close', () => { clearInterval(ping); clients.delete(res); });
    return;
  }

  if (req.method !== 'POST') return json(res, 404, { error: 'not found' });
  const body = await readBody(req);
  const pb = state.playback;

  switch (p) {
    case '/api/toggle':
      pb.paused = !pb.paused;
      break;
    case '/api/play': {
      const t = body.id ? CATALOG.find(x => x.id === body.id) : search(body.query || '')[0];
      if (!t) return json(res, 404, { error: 'no match' });
      if (pb.track) pb.queue.unshift(pb.track);
      pb.track = t; pb.position = 0; pb.paused = false;
      break;
    }
    case '/api/skip': {
      const next = pb.queue.shift();
      if (next) { pb.track = next; pb.position = 0; pb.paused = false; }
      else { pb.position = pb.track?.duration || 0; pb.paused = true; }
      break;
    }
    case '/api/seek':
      pb.position = Math.max(0, Math.min(body.position || 0, pb.track?.duration || 0));
      break;
    case '/api/loop':
      pb.loop = body.mode ?? 0;
      break;
    case '/api/queue/add': {
      const t = CATALOG.find(x => x.id === body.id);
      if (!t) return json(res, 404, { error: 'unknown track' });
      pb.queue.push(t);
      break;
    }
    case '/api/queue/remove':
      pb.queue.splice(body.index, 1);
      break;
    case '/api/queue/clear':
      pb.queue = [];
      break;
    case '/api/chat': {
      const msg = {
        id: `s${Date.now()}`,
        from: state.me.id,
        name: 'you',
        mine: true,
        kind: body.kind || 'text',
        text: body.text || '',
        ts: stamp(),
        reply: body.replyTo ? { name: body.replyTo, text: '…' } : null,
      };
      state.messages.push(msg);
      broadcast('messages', state.messages);
      return json(res, 200, { ok: true, message: msg });
    }
    case '/api/members': {
      const m = state.members.find(x => String(x.id) === String(body.id));
      if (!m) return json(res, 404, { error: 'unknown member' });
      if (body.action === 'promote') m.role = m.role === 'admin' ? 'member' : 'admin';
      else {
        state.members = state.members.filter(x => x !== m);
        state.room.members = state.members.length;
      }
      break;
    }
    case '/api/settings':
      Object.assign(settings, body);
      break;
    default:
      return json(res, 404, { error: 'not found' });
  }

  broadcast('state', state);
  return json(res, 200, state);
}

/* ───────────────────────── proxy to the real bot ───────────────────────── */
async function proxy(req, res, url) {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const upstream = await fetch(PROXY + url.pathname + url.search, {
    method: req.method,
    headers: { 'content-type': req.headers['content-type'] || 'application/json', accept: req.headers.accept || '*/*' },
    body: req.method === 'POST' ? Buffer.concat(chunks) : undefined,
  });
  res.writeHead(upstream.status, {
    'content-type': upstream.headers.get('content-type') || 'application/json',
    'cache-control': 'no-store',
  });
  if (upstream.body) for await (const c of upstream.body) res.write(c);
  res.end();
}

/* ───────────────────────── static ───────────────────────── */
async function serveStatic(req, res, url) {
  let rel = decodeURIComponent(url.pathname);
  if (rel === '/' || rel === '') rel = '/index.html';
  const file = normalize(join(ROOT, rel));
  if (!file.startsWith(ROOT)) { res.writeHead(403).end('forbidden'); return; }
  try {
    const info = await stat(file);
    if (info.isDirectory()) throw new Error('dir');
    const buf = await readFile(file);
    res.writeHead(200, {
      'content-type': MIME[extname(file)] || 'application/octet-stream',
      'content-length': buf.length,
      'cache-control': extname(file) === '.html' ? 'no-cache' : 'public, max-age=3600',
    });
    res.end(buf);
  } catch {
    res.writeHead(404, { 'content-type': 'text/plain' }).end('not found');
  }
}

/* ───────────────────────── server ───────────────────────── */
const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`);
  try {
    if (url.pathname.startsWith('/api/')) return await handleApi(req, res, url);
    return await serveStatic(req, res, url);
  } catch (err) {
    console.error(err);
    if (!res.headersSent) json(res, 500, { error: 'internal' });
    else res.end();
  }
});

server.listen(PORT, '0.0.0.0', () => {
  console.log(`\n  Parvati Music mini app`);
  console.log(`  → http://localhost:${PORT}${PROXY ? `   (proxying ${PROXY})` : '   (demo room)'}`);
  if (TOKEN) console.log(`  → token required: ?token=${TOKEN}\n`); else console.log('');
});
