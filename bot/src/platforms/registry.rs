// registry
// (platform ordering, query resolution, download dispatch, autoplay tiers).

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::models::{PLATFORM_YOUTUBE, Track};
use crate::platforms::iota::{iota_media_id, IotaApi, ProgressFn};
use crate::platforms::yuki::Yuki;
use crate::platforms::youtube::YouTubePlatform;

pub use crate::platforms::artwork::artwork_local_path;

struct Registry {
    iota: IotaApi,
    yuki: Yuki,
    yt: YouTubePlatform,
}

// kept a global sorted vector; Rust replaces it wholesale on init so the
// configured bases can be swapped (tests rely on re-init).
static REG: Mutex<Option<Registry>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Registry>> {
    REG.lock().unwrap_or_else(|e| e.into_inner())
}

/// init_platforms: registers iota (95), yuki (85) and youtube.
pub fn init(cfg: &Config) {
    *lock() = Some(Registry {
        iota: IotaApi::new(cfg),
        yuki: Yuki::new(cfg),
        yt: YouTubePlatform::new(cfg.queue_limit),
    });
}

pub fn set_iota_base(base: &str) {
    if let Some(r) = lock().as_mut() {
        r.iota.set_base(base);
    }
}

pub fn set_yuki_base(base: &str) {
    if let Some(r) = lock().as_mut() {
        r.yuki.set_base(base);
    }
}

/// Test hook: redirect InnerTube calls for the registered YouTube platform.
pub fn set_youtube_base(base: &str) {
    if let Some(r) = lock().as_mut() {
        r.yt.set_base(base);
    }
}

/// find_for + resolve_query: a direct platform wins when it can_get the
/// query; otherwise YouTube search collapses to its first hit.
pub fn resolve_query(query: &str, video: bool) -> Vec<Track> {
    if YouTubePlatform::can_get(query) {
        if let Some(got) = lock().as_ref().map(|r| r.yt.get(query, video)) {
            if !got.is_empty() {
                return got;
            }
        }
    }
    let tracks = lock().as_ref().map(|r| r.yt.get(query, video)).unwrap_or_default();
    if tracks.is_empty() {
        return Vec::new();
    }
    vec![tracks[0].clone()]
}

pub fn search_tracks(query: &str, video: bool) -> Vec<Track> {
    lock().as_ref().map(|r| r.yt.get(query, video)).unwrap_or_default()
}

/// download_track: priority order, "<Platform>: <err>"
/// last-error strings, "no downloader for source: <src>" fallback.
pub fn download_track(
    track: &Track,
    err: &mut String,
    cancel: &AtomicBool,
    progress: &ProgressFn,
) -> String {
    let mut guard = lock();
    let Some(r) = guard.as_mut() else {
        *err = format!("no downloader for source: {}", track.source);
        return String::new();
    };
    let mut last = String::new();
    if r.iota.can_download(&track.source) {
        let mut e = String::new();
        let path = r.iota.download(track, &mut e, cancel, progress);
        if !path.is_empty() {
            return path;
        }
        last = format!("IotaApi: {e}");
        if e == "canceled" {
            *err = last;
            return String::new();
        }
    }
    if r.yuki.can_download(&track.source) {
        let mut e = String::new();
        let path = r.yuki.download(track, &mut e, cancel, progress);
        if !path.is_empty() {
            return path;
        }
        last = format!("Yuki: {e}");
        if e == "canceled" {
            *err = last;
            return String::new();
        }
    }
    *err = if last.is_empty() {
        format!("no downloader for source: {}", track.source)
    } else {
        last
    };
    String::new()
}

/// clean_song_title_for_search.
pub fn clean_song_title_for_search(title: &str) -> String {
    let mut t = title.to_string();
    for sep in ["|", "(", "["] {
        if let Some(p) = t.find(sep) {
            t.truncate(p);
        }
    }
    t.trim().to_string()
}

fn in_history(history: &[String], needle: &str) -> bool {
    history.iter().any(|h| h == needle)
}

/// autoplay_tracks: Tier 0 iota /similar, Tier 1 YT radio
/// mix, Tier 2 "<clean title> similar songs".
pub fn autoplay_tracks(last: &Track, limit: i64, history: &[String]) -> Vec<Track> {
    let mut filtered: Vec<Track> = Vec::new();
    let accept = |t: &Track| -> bool {
        if t.id.is_empty() && t.url.is_empty() {
            return false;
        }
        if !t.id.is_empty() && t.id == last.id {
            return false;
        }
        if !t.id.is_empty() && in_history(history, &format!("id:{}", t.id)) {
            return false;
        }
        let n = crate::models::normalize_track_title(&t.title);
        if !n.is_empty() && in_history(history, &format!("title:{n}")) {
            return false;
        }
        true
    };

    // Tier 0: iota /similar (excludes the input and itself)
    let media = iota_media_id(last);
    if !media.is_empty() {
        let similar = lock().as_ref().map(|r| r.iota.similar(&media, 8)).unwrap_or_default();
        for t in similar {
            if !t.id.is_empty() && t.id == media {
                continue;
            }
            if !accept(&t) {
                continue;
            }
            let mut c = t;
            c.video = last.video;
            filtered.push(c);
            if limit > 0 && filtered.len() as i64 >= limit {
                break;
            }
        }
    }

 // Tier 1: YouTube radio mix
    if filtered.is_empty() {
        let mut vid = String::new();
        if last.source == PLATFORM_YOUTUBE {
            vid = last.id.clone();
        } else if let Some(rest) = last.id.strip_prefix("yt:") {
            vid = rest.to_string();
        }
        if vid.len() == 11 {
            if let Some(mix) = lock().as_ref().map(|r| r.yt.mix_for(&vid, 15)) {
                for c in mix {
                    if !accept(&c) {
                        continue;
                    }
                    let mut x = c;
                    x.video = last.video;
                    filtered.push(x);
                }
            }
        }
    }

    // Tier 2: "<clean title> similar songs"
    if filtered.is_empty() {
        let query = format!("{} similar songs", clean_song_title_for_search(&last.title));
        for t in search_tracks(&query, last.video) {
            if !accept(&t) {
                continue;
            }
            let mut c = t;
            c.video = last.video;
            filtered.push(c);
            if limit > 0 && filtered.len() as i64 >= limit {
                break;
            }
        }
    }
    filtered
}

/// golden_zone_pick: random pick from the top 4.
pub fn golden_zone_pick(candidates: &[Track]) -> Option<Track> {
    let pool = candidates.len().min(4);
    if pool == 0 {
        return None;
    }
    let i = if pool == 1 {
        0
    } else {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        nanos % pool
    };
    Some(candidates[i].clone())
}
