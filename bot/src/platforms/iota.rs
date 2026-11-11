// iota
// (search-only platform with a download endpoint; error strings redact the key).

use std::io::Read;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::{parv_debug, parv_warn};
use crate::models::{PLATFORM_DEEZER, PLATFORM_IOTA_API, PLATFORM_SPOTIFY, PLATFORM_YOUTUBE, Track};

const UA: &str = "ParvatiMusic/iota";
const MIN_FILE_BYTES: u64 = 10000;

pub type ProgressFn = dyn Fn(f64, f64, i64, f64) + Send + Sync;

/// prefix by source, bare for IotaApi, empty otherwise
pub fn iota_media_id(t: &Track) -> String {
    if t.id.is_empty() {
        return String::new();
    }
    // Catalogue rows already carry their provider prefix (`dz:…`, `am:…`).
    if t.id.contains(':') {
        return t.id.clone();
    }
    if t.source == PLATFORM_YOUTUBE {
        format!("yt:{}", t.id)
    } else if t.source == PLATFORM_SPOTIFY {
        format!("sp:{}", t.id)
    } else if t.source == PLATFORM_DEEZER {
        format!("dz:{}", t.id)
    } else if t.source == PLATFORM_IOTA_API {
        t.id.clone()
    } else {
        String::new()
    }
}

/// curl_easy_escape parity: percent-encode everything but unreserved chars.
pub fn escape_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn sanitize(e: &str, key: &str) -> String {
    if key.is_empty() {
        return e.to_string();
    }
    e.replace(key, "***REDACTED***")
}

fn is_transient(code: u16) -> bool {
    code == 429 || code == 502 || code == 503 || code == 504 || code >= 500
}

/// Human label for a catalogue id prefix, so a search row reads "Deezer" or
/// "Apple Music" instead of the internal backend name.
pub fn provider_label(id: &str) -> &'static str {
    match id.split_once(':').map(|(p, _)| p).unwrap_or("") {
        "dz" => "Deezer",
        "am" => "Apple Music",
        "sp" => "Spotify",
        "mb" => "MusicBrainz",
        "sc" => "SoundCloud",
        "yt" => PLATFORM_YOUTUBE,
        _ => PLATFORM_IOTA_API,
    }
}

fn media_dir(track: &Track) -> &'static str {
    if track.video { "video" } else { "audio" }
}

/// any nonempty downloads/{kind}_{id}* counts as cache hit
fn find_file(track: &Track) -> String {
    let prefix = format!("{}_{}", media_dir(track), track.id);
    let Ok(entries) = std::fs::read_dir("downloads") else {
        return String::new();
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // The name is `audio_<id><ext>`; requiring the dot after the id stops
        // one id from matching another that merely starts with it.
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        // `.visual` is a legacy cover-video marker and `.part` an
        // unfinished download — neither is playable media.
        if !rest.starts_with('.') || rest == ".visual" || rest == ".part" {
            continue;
        }
        if let Ok(md) = e.metadata() {
            if md.len() > 0 {
                return e.path().to_string_lossy().into_owned();
            }
        }
    }
    String::new()
}

pub struct IotaApi {
    /// Backends in priority order: the real Iota API first, then any local
    /// mirror that speaks the same `/download/{media_id}` contract. A play
    /// walks the list instead of failing when one backend cannot reach its
    /// upstream (YouTube refuses whole datacentre ranges).
    bases: Vec<String>,
    key: String,
    http: reqwest::blocking::Client,
    /// base -> unix second until which it is skipped after repeated failures.
    dead: std::sync::Mutex<Vec<(String, u64)>>,
}

