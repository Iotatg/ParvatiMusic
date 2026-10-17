// youtube — InnerTube WEB client.

use serde_json::{Value, json};

use crate::models::{PLATFORM_YOUTUBE, Track};

pub const INNER_TUBE_KEY: &str = "AIzaSyBOti4mM-6x9WDnZIjIeyEU21OpBXqWBgw";
pub const INNER_TUBE_VER: &str = "2.20250101.01.00";
const INNER_TUBE_NAME: &str = "WEB";
const MOBILE_UA: &str = "Mozilla/5.0 (Linux; Android 13) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Mobile Safari/537.36";
const DEFAULT_BASE: &str = "https://m.youtube.com";

pub struct YouTubePlatform {
    limit: i64,
    base: String,
    http: reqwest::blocking::Client,
}

/// colon-separated, right-to-left place values
pub fn parse_duration(s: &str) -> i64 {
    let mut total: i64 = 0;
    let mut mult: i64 = 1;
    let mut parts: Vec<i64> = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch == ':' {
            parts.push(cur.trim().parse().unwrap_or(0));
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        parts.push(cur.trim().parse().unwrap_or(0));
    }
    for p in parts.into_iter().rev() {
        total += p * mult;
        mult *= 60;
    }
    total
}

fn jstr(j: &Value) -> String {
    match j {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// walk a path of keys / array indices, Null on miss
fn dig<'a>(cur: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = cur;
    for k in path {
        match cur {
            Value::Object(map) => cur = map.get(*k)?,
            Value::Array(arr) => {
                let i: usize = k.parse().ok()?;
                cur = arr.get(i)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

fn as_str_at(v: Option<&Value>) -> String {
    v.map(jstr).unwrap_or_default()
}

/// last thumbnail of `thumbnail` or `videoDetails.thumbnail`
fn thumb_url(vid: &Value) -> String {
    let thumbs = dig(vid, &["thumbnail", "thumbnails"])
        .or_else(|| dig(vid, &["videoDetails", "thumbnail", "thumbnails"]));
    match thumbs {
        Some(Value::Array(arr)) if !arr.is_empty() => as_str_at(arr.last().and_then(|t| dig(t, &["url"]))),
        _ => String::new(),
    }
}

/// live badge OR "watching" in viewCountText.runs[0]
fn is_live(vid: &Value) -> bool {
    if let Some(Value::Array(badges)) = vid.get("badges") {
        for b in badges {
            let style = as_str_at(dig(b, &["metadataBadgeRenderer", "style"]));
            if style == "BADGE_STYLE_TYPE_LIVE_NOW" {
                return true;
            }
        }
    }
    let vc = as_str_at(dig(vid, &["viewCountText", "runs", "0", "text"]));
    vc.to_lowercase().contains("watching")
}

/// (scheme)?(www.|m.|music.)?(youtube.com|youtu.be)/
fn looks_like_yt_link(q: &str) -> bool {
    let q = q.trim();
    let rest = q
        .strip_prefix("https://")
        .or_else(|| q.strip_prefix("http://"))
        .unwrap_or(q);
    let rest = rest
        .strip_prefix("www.")
        .or_else(|| rest.strip_prefix("m."))
        .or_else(|| rest.strip_prefix("music."))
        .unwrap_or(rest);
    for host in ["youtube.com/", "youtu.be/"] {
        if let Some(r) = rest.strip_prefix(host) {
            return !r.is_empty();
        }
        if rest.starts_with(host) {
            return true;
        }
    }
    false
}

/// watch?v=|embed/|shorts/|live/ + youtu.be/ → 11-char id
fn extract_video(u: &str) -> String {
    if let Some(pos) = u.find("youtu.be/") {
        let after = &u[pos + "youtu.be/".len()..];
        let id: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if id.len() == 11 {
            return id;
        }
    }
    for marker in ["youtube.com/watch?v=", "youtube.com/embed/", "youtube.com/shorts/", "youtube.com/live/"] {
        if let Some(pos) = u.find(marker) {
            let after = &u[pos + marker.len()..];
            let id: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if id.len() == 11 {
                return id;
            }
        }
    }
 // re_vid2 fallback: (?:v=|/) 11 chars — emulate via v= or a path segment.
    if let Some(pos) = u.find("v=") {
        let after = &u[pos + 2..];
        let id: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if id.len() == 11 {
            return id;
        }
    }
    String::new()
}

/// (?|&)list=([A-Za-z0-9_-]+)
fn extract_playlist(u: &str) -> String {
    if let Some(pos) = u.find("list=") {
        let after = &u[pos + "list=".len()..];
        return after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
    }
    String::new()
}

impl YouTubePlatform {
    pub fn new(limit: i64) -> YouTubePlatform {
        YouTubePlatform {
            limit,
            base: DEFAULT_BASE.to_string(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Test hook: redirect InnerTube calls (default https://m.youtube.com).
    pub fn set_base(&mut self, base: &str) {
        self.base = base.to_string();
    }

 /// regex_search parity, no regex dependency.
    pub fn can_get(query: &str) -> bool {
        looks_like_yt_link(query)
    }

 /// get: search vs link dispatch, video flag applied.
    pub fn get(&self, query: &str, video: bool) -> Vec<Track> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut tracks = if !looks_like_yt_link(query) {
            self.perform_search(query, self.limit)
        } else {
            let pl = extract_playlist(query);
            let vid = extract_video(query);
            if !pl.is_empty() && !vid.is_empty() {
                let v = self.handle_track(query);
                let p = self.handle_playlist(query);
                if !v.is_empty() {
                    let mut t = v.clone();
                    for q in p.iter() {
                        if q.id != v[0].id {
                            t.push(q.clone());
                        }
                    }
                    t
                } else if p.is_empty() {
                    Vec::new()
                } else {
                    p
                }
            } else if !pl.is_empty() {
                self.handle_playlist(query)
            } else {
                self.handle_track(query)
            }
        };
        for t in tracks.iter_mut() {
            t.video = video;
        }
        tracks
    }

 /// (fetch_mix on RD+video_id; 11-char guard).
    pub fn mix_for(&self, video_id: &str, limit: i64) -> Vec<Track> {
        if video_id.len() != 11 {
            return Vec::new();
        }
        self.fetch_mix(&format!("RD{video_id}"), limit)
    }

 /// POST {base}/youtubei/v1/{ep}?key=… (>=400 → Null).
    fn call_innertube(&self, endpoint: &str, payload: &Value) -> Option<Value> {
        let url = format!("{}/youtubei/v1/{endpoint}?key={INNER_TUBE_KEY}", self.base);
        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("User-Agent", MOBILE_UA)
            .json(payload)
            .send()
            .ok()?;
        if resp.status().as_u16() >= 400 {
            return None;
        }
        resp.json::<Value>().ok()
    }

    fn search_context() -> Value {
        json!({
            "context": { "client": {
                "clientName": INNER_TUBE_NAME,
                "clientVersion": INNER_TUBE_VER,
                "hl": "en-IN",
                "gl": "IN"
            } }
        })
    }

    fn plain_context() -> Value {
        json!({
            "context": { "client": {
                "clientName": INNER_TUBE_NAME,
                "clientVersion": INNER_TUBE_VER
            } }
        })
    }

 /// (search payload with params=CAASAhAB).
    fn perform_search(&self, query: &str, limit: i64) -> Vec<Track> {
        let mut payload = Self::search_context();
        payload["query"] = json!(query);
        payload["params"] = json!("CAASAhAB");
        let Some(result) = self.call_innertube("search", &payload) else {
            return Vec::new();
        };
        let contents = dig(
            &result,
            &[
                "contents",
                "twoColumnSearchResultsRenderer",
                "primaryContents",
                "sectionListRenderer",
                "contents",
            ],
        )
        .cloned()
        .unwrap_or(Value::Null);
        let mut tracks = Vec::new();
        Self::parse_nodes(&contents, &mut tracks, limit, "videoRenderer");
        tracks
    }

 /// recursive walk; arrays recurse, objects recurse    /// all values unless the key is present.
    fn parse_nodes(node: &Value, tracks: &mut Vec<Track>, limit: i64, key: &str) {
        if limit > 0 && tracks.len() as i64 >= limit {
            return;
        }
        match node {
            Value::Array(items) => {
                for item in items {
                    if limit > 0 && tracks.len() as i64 >= limit {
                        return;
                    }
                    Self::parse_nodes(item, tracks, limit, key);
                }
            }
            Value::Object(map) => {
                if let Some(vid) = map.get(key).and_then(|v| v.as_object()) {
                    if key == "videoRenderer" && is_live(&Value::Object(vid.clone())) {
                        return;
                    }
                    let vidj = Value::Object(vid.clone());
                    let id = as_str_at(vidj.get("videoId"));
                    if id.is_empty() {
                        return;
                    }
                    let duration_text = as_str_at(dig(&vidj, &["lengthText", "simpleText"]));
                    let duration_text = if duration_text.is_empty() {
                        as_str_at(dig(&vidj, &["lengthText", "runs", "0", "text"]))
                    } else {
                        duration_text
                    };
                    let title = as_str_at(dig(&vidj, &["title", "runs", "0", "text"]));
                    let title = if title.is_empty() {
                        as_str_at(dig(&vidj, &["title", "simpleText"]))
                    } else {
                        title
                    };
                    if title.is_empty() {
                        return;
                    }
                    tracks.push(Track {
                        url: format!("https://www.youtube.com/watch?v={id}"),
                        title,
                        id,
                        artwork: thumb_url(&vidj),
                        duration: parse_duration(&duration_text),
                        source: PLATFORM_YOUTUBE.to_string(),
                        ..Default::default()
                    });
                    return;
                }
                for v in map.values() {
                    if limit > 0 && tracks.len() as i64 >= limit {
                        return;
                    }
                    Self::parse_nodes(v, tracks, limit, key);
                }
            }
            _ => {}
        }
    }

 /// player endpoint, search fallback.
    fn handle_track(&self, raw: &str) -> Vec<Track> {
        let video_id = extract_video(raw);
        if video_id.is_empty() {
            return Vec::new();
        }
        let mut payload = Self::plain_context();
        payload["videoId"] = json!(video_id);
        if let Some(result) = self.call_innertube("player", &payload) {
            if let Some(d) = result.get("videoDetails") {
                let id = as_str_at(d.get("videoId"));
                let id = if id.is_empty() { video_id.clone() } else { id };
                let title = as_str_at(d.get("title"));
                let t = Track {
                    duration: as_str_at(d.get("lengthSeconds")).parse().unwrap_or(0),
                    artwork: thumb_url(&result),
                    url: format!("https://www.youtube.com/watch?v={id}"),
                    title,
                    id: id.clone(),
                    source: PLATFORM_YOUTUBE.to_string(),
                    ..Default::default()
                };
                if !t.title.is_empty() {
                    return vec![t];
                }
            }
        }
        let found = self.perform_search(&video_id, 8);
        for t in found.iter() {
            if t.id == video_id {
                return vec![t.clone()];
            }
        }
        found.into_iter().take(1).collect()
    }

 /// RD mix or browse VL+id.
    fn handle_playlist(&self, raw: &str) -> Vec<Track> {
        let playlist_id = extract_playlist(raw);
        if playlist_id.is_empty() {
            return Vec::new();
        }
        if playlist_id.starts_with("RD") {
            return self.fetch_mix(&playlist_id, self.limit);
        }
        let browse = if playlist_id.starts_with("VL") {
            playlist_id.clone()
        } else {
            format!("VL{playlist_id}")
        };
        let mut payload = Self::plain_context();
        payload["browseId"] = json!(browse);
        let Some(result) = self.call_innertube("browse", &payload) else {
            return Vec::new();
        };
        let mut tracks = Vec::new();
        Self::parse_nodes(&result, &mut tracks, self.limit, "playlistVideoRenderer");
        tracks
    }

 /// /next playlistPanelVideoRenderer walk
    fn fetch_mix(&self, playlist_id: &str, limit: i64) -> Vec<Track> {
        let mut payload = Self::plain_context();
        payload["playlistId"] = json!(playlist_id);
        let Some(result) = self.call_innertube("next", &payload) else {
            return Vec::new();
        };
        let Some(items) = dig(
            &result,
            &[
                "contents",
                "twoColumnWatchNextResults",
                "playlist",
                "playlist",
                "contents",
            ],
        )
        .and_then(|v| v.as_array())
        else {
            return Vec::new();
        };
        let mut tracks = Vec::new();
        for item in items {
            if limit > 0 && tracks.len() as i64 >= limit {
                break;
            }
            let Some(vid) = item.get("playlistPanelVideoRenderer") else {
                continue;
            };
            let id = as_str_at(vid.get("videoId"));
            if id.is_empty() {
                continue;
            }
            tracks.push(Track {
                url: format!("https://www.youtube.com/watch?v={id}"),
                title: as_str_at(dig(vid, &["title", "simpleText"])),
                id,
                artwork: thumb_url(vid),
                duration: parse_duration(&as_str_at(dig(vid, &["lengthText", "simpleText"]))),
                source: PLATFORM_YOUTUBE.to_string(),
                ..Default::default()
            });
        }
        tracks
    }
}
