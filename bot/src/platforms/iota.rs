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
        if name.starts_with(&prefix) {
            if let Ok(md) = e.metadata() {
                if md.len() > 0 {
                    return e.path().to_string_lossy().into_owned();
                }
            }
        }
    }
    String::new()
}

pub struct IotaApi {
    url: String,
    key: String,
    http: reqwest::blocking::Client,
}

impl IotaApi {
    pub fn new(cfg: &Config) -> IotaApi {
        IotaApi {
            url: cfg.iota_api_url.trim_end_matches('/').to_string(),
            key: cfg.iota_api_key.clone(),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn active(&self) -> bool {
        !self.url.is_empty() && !self.key.is_empty()
    }

    /// Test hook: redirect the /similar and /download endpoints (key preserved).
    pub fn set_base(&mut self, base: &str) {
        self.url = base.trim_end_matches('/').to_string();
    }

    pub fn can_download(&self, source: &str) -> bool {
        if !self.active() {
            return false;
        }
        source == PLATFORM_YOUTUBE
            || source == PLATFORM_SPOTIFY
            || source == PLATFORM_DEEZER
            || source == PLATFORM_IOTA_API
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
        let url = format!(
            "{}/similar/{}?key={}&limit={limit}",
            self.url,
            escape_path(media_id),
            self.key
        );
        let Ok(resp) = self
            .http
            .get(&url)
            .header("User-Agent", UA)
            .header("X-API-Key", &self.key)
            .send()
        else {
            return Vec::new();
        };
        let status = resp.status().as_u16();
        if status >= 400 {
            return Vec::new();
        }
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
        }
        let (kind, ext, timeout) = if track.video {
            ("video", ".mp4", 600u64)
        } else {
            ("audio", ".m4a", 180u64)
        };
        let path = format!("downloads/{}_{}{}", media_dir(track), track.id, ext);
        let _ = std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new(".")));
        let mut stream = format!(
            "{}/download/{}?kind={kind}&key={}",
            self.url,
            escape_path(&media_id),
            self.key
        );
        if kind == "video" {
            stream.push_str("&max_height=1080");
        }

        let mut last = String::new();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .build()
            .unwrap_or_default();
        for attempt in 1..=3 {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                *err = "canceled".into();
                return String::new();
            }
            if attempt > 1 {
                std::thread::sleep(Duration::from_secs(1));
            }
            let mut out = match std::fs::File::create(&path) {
                Ok(f) => f,
                Err(_) => {
                    last = format!("iotaapi: cannot open {path}");
                    continue;
                }
            };
            let sent = client
                .get(&stream)
                .header("User-Agent", UA)
                .header("X-API-Key", &self.key)
                .send();
            let Ok(resp) = sent else {
                let _ = std::fs::remove_file(&path);
                last = sanitize("iotaapi request failed: send error", &self.key);
                continue;
            };
            let status = resp.status().as_u16();
            if status >= 400 {
                drop(out);
                let _ = std::fs::remove_file(&path);
                last = sanitize(&format!("iotaapi returned {status}"), &self.key);
                if !is_transient(status) {
                    *err = last;
                    return String::new();
                }
                continue;
            }
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                drop(out);
                let _ = std::fs::remove_file(&path);
                *err = "canceled".into();
                return String::new();
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
                let _ = std::fs::remove_file(&path);
                if e == "canceled" {
                    *err = "canceled".into();
                    return String::new();
                }
                last = sanitize(&e, &self.key);
                continue;
            }
            let sz = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if sz < MIN_FILE_BYTES {
                let _ = std::fs::remove_file(&path);
                last = "iotaapi returned an invalid or empty file".into();
                continue;
            }
            return path;
        }
        *err = last;
        String::new()
    }
}