const BREAKER_SECONDS: u64 = 60;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl IotaApi {
    pub fn new(cfg: &Config) -> IotaApi {
        let mut bases: Vec<String> = Vec::new();
        for raw in std::iter::once(cfg.iota_api_url.as_str())
            .chain(cfg.iota_api_fallback.iter().map(|s| s.as_str()))
        {
            let b = raw.trim().trim_end_matches('/').to_string();
            if !b.is_empty() && !bases.contains(&b) {
                bases.push(b);
            }
        }
        IotaApi {
            bases,
            key: cfg.iota_api_key.clone(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
            dead: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn active(&self) -> bool {
        !self.bases.is_empty() && !self.key.is_empty()
    }

    /// Bases to try right now: healthy ones first, dead ones only when every
    /// base is cooling down (a stale breaker must not disable playback).
    fn live_bases(&self) -> Vec<String> {
        let now = now_secs();
        let dead = self.dead.lock().unwrap_or_else(|e| e.into_inner());
        let alive: Vec<String> = self
            .bases
            .iter()
            .filter(|b| !dead.iter().any(|(d, until)| d == *b && *until > now))
            .cloned()
            .collect();
        if alive.is_empty() {
            self.bases.clone()
        } else {
            alive
        }
    }

    fn mark_dead(&self, base: &str) {
        let until = now_secs() + BREAKER_SECONDS;
        let mut dead = self.dead.lock().unwrap_or_else(|e| e.into_inner());
        match dead.iter_mut().find(|(d, _)| d == base) {
            Some(slot) => slot.1 = until,
            None => dead.push((base.to_string(), until)),
        }
        parv_warn!("iota", "backend cooling down for {}s: {base}", BREAKER_SECONDS);
    }

    fn mark_alive(&self, base: &str) {
        let mut dead = self.dead.lock().unwrap_or_else(|e| e.into_inner());
        dead.retain(|(d, _)| d != base);
    }

    /// Test hook: redirect the /similar and /download endpoints (key preserved).
    pub fn set_base(&mut self, base: &str) {
        let b = base.trim_end_matches('/').to_string();
        self.bases = if b.is_empty() { Vec::new() } else { vec![b] };
    }

    pub fn can_download(&self, source: &str) -> bool {
        if !self.active() {
            return false;
        }
        source == PLATFORM_YOUTUBE
            || source == PLATFORM_SPOTIFY
            || source == PLATFORM_DEEZER
            || source == PLATFORM_IOTA_API
            || matches!(source, "Deezer" | "Apple Music" | "MusicBrainz" | "SoundCloud")
    }

    /// Path of an already-downloaded copy, empty when there is none. Used to
    /// answer a replay instantly instead of re-fetching the same audio.
    pub fn cached_path(&self, track: &Track) -> String {
        find_file(track)
    }

    /// Catalogue search. Returns tracks whose ids are bare Iota media ids
    /// (`dz:123`, `yt:abc`), so `download` can hand them straight back to the
    /// backend without a second resolution step.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Track> {
        if !self.active() || query.trim().is_empty() {
            return Vec::new();
        }
        let mut limit = limit as i64;
        if limit <= 0 || limit > 25 {
            limit = 10;
        }
        let mut out = Vec::new();
        for base in self.live_bases() {
            let url = format!(
                "{}/search?q={}&limit={limit}&key={}",
                base,
                escape_path(query.trim()),
                self.key
            );
            let Ok(resp) = self
                .http
                .get(&url)
                .header("User-Agent", UA)
                .header("X-API-Key", &self.key)
                .send()
            else {
                self.mark_dead(&base);
                continue;
            };
            if resp.status().as_u16() >= 400 {
                self.mark_dead(&base);
                continue;
            }
            self.mark_alive(&base);
            let Ok(body) = resp.text() else { return Vec::new() };
            let Ok(j) = serde_json::from_str::<serde_json::Value>(&body) else {
                parv_warn!("iota", "search: parse failed: {}", &body[..body.len().min(120)]);
                return Vec::new();
            };
            for item in j.get("results").and_then(|r| r.as_array()).into_iter().flatten() {
                let Some(item) = item.as_object() else { continue };
                let id = item.get("media_id").and_then(|v| v.as_str()).unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                let mut title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let artist = item.get("artist").and_then(|v| v.as_str()).unwrap_or("");
                if !artist.is_empty() && !title.is_empty() {
                    title = format!("{title} - {artist}");
                }
                out.push(Track {
                    id: id.to_string(),
                    title,
                    duration: item.get("duration_seconds").and_then(|v| v.as_i64()).unwrap_or(0),
                    artwork: item.get("thumbnail_url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    source: provider_label(id).to_string(),
                    ..Default::default()
                });
            }
            break;
        }
        out
    }

 /// (:similar endpoint).
    pub fn similar(&self, media_id: &str, limit: usize) -> Vec<Track> {
        if !self.active() || media_id.is_empty() {
            return Vec::new();
        }
        let mut limit = limit as i64;
        if limit <= 0 || limit > 20 {
            limit = 8;
        }
        let mut resp = None;
        for base in self.live_bases() {
            let url = format!(
                "{}/similar/{}?key={}&limit={limit}",
                base,
                escape_path(media_id),
                self.key
            );
            let Ok(r) = self
                .http
                .get(&url)
                .header("User-Agent", UA)
                .header("X-API-Key", &self.key)
                .send()
            else {
                self.mark_dead(&base);
                continue;
            };
            if r.status().as_u16() >= 400 {
                self.mark_dead(&base);
                continue;
            }
            self.mark_alive(&base);
            resp = Some(r);
            break;
        }
        let Some(resp) = resp else {
            return Vec::new();
        };
        let Ok(body) = resp.text() else {
            return Vec::new();
        };
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&body) else {
            parv_warn!("iota", "similar: parse failed for body prefix: {}", &body[..body.len().min(120)]);
            return Vec::new();
        };
        if !j.is_object() {
            parv_warn!("iota", "similar: non-object response");
            return Vec::new();
        }
        let mut out = Vec::new();
        for item in j.get("results").and_then(|r| r.as_array()).into_iter().flatten() {
            let Some(item) = item.as_object() else { continue };
            let id = item.get("media_id").and_then(|v| v.as_str()).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let mut title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let artist = item.get("artist").and_then(|v| v.as_str()).unwrap_or("");
            if !artist.is_empty() && !title.is_empty() {
                title = format!("{title} - {artist}");
            }
            out.push(Track {
                id: id.to_string(),
                title,
                duration: item.get("duration_seconds").and_then(|v| v.as_i64()).unwrap_or(0),
                artwork: item.get("thumbnail_url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                source: PLATFORM_IOTA_API.to_string(),
                ..Default::default()
            });
        }
        out
    }

 /// (download with 3 attempts, 429/5xx retry, key redaction).
    pub fn download(
        &self,
        track: &Track,
        err: &mut String,
        cancel: &AtomicBool,
        progress: &ProgressFn,
    ) -> String {
        let cached = find_file(track);
        if !cached.is_empty() {
            parv_debug!("IotaApi", "cache hit {cached}");
            return cached;
        }
        let media_id = iota_media_id(track);
        if media_id.is_empty() {
            *err = "iotaapi: missing or unsupported media id".into();
            return String::new();
        }        // Audio is delivered as MP3 (192 kbps from the downloader), so the
        // extension matches the bytes and browsers can seek in it.
        let (kind, ext, timeout) = if track.video {
            ("video", ".mp4", 600u64)
        } else {
            ("audio", ".mp3", 180u64)
        };
        let path = format!("downloads/{}_{}{}", media_dir(track), track.id, ext);
        let _ = std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new(".")));
        let mut query = format!("/download/{}?kind={kind}&key={}", escape_path(&media_id), self.key);
        if kind == "video" {
            query.push_str("&max_height=720");
        }
        // The title lets a local downloader fall back to another catalogue
        // (SoundCloud) when YouTube refuses this host's IP.
        if !track.title.is_empty() {
            query.push_str("&q=");
            query.push_str(&escape_path(&track.title));
        }
        // Ask for a high-quality source: 192 kbps is the floor the app plays at.
        if kind == "audio" {
            query.push_str("&format=mp3&bitrate=192");
        }

        let mut last = String::new();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .unwrap_or_default();
        let mut bases = self.live_bases();
        if media_id.starts_with("yt:") && bases.len() > 1 {
            // The local downloader (last base) resolves the exact YouTube id
            // itself: for audio it can fall back to SoundCloud when YouTube
            // refuses this host, and for video it merges the 720p H.264 stream
            // with its audio. It has no upstream relay in between, so try it
            // first and keep the remote API as the second chance.
            let local = bases.remove(bases.len() - 1);
            bases.insert(0, local);
        }
        for base in bases {
            let stream = format!("{base}{query}");
            match self.download_from(&client, &base, &stream, &path, cancel, progress, &mut last) {
                Ok(p) => {
                    self.mark_alive(&base);
                    return p;
                }
                Err(true) => {
                    // "canceled" — the caller asked to stop, do not try more.
                    *err = "canceled".into();
                    return String::new();
                }
                Err(false) => {
                    // "502" from a backend means the *upstream* (YouTube,
                    // SoundCloud) refused — the backend itself is fine. Cooling
                    // it down would disable the only working download path for
                    // minutes, so only real connectivity failures trip it.
                    if !last.contains("returned 5") {
                        self.mark_dead(&base);
                    }
                    continue;
                }
            }
        }
        *err = last;
        return String::new();
    }

    /// One backend, up to three attempts. `Err(true)` means the user canceled.
    #[allow(clippy::too_many_arguments)]
    fn download_from(
        &self,
        client: &reqwest::blocking::Client,
        base: &str,
        stream: &str,
        path: &str,
        cancel: &AtomicBool,
        progress: &ProgressFn,
        last: &mut String,
    ) -> Result<String, bool> {
        for attempt in 1..=3 {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(true);
            }
            if attempt > 1 {
                std::thread::sleep(Duration::from_secs(1));
            }
            let mut out = match std::fs::File::create(path) {
                Ok(f) => f,
                Err(_) => {
                    *last = format!("iotaapi: cannot open {path}");
                    continue;
                }
            };
            let sent = client
                .get(stream)
                .header("User-Agent", UA)
                .header("X-API-Key", &self.key)
                .send();
            let Ok(resp) = sent else {
                let _ = std::fs::remove_file(path);
                *last = sanitize("iotaapi request failed: send error", &self.key);
                break;
            };
            let status = resp.status().as_u16();
            if status >= 400 {
                drop(out);
                let _ = std::fs::remove_file(path);
                *last = sanitize(&format!("{base} returned {status}"), &self.key);
                // A 4xx will not get better by retrying this backend, but the
                // next backend may still serve the same song. A 502 means the
                // backend already exhausted its own ladder (player clients,
                // then SoundCloud), so retrying here just triples the wait.
                if status == 502 || !is_transient(status) {
                    break;
                }
                continue;
            }
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                drop(out);
                let _ = std::fs::remove_file(path);
                return Err(true);
            }
            let total = resp.content_length().unwrap_or(0) as f64;
            let mut reader = resp;
            let mut buf = [0u8; 65536];
            let start = Instant::now();
            let mut downloaded: f64 = 0.0;
            let mut last_size: f64 = 0.0;
            let mut last_time = start;
            let mut failed = None;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                            failed = Some("canceled".to_string());
                            break;
                        }
                        if std::io::Write::write_all(&mut out, &buf[..n]).is_err() {
                            failed = Some("iotaapi: write failed".to_string());
                            break;
                        }
                        downloaded += n as f64;
                        let now = Instant::now();
                        let elapsed = now.duration_since(start).as_secs_f64();
                        let interval = now.duration_since(last_time).as_secs_f64();
                        let speed = if interval > 0.0 && downloaded >= last_size {
                            (downloaded - last_size) / interval
                        } else {
                            0.0
                        };
                        last_size = downloaded;
                        last_time = now;
                        let pct = if total > 0.0 { downloaded / total * 100.0 } else { 0.0 };
                        let eta = if speed > 0.0 && total > downloaded {
                            ((total - downloaded) / speed) as i64
                        } else {
                            0
                        };
                        progress(pct, speed, eta, elapsed);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        failed = Some("iotaapi request failed: read error".to_string());
                        break;
                    }
                }
            }
            drop(out);
            if let Some(e) = failed {
                let _ = std::fs::remove_file(path);
                if e == "canceled" {
                    return Err(true);
                }
                *last = sanitize(&e, &self.key);
                continue;
            }
            let sz = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if sz < MIN_FILE_BYTES {
                let _ = std::fs::remove_file(path);
                *last = "iotaapi returned an invalid or empty file".into();
                continue;
            }
            return Ok(path.to_string());
        }
        Err(false)
    }

}
