// yuki
// (YouTube-only stream download endpoint).

use std::io::Read;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::parv_debug;
use crate::models::{PLATFORM_YOUTUBE, Track};

use super::iota::{escape_path, ProgressFn};

const UA: &str = "ParvatiMusic/yuki";
const MIN_FILE_BYTES: u64 = 10000;

fn sanitize(e: &str, key: &str) -> String {
    if key.is_empty() {
        return e.to_string();
    }
    e.replace(key, "***REDACTED***")
}

fn is_transient(code: u16) -> bool {
    code == 429 || code >= 500
}

fn media_dir(track: &Track) -> &'static str {
    if track.video { "video" } else { "audio" }
}

/// (shared naming with iota's find_file).
pub fn find_file(track: &Track) -> String {
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

pub struct Yuki {
    url: String,
    key: String,
}

impl Yuki {
    pub fn new(cfg: &Config) -> Yuki {
        Yuki {
            url: cfg.yuki_api_url.trim_end_matches('/').to_string(),
            key: cfg.yuki_api_key.clone(),
        }
    }

    pub fn active(&self) -> bool {
        !self.url.is_empty() && !self.key.is_empty()
    }

    /// Test hook: redirect the /stream endpoint (key preserved).
    pub fn set_base(&mut self, base: &str) {
        self.url = base.trim_end_matches('/').to_string();
    }

    pub fn can_download(&self, source: &str) -> bool {
        self.active() && source == PLATFORM_YOUTUBE
    }

    pub fn download(
        &self,
        track: &Track,
        err: &mut String,
        cancel: &AtomicBool,
        progress: &ProgressFn,
    ) -> String {
        let cached = find_file(track);
        if !cached.is_empty() {
            parv_debug!("Yuki", "cache hit {cached}");
            return cached;
        }
        let video_id = track.id.clone();
        if video_id.is_empty() {
            *err = "yuki: missing video id".into();
            return String::new();
        }
        let (stream_type, quality, ext, timeout) = if track.video {
            ("video", "480", ".mp4", 600u64)
        } else {
            ("audio", "128", ".mp3", 180u64)
        };
        let stream = format!(
            "{}/stream/{}?key={}&type={stream_type}&quality={quality}",
            self.url,
            escape_path(&video_id),
            self.key
        );
        let path = format!("downloads/{}_{}{}", media_dir(track), video_id, ext);
        let _ = std::fs::create_dir_all("downloads");

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
                    last = format!("yuki: cannot open {path}");
                    continue;
                }
            };
            let sent = client.get(&stream).header("User-Agent", UA).send();
            let Ok(resp) = sent else {
                let _ = std::fs::remove_file(&path);
                last = sanitize("yuki request failed: send error", &self.key);
                continue;
            };
            let status = resp.status().as_u16();
            if status >= 400 {
                drop(out);
                let _ = std::fs::remove_file(&path);
                last = sanitize(&format!("yuki returned {status}"), &self.key);
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
                            failed = Some("yuki: write failed".to_string());
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
                        failed = Some("yuki request failed: read error".to_string());
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
                last = "yuki returned an invalid or empty file".into();
                continue;
            }
            return path;
        }
        *err = last;
        String::new()
    }
}
